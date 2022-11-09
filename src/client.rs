use anyhow::{anyhow, bail, Result, ensure};
use clap::Parser;
use commands::{Command, Program, Get, Account};
use lib::Transaction;
use log::{info, debug};
use snarkvm::prelude::{Process};
use snarkvm::{
    circuit::AleoV0,
    prelude::Value,
    prelude::{Identifier, Testnet3},
};
use tendermint_rpc::query::{Query};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tendermint_rpc::{Client, HttpClient, Order};

mod account;
mod commands;

const BLOCKCHAIN_URL: &str = "http://127.0.0.1:26657";

#[derive(Debug, Parser)]
#[clap()]
pub struct Cli {
    /// Specify a subcommand.
    #[clap(subcommand)]
    command: Command,

    /// The account credentials file.
    #[clap(short, long, global = true)]
    file: Option<PathBuf>,

    /// Whether to output results as JSON
    #[clap(value_parser, long, short, default_value_t = false)]
    pub json: bool,
}

#[tokio::main()]
async fn main() -> Result<()> {
    simple_logger::SimpleLogger::new().env().init().unwrap();
    let cli = Cli::parse();

    match cli.command {
        Command::Account(Account::New) => {
            let account = account::Credentials::new()?;
            account.save(cli.file)?
        }
        Command::Program(Program::Deploy { path }) => {
            let transaction = generate_deployment(&path)?;
            broadcast_to_blockchain(&transaction).await?;
        }
        Command::Program(Program::Execute {
            path,
            function,
            inputs,
        }) => {
            let credentials = account::Credentials::load(cli.file)
                .map_err(|_| anyhow!("credentials not found"))?;

            let transaction = generate_execution(&path, function, &inputs, &credentials)?;
            broadcast_to_blockchain(&transaction).await?;
        }
        Command::Get(Get { transaction_id }) => {
            let tx = get_transaction(&transaction_id).await?;
            
            if cli.json {
                println!("{}",serde_json::to_string(&tx).unwrap());
            } else {
                println!("{:?}", tx);
            }
        }
        _ => todo!("Command has not been implemented yet"),
    };


    Ok(())
}

async fn get_transaction(tx_id: &str) -> Result<Transaction>{
    let client = HttpClient::new(BLOCKCHAIN_URL)?;
    // todo: this index key might have to be a part of the shared lib so that both the CLI and the ABCI can be in sync
    let query = Query::contains("app.tx_id", tx_id);
    
    let response = client
        .tx_search(query, false, 1, 1, Order::Ascending)
        .await?;

    // early return with error if no transaction has been indexed for that tx id
    ensure!(response.total_count > 0, "Transaction ID is invalid or has not yet been committed to the blockchain");

    let tx_bytes: Vec<u8> = response.txs.into_iter().next().unwrap().tx.into();
    let transaction: Transaction = bincode::deserialize(&tx_bytes)?;

    Ok(transaction)
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
    credentials: &account::Credentials,
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
    let authorization = process.authorize::<AleoV0, _>(
        &credentials.private_key,
        program_id,
        function_name,
        inputs,
        rng,
    )?;
    let (response, execution) = process.execute::<AleoV0, _>(authorization, rng)?;

    debug!("outputs {:?}", response.outputs());

    // using uuid here too for consistency, although in the case of Transaction::from_execution the additional fee is optional
    let transaction_id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Execution(transaction_id, execution))
}

async fn broadcast_to_blockchain(transaction: &Transaction) -> Result<()> {
    let transaction_serialized = bincode::serialize(&transaction).unwrap();

    let client = HttpClient::new(BLOCKCHAIN_URL).unwrap();

    println!("Transaction ID: {}", transaction.id());

    let response = client
        .broadcast_tx_sync(transaction_serialized.into())
        .await?;

    debug!("Response from CheckTx: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(()),
        tendermint::abci::Code::Err(v) => Err(anyhow!(
            "Transaction failed to validate (CheckTx response status code: {})",
            v
        )),
    }
}
