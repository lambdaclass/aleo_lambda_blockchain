use anyhow::{anyhow, Result};
use lib::vm::{self, Program, ProgramID, VerifyingKeyMap};
use log::error;
use rand::thread_rng;
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
        let db_programs = rocksdb::DB::open_default(format!("{}.deployed.db", path))?;

        if !db_programs.key_may_exist("credits.aleo") {
            // Include credits program as a string
            let program_str =
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/aleo/credits.aleo"));
            let mut rng = thread_rng();

            // Compute the 'credits.aleo' program stack.
            let deployment = vm::generate_deployment(program_str, &mut rng)?;
            let credits_program_keys = (
                deployment.program().clone(),
                deployment.verifying_keys().clone(),
            );
            db_programs
                .put(
                    deployment.program_id().to_string().into_bytes(),
                    bincode::serialize(&credits_program_keys)?,
                )
                .unwrap_or_else(|e| error!("failed to write to db {}", e));
        }

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
                        //for some reason `db_programs.key_may_exist(program_id);` does not work
                        let result = !(matches!(
                            db_programs.get(program_id.to_string().as_bytes()),
                            Ok(None)
                        ));
                        reply_to.send(result).unwrap_or_else(|e| error!("{}", e));
                    }
                };
            }
        });
        Ok(Self { command_sender })
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
}

#[cfg(test)]
mod tests {
    use lib::vm::{self, Program};
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
        let program_str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/aleo/credits.aleo"));
        let program = Program::from_str(program_str).unwrap();

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

        program_store.add(
            deployment.program_id(),
            deployment.program(),
            deployment.verifying_keys(),
        )?;

        Ok(deployment)
    }
}
