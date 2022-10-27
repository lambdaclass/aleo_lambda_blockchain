use anyhow::{anyhow, Result};
use bytes::Bytes;
use clap::Parser;
use std::net::SocketAddr;

use std::sync::Arc;
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

use tendermint_rpc::{HttpClient, Client};

use log::debug;

use async_trait::async_trait;


#[derive(Debug, Parser)]
struct Cli {
    /// Defines whether the executable should attempt to send (transaction_id, transaction) to the blockchain after verifying it
    #[clap(short, long, default_value_t=false)]
    send_to_blockchain: bool,
}

#[derive(Clone)]
struct ExecutionHandler {
    send_to_blockchain: bool,
}

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
        let transaction_id = transaction.id().to_string();
        // Put(transaction_id, transaction)

        if self.send_to_blockchain {
            return send_to_blockchain(&transaction_id, transaction).await;
        }

        Ok(())
    }
}

#[tokio::main()]
async fn main() -> Result<()> {
    let address = "127.0.0.1:6200".parse::<SocketAddr>().unwrap();
    simple_logger::SimpleLogger::new().env().init().unwrap();

    let cli = Cli::parse();

    let receiver = Receiver::new(address, ExecutionHandler {send_to_blockchain: cli.send_to_blockchain});
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

async fn send_to_blockchain(transaction_id: &str, transaction: Transaction<Testnet3>) -> Result<()> {
    let client = HttpClient::new("http://127.0.0.1:26657")
    .unwrap();

    // sends a transaction to KV ABCI app in the form of transaction_id=transaction
    // TODO: transactions should be in a 'codec' module that ABCI app knows about
    let tx_string = format!("{}={}",transaction_id, transaction);
    let tx = tx_string.as_bytes().to_owned();

    println!("Sending transaction '{}'", tx_string);

    let response = client
        .broadcast_tx_sync(tx.into())
        .await?;

    debug!("Response from CheckTx: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(()),
        tendermint::abci::Code::Err(v) => Err(anyhow!("Transaction failed to validate (CheckTx response status code: {})", v))
    }
}

