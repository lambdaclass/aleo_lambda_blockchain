use anyhow::{anyhow, Result};
use indexmap::IndexMap;
use lib::vm::{self, VerifyingKeyMap};
use log::{debug, error};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::thread;

pub type StoredProgram = (vm::Program, vm::VerifyingKeyMap);

type Key = vm::ProgramID;
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
    pub fn get(&self, program_id: &vm::ProgramID) -> Result<Option<StoredProgram>> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender
            .send(Command::Get(program_id.to_owned(), reply_sender))?;

        reply_receiver.recv()?
    }

    /// Adds a program to the store
    pub fn add(
        &self,
        program_id: &vm::ProgramID,
        program: &vm::Program,
        verifying_keys: &vm::VerifyingKeyMap,
    ) -> Result<()> {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender.send(Command::Add(
            program_id.to_owned(),
            Box::new((program.clone(), verifying_keys.clone())),
            reply_sender,
        ))?;

        reply_receiver.recv()?
    }

    /// Returns whether a program ID is already stored
    pub fn exists(&self, program_id: &vm::ProgramID) -> bool {
        let (reply_sender, reply_receiver) = sync_channel(0);

        self.command_sender
            .send(Command::Exists(program_id.to_owned(), reply_sender))
            .unwrap();

        reply_receiver.recv().unwrap_or(false)
    }

    fn load_credits(&self) -> Result<()> {
        let (credits_program, _keys) = lib::load_credits();

        if self.exists(&credits_program.id().to_string()) {
            debug!("Credits program already exists in program store");
            Ok(())
        } else {
            debug!("Loading credits.aleo as part of Program Store initialization");
            let mut key_map = IndexMap::new();

            for (function_name, _function) in credits_program.functions() {
                let (_, verifying_key) = vm::get_credits_key(&credits_program, function_name)?;
                key_map.insert(function_name.to_string(), verifying_key);
            }

            #[cfg(feature = "snarkvm_backend")]
            self.add(&credits_program.id().to_string(), &credits_program, key_map)?;

            #[cfg(feature = "vmtropy_backend")]
            self.add(
                &credits_program.id().to_string(),
                &credits_program,
                &VerifyingKeyMap { map: key_map },
            )?;

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lib::vm;
    use lib::vm::Program;
    use std::{fs, str::FromStr};
    use tendermint::signature::Verifier;

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

        let get_program = store.get(&program.id().to_string());

        assert!(get_program.unwrap().is_none());

        let storage_attempt = store_program(&store, "/aleo/hello.aleo");
        assert!(
            storage_attempt.is_ok() && store.exists(&storage_attempt.unwrap().id().to_string())
        );

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

        assert!(store.exists(&program.id().to_string()));
    }

    fn store_program(program_store: &ProgramStore, path: &str) -> Result<vm::Program> {
        let program_path = format!("{}{}", env!("CARGO_MANIFEST_DIR"), path);

        let program_string = fs::read_to_string(program_path).unwrap();

        // generate program keys (proving and verifying) and keep the verifying one for the store
        let (program, program_build) = vm::build_program(&program_string)?;

        let keys = program_build
            .map
            .into_iter()
            .map(|(i, (_, verifying_key))| (i, verifying_key))
            .collect();

        program_store.add(&program.id().to_string(), &program, &keys)?;

        Ok(program)
    }
}
