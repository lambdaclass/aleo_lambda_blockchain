use anyhow::{anyhow, Result};
use bytes::BytesMut;
use lib::Transaction;
use snarkvm::{
    circuit::AleoV0,
    prelude::{Process, ProgramMemory, ProgramStore, Testnet3},
};
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

// NOTE: in the sample app, the following const was defined on one of the internal libraries, but was not `pub`
// so we had to extract it here

/// The maximum number of bytes we expect in a varint. We use this to check if
/// we're encountering a decoding error for a varint.
pub const MAX_VARINT_LENGTH: usize = 16;

/// An Tendermint ABCI application that works with a SnarkVM backend.
/// This struct implements the ABCI application hooks, forwarding commands through
/// a channel for the parts that require knowledge of the application state and the SnarkVM details.
/// For reference see https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview
#[derive(Debug, Clone)]
pub struct SnarkVMApp {
    cmd_tx: Sender<Command>,
}

impl Application for SnarkVMApp {
    /// This hook provides information about the ABCI application.
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        debug!(
            "Got info request. Tendermint version: {}; Block version: {}; P2P version: {}",
            request.version, request.block_version, request.p2p_version
        );

        let (result_tx, result_rx) = channel();
        channel_send(&self.cmd_tx, Command::GetInfo { result_tx }).unwrap();
        let (last_block_height, last_block_app_hash) = channel_recv(&result_rx).unwrap();

        ResponseInfo {
            data: "snarkvm-app".to_string(),
            version: "0.1.0".to_string(),
            app_version: 1,
            last_block_height,
            last_block_app_hash,
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
        if let Err(err) = self.send_verify_transaction(tx) {
            ResponseCheckTx {
                code: 1,
                data: Vec::default(),
                log: format!("Could not verify transaction: {}", err),
                info: format!("Could not verify transaction: {}", err),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![],
                codespace: "".to_string(),
                ..Default::default()
            }
        } else {
            ResponseCheckTx {
                code: 0,
                data: Vec::default(),
                log: "".to_string(),
                info: "".to_string(),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![],
                codespace: "".to_string(),
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
        let result = self
            .send_verify_transaction(tx.clone())
            .and_then(|_| self.send_finalize_transaction(tx.clone()));

        // prepare this transaction to be queried by app.tx_id
        let index_event = Event {
            r#type: "app".to_string(),
            attributes: vec![EventAttribute {
                key: "tx_id".to_string().into_bytes(),
                value: tx.id().to_string().into_bytes(),
                index: true,
            }],
        };

        match result {
            Ok(_) => ResponseDeliverTx {
                code: 0,
                data: Vec::default(),
                log: "".to_string(),
                info: "".to_string(),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![index_event],
                codespace: "".to_string(),
            },
            Err(e) => ResponseDeliverTx {
                code: 1,
                data: Vec::default(),
                log: format!("Could not verify transaction: {}", e),
                info: format!("Could not verify transaction: {}", e),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![],
                codespace: "".to_string(),
            },
        }
    }

    /// This hook is used to compute a cryptographic commitment to the current application state,
    /// to be stored in the header of the next Block.
    fn commit(&self) -> ResponseCommit {
        let (result_tx, result_rx) = channel();
        channel_send(&self.cmd_tx, Command::Commit { result_tx }).unwrap();
        let (height, app_hash) = channel_recv(&result_rx).unwrap();
        info!("Committed height {}", height);
        ResponseCommit {
            data: app_hash,
            retain_height: height - 1,
        }
    }
}

impl SnarkVMApp {
    /// Constructor.
    pub fn new() -> (Self, SnarkVMDriver) {
        let (cmd_tx, cmd_rx) = channel();
        (Self { cmd_tx }, SnarkVMDriver::new(cmd_rx))
    }

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

/// Driver that manages snarkvm-specific application state, for example the known deployed programs.
/// The driver listens for commands that need to interact with the state.
// NOTE: this driver shouldn't necessarily need to know about transactions, but rather about programs:
// how to validate new programs and execution proofs. To arrange the code like that we likely need to
// replace the SnarkVM process that we currently use to track the program data.
pub struct SnarkVMDriver {
    /// The SnarkVM Process keeps track of in-memory application state such as the verifying keys of deployed program's functions.
    process: Process<Testnet3>,

    /// A SnarkVM program store. this is only used to comply with the current SnarkVM Process API, but it's
    /// not actually relied-upon for storage and will eventually be removed.
    store: ProgramStore<Testnet3, ProgramMemory<Testnet3>>,

    /// The last known height of the blockchain. Required by Tendermint to track potential drift between the ABCI and the blockchain.
    height: i64,
    /// A hash of the current application state. Required by Tendermint to track inconsistencies between the blockchain and the app
    /// (inconsistencies will be treated as blockchain forks by Tendermint.)
    app_hash: Vec<u8>,

    /// Used to listen for commands from the ABCI application
    cmd_rx: Receiver<Command>,
}

impl SnarkVMDriver {
    fn new(cmd_rx: Receiver<Command>) -> Self {
        Self {
            store: ProgramStore::<_, ProgramMemory<_>>::open(None).unwrap(),
            process: Process::load().unwrap(),
            height: 0,
            app_hash: vec![0_u8; MAX_VARINT_LENGTH],
            cmd_rx,
        }
    }

    /// Run the driver in the current thread (blocking).
    pub fn run(mut self) -> Result<()> {
        loop {
            let cmd = self.cmd_rx.recv().map_err(|e| anyhow!("{}", e))?;
            match cmd {
                Command::GetInfo { result_tx } => {
                    channel_send(&result_tx, (self.height, self.app_hash.clone()))?;
                }
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
                Command::Commit { result_tx } => self.commit(&result_tx)?,
            }
        }
    }

    fn verify(&mut self, transaction: Transaction) -> Result<()> {
        let rng = &mut rand::thread_rng();
        let result = match transaction {
            Transaction::Deployment { ref deployment, .. } => {
                self.process.verify_deployment::<AleoV0, _>(deployment, rng)
            }
            Transaction::Execution { ref execution, .. } => {
                self.process.verify_execution(execution)
            }
        };

        match result {
            Err(ref e) => error!("Transaction {} verification failed: {}", transaction, e),
            _ => info!("Transaction {} verification successful", transaction),
        };
        result
    }

    fn finalize(&mut self, transaction: Transaction) -> Result<()> {
        match transaction {
            Transaction::Deployment { deployment, .. } => {
                // there is a finalize execution but it's not clear that we actually need it
                self.process.finalize_deployment(&self.store, &deployment)
            }
            Transaction::Execution { .. } => {
                // we run finalize to save the program in the process for later execute verification
                // it's not clear that we're interested in the store here, but it's required for that function
                // note we could've use process.load_deployment instead but that one is private
                Ok(())
            }
        }
    }

    fn commit(&mut self, result_tx: &Sender<(i64, Vec<u8>)>) -> Result<()> {
        // As in the Go-based key/value store, simply encode the number of
        // items as the "app hash"
        let app_hash = BytesMut::with_capacity(MAX_VARINT_LENGTH);
        //prost::encoding::encode_varint(self.store.len() as u64, &mut app_hash);
        self.app_hash = app_hash.to_vec();
        self.height += 1;
        channel_send(result_tx, (self.height, self.app_hash.clone()))
    }
}

#[derive(Debug, Clone)]
enum Command {
    /// Get the height of the last commit.
    GetInfo { result_tx: Sender<(i64, Vec<u8>)> },
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
    /// Commit the current state of the application, which involves recomputing
    /// the application's hash.
    Commit { result_tx: Sender<(i64, Vec<u8>)> },
}

fn channel_send<T>(tx: &Sender<T>, value: T) -> Result<()> {
    tx.send(value).map_err(|e| anyhow!("{}", e))
}

fn channel_recv<T>(rx: &Receiver<T>) -> Result<T> {
    rx.recv().map_err(Into::into)
}
