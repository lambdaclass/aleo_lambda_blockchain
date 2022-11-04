
use std::{
    sync::mpsc::{channel, Receiver, Sender},
};

use bytes::BytesMut;
use tendermint_proto::abci::{
    Event, EventAttribute, RequestCheckTx, RequestDeliverTx, RequestInfo, RequestQuery,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseInfo, ResponseQuery,
};

use tendermint_abci::Application;

// NOTE: in the sample app, the following const was defined on one of the internal libraries, but was not `pub`
// so we had to extract it here

/// The maximum number of bytes we expect in a varint. We use this to check if
/// we're encountering a decoding error for a varint.
pub const MAX_VARINT_LENGTH: usize = 16;

use tracing::{debug, info};

use tendermint_abci::Error;

/// In-memory, hashmap-backed key/value store ABCI application.
///
/// This structure effectively just serves as a handle to the actual key/value
/// store - the [`KeyValueStoreDriver`].
///
/// ## Example
/// ```rust
/// use tendermint_abci::{KeyValueStoreApp, ServerBuilder, ClientBuilder};
/// use tendermint_proto::abci::{RequestEcho, RequestDeliverTx, RequestQuery};
///
/// // Create our key/value store application
/// let (app, driver) = KeyValueStoreApp::new();
/// // Create our server, binding it to TCP port 26658 on localhost and
/// // supplying it with our key/value store application
/// let server = ServerBuilder::default().bind("127.0.0.1:26658", app).unwrap();
/// let server_addr = server.local_addr();
///
/// // We want the driver and the server to run in the background while we
/// // interact with them via the client in the foreground
/// std::thread::spawn(move || driver.run());
/// std::thread::spawn(move || server.listen());
///
/// let mut client = ClientBuilder::default().connect(server_addr).unwrap();
/// let res = client
///     .echo(RequestEcho {
///         message: "Hello ABCI!".to_string(),
///     })
///     .unwrap();
/// assert_eq!(res.message, "Hello ABCI!");
///
/// // Deliver a transaction and then commit the transaction
/// client
///     .deliver_tx(RequestDeliverTx {
///         tx: "test-key=test-value".into(),
///     })
///     .unwrap();
/// client.commit().unwrap();
///
/// // We should be able to query for the data we just delivered above
/// let res = client
///     .query(RequestQuery {
///         data: "test-key".into(),
///         path: "".to_string(),
///         height: 0,
///         prove: false,
///     })
///     .unwrap();
/// assert_eq!(res.value, "test-value".as_bytes().to_owned());
/// ```
#[derive(Debug, Clone)]
pub struct RocksDbKeyValueStoreApp {
    cmd_tx: Sender<Command>,
}

impl RocksDbKeyValueStoreApp {
    /// Constructor.
    pub fn new() -> (Self, RocksDBKeyValueStoreDriver) {
        let (cmd_tx, cmd_rx) = channel();
        (Self { cmd_tx }, RocksDBKeyValueStoreDriver::new(cmd_rx))
    }

    /// Attempt to retrieve the value associated with the given key.
    pub fn get<K: AsRef<str>>(&self, key: K) -> Result<(i64, Option<String>), Error> {
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::Get {
                key: key.as_ref().to_string(),
                result_tx,
            },
        )?;
        channel_recv(&result_rx)
    }

    /// Attempt to set the value associated with the given key.
    ///
    /// Optionally returns any pre-existing value associated with the given
    /// key.
    pub fn set<K, V>(&self, key: K, value: V) -> Result<Option<String>, Error>
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let (result_tx, result_rx) = channel();
        channel_send(
            &self.cmd_tx,
            Command::Set {
                key: key.as_ref().to_string(),
                value: value.as_ref().to_string(),
                result_tx,
            },
        )?;
        channel_recv(&result_rx)
    }
}

impl Application for RocksDbKeyValueStoreApp {
    fn info(&self, request: RequestInfo) -> ResponseInfo {
        debug!(
            "Got info request. Tendermint version: {}; Block version: {}; P2P version: {}",
            request.version, request.block_version, request.p2p_version
        );

        let (result_tx, result_rx) = channel();
        channel_send(&self.cmd_tx, Command::GetInfo { result_tx }).unwrap();
        let (last_block_height, last_block_app_hash) = channel_recv(&result_rx).unwrap();

        ResponseInfo {
            data: "kvstore-rs".to_string(),
            version: "0.1.0".to_string(),
            app_version: 1,
            last_block_height,
            last_block_app_hash: last_block_app_hash.into(),
        }
    }

    fn query(&self, request: RequestQuery) -> ResponseQuery {
        let key = match std::str::from_utf8(&request.data) {
            Ok(s) => s,
            Err(e) => panic!("Failed to intepret key as UTF-8: {}", e),
        };
        debug!("Attempting to get key: {}", key);
        match self.get(key) {
            Ok((height, value_opt)) => match value_opt {
                Some(value) => ResponseQuery {
                    code: 0,
                    log: "exists".to_string(),
                    info: "".to_string(),
                    index: 0,
                    key: request.data,
                    value: value.into_bytes().into(),
                    proof_ops: None,
                    height,
                    codespace: "".to_string(),
                },
                None => ResponseQuery {
                    code: 0,
                    log: "does not exist".to_string(),
                    info: "".to_string(),
                    index: 0,
                    key: request.data,
                    value: Default::default(),
                    proof_ops: None,
                    height,
                    codespace: "".to_string(),
                },
            },
            Err(e) => panic!("Failed to get key \"{}\": {:?}", key, e),
        }
    }

    // ref: https://github.com/tendermint/tendermint/blob/v0.34.x/spec/abci/abci.md
    fn check_tx(&self, _request: RequestCheckTx) -> ResponseCheckTx {
        ResponseCheckTx {
            code: 0,
            data: Default::default(),
            log: "".to_string(),
            info: "".to_string(),
            gas_wanted: 1,
            gas_used: 0,
            events: vec![],
            codespace: "".to_string(),
            ..Default::default()
        }
    }

    fn deliver_tx(&self, request: RequestDeliverTx) -> ResponseDeliverTx {
        let tx = std::str::from_utf8(&request.tx).unwrap();
        let tx_parts = tx.split('=').collect::<Vec<&str>>();
        let (key, value) = if tx_parts.len() == 2 {
            (tx_parts[0], tx_parts[1])
        } else {
            (tx, tx)
        };
        let _ = self.set(key, value).unwrap();

        ResponseDeliverTx {
            code: 0,
            data: Default::default(),
            log: "".to_string(),
            info: "".to_string(),
            gas_wanted: 0,
            gas_used: 0,
            events: vec![Event {
                r#type: "app".to_string(),
                attributes: vec![
                    EventAttribute {
                        key: "key".to_string().into_bytes().into(),
                        value: key.to_string().into_bytes().into(),
                        index: true
                    },
                    EventAttribute {
                        key: "index_key".to_string().into_bytes().into(),
                        value: "index is working".to_string().into_bytes().into(),
                        index: true,
                    },
                    EventAttribute {
                        key: "noindex_key".to_string().into_bytes().into(),
                        value: "index is working".to_string().into_bytes().into(),
                        index: false,
                    },
                ],
            }],
            codespace: "".to_string(),
        }
    }

    fn commit(&self) -> ResponseCommit {
        let (result_tx, result_rx) = channel();
        channel_send(&self.cmd_tx, Command::Commit { result_tx }).unwrap();
        let (height, app_hash) = channel_recv(&result_rx).unwrap();
        info!("Committed height {}", height);
        ResponseCommit {
            data: app_hash.into(),
            retain_height: height - 1,
        }
    }
}

/// Manages key/value store state.
#[derive(Debug)]
pub struct RocksDBKeyValueStoreDriver {
    app_hash: Vec<u8>,
    cmd_rx: Receiver<Command>,
}

const BLOCK_HEIGHT_KEY: &[u8] = b"__max_block_height"; 

impl RocksDBKeyValueStoreDriver {
    fn new(cmd_rx: Receiver<Command>) -> Self {
        let db = rocksdb::DB::open_default(".db_abci").unwrap();

        let db_block_height = db.get(BLOCK_HEIGHT_KEY).unwrap();
        
        // if no block height is found, the database is probably new, 
        // so we have to insert the key in order for the code to use it

        let last_block_height = if db_block_height.is_none() {
            let _ = db.put(BLOCK_HEIGHT_KEY, "0");
            "0".to_string()
        } else {
            String::from_utf8(db_block_height.unwrap()).unwrap()
        };

        // create the app hash based on the last block height found

        Self {
            app_hash: Self::compute_app_hash(str::parse(&last_block_height).unwrap()),
            cmd_rx,
        }
    }

    /// Run the driver in the current thread (blocking).
    pub fn run(mut self) -> Result<(), Error> {
        let db = rocksdb::DB::open_default(".db_abci").unwrap();

        loop {
            let cmd = self.cmd_rx.recv().map_err(Error::channel_recv)?;

            match cmd {
                Command::GetInfo { result_tx } => {
                    let block_height_str = String::from_utf8(db.get(BLOCK_HEIGHT_KEY).unwrap().unwrap()).unwrap();
                    channel_send(&result_tx, (block_height_str.parse::<i64>().unwrap(), self.app_hash.clone()))?
                }
                Command::Get { key, result_tx } => {
                    debug!("Getting value for \"{}\"", key);

                    // very hacky (unwrappy) code
                    // get value from KV store and send it, along with the block height
                    let v = db.get(key).unwrap().map(|x| String::from_utf8(x).unwrap());
                    let block_height_str = String::from_utf8(db.get(BLOCK_HEIGHT_KEY).unwrap().unwrap()).unwrap();

                    channel_send(
                        &result_tx,
                        (block_height_str.parse::<i64>().unwrap(), v),
                    )?;
                }
                Command::Set {
                    key,
                    value,
                    result_tx,
                } => {
                    debug!("Setting \"{}\" = \"{}\"", key, value);
                    let res = db.put(key, &value).and(Ok(Some(value))).unwrap();
                    channel_send(&result_tx, res)?;
                }
                Command::Commit { result_tx } =>{ // executed with deliver_tx
                    // get last block height from db and increase it since we are committing the transaction
                    let block_height_str = String::from_utf8(db.get(BLOCK_HEIGHT_KEY).unwrap().unwrap()).unwrap();
                    let new_block_height = str::parse::<i64>(&block_height_str).unwrap() + 1;

                    // commit the new block height to the rocks DB store
                    let _ = db.put(BLOCK_HEIGHT_KEY, &format!("{}", new_block_height));

                    self.commit(result_tx,new_block_height)?
                },
            }
        }
    }

    fn commit(&mut self, result_tx: Sender<(i64, Vec<u8>)>, new_block_height: i64) -> Result<(), Error> {
        // encode the block height as the app hash, which might not be very representative of the state
        // in the original app, the app hash was the number of items in the set, not the block height
        let mut app_hash = BytesMut::with_capacity(MAX_VARINT_LENGTH);
        prost::encoding::encode_varint(new_block_height as u64, &mut app_hash);
        self.app_hash = Self::compute_app_hash(new_block_height);

        channel_send(&result_tx, (new_block_height, self.app_hash.clone()))
    }

    fn compute_app_hash(block_height: i64) -> Vec<u8> {
        let mut app_hash = BytesMut::with_capacity(MAX_VARINT_LENGTH);
        prost::encoding::encode_varint(block_height as u64, &mut app_hash);
        
        app_hash.to_vec()
    }
}

#[derive(Debug, Clone)]
enum Command {
    /// Get the height of the last commit.
    GetInfo { result_tx: Sender<(i64, Vec<u8>)> },
    /// Get the key associated with `key`.
    Get {
        key: String,
        result_tx: Sender<(i64, Option<String>)>,
    },
    /// Set the value of `key` to to `value`.
    Set {
        key: String,
        value: String,
        result_tx: Sender<Option<String>>,
    },
    /// Commit the current state of the application, which involves recomputing
    /// the application's hash.
    Commit { result_tx: Sender<(i64, Vec<u8>)> },
}

fn channel_send<T>(tx: &Sender<T>, value: T) -> Result<(), Error> {
    tx.send(value).map_err(Error::send)
}

fn channel_recv<T>(rx: &Receiver<T>) -> Result<T, Error> {
    rx.recv().map_err(Error::channel_recv)
}
