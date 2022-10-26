use anyhow::Result;
use bytes::Bytes;
use std::net::SocketAddr;

use std::{path::Path, str::FromStr};

use futures::sink::SinkExt as _;
use lib::network::{MessageHandler, Receiver, Writer};

use snarkvm::{
    circuit::AleoV0,
    file::VerifierFile,
    package::Package,
    prelude::{Execution, Transaction},
    prelude::{Identifier, Testnet3},
};

use log::debug;

use async_trait::async_trait;

#[derive(Clone)]
struct ExecutionHandler {}

#[async_trait]
impl MessageHandler for ExecutionHandler {
    async fn dispatch(&mut self, writer: &mut Writer, message: Bytes) -> Result<()> {
        // Reply with an ACK.
        let _ = writer.send(Bytes::from("Ack")).await;

        // Deserialize the message.
        let execution: String = bincode::deserialize(&message)?;
        let program_path = Path::new("../hello");
        let function_identifier = FromStr::from_str("hello")?;
        verify_execution(program_path, function_identifier, &execution)?;

        let execution_struct: Execution<Testnet3> = FromStr::from_str(&execution).unwrap();
        let transaction = Transaction::from_execution(execution_struct, None).unwrap();
        debug!("{}", transaction);

        // Here we would insert the transaction into the KV Store
        let _transaction_id = transaction.id().to_string();
        // Put(transaction_id, transaction)

        Ok(())
    }
}

#[tokio::main()]
async fn main() -> Result<()> {
    let address = "127.0.0.1:6200".parse::<SocketAddr>().unwrap();
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let receiver = Receiver::new(address, ExecutionHandler {});
    receiver.run().await;

    Ok(())
}

fn verify_execution(
    path: &Path,
    function: Identifier<Testnet3>,
    execution_json: &str,
) -> Result<()> {
    let execution =
        FromStr::from_str(std::str::from_utf8(execution_json.as_bytes()).unwrap()).unwrap();

    let package: Package<Testnet3> = Package::open(path).unwrap();
    package.build::<AleoV0>(None)?;

    let build_dir = package.build_directory();
    let process = package.get_process()?;

    let verifier = VerifierFile::open(build_dir.as_path(), &function)?;

    let program_id = package.program_id();
    process.insert_verifying_key(program_id, &function, verifier.verifying_key().clone())?;

    process.verify_execution(&execution).unwrap();

    Ok(())
}
