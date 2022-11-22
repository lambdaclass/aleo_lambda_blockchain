use anyhow::{anyhow, bail, Result};
use lib::{
    vm::{self, Process},
    Transaction,
};
use snarkvm::prelude::Itertools;
use std::{
    convert::Into,
    sync::mpsc::{channel, Receiver, Sender},
};
use tendermint_abci::Application;
use tendermint_proto::abci::{
    Event, EventAttribute, RequestCheckTx, RequestDeliverTx, RequestInfo, RequestQuery,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseInfo, ResponseQuery,
};
use tracing::{debug, error, info};

use crate::record_store::RecordStore;

/// An Tendermint ABCI application that works with a SnarkVM backend.
/// This struct implements the ABCI application hooks, forwarding commands through
/// a channel for the parts that require knowledge of the application state and the SnarkVM details.
/// For reference see https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview
#[derive(Debug, Clone)]
pub struct SnarkVMApp {
    // FIXME this should be removed once we introduce a program store instead of using the snarkvm process through the driver
    cmd_tx: Sender<Command>,
    records: RecordStore,
}

impl Application for SnarkVMApp {
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
    /// mempool and relying it to other nodes.
    fn check_tx(&self, request: RequestCheckTx) -> ResponseCheckTx {
        info!("Check Tx");

        let tx = bincode::deserialize(&request.tx).unwrap();

        // TODO do we need to explicitly check the record serial number to guarantee it's being spent
        // by its owner? or is that already included in the execution verification?

        let result = self
            .check_no_duplicate_records(&tx)
            .and_then(|_| self.check_inputs_are_unspent(&tx))
            .and_then(|_| self.send_verify_transaction(tx.clone()));

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
            .and_then(|_| self.send_verify_transaction(tx.clone()))
            .and_then(|_| self.spend_input_records(&tx))
            .and_then(|_| self.add_output_records(&tx))
            // FIXME we currently apply tx changes --program deploys-- right away
            // but this should be done in the commit phase.
            .and_then(|_| self.send_finalize_transaction(tx.clone()));

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
    pub fn new() -> (Self, SnarkVMDriver) {
        let (cmd_tx, cmd_rx) = channel();
        (
            Self {
                cmd_tx,

                // we rather crash than start with a badly initialized store
                records: RecordStore::new("records").expect("cannot create a record store"),
            },
            SnarkVMDriver::new(cmd_rx),
        )
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
        if let Transaction::Execution { ref execution, .. } = transaction {
            execution
                .iter()
                .flat_map(|transition| transition.output_records())
                .map(|(commitment, record)| self.records.add(*commitment, record.clone()))
                .find(|result| result.is_err())
                .unwrap_or_else(|| Ok(()))
        } else {
            Ok(())
        }
    }

    // FIXME this is the part of the transaction validation that relies on snarkvm's Process struct
    // once the state is managed out of snarkvm, we can remove this channel and handle the validation here
    fn send_verify_transaction(&self, transaction: Transaction) -> Result<()> {
        info!("checking transaction: {}", transaction);
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::VerifyTransaction {
                transaction,
                result_tx,
            },
        )?;
        channel_recv(&result_rx)?
    }

    // FIXME this is the part of the transaction state changes that rely on snarkvm's Process struct
    // once the state is managed out of snarkvm, we can remove this channel and handle the validation here
    fn send_finalize_transaction(&self, transaction: Transaction) -> Result<()> {
        info!("finalizing transaction: {} ", transaction);
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::FinalizeTransaction {
                transaction,
                result_tx,
            },
        )?;
        channel_recv(&result_rx)?
    }
}

// FIXME this should be removed once we introduce a program store instead of using the snarkvm process through the driver
/// Driver that manages snarkvm-specific application state, for example the known deployed programs.
/// The driver listens for commands that need to interact with the state.
pub struct SnarkVMDriver {
    /// The SnarkVM Process keeps track of in-memory application state such as the verifying keys of deployed program's functions.
    process: Process,

    /// Used to listen for commands from the ABCI application
    cmd_rx: Receiver<Command>,
}

impl SnarkVMDriver {
    fn new(cmd_rx: Receiver<Command>) -> Self {
        Self {
            process: Process::load().unwrap(),
            cmd_rx,
        }
    }

    /// Run the driver in the current thread (blocking).
    pub fn run(mut self) -> Result<()> {
        loop {
            let cmd = self.cmd_rx.recv().map_err(|e| anyhow!("{}", e))?;
            match cmd {
                Command::VerifyTransaction {
                    transaction,
                    result_tx,
                } => {
                    channel_send(&result_tx, self.verify(transaction))?;
                }
                Command::FinalizeTransaction {
                    transaction,
                    result_tx,
                } => {
                    channel_send(&result_tx, self.finalize(transaction))?;
                }
            }
        }
    }

    fn finalize(&mut self, transaction: Transaction) -> Result<()> {
        match transaction {
            Transaction::Deployment { deployment, .. } => {
                // there is a finalize execution but it's not clear that we actually need it
                vm::finalize_deployment(&deployment, &mut self.process)
            }
            Transaction::Execution { .. } => {
                // we run finalize to save the program in the process for later execute verification
                // it's not clear that we're interested in the store here, but it's required for that function
                // note we could've use process.load_deployment instead but that one is private
                Ok(())
            }
        }
    }

    fn verify(&mut self, transaction: Transaction) -> Result<()> {
        let rng = &mut rand::thread_rng();
        let result = match transaction {
            Transaction::Deployment { ref deployment, .. } => {
                vm::verify_deployment(deployment, &self.process, rng)
            }
            Transaction::Execution { ref execution, .. } => {
                vm::verify_execution(execution, &self.process)
            }
        };

        match result {
            Err(ref e) => error!("Transaction {} verification failed: {}", transaction, e),
            _ => info!("Transaction {} verification successful", transaction),
        };
        result
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
            bincode::deserialize(&bytes).unwrap()
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

#[derive(Debug, Clone)]
enum Command {
    /// Send a transaction for SnarkVM verification, e.g. to check an execution proof.
    VerifyTransaction {
        transaction: Transaction,
        result_tx: Sender<Result<()>>,
    },
    /// Apply the transaction side-effects to the application (off-ledger) state, for
    /// example adding the program verfying keys to the SnarkVM process.
    FinalizeTransaction {
        transaction: Transaction,
        result_tx: Sender<Result<()>>,
    },
}

fn channel_send<T>(tx: &Sender<T>, value: T) -> Result<()> {
    tx.send(value).map_err(|e| anyhow!("{}", e))
}

fn channel_recv<T>(rx: &Receiver<T>) -> Result<T> {
    rx.recv().map_err(Into::into)
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
