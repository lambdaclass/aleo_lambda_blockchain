use anyhow::{anyhow, Result};
use clap::Parser;
use lib::Transaction;
use log::info;
use snarkvm::prelude::Process;
use snarkvm::{
    circuit::AleoV0,
    package::Package,
    prelude::Value,
    prelude::{Identifier, Testnet3},
};
use std::path::{Path, PathBuf};
use tendermint_rpc::{Client, HttpClient};

#[derive(Debug, Parser)]
#[clap()]
pub enum Command {
    // user-generated commands
    Deploy {
        #[clap(value_parser)]
        path: PathBuf,
    },

    Run {
        #[clap(value_parser)]
        path: PathBuf,

        #[clap(value_parser)]
        function: Identifier<Testnet3>,

        #[clap(value_parser)]
        inputs: Vec<Value<Testnet3>>,
    },
}

#[derive(Debug, Parser)]
pub struct Cli {
    #[clap(subcommand)]
    command: Command,
}

#[tokio::main()]
async fn main() -> Result<()> {
    simple_logger::SimpleLogger::new().env().init().unwrap();
    let cli = Cli::parse();

    let transaction = match cli.command {
        Command::Deploy { path } => generate_deployment(&path)?,
        Command::Run {
            path,
            function,
            inputs,
        } => generate_execution(&path, function, &inputs)?,
    };

    send_to_blockchain(&transaction).await
}

fn generate_deployment(path: &Path) -> Result<Transaction> {
    let package: Package<Testnet3> = Package::open(path)?;
    let program = package.program();
    let rng = &mut rand::thread_rng();

    info!("Deploying program {}", program);

    // NOTE: we're skipping the part of imported programs
    // https://github.com/Entropy1729/snarkVM/blob/2c4e282df46ed71c809fd4b49738fd78562354ac/vm/package/deploy.rs#L149

    // for some reason a new process is needed, the package current one would fail
    let process = Process::<Testnet3>::load()?;
    let deployment = process.deploy::<AleoV0, _>(program, rng)?;

    // using a uuid for txid, just to skip having to use an additional fee record which now is necessary to run
    // Transaction::from_deployment
    let transaction_id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Deployment(transaction_id, deployment))
}

fn generate_execution(
    path: &Path,
    function: Identifier<Testnet3>,
    inputs: &[Value<Testnet3>],
) -> Result<Transaction> {
    let package: Package<Testnet3> = Package::open(path).unwrap();
    package.build::<AleoV0>(None)?;

    let rng = &mut rand::thread_rng();

    let program = package.program_id();
    info!(
        "executing program {} function {} inputs {:?}",
        program, function, inputs
    );

    let (response, execution) = package.run::<AleoV0, _>(
        None,
        package.manifest_file().development_private_key(),
        function,
        inputs,
        rng,
    )?;

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
