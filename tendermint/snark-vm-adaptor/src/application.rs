use anyhow::{anyhow, Result};
use bytes::BytesMut;
use display_json::DisplayAsJson;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use snarkvm::{
    circuit::AleoV0,
    file::VerifierFile,
    package::Package,
    prelude::{Identifier, Testnet3},
};
use std::{
    collections::HashMap,
    convert::Into,
    path::Path,
    str::FromStr,
    sync::mpsc::{channel, Receiver, Sender},
};
use tendermint_abci::Application;
use tendermint_proto::abci::{
    Event, EventAttribute, RequestCheckTx, RequestDeliverTx, RequestInfo, RequestQuery,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseInfo, ResponseQuery,
};
use tracing::{debug, info};

// NOTE: in the sample app, the following const was defined on one of the internal libraries, but was not `pub`
// so we had to extract it here

/// The maximum number of bytes we expect in a varint. We use this to check if
/// we're encountering a decoding error for a varint.
pub const MAX_VARINT_LENGTH: usize = 16;

#[derive(Serialize, Deserialize, DisplayAsJson)]
struct Transaction<'a> {
    id: String,
    r#type: String,
    #[serde(borrow)]
    execution: &'a RawValue,
}

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

    /// Attempt to verify an execution
    pub fn verify_execution(&self, transaction: &[u8]) -> Result<(i64, Result<()>)> {
        let tx: Transaction = serde_json::from_str(std::str::from_utf8(transaction)?)?;
        info!("Verifying Execution: {}", tx.execution.to_string());
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::VerifyExecution {
                path: "../../hello".to_string(),
                function: FromStr::from_str("hello")?,
                execution_json: tx.execution.to_string(),
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
        match self.verify_execution(&request.tx).unwrap() {
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
        match self.verify_execution(&request.tx).unwrap() {
            (_height, Ok(_s)) => ResponseDeliverTx {
                code: 0,
                data: Vec::default(),
                log: "".to_string(),
                info: "".to_string(),
                gas_wanted: 0,
                gas_used: 0,
                events: vec![Event {
                    r#type: "app".to_string(),
                    attributes: vec![
                        EventAttribute {
                            key: "key".to_string().into_bytes(),
                            value: "key".to_string().into_bytes(),
                            index: true,
                        },
                        EventAttribute {
                            key: "index_key".to_string().into_bytes(),
                            value: "index is working".to_string().into_bytes(),
                            index: true,
                        },
                        EventAttribute {
                            key: "noindex_key".to_string().into_bytes(),
                            value: "index is working".to_string().into_bytes(),
                            index: false,
                        },
                    ],
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
#[derive(Debug)]
pub struct SnarkVMDriver {
    store: HashMap<String, String>,
    height: i64,
    app_hash: Vec<u8>,
    cmd_rx: Receiver<Command>,
}

impl SnarkVMDriver {
    fn new(cmd_rx: Receiver<Command>) -> Self {
        Self {
            store: HashMap::new(),
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
                    path,
                    function,
                    execution_json,
                    result_tx,
                } => {
                    debug!("Verifying \"{}\"", execution_json);
                    channel_send(
                        &result_tx,
                        (
                            self.height,
                            Self::verify_execution(&path, function, &execution_json),
                        ),
                    )?;
                }
                Command::Commit { result_tx } => self.commit(&result_tx)?,
            }
        }
    }

    fn verify_execution(
        path: &str,
        function: Identifier<Testnet3>,
        execution_json: &str,
    ) -> Result<()> {
        let program_path = Path::new(&path);
        let execution = FromStr::from_str(std::str::from_utf8(execution_json.as_bytes())?)?;

        let package: Package<Testnet3> = Package::open(program_path)?;
        package.build::<AleoV0>(None)?;

        let build_dir = package.build_directory();
        let process = package.get_process()?;

        let verifier = VerifierFile::open(build_dir.as_path(), &function)?;

        let program_id = package.program_id();
        process.insert_verifying_key(program_id, &function, verifier.verifying_key().clone())?;

        process.verify_execution(&execution)
    }

    fn commit(&mut self, result_tx: &Sender<(i64, Vec<u8>)>) -> Result<()> {
        // As in the Go-based key/value store, simply encode the number of
        // items as the "app hash"
        let mut app_hash = BytesMut::with_capacity(MAX_VARINT_LENGTH);
        prost::encoding::encode_varint(self.store.len() as u64, &mut app_hash);
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
        path: String,
        function: Identifier<Testnet3>,
        execution_json: String,
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
