use anyhow::{anyhow, Result};
use lib::vm::{Program, ProgramID, VerifyingKey, VerifyingKeyMap};
use log::{debug, error};
use snarkvm::parameters;
use snarkvm::prelude::FromBytes;
use std::str::FromStr;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::thread;

pub type StoredProgram = (Program, VerifyingKeyMap);

type Key = ProgramID;
type Value = StoredProgram;

/// The program store tracks programs that have been deployed to the OS
#[derive(Clone, Debug)]
pub struct ProgramStore {
    /// Channel used to send operations to the task that manages the store state.
    command_sender: Sender<Command>,
}

#[derive(Debug)]
enum Command {
    Add(Key, Box<Value>, SyncSender<Result<()>>),
    Get(Key, SyncSender<Result<Option<Value>>>),
    Exists(Key, SyncSender<bool>),
}

impl ProgramStore {
    /// Start a new record store on a new thread
    pub fn new(path: &str) -> Result<Self> {
        let db_programs = rocksdb::DB::open_default(format!("{path}.deployed.db"))?;

        let (command_sender, command_receiver): (Sender<Command>, Receiver<Command>) = channel();

        thread::spawn(move || {
            while let Ok(command) = command_receiver.recv() {
                match command {
                    Command::Add(program_id, program_keys, reply_to) => {
                        let result = if db_programs
                            .get(program_id.to_string().as_bytes())
                            .unwrap_or(None)
                            .is_some()
                        {
                            Err(anyhow!(
                                "Program {} already exists in the store",
                                &program_id,
                            ))
                        } else {
                            let program_keys = bincode::serialize(&program_keys);
                            Ok(db_programs
                                .put(program_id.to_string().as_bytes(), program_keys.unwrap())
                                .unwrap_or_else(|e| error!("failed to write to db {}", e)))
                        };

                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::Get(program_id, reply_to) => {
                        let result = db_programs
                            .get(program_id.to_string().as_bytes())
                            .unwrap_or(None)
                            .map(|value| bincode::deserialize::<Value>(&value).unwrap());

                        reply_to
                            .send(Ok(result))
                            .unwrap_or_else(|e| error!("{}", e));
                    }
                    Command::Exists(program_id, reply_to) => {
                        let result = db_programs.key_may_exist(program_id.to_string().as_bytes());
                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                };
            }
        });
        let program_store = Self { command_sender };

        program_store.load_credits()?;
        Ok(program_store)
    }

    /// Returns a program
    pub fn get(&self, program_id: &ProgramID) -> Result<Option<StoredProgram>> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender
            .send(Command::Get(*program_id, reply_sender))?;

        reply_receiver.recv()?
    }

    /// Adds a program to the store
    pub fn add(
        &self,
        program_id: &ProgramID,
        program: &Program,
        verifying_keys: &VerifyingKeyMap,
    ) -> Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender.send(Command::Add(
            *program_id,
            Box::new((program.clone(), verifying_keys.clone())),
            reply_sender,
        ))?;

        reply_receiver.recv()?
    }

    /// Returns whether a program ID is already stored
    pub fn exists(&self, program_id: &ProgramID) -> bool {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender
            .send(Command::Exists(*program_id, reply_sender))
            .unwrap();

        reply_receiver.recv().unwrap_or(false)
    }

    fn load_credits(&self) -> Result<()> {
        let credits_program = Program::credits()?;

        if self.exists(&ProgramID::from_str("credits.aleo")?) {
            debug!("Credits program already exists in program store");
            Ok(())
        } else {
            debug!("Loading credits.aleo as part of Program Store initialization");
            let mut key_map = VerifyingKeyMap::new();

            for function_name in credits_program.functions().keys() {
                let (_, verifying_key) = parameters::testnet3::TESTNET3_CREDITS_PROGRAM
                    .get(&function_name.to_string())
                    .ok_or_else(|| {
                        anyhow!("Circuit keys for credits.aleo/{function_name}' not found")
                    })?;

                let verifying_key = VerifyingKey::from_bytes_le(verifying_key)?;

                key_map.insert(*function_name, verifying_key);
            }

            self.add(credits_program.id(), &credits_program, &key_map)
        }
    }
}

#[cfg(test)]
mod tests {
    use lib::vm::{self, Program};
    use rand::thread_rng;
    use snarkvm::prelude::Testnet3;

    use super::*;
    use std::{fs, str::FromStr};

    #[ctor::ctor]
    fn init() {
        // todo: this fails with error because it's already initialised
        /* simple_logger::SimpleLogger::new()
        .env()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();  */

        fs::remove_dir_all(db_path("")).unwrap_or_default();
    }

    fn db_path(suffix: &str) -> String {
        format!(".db_test/{suffix}")
    }

    #[test]
    fn add_program() {
        let store = ProgramStore::new(&db_path("program")).unwrap();

        let program_path = format!("{}{}", env!("CARGO_MANIFEST_DIR"), "/aleo/hello.aleo");
        let program =
            Program::from_str(fs::read_to_string(program_path).unwrap().as_str()).unwrap();

        let get_program = store.get(program.id());

        assert!(get_program.unwrap().is_none());

        let storage_attempt = store_program(&store, "/aleo/hello.aleo");
        assert!(storage_attempt.is_ok() && store.exists(storage_attempt.unwrap().program_id()));

        // FIXME patching rocksdb weird behavior
        std::mem::forget(store);
    }

    #[test]
    fn credits_loaded() {
        let program = Program::credits().expect("Problem loading Credits");

        {
            let store = rocksdb::DB::open_default(db_path("credits")).unwrap();
            let get_program = store.get(program.id().to_string().into_bytes());
            assert!(get_program.unwrap().is_none());
        }
        let store = ProgramStore::new(&db_path("credits")).unwrap();

        assert!(store.exists(program.id()));
    }

    fn store_program(
        program_store: &ProgramStore,
        path: &str,
    ) -> Result<snarkvm::prelude::Deployment<Testnet3>> {
        let mut rng = thread_rng();
        let program_path = format!("{}{}", env!("CARGO_MANIFEST_DIR"), path);

        let program_string = fs::read_to_string(program_path).unwrap();
        let deployment = vm::generate_deployment(&program_string, &mut rng).unwrap();

        let verifying_keys = vm::get_verifying_key_map(&deployment);

        program_store.add(
            deployment.program_id(),
            deployment.program(),
            &verifying_keys,
        )?;

        Ok(deployment)
    }
}
