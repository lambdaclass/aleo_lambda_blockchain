use anyhow::{anyhow, Result};
use lib::vm::{self, EncryptedRecord, Field};
use log::error;
use rocksdb::{Direction, IteratorMode, WriteBatch};
use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::thread;

// because both serial numbers and Commitments are really fields, define types to differentiate them
type SerialNumber = Field;
type Commitment = Field;

// TODO: Key and Value types should be concrete types instead of serialized data like in
// program store, so that type errors bubble up asap (ie from the interaction with the DB)
type Key = Vec<u8>;
type Value = Vec<u8>;

/// Internal channel reply for the scan command
type ScanReply = (Vec<(Key, Value)>, Option<Key>);
/// Public return type for the scan command.
type ScanResult = (Vec<(Commitment, vm::EncryptedRecord)>, Option<SerialNumber>);

/// The record store tracks the known unspent and spent record sets (similar to bitcoin's UTXO set)
/// according to the transactions that are committed to the ledger.
/// Because of how Tendermint ABCI applications are structured, this store is prepared to buffer
/// updates (new unspent record additions and spending of known records) while transactions are being
/// processed, and apply them together when the block is committed.
#[derive(Clone, Debug)]
pub struct RecordStore {
    /// Channel used to send operations to the task that manages the store state.
    command_sender: Sender<Command>,
}

#[derive(Debug)]
enum Command {
    Add(Key, Value, SyncSender<Result<()>>),
    Spend(Key, SyncSender<Result<()>>),
    IsUnspent(Key, SyncSender<bool>),
    Commit,
    ScanSpentRecords(SyncSender<HashSet<SerialNumber>>),
    ScanRecords {
        from: Option<Key>,
        limit: Option<usize>,
        reply_sender: SyncSender<ScanReply>,
    },
}

impl RecordStore {
    /// Start a new record store on a new thread
    pub fn new(path: &str) -> Result<Self> {
        // TODO review column families, may be a more natural way to separate spent/unspent on the same db and still get the benefits
        // https://github.com/EighteenZi/rocksdb_wiki/blob/master/Column-Families.md
        // we may also like to try something other than rocksdb here, e.g. sqlite

        // TODO: comment on this
        let db_records = rocksdb::DB::open_default(format!("{path}.records.db"))?;

        // DB to track spent record serial_numbers. These are tracked to ensure that records aren't spent more than once
        // (without having to _know_ the actual record contents).
        let db_spent = rocksdb::DB::open_default(format!("{path}.spent.db"))?;

        // map to store temporary unspent record additions until a block is comitted.
        let mut record_buffer = HashMap::new();

        // map to store temporary spent record additions until a block is comitted.
        let mut spent_buffer = HashMap::new();

        let (command_sender, command_receiver): (Sender<Command>, Receiver<Command>) = channel();

        thread::spawn(move || {
            while let Ok(command) = command_receiver.recv() {
                match command {
                    Command::Add(commitment, ciphertext, reply_to) => {
                        // TODO: Remove/change this into something secure (merkle path to valid records exists)
                        // Because tracking existence and spent status leads to security concerns, existence of records will
                        // have to be proven by the execution. Until this is implemented, return Ok by default here and assume the record exists.
                        let result = if record_buffer.contains_key(&commitment)
                            || key_exists_or_fails(&db_records, &commitment)
                        {
                            Err(anyhow!(
                                "record {} already exists",
                                String::from_utf8_lossy(&commitment)
                            ))
                        } else {
                            record_buffer.insert(commitment, ciphertext);
                            Ok(())
                        };
                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::Spend(serial_number, reply_to) => {
                        // TODO: [related to above] implement record existence check and handle case where it exists and it doesn't
                        let result = if key_exists_or_fails(&db_spent, &serial_number)
                            || spent_buffer.contains_key(&serial_number)
                        {
                            Err(anyhow!("record already spent"))
                        } else {
                            spent_buffer.insert(serial_number, "1".as_bytes());
                            Ok(())
                        };

                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::IsUnspent(serial_number, reply_to) => {
                        // TODO: [related to above] handle record existence scenarios
                        let is_unspent = !key_exists_or_fails(&db_spent, &serial_number)
                            && !spent_buffer.contains_key(&serial_number);
                        reply_to
                            .send(is_unspent)
                            .unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::Commit => {
                        // add new records to store
                        let mut batch = WriteBatch::default();
                        for (key, value) in record_buffer.iter() {
                            batch.put(key, value);
                        }
                        db_records
                            .write(batch)
                            .unwrap_or_else(|e| error!("failed to write to db {}", e));

                        // add all buffer spent to db spent, i.e. persisted consumed records (as a serial number for security)
                        let mut batch = WriteBatch::default();
                        for (key, value) in spent_buffer.iter() {
                            batch.put(key.clone(), value);
                        }

                        db_spent
                            .write(batch)
                            .unwrap_or_else(|e| error!("failed to write to db {}", e));

                        // remove all buffer spent from db unspent, i.e. consumed records should only be kept in spent db
                        let mut batch = WriteBatch::default();
                        for key in spent_buffer.keys() {
                            batch.delete(key);
                        }
                        spent_buffer.clear();
                    }
                    Command::ScanRecords {
                        from,
                        limit,
                        reply_sender: reply_to,
                    } => {
                        let iterator_mode = from.as_ref().map_or(IteratorMode::Start, |key| {
                            IteratorMode::From(key, Direction::Forward)
                        });
                        let mut records = vec![];
                        let mut last_key = None;
                        for item in db_records.iterator(iterator_mode) {
                            if limit.map_or(false, |l| records.len() >= l) {
                                break;
                            }
                            if let Ok((key, record)) = item {
                                records.push((key.to_vec(), record.to_vec()));
                                last_key = Some(key.to_vec());
                            }
                        }
                        reply_to
                            .send((records, last_key))
                            .unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::ScanSpentRecords(reply_sender) => {
                        let spent_records = db_spent
                            .iterator(IteratorMode::Start)
                            .filter_map(|s| {
                                s.map(|(k, _)| {
                                    SerialNumber::from_str(&String::from_utf8_lossy(&k)).unwrap()
                                })
                                .ok()
                            })
                            .collect();
                        reply_sender
                            .send(spent_records)
                            .unwrap_or_else(|e| error!("{}", e));
                    }
                };
            }
        });
        Ok(Self { command_sender })
    }

    /// Saves a new unspent record to the write buffer
    #[allow(clippy::redundant_clone)] // commitments/serial numbers are strings on VMTropy and so clippy generates a warning for `.to_string()`
    pub fn add(&self, commitment: Commitment, record: vm::EncryptedRecord) -> Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        let commitment = commitment.to_string().into_bytes();
        let ciphertext = record.to_string().into_bytes();

        self.command_sender
            .send(Command::Add(commitment, ciphertext, reply_sender))?;
        reply_receiver.recv()?
    }

    /// Marks a record as spent in the write buffer.
    /// Fails if the record is not found or was already spent.
    pub fn spend(&self, serial_number: &SerialNumber) -> Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        let serial_number = serial_number.to_string().into_bytes();
        self.command_sender
            .send(Command::Spend(serial_number, reply_sender))?;
        reply_receiver.recv()?
    }

    /// Commit write buffer changes to persistent storage and empty the buffer.
    pub fn commit(&self) -> Result<()> {
        Ok(self.command_sender.send(Command::Commit)?)
    }

    /// Returns whether a record by the given serial_number is known and not spent
    pub fn is_unspent(&self, serial_number: &SerialNumber) -> Result<bool> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        let serial_number = serial_number.to_string().into_bytes();
        self.command_sender
            .send(Command::IsUnspent(serial_number, reply_sender))?;
        Ok(reply_receiver.recv()?)
    }

    /// Return up to `limit` record ciphertexts
    #[allow(clippy::redundant_clone)] // commitments/serial numbers are strings on VMTropy and so clippy generates a warning for `.to_string()`
    pub fn scan(&self, from: Option<SerialNumber>, limit: Option<usize>) -> Result<ScanResult> {
        let from = from.map(|commitment| commitment.to_string().into_bytes());
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender.send(Command::ScanRecords {
            from,
            limit,
            reply_sender,
        })?;

        let (results, last_key) = reply_receiver.recv()?;
        let last_key = last_key
            .map(|commitment| Commitment::from_str(&String::from_utf8_lossy(&commitment)).unwrap());
        let results = results
            .iter()
            .map(|(commitment, record)| {
                let commitment =
                    Commitment::from_str(&String::from_utf8_lossy(commitment)).unwrap();

                let record = EncryptedRecord::from_str(&String::from_utf8_lossy(record)).unwrap();

                (commitment, record)
            })
            .collect();
        Ok((results, last_key))
    }

    // TODO: implement way of limiting response size/count or optimization for better scaling
    /// Return all serial numbers
    pub fn scan_spent(&self) -> Result<HashSet<SerialNumber>> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender
            .send(Command::ScanSpentRecords(reply_sender))?;

        let results = reply_receiver.recv()?;
        Ok(results)
    }
}

/// TODO explain the need for this
fn key_exists_or_fails(db: &rocksdb::DB, key: &Key) -> bool {
    !matches!(db.get(key), Ok(None))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    #[allow(unused_imports)]
    use indexmap::IndexMap;
    #[allow(unused_imports)]
    use lib::vm::{compute_serial_number, PrivateKey, Record, ViewKey};

    #[ctor::ctor]
    fn init() {
        simple_logger::SimpleLogger::new()
            .env()
            .with_level(log::LevelFilter::Info)
            .init()
            .unwrap();

        fs::remove_dir_all(db_path("")).unwrap_or_default();
    }

    fn db_path(suffix: &str) -> String {
        format!(".db_test/{suffix}")
    }

    #[test]
    fn add_and_spend_record() {
        let store = RecordStore::new(&db_path("records1")).unwrap();
        let (record, commitment, serial_number) = new_record();
        store.add(commitment, record).unwrap();
        assert!(store.is_unspent(&serial_number).unwrap());
        store.commit().unwrap();
        assert!(store.is_unspent(&serial_number).unwrap());
        store.spend(&serial_number).unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());
        store.commit().unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());

        let msg = store
            .spend(&serial_number)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn no_double_add_record() {
        let store = RecordStore::new(&db_path("records2")).unwrap();

        let (record, commitment, _) = new_record();
        store.add(commitment.clone(), record.clone()).unwrap();
        let msg = store
            .add(commitment.clone(), record)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!(format!("record {commitment} already exists"), msg);
        store.commit().unwrap();

        let (record, commitment, _) = new_record();
        store.add(commitment.clone(), record.clone()).unwrap();
        store.commit().unwrap();
        let msg = store
            .add(commitment.clone(), record)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!(format!("record {commitment} already exists"), msg);

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    fn spend_before_commit() {
        let store = RecordStore::new(&db_path("records3")).unwrap();

        let (record, commitment, serial_number) = new_record();
        store.add(commitment, record).unwrap();
        assert!(store.is_unspent(&serial_number).unwrap());
        store.spend(&serial_number).unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());
        store.commit().unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    fn no_double_spend_record() {
        let store = RecordStore::new(&db_path("records4")).unwrap();

        // add, commit, spend, commit, fail spend
        let (record, commitment, serial_number) = new_record();
        store.add(commitment, record).unwrap();
        store.commit().unwrap();
        assert!(store.is_unspent(&serial_number).unwrap());
        store.spend(&serial_number).unwrap();
        store.commit().unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());
        let msg = store
            .spend(&serial_number)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);

        // add, commit, spend, fail spend, commit, fail spend
        let (record, commitment, serial_number) = new_record();
        store.add(commitment, record).unwrap();
        store.commit().unwrap();
        assert!(store.is_unspent(&serial_number).unwrap());
        store.spend(&serial_number).unwrap();
        let msg = store
            .spend(&serial_number)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);
        store.commit().unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());
        let msg = store
            .spend(&serial_number)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);

        // add, spend, fail spend, commit
        let (record, commitment, serial_number) = new_record();
        store.add(commitment, record).unwrap();
        store.spend(&serial_number).unwrap();
        let msg = store
            .spend(&serial_number)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);
        store.commit().unwrap();
        assert!(!store.is_unspent(&serial_number).unwrap());

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    // TODO: (check if it's possible) make a test for validating behavior related to spending a non-existant record

    #[cfg(feature = "vmtropy_backend")]
    fn new_record() -> (EncryptedRecord, Commitment, SerialNumber) {
        use vmtropy::snarkvm::prelude::{Scalar, Uniform};

        let address =
            String::from("aleo1330ghze6tqvc0s9vd43mnetxlnyfypgf6rw597gn4723lp2wt5gqfk09ry");
        let mut record = Record::new_from_aleo_address(address, 5, IndexMap::new(), None);

        let private_key = PrivateKey::new(&mut rand::thread_rng()).unwrap();
        let rng = &mut rand::thread_rng();
        let randomizer = Scalar::rand(rng);
        let record_ciphertext = record.encrypt(randomizer).unwrap();
        let commitment = record.commitment().unwrap();
        let serial_number = compute_serial_number(private_key, commitment.clone()).unwrap();

        (record_ciphertext, commitment, serial_number)
    }

    #[cfg(feature = "snarkvm_backend")]
    fn new_record() -> (EncryptedRecord, Commitment, SerialNumber) {
        use lib::vm::{Identifier, ProgramID};
        use snarkvm::prelude::{Network, Testnet3, Uniform};

        let rng = &mut rand::thread_rng();
        let randomizer = Uniform::rand(rng);
        let nonce = Testnet3::g_scalar_multiply(&randomizer);
        let record = lib::vm::Record::from_str(
            &format!("{{ owner: aleo1330ghze6tqvc0s9vd43mnetxlnyfypgf6rw597gn4723lp2wt5gqfk09ry.private, gates: 5u64.private, token_amount: 100u64.private, _nonce: {nonce}.public }}"),
        ).unwrap();
        let program_id = ProgramID::from_str("foo.aleo").unwrap();
        let name = Identifier::from_str("bar").unwrap();
        let commitment = record.to_commitment(&program_id, &name).unwrap();
        let record_ciphertext = record.encrypt(randomizer).unwrap();

        // compute serial number to check for spending status
        let pk =
            PrivateKey::from_str("APrivateKey1zkpCT3zCj49nmVoeBXa21EGLjTUc7AKAcMNKLXzP7kc4cgx")
                .unwrap();
        let serial_number = vm::compute_serial_number(pk, commitment).unwrap();

        (record_ciphertext, commitment, serial_number)
    }
}
