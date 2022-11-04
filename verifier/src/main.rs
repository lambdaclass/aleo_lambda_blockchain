use anyhow::Result;
use bytes::Bytes;
use futures::sink::SinkExt as _;
use lib::network::{MessageHandler, Receiver, Writer};
use log::{error, info};
use serde::Deserialize;
use snarkvm::{
    circuit::AleoV0,
    prelude::{Deployment, Process, ProgramMemory, ProgramStore},
};
use snarkvm::{prelude::Execution, prelude::Testnet3};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

// FIXME this will be duplicated for now, put in shared location
#[derive(Deserialize, Debug)]
enum Transaction {
    Deployment(String, Deployment<Testnet3>),
    Execution(String, Execution<Testnet3>),
}

use async_trait::async_trait;

#[derive(Clone)]
struct VerifierHandler {
    // FIXME use channels instead and bla bla bla
    // this is sort of a global map of state. in SnarkVM is contained by a VM struct inside the ledger
    // may not be a good way to track the state of the world
    process: Arc<Mutex<Process<Testnet3>>>,

    // afaict we only keep this around because finalize needs it
    store: ProgramStore<Testnet3, ProgramMemory<Testnet3>>,
}

#[async_trait]
impl MessageHandler for VerifierHandler {
    async fn dispatch(&mut self, writer: &mut Writer, message: Bytes) -> Result<()> {
        // Reply with an ACK.
        let _ = writer.send(Bytes::from("Ack")).await;

        // Deserialize the message.
        match bincode::deserialize::<Transaction>(&message)? {
            Transaction::Deployment(transaction_id, deployment) => {
                let program = deployment.program_id();
                info!(
                    "Received deployment txid {} program {}",
                    transaction_id, program
                );

                let rng = &mut rand::thread_rng();
                let mut process = self.process.lock().unwrap();
                match process.verify_deployment::<AleoV0, _>(&deployment, rng) {
                    Err(_) => error!("Deployment verification failed"),
                    _ => info!("Deployment verification successful, storing program"),
                }

                // we run finalize to save the program in the process for later execute verification
                // it's not clear that we're interested in the store here, but it's required for that function
                // note we could've use process.load_deployment instead but that one is private
                process.finalize_deployment(&self.store, &deployment)?;

                // TODO store the transaction in the ledger
            }
            Transaction::Execution(transaction_id, execution) => {
                let transition = execution.peek().unwrap();
                let program = transition.program_id();
                info!(
                    "Received execution txid {} program {}",
                    transaction_id, program
                );

                match self.process.lock().unwrap().verify_execution(&execution) {
                    Err(_) => error!("Execution verification failed"),
                    _ => info!("Execution verification successful"),
                }

                // there is a finalize execution but it's not clear that we actually need it

                // TODO store the transaction in the ledger
            }
        }
        Ok(())
    }
}

#[tokio::main()]
async fn main() -> Result<()> {
    let address = "127.0.0.1:6200".parse::<SocketAddr>().unwrap();
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let process = Arc::new(Mutex::new(Process::load()?));
    // accept the mystery
    let store = ProgramStore::<_, ProgramMemory<_>>::open(None).unwrap();
    let receiver = Receiver::new(address, VerifierHandler { process, store });
    receiver.run().await;

    Ok(())
}
