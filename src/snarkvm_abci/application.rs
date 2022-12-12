use std::sync::{Arc, Mutex};

use crate::record_store::RecordStore;
use crate::{program_store::ProgramStore, validator_set::ValidatorSet};
use anyhow::{anyhow, bail, ensure, Result};
use lib::{query::AbciQuery::RecordsUnspentOwned, transaction::Transaction, vm, GenesisState};
use snarkvm::prelude::Itertools;
use tendermint_abci::Application;
use tendermint_proto::abci;

use tracing::{debug, error, info};

/// An Tendermint ABCI application that works with a SnarkVM backend.
/// This struct implements the ABCI application hooks, forwarding commands through
/// a channel for the parts that require knowledge of the application state and the SnarkVM details.
/// For reference see https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview
#[derive(Debug, Clone)]
pub struct SnarkVMApp {
    records: RecordStore,
    programs: ProgramStore,

    // NOTE: Wrapping in mutex here because we need mut access to ValidatorSet and the alternative to setup
    // a channel was overkilll for this particular case. Also, at the moment we only ever access these field
    // from a single tendermint abci connection (the consensus connection), but using Rc instead of Arc would
    // introduce subtle bugs should that ever change.
    validators: Arc<Mutex<ValidatorSet>>,
}

impl Application for SnarkVMApp {
    /// This hook is called once upon genesis. It's used to load a default set of records which
    /// make the initial distribution of credits in the system.
    fn init_chain(&self, request: abci::RequestInitChain) -> abci::ResponseInitChain {
        info!("Loading genesis");
        let state: GenesisState =
            serde_json::from_slice(&request.app_state_bytes).expect("invalid genesis state");

        for (commitment, record) in state.records {
            debug!("Storing genesis record {}", commitment);
            self.records
                .add(commitment, record)
                .expect("failure adding genesis records");
        }

        self.validators
            .lock()
            .unwrap()
            .set_validators(state.validators);

        Default::default()
    }

    /// This hook provides information about the ABCI application.
    fn info(&self, request: abci::RequestInfo) -> abci::ResponseInfo {
        debug!(
            "Got info request. Tendermint version: {}; Block version: {}; P2P version: {}",
            request.version, request.block_version, request.p2p_version
        );

        abci::ResponseInfo {
            data: "snarkvm-app".to_string(),
            version: "0.1.0".to_string(),
            app_version: 1,
            last_block_height: HeightFile::read_or_create(),

            // using a fixed hash, see the commit() hook
            last_block_app_hash: vec![],
        }
    }

    /// This hook is to query the application for data at the current or past height.
    fn query(&self, request: abci::RequestQuery) -> abci::ResponseQuery {
        let RecordsUnspentOwned { address, view_key } =
            bincode::deserialize(&request.data).unwrap();
        info!("Fetching records");
        // TODO: This fetches all the records from the RecordStore to filter here the
        // owned ones. With a large database this will involve a lot of data/time
        // so we should think of a better way to handle this. (eg. pagination or asynchronous
        // querying)
        // https://trello.com/c/bP8Nbs7C/170-handle-record-querying-properly-in-recordstore
        match self.records.scan(None, None) {
            Ok((records, _last_key)) => {
                let result: Vec<(vm::Field, vm::EncryptedRecord)> = records
                    .into_iter()
                    .filter(|(_, record)| record.is_owner(&address, &view_key))
                    .collect();
                abci::ResponseQuery {
                    value: bincode::serialize(&result).unwrap(),
                    ..Default::default()
                }
            }
            Err(e) => abci::ResponseQuery {
                code: 1,
                log: format!("Error running query: {e}"),
                info: format!("Error running query: {e}"),
                ..Default::default()
            },
        }
    }

    /// This ABCI hook validates an incoming transaction before inserting it in the
    /// mempool and relaying it to other nodes.
    fn check_tx(&self, request: abci::RequestCheckTx) -> abci::ResponseCheckTx {
        info!("Check Tx");

        let tx = bincode::deserialize(&request.tx).unwrap();

        // TODO do we need to explicitly check the record serial number to guarantee it's being spent
        // by its owner? or is that already included in the execution verification?

        let result = self
            .check_no_duplicate_records(&tx)
            .and_then(|_| self.check_inputs_are_unspent(&tx))
            .and_then(|_| self.validate_transaction(&tx));

        if let Err(err) = result {
            abci::ResponseCheckTx {
                code: 1,
                log: format!("Could not verify transaction: {err}"),
                info: format!("Could not verify transaction: {err}"),
                ..Default::default()
            }
        } else {
            abci::ResponseCheckTx {
                ..Default::default()
            }
        }
    }

    /// This hook is called before the app starts processing transactions on a block.
    /// Used to store current proposer and the previous block's voters to assign fees and coinbase
    /// credits when the block is committed.
    fn begin_block(&self, request: abci::RequestBeginBlock) -> abci::ResponseBeginBlock {
        // a call to begin block without header doesn't seem to make sense, verify it can happen
        // supporting this case is cumbersome, assuming it won't happen until proven wrong
        let header = request
            .header
            .expect("received block without header, aborting");

        // store current block proposer and previous block voters in the validator set
        // NOTE: because of how tendermint makes information available to this hook,
        // the block rewards go to this block's porposer and the **previous** block voters.
        // This could be revisited if it's a problem.
        let votes = request
            .last_commit_info
            .map(|last_commit| last_commit.votes)
            .unwrap_or_default()
            .iter()
            .filter_map(|vote_info| {
                if !vote_info.signed_last_block {
                    // don't count validators that didn't participate in previous round
                    return None;
                }

                if let Some(validator) = vote_info.validator.clone() {
                    Some((validator.address, validator.power as u64))
                } else {
                    // If there's no associated validator data, we can't use this vote
                    None
                }
            })
            .collect();
        self.validators.lock().unwrap().prepare(
            header.proposer_address.clone(),
            votes,
            header.height as u64,
        );

        Default::default()
    }

    /// This ABCI hook validates a transaction and applies it to the application state,
    /// for example storing the program verifying keys upon a valid deployment.
    /// Here is also where transactions are indexed for querying the blockchain.
    fn deliver_tx(&self, request: abci::RequestDeliverTx) -> abci::ResponseDeliverTx {
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
            .and_then(|_| self.collect_fees(&tx))
            .and_then(|_| self.spend_input_records(&tx))
            .and_then(|_| self.add_output_records(&tx))
            .and_then(|_| self.store_program(&tx));
        // FIXME we currently apply tx changes --program deploys-- right away
        // but this should be done in the commit phase.

        match result {
            Ok(_) => {
                // prepare this transaction to be queried by app.tx_id
                let index_event = abci::Event {
                    r#type: "app".to_string(),
                    attributes: vec![abci::EventAttribute {
                        key: "tx_id".to_string().into_bytes(),
                        value: tx.id().to_string().into_bytes(),
                        index: true,
                    }],
                };

                abci::ResponseDeliverTx {
                    events: vec![index_event],
                    ..Default::default()
                }
            }
            Err(e) => abci::ResponseDeliverTx {
                code: 1,
                log: format!("Error delivering transaction: {e}"),
                info: format!("Error delivering transaction: {e}"),
                ..Default::default()
            },
        }
    }

    /// This hook commits is called when the block is comitted (after deliver_tx has been called for each transaction).
    /// Changes to application should take effect here. Tendermint guarantees that no transaction is processed while this
    /// hook is running.
    /// The result includes a hash of the application state which will be included in the block header.
    /// This hash should be deterministic, different app state hashes will produce blockchain forks.
    /// New credits records are created to assign validator rewards.
    fn commit(&self) -> abci::ResponseCommit {
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

        for (commitment, record) in self.validators.lock().unwrap().rewards() {
            if let Err(err) = self.records.add(commitment, record) {
                error!("Failed to add reward record to store {}", err);
            }
        }

        info!("Committing height {}", height);
        abci::ResponseCommit {
            data: app_hash,
            retain_height: 0,
        }
    }
}

impl SnarkVMApp {
    /// Constructor.
    pub fn new() -> Self {
        Self {
            // we rather crash than start with badly initialized stores
            programs: ProgramStore::new("programs").expect("could not create a program store"),
            records: RecordStore::new("records").expect("could not create a record store"),
            validators: Arc::new(Mutex::new(ValidatorSet::new("abci.validators"))),
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
            .unwrap_or(Ok(()))
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
                .unwrap_or(Ok(()))
        } else {
            Ok(())
        }
    }

    fn validate_transaction(&self, transaction: &Transaction) -> Result<()> {
        let result = match transaction {
            Transaction::Deployment {
                ref program,
                verifying_keys,
                ..
            } => {
                ensure!(
                    !self.programs.exists(program.id()),
                    format!("Program already exists: {}", program.id())
                );

                // verify deployment is correct and keys are valid
                vm::verify_deployment(program, verifying_keys.clone())
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
                    format!("Program already exists: {}", program.id())
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

    /// Add the transaction fees to the current block's validator rewards.
    fn collect_fees(&self, transaction: &Transaction) -> Result<()> {
        let fees = match transaction {
            Transaction::Deployment { .. } => {
                // TODO deployment should have an optional fee transition
                0
            }
            Transaction::Source { .. } => {
                // TODO deployment should have an optional fee transition
                0
            }
            Transaction::Execution { transitions, .. } => transitions
                .iter()
                .fold(0, |acc, transition| acc + transition.fee()),
        };

        self.validators.lock().unwrap().add(fees as u64);
        Ok(())
    }

    /// Apply the transaction side-effects to the application (off-ledger) state, for
    /// example adding the programs to the program store.
    fn store_program(&self, transaction: &Transaction) -> Result<()> {
        match transaction {
            Transaction::Deployment {
                program,
                verifying_keys,
                ..
            } => self.programs.add(program.id(), program, verifying_keys),
            Transaction::Execution { .. } => {
                // we run finalize to save the program in the process for later execute verification
                // it's not clear that we're interested in the store here, but it's required for that function
                // note we could've use process.load_deployment instead but that one is private
                Ok(())
            }
            Transaction::Source { program, .. } => {
                let rng = &mut rand::thread_rng();
                let program = vm::generate_program(&program.to_string())?;

                self.programs.add(
                    program.id(),
                    &program,
                    &vm::generate_verifying_keys(&program, rng)?,
                )
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
