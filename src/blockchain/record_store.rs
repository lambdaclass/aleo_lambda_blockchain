use anyhow::{anyhow, Result};
use lib::vm::{EncryptedRecord, Field as Commitment};
use log::error;
use rocksdb::{Direction, IteratorMode, WriteBatch};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::thread;

type Key = Vec<u8>;
type Value = Vec<u8>;

/// Internal channel reply for the scan command
type ScanReply = (Vec<(Key, Value)>, Option<Key>);
/// Public return type for the scan command.
type ScanResult = (Vec<(Commitment, EncryptedRecord)>, Option<Commitment>);

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
    Scan {
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

        // DB to track unspent record commitments and ciphertexts. These come from execution outputs and can be used as inputs in the future.
        let db_unspent = rocksdb::DB::open_default(format!("{path}.unspent.db"))?;

        // DB to track spent record commitments and ciphertexts. These are tracked to ensure that records aren't spent more than once
        // (without having to _know_ the actual record contents).
        let db_spent = rocksdb::DB::open_default(format!("{path}.spent.db"))?;

        // map to store temporary unspent record additions until a block is comitted.
        let mut buffer_unspent = HashMap::new();

        // map to store temporary spent record additions until a block is comitted.
        let mut buffer_spent = HashMap::new();

        let (command_sender, command_receiver): (Sender<Command>, Receiver<Command>) = channel();

        thread::spawn(move || {
            while let Ok(command) = command_receiver.recv() {
                match command {
                    Command::Add(commitment, ciphertext, reply_to) => {
                        let result = if buffer_unspent.contains_key(&commitment)
                            || buffer_spent.contains_key(&commitment)
                            || key_exists_or_fails(&db_unspent, &commitment)
                            || key_exists_or_fails(&db_spent, &commitment)
                        {
                            Err(anyhow!(
                                "record {} already exists",
                                String::from_utf8_lossy(&commitment)
                            ))
                        } else {
                            buffer_unspent.insert(commitment, ciphertext);
                            Ok(())
                        };

                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::Spend(commitment, reply_to) => {
                        let result = if key_exists_or_fails(&db_spent, &commitment)
                            || buffer_spent.contains_key(&commitment)
                        {
                            Err(anyhow!("record already spent"))
                        } else if let Some(value) = buffer_unspent.get(&commitment) {
                            // NOTE: this assumes it's valid to spend an output from an unconfirmed transaction.
                            buffer_spent.insert(commitment, value.clone());
                            Ok(())
                        } else if let Some(value) = db_unspent.get(&commitment).unwrap_or(None) {
                            buffer_spent.insert(commitment, value.clone());
                            Ok(())
                        } else {
                            Err(anyhow!("record doesn't exist"))
                        };

                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::IsUnspent(commitment, reply_to) => {
                        let is_unspent = (key_exists_or_fails(&db_unspent, &commitment)
                            || buffer_unspent.contains_key(&commitment))
                            && !buffer_spent.contains_key(&commitment);
                        reply_to
                            .send(is_unspent)
                            .unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::Commit => {
                        // remove all buffer spent from buffer unspent, i.e. record generated and consumed in the same block
                        for key in buffer_spent.keys() {
                            buffer_unspent.remove(key);
                        }

                        // add all buffer spent to db spent, i.e. persisted consumed records
                        let mut batch = WriteBatch::default();
                        for (key, value) in buffer_spent.iter() {
                            batch.put(key.clone(), value.clone());
                        }
                        db_spent
                            .write(batch)
                            .unwrap_or_else(|e| error!("failed to write to db {}", e));

                        // add remaining buffer unspent to db unspent, i.e. persist available records
                        let mut batch = WriteBatch::default();
                        for (key, value) in buffer_unspent.iter() {
                            batch.put(key, value);
                        }
                        db_unspent
                            .write(batch)
                            .unwrap_or_else(|e| error!("failed to write to db {}", e));

                        // remove all buffer spent from db unspent, i.e. consumed records should only be kept in spent db
                        let mut batch = WriteBatch::default();
                        for key in buffer_spent.keys() {
                            batch.delete(key);
                        }
                        db_unspent
                            .write(batch)
                            .unwrap_or_else(|e| error!("failed to write to db {}", e));
                    }
                    Command::Scan {
                        from,
                        limit,
                        reply_sender: reply_to,
                    } => {
                        let iterator_mode = from.as_ref().map_or(IteratorMode::Start, |key| {
                            IteratorMode::From(key, Direction::Forward)
                        });
                        let mut records = vec![];
                        let mut last_key = None;
                        for item in db_unspent.iterator(iterator_mode) {
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
                };
            }
        });
        Ok(Self { command_sender })
    }

    /// Saves a new unspent record to the write buffer
    pub fn add(&self, commitment: Commitment, record: EncryptedRecord) -> Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        let commitment = commitment.to_string().into_bytes();
        let ciphertext = record.to_string().into_bytes();

        self.command_sender
            .send(Command::Add(commitment, ciphertext, reply_sender))?;
        reply_receiver.recv()?
    }

    /// Marks a record as spent in the write buffer.
    /// Fails if the record is not found or was already spent.
    pub fn spend(&self, commitment: &Commitment) -> Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        let commitment = commitment.to_string().into_bytes();
        self.command_sender
            .send(Command::Spend(commitment, reply_sender))?;
        reply_receiver.recv()?
    }

    /// Commit write buffer changes to persistent storage and empty the buffer.
    pub fn commit(&self) -> Result<()> {
        Ok(self.command_sender.send(Command::Commit)?)
    }

    /// Returns whether a record by the given commitment is known and not spent
    pub fn is_unspent(&self, commitment: &Commitment) -> Result<bool> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        let commitment = commitment.to_string().into_bytes();
        self.command_sender
            .send(Command::IsUnspent(commitment, reply_sender))?;
        Ok(reply_receiver.recv()?)
    }

    /// Given an account view key, return up to `limit` record ciphertexts
    pub fn scan(&self, from: Option<Commitment>, limit: Option<usize>) -> Result<ScanResult> {
        let from = from.map(|commitment| commitment.to_string().into_bytes());
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender.send(Command::Scan {
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
}

/// TODO explain the need for this
fn key_exists_or_fails(db: &rocksdb::DB, key: &Key) -> bool {
    !matches!(db.get(key), Ok(None))
}

#[cfg(test)]
mod tests {
    use snarkvm::prelude::{Identifier, Network, ProgramID, Testnet3, Uniform};

    use super::*;
    use std::{fs, str::FromStr};
    type PublicRecord = lib::vm::Record;

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
        let (record, commitment) = new_record();

        store.add(commitment, record).unwrap();
        assert!(store.is_unspent(&commitment).unwrap());
        store.commit().unwrap();
        assert!(store.is_unspent(&commitment).unwrap());
        store.spend(&commitment).unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());
        store.commit().unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());

        let msg = store
            .spend(&commitment)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    fn no_double_add_record() {
        let store = RecordStore::new(&db_path("records2")).unwrap();

        let (record, commitment) = new_record();
        store.add(commitment, record.clone()).unwrap();
        let msg = store
            .add(commitment, record)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!(format!("record {commitment} already exists"), msg);
        store.commit().unwrap();

        let (record, commitment) = new_record();
        store.add(commitment, record.clone()).unwrap();
        store.commit().unwrap();
        let msg = store
            .add(commitment, record)
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

        let (record, commitment) = new_record();
        store.add(commitment, record).unwrap();
        assert!(store.is_unspent(&commitment).unwrap());
        store.spend(&commitment).unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());
        store.commit().unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    fn no_double_spend_record() {
        let store = RecordStore::new(&db_path("records4")).unwrap();

        // add, commit, spend, commit, fail spend
        let (record, commitment) = new_record();
        store.add(commitment, record).unwrap();
        store.commit().unwrap();
        assert!(store.is_unspent(&commitment).unwrap());
        store.spend(&commitment).unwrap();
        store.commit().unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());
        let msg = store
            .spend(&commitment)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);

        // add, commit, spend, fail spend, commit, fail spend
        let (record, commitment) = new_record();
        store.add(commitment, record).unwrap();
        store.commit().unwrap();
        assert!(store.is_unspent(&commitment).unwrap());
        store.spend(&commitment).unwrap();
        let msg = store
            .spend(&commitment)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);
        store.commit().unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());
        let msg = store
            .spend(&commitment)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);

        // add, spend, fail spend, commit
        let (record, commitment) = new_record();
        store.add(commitment, record).unwrap();
        store.spend(&commitment).unwrap();
        let msg = store
            .spend(&commitment)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record already spent", msg);
        store.commit().unwrap();
        assert!(!store.is_unspent(&commitment).unwrap());

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    fn no_spend_unknown() {
        // create record but don't add it
        let store = RecordStore::new(&db_path("records5")).unwrap();
        let (_record, commitment) = new_record();

        // when it's unknown it's "not unspent"
        assert!(!store.is_unspent(&commitment).unwrap());

        let msg = store
            .spend(&commitment)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert_eq!("record doesn't exist", msg);

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    fn new_record() -> (EncryptedRecord, Commitment) {
        let rng = &mut rand::thread_rng();
        let randomizer = Uniform::rand(rng);
        let nonce = Testnet3::g_scalar_multiply(&randomizer);
        let record = PublicRecord::from_str(
            &format!("{{ owner: aleo1d5hg2z3ma00382pngntdp68e74zv54jdxy249qhaujhks9c72yrs33ddah.private, gates: 5u64.private, token_amount: 100u64.private, _nonce: {nonce}.public }}"),
        ).unwrap();
        let program_id = ProgramID::from_str("foo.aleo").unwrap();
        let name = Identifier::from_str("bar").unwrap();
        let commitment = record.to_commitment(&program_id, &name).unwrap();
        let record_ciphertext = record.encrypt(randomizer).unwrap();
        (record_ciphertext, commitment)
    }
}
