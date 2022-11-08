use anyhow::{anyhow, bail, Result};
use clap::Parser;
use commands::{Command, Program};
use lib::Transaction;
use log::info;
use snarkvm::prelude::{PrivateKey, Process};
use snarkvm::{
    circuit::AleoV0,
    prelude::Value,
    prelude::{Identifier, Testnet3},
};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use tendermint_rpc::{Client, HttpClient};

mod commands;

#[derive(Debug, Parser)]
#[clap()]
pub struct Cli {
    /// Specify a subcommand.
    #[clap(subcommand)]
    pub command: Command,
}

#[tokio::main()]
async fn main() -> Result<()> {
    simple_logger::SimpleLogger::new().env().init().unwrap();
    let cli = Cli::parse();

    let transaction = match cli.command {
        Command::Program(Program::Deploy { path }) => generate_deployment(&path)?,
        Command::Program(Program::Execute {
            path,
            function,
            inputs,
            private_key,
        }) => generate_execution(&path, function, &inputs, &private_key)?,
        _ => todo!("Command has not been implemented yet"),
    };

    send_to_blockchain(&transaction).await
}

fn generate_deployment(path: &Path) -> Result<Transaction> {
    let program_string = fs::read_to_string(path).unwrap();
    let program = snarkvm::prelude::Program::from_str(&program_string).unwrap();

    let rng = &mut rand::thread_rng();

    info!("Deploying program {}", program);

    // NOTE: we're skipping the part of imported programs
    // https://github.com/Entropy1729/snarkVM/blob/2c4e282df46ed71c809fd4b49738fd78562354ac/vm/package/deploy.rs#L149

    // for some reason a new process is needed, the package current one would fail
    let process = Process::<Testnet3>::load()?;
    let deployment = process.deploy::<AleoV0, _>(&program, rng)?;

    // using a uuid for txid, just to skip having to use an additional fee record which now is necessary to run
    // Transaction::from_deployment
    let transaction_id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Deployment(transaction_id, deployment))
}

// TODO move the low level SnarkVM stuff to a helper vm module
fn generate_execution(
    path: &Path,
    function_name: Identifier<Testnet3>,
    inputs: &[Value<Testnet3>],
    private_key: &PrivateKey<Testnet3>,
) -> Result<Transaction> {
    let rng = &mut rand::thread_rng();
    let program_string = fs::read_to_string(path).unwrap();
    let program = snarkvm::prelude::Program::from_str(&program_string).unwrap();
    let program_id = program.id();

    if !program.contains_function(&function_name) {
        bail!("Function '{function_name}' does not exist.")
    }

    let mut process = Process::<Testnet3>::load()?;
    process.add_program(&program).unwrap();

    // Synthesize each proving and verifying key.
    for function_name in program.functions().keys() {
        process.synthesize_key::<AleoV0, _>(program_id, function_name, &mut rand::thread_rng())?
    }

    info!(
        "executing program {} function {} inputs {:?}",
        program, function_name, inputs
    );

    // Execute the circuit.
    let authorization =
        process.authorize::<AleoV0, _>(private_key, program_id, function_name, inputs, rng)?;
    let (response, execution) = process.execute::<AleoV0, _>(authorization, rng)?;

    info!("outputs {:?}", response.outputs());

    // using uuid here too for consistency, although in the case of Transaction::from_execution the additional fee is optional
    let transaction_id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Execution(transaction_id, execution))
}

async fn send_to_blockchain(transaction: &Transaction) -> Result<()> {
    let transaction_serialized = bincode::serialize(&transaction).unwrap();

    let client = HttpClient::new("http://127.0.0.1:26657").unwrap();

    // TODO: transactions should be in a 'codec' module that ABCI app knows about
    //let tx_string = format!("{:?}",transaction);
    //let tx = tx_string.as_bytes().to_owned();

    let response = client
        .broadcast_tx_sync(transaction_serialized.into())
        .await?;

    info!("Response from CheckTx: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(()),
        tendermint::abci::Code::Err(v) => Err(anyhow!(
            "Transaction failed to validate (CheckTx response status code: {})",
            v
        )),
    }
}
