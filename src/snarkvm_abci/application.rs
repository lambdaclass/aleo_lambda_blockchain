use anyhow::{anyhow, Result};
use bytes::BytesMut;
use lib::Transaction;
use snarkvm::{
    circuit::AleoV0,
    prelude::{Deployment, Execution, Process, ProgramMemory, ProgramStore, Testnet3},
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

#[derive(Debug, Clone)]
pub struct SnarkVMApp {
    cmd_tx: Sender<Command>,
}

impl SnarkVMApp {
    /// Constructor.
    pub fn new() -> (Self, SnarkVMDriver) {
        let (cmd_tx, cmd_rx) = channel();
        (Self { cmd_tx }, SnarkVMDriver::new(cmd_rx))
    }

    fn check_transaction(&self, transaction: &[u8]) -> Result<(i64, Result<()>)> {
        match bincode::deserialize(transaction)? {
            Transaction::Execution(id, execution) => self.verify_execution(id, execution),
            Transaction::Deployment(id, deployment) => self.run_deployment(id, deployment),
        }
    }

    fn verify_execution(
        &self,
        id: String,
        execution: Execution<Testnet3>,
    ) -> Result<(i64, Result<()>)> {
        info!("Verifying Execution: {}", execution.to_string());
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::VerifyExecution {
                id,
                execution,
                result_tx,
            },
        )?;
        channel_recv(&result_rx)
    }

    fn run_deployment(
        &self,
        id: String,
        deployment: Deployment<Testnet3>,
    ) -> Result<(i64, Result<()>)> {
        info!("Running Deployment: {}", deployment.to_string());
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::RunDeployment {
                id,
                deployment,
                result_tx,
            },
        )?;
        channel_recv(&result_rx)
    }
}

impl Application for SnarkVMApp {
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

    fn check_tx(&self, request: RequestCheckTx) -> ResponseCheckTx {
        info!("Check Tx");
        match self.check_transaction(&request.tx).unwrap() {
            (_height, Ok(_s)) => ResponseCheckTx {
                code: 0,
                data: Vec::default(),
                log: "".to_string(),
                info: "".to_string(),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![],
                codespace: "".to_string(),
                ..Default::default()
            },
            (_height, Err(e)) => ResponseCheckTx {
                code: 1,
                data: Vec::default(),
                log: format!("Could not verify transaction: {}", e),
                info: format!("Could not verify transaction: {}", e),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![],
                codespace: "".to_string(),
                ..Default::default()
            },
        }
    }

    fn deliver_tx(&self, request: RequestDeliverTx) -> ResponseDeliverTx {
        info!("Deliver Tx");

        let tx: Transaction = bincode::deserialize(&request.tx).unwrap();

        match self.check_transaction(&request.tx).unwrap() {
            (_height, Ok(_)) => ResponseDeliverTx {
                code: 0,
                data: Vec::default(),
                log: "".to_string(),
                info: "".to_string(),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![Event {
                    r#type: "app".to_string(),
                    attributes: vec![EventAttribute {
                        key: "tx_id".to_string().into_bytes(),
                        value: tx.id().to_string().into_bytes(),
                        index: true,
                    }],
                }],
                codespace: "".to_string(),
            },
            (_height, Err(e)) => ResponseDeliverTx {
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

/// Interacts with `snarkVM` state.
pub struct SnarkVMDriver {
    store: ProgramStore<Testnet3, ProgramMemory<Testnet3>>,
    process: Process<Testnet3>,
    height: i64,
    app_hash: Vec<u8>,
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
                Command::VerifyExecution {
                    id,
                    execution,
                    result_tx,
                } => {
                    debug!("Verifying \"{}\"", execution.to_string());
                    channel_send(
                        &result_tx,
                        (self.height, self.verify_execution(id, execution)),
                    )?;
                }
                Command::RunDeployment {
                    id,
                    deployment,
                    result_tx,
                } => {
                    channel_send(
                        &result_tx,
                        (self.height, self.run_deployment(id, deployment)),
                    )?;
                }
                Command::Commit { result_tx } => self.commit(&result_tx)?,
            }
        }
    }

    fn verify_execution(&mut self, id: String, execution: Execution<Testnet3>) -> Result<()> {
        let transition = execution.peek().unwrap();
        let program = transition.program_id();

        info!("Received execution tx id {} program {}", id, program);

        let result = self.process.verify_execution(&execution);

        match result {
            Err(ref e) => error!("Execution verification failed: {}", e),
            _ => info!("Execution verification successful"),
        };
        result
        // there is a finalize execution but it's not clear that we actually need it
    }

    fn run_deployment(&mut self, id: String, deployment: Deployment<Testnet3>) -> Result<()> {
        let program = deployment.program_id();
        info!("Received deployment tx id {} program {}", id, program);

        let rng = &mut rand::thread_rng();

        let result = self
            .process
            .verify_deployment::<AleoV0, _>(&deployment, rng);
        match result {
            Err(ref e) => error!("Deployment verification failed: {}", e),
            _ => info!("Deployment verification successful, storing program"),
        }

        // we run finalize to save the program in the process for later execute verification
        // it's not clear that we're interested in the store here, but it's required for that function
        // note we could've use process.load_deployment instead but that one is private
        self.process.finalize_deployment(&self.store, &deployment)
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
    /// Verify the execution of a program.
    VerifyExecution {
        id: String,
        execution: Execution<Testnet3>,
        result_tx: Sender<(i64, Result<()>)>,
    },
    RunDeployment {
        id: String,
        deployment: Deployment<Testnet3>,
        result_tx: Sender<(i64, Result<()>)>,
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
