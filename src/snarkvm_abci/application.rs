use anyhow::{anyhow, bail, ensure, Result};
use lib::{
    transaction::Transaction,
    vm::{self},
    GenesisState,
};
use snarkvm::prelude::Itertools;

use tendermint_abci::Application;
use tendermint_proto::abci::{
    Event, EventAttribute, RequestCheckTx, RequestDeliverTx, RequestInfo, RequestInitChain,
    RequestQuery, ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseInfo,
    ResponseInitChain, ResponseQuery,
};
use tracing::{debug, error, info};

use crate::program_store::ProgramStore;
use crate::record_store::RecordStore;

/// An Tendermint ABCI application that works with a SnarkVM backend.
/// This struct implements the ABCI application hooks, forwarding commands through
/// a channel for the parts that require knowledge of the application state and the SnarkVM details.
/// For reference see https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview
#[derive(Debug, Clone)]
pub struct SnarkVMApp {
    records: RecordStore,
    programs: ProgramStore,
}

impl Application for SnarkVMApp {
    /// This hook is called once upon genesis. It's used to load a default set of records which
    /// make the initial distribution of credits in the system.
    fn init_chain(&self, request: RequestInitChain) -> ResponseInitChain {
        info!("Loading genesis");
        let state: GenesisState =
            serde_json::from_slice(&request.app_state_bytes).expect("invalid genesis state");
        for (commitment, record) in state.records {
            debug!("Storing genesis record {}", commitment);
            self.records
                .add(commitment, record)
                .expect("failure adding genesis records");
        }
        Default::default()
    }

    /// This hook provides information about the ABCI application.
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        debug!(
            "Got info request. Tendermint version: {}; Block version: {}; P2P version: {}",
            request.version, request.block_version, request.p2p_version
        );

        ResponseInfo {
            data: "snarkvm-app".to_string(),
            version: "0.1.0".to_string(),
            app_version: 1,
            last_block_height: HeightFile::read_or_create(),

            // using a fixed hash, see the commit() hook
            last_block_app_hash: vec![],
        }
    }

    /// This hook is to query the application for data at the current or past height.
    fn query(&self, request: RequestQuery) -> ResponseQuery {
        let key = match std::str::from_utf8(&request.data) {
            Ok(s) => s,
            Err(e) => panic!("Failed to intepret key as UTF-8: {}", e),
        };
        debug!("Attempting to get key: {}", key);
        // ResponseQuery {
        //     code: 0,
        //     log: "does not exist".to_string(),
        //     info: "".to_string(),
        //     index: 0,
        //     key: request.data,
        //     value: Default::default(),
        //     proof_ops: None,
        //     height,
        //     codespace: "".to_string(),
        // }
        panic!("Not implemented \"{}\"", key)
    }

    /// This ABCI hook validates an incoming transaction before inserting it in the
    /// mempool and relaying it to other nodes.
    fn check_tx(&self, request: RequestCheckTx) -> ResponseCheckTx {
        info!("Check Tx");

        let tx = bincode::deserialize(&request.tx).unwrap();

        // TODO do we need to explicitly check the record serial number to guarantee it's being spent
        // by its owner? or is that already included in the execution verification?

        let result = self
            .check_no_duplicate_records(&tx)
            .and_then(|_| self.check_inputs_are_unspent(&tx))
            .and_then(|_| self.validate_transaction(&tx));

        if let Err(err) = result {
            ResponseCheckTx {
                code: 1,
                log: format!("Could not verify transaction: {}", err),
                info: format!("Could not verify transaction: {}", err),
                ..Default::default()
            }
        } else {
            ResponseCheckTx {
                ..Default::default()
            }
        }
    }

    /// This ABCI hook validates a transaction and applies it to the application state,
    /// for example storing the program verifying keys upon a valid deployment.
    /// Here is also where transactions are indexed for querying the blockchain.
    fn deliver_tx(&self, request: RequestDeliverTx) -> ResponseDeliverTx {
        info!("Deliver Tx");

        let tx: Transaction = bincode::deserialize(&request.tx).unwrap();

        // we need to repeat the same validations as deliver_tx and only, because the protocol can't
        // guarantee that a bynzantine validator won't propose a block with invalid transactions.
        // if validation they pass  apply (but not commit) the application state changes.
        // Note that we check for duplicate records within the transaction before attempting to spend them
        // so we don't end up with a half-applied transaction in the record store.
        let result = self
            .check_no_duplicate_records(&tx)
            .and_then(|_| self.check_inputs_are_unspent(&tx))
            .and_then(|_| self.validate_transaction(&tx))
            .and_then(|_| self.spend_input_records(&tx))
            .and_then(|_| self.add_output_records(&tx))
            .and_then(|_| self.store_program(&tx));
        // FIXME we currently apply tx changes --program deploys-- right away
        // but this should be done in the commit phase.

        match result {
            Ok(_) => {
                // prepare this transaction to be queried by app.tx_id
                let index_event = Event {
                    r#type: "app".to_string(),
                    attributes: vec![EventAttribute {
                        key: "tx_id".to_string().into_bytes(),
                        value: tx.id().to_string().into_bytes(),
                        index: true,
                    }],
                };

                ResponseDeliverTx {
                    events: vec![index_event],
                    ..Default::default()
                }
            }
            Err(e) => ResponseDeliverTx {
                code: 1,
                log: format!("Error delivering transaction: {}", e),
                info: format!("Error delivering transaction: {}", e),
                ..Default::default()
            },
        }
    }

    /// This hook commits is called when the block is comitted (after deliver_tx has been called for each transaction).
    /// Changes to application should take effect here. Tendermint guarantees that no transaction is processed while this
    /// hook is running.
    /// The result includes a hash of the application state which will be included in the block header.
    /// This hash should be deterministic, different app state hashes will produce blockchain forks.
    fn commit(&self) -> ResponseCommit {
        // the app hash is intended to capture the state of the application that's not contained directly
        // in the blockchain transactions (as tendermint already accounts for that with other hashes).
        // we could do something in the RecordStore and ProgramStore to track state changes there and
        // calculate a hash based on that, if we expected some aspect of that data not to be completely
        // determined by the list of committed transactions (for example if we expected different versions
        // of the app with differing logic to coexist). At this stage it seems overkill to add support for that
        // scenario so we just to use a fixed hash. See below for more discussion on the use of app hash:
        // https://github.com/tendermint/tendermint/issues/1179
        // https://github.com/tendermint/tendermint/blob/v0.34.x/spec/abci/apps.md#query-proofs
        let app_hash = vec![];

        // apply pending changes in the record store: mark used records as spent, add inputs as unspent
        if let Err(err) = self.records.commit() {
            error!("Failure while committing the record store {}", err);
        }

        let height = HeightFile::increment();

        info!("Committing height {}", height);
        ResponseCommit {
            data: app_hash,
            retain_height: 0,
        }
    }
}

impl SnarkVMApp {
    /// Constructor.
    pub fn new() -> Self {
        Self {
            programs: ProgramStore::new("programs").expect("could not create a program store"),
            // we rather crash than start with a badly initialized store
            records: RecordStore::new("records").expect("could not create a record store"),
        }
    }

    /// Fail if the same record appears more than once as a function input in the transaction.
    fn check_no_duplicate_records(&self, transaction: &Transaction) -> Result<()> {
        let commitments = transaction.origin_commitments();
        if let Some(commitment) = commitments.iter().duplicates().next() {
            bail!(
                "input record commitment {} in transaction {} is duplicate",
                commitment,
                transaction.id()
            );
        }
        Ok(())
    }

    /// the transaction should be rejected if it's input records don't exist
    /// or they aren't known to be unspent either in the ledger or in an unconfirmed transaction output
    fn check_inputs_are_unspent(&self, transaction: &Transaction) -> Result<()> {
        let commitments = transaction.origin_commitments();
        let already_spent = commitments
            .iter()
            .find(|commitment| !self.records.is_unspent(commitment).unwrap_or(true));
        if let Some(commitment) = already_spent {
            bail!(
                "input record commitment {} is unknown or already spent",
                commitment
            )
        }
        Ok(())
    }

    /// Mark all input records as spent in the record store. This operation could fail if the records are unknown or already spent,
    /// but it's assumed the that was validated before as to prevent half-applied transactions in the block.
    fn spend_input_records(&self, transaction: &Transaction) -> Result<()> {
        let commitments = transaction.origin_commitments();
        commitments
            .iter()
            .map(|commitment| self.records.spend(commitment))
            .find(|result| result.is_err())
            .unwrap_or_else(|| Ok(()))
    }

    /// Add the tranasction output records as unspent in the record store.
    fn add_output_records(&self, transaction: &Transaction) -> Result<()> {
        if let Transaction::Execution {
            ref transitions, ..
        } = transaction
        {
            transitions
                .iter()
                .flat_map(|transition| transition.output_records())
                .map(|(commitment, record)| self.records.add(*commitment, record.clone()))
                .find(|result| result.is_err())
                .unwrap_or_else(|| Ok(()))
        } else {
            Ok(())
        }
    }

    fn validate_transaction(&self, transaction: &Transaction) -> Result<()> {
        let rng = &mut rand::thread_rng();

        let result = match transaction {
            Transaction::Deployment { ref deployment, .. } => {
                ensure!(
                    !self.programs.exists(deployment.program_id()),
                    "Program already exists"
                );

                // verify deployment is correct and keys are valid
                vm::verify_deployment(deployment, rng)
            }
            Transaction::Execution {
                ref transitions, ..
            } => {
                let transition = transitions
                    .first()
                    .ok_or_else(|| anyhow!("missing transition"))?;

                // TODO this assumes only one transition represents the program, is this correct?
                let stored_keys = self.programs.get(transition.program_id())?;

                // only verify if we have the program available
                // TODO review if we really need to store the program
                if let Some((_program, keys)) = stored_keys {
                    vm::verify_execution(transitions, &keys)
                } else {
                    bail!(format!(
                        "Program {} does not exist",
                        transition.program_id()
                    ))
                }
            }
            Transaction::Source { program, .. } => {
                ensure!(
                    !self.programs.exists(program.id()),
                    "Program already exists"
                );

                // validate that the program is parsed correctly
                vm::generate_program(&program.to_string()).map(|_| ())
            }
        };

        match result {
            Err(ref e) => error!("Transaction {} verification failed: {}", transaction, e),
            _ => info!("Transaction {} verification successful", transaction),
        };
        result
    }

    /// Apply the transaction side-effects to the application (off-ledger) state, for
    /// example adding the programs to the program store.
    fn store_program(&self, transaction: &Transaction) -> Result<()> {
        match transaction {
            Transaction::Deployment { deployment, .. } => self.programs.add(
                deployment.program_id(),
                deployment.program(),
                deployment.verifying_keys(),
            ),
            Transaction::Execution { .. } => {
                // we run finalize to save the program in the process for later execute verification
                // it's not clear that we're interested in the store here, but it's required for that function
                // note we could've use process.load_deployment instead but that one is private
                Ok(())
            }
            Transaction::Source { program, .. } => {
                let rng = &mut rand::thread_rng();
                let compiled_program = vm::generate_deployment(&program.to_string(), rng)?;

                self.programs
                    .add(program.id(), program, compiled_program.verifying_keys())
            }
        }
    }
}

/// Local file used to track the last block height seen by the abci application.
struct HeightFile;

impl HeightFile {
    const PATH: &str = "abci.height";

    fn read_or_create() -> i64 {
        // if height file is missing or unreadable, create a new one from zero height
        if let Ok(bytes) = std::fs::read(Self::PATH) {
            // if contents are not readable, crash intentionally
            bincode::deserialize(&bytes).expect("Contents of height file are not readable")
        } else {
            std::fs::write(Self::PATH, bincode::serialize(&0i64).unwrap()).unwrap();
            0i64
        }
    }

    fn increment() -> i64 {
        // if the file is missing or contents are unexpected, we crash intentionally;
        let mut height: i64 = bincode::deserialize(&std::fs::read(Self::PATH).unwrap()).unwrap();
        height += 1;
        std::fs::write(Self::PATH, bincode::serialize(&height).unwrap()).unwrap();
        height
    }
}

// just covering a few special cases here. lower level test are done in record store, higher level in integration tests.
#[cfg(test)]
mod tests {
    // use super::*;

    #[test]
    fn test_check_tx() {
        // TODO
        // fail if duplicate (non spent) inputs
        // fail if already spent inputs
        // succeed otherwise
    }

    #[test]
    fn test_deliver_tx() {
        // fail if duplicate (non spent) inputs
        // check that they remain unspent

        // fail if already spent inputs

        // check that inputs are not unspent anymore
        // check that outputs are now unspent
    }
}
