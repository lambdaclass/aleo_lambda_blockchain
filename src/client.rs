use anyhow::{anyhow, bail, ensure, Result};
use clap::Parser;
use commands::{Account, Command, Get, Program};
use lib::{transaction::Transaction, vm, GetDecryptionResponse};
use log::debug;
use rand::thread_rng;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tendermint_rpc::query::Query;
use tendermint_rpc::{Client, HttpClient, Order};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub mod account;
mod commands;

/// Default tendermint url
const LOCAL_BLOCKCHAIN_URL: &str = "http://127.0.0.1:26657";

#[derive(Debug, Parser)]
#[clap()]
pub struct Cli {
    /// Specify a subcommand.
    #[clap(subcommand)]
    command: Command,

    /// Output log lines to stdout based on the desired log level (RUST_LOG env var).
    #[clap(short, long, global = false)]
    verbose: bool,

    /// tendermint node url
    #[clap(short, long, env = "BLOCKCHAIN_URL", default_value = LOCAL_BLOCKCHAIN_URL)]
    url: String,
}

#[tokio::main()]
async fn main() {
    let cli = Cli::parse();

    if cli.verbose {
        tracing_subscriber::fmt()
            // Use a more compact, abbreviated log format
            .compact()
            .with_env_filter(EnvFilter::from_default_env())
            // Build and init the subscriber
            .finish()
            .init();
    }

    match run(cli.command, cli.url).await {
        Err(err) => {
            let mut output = HashMap::new();
            output.insert("error", err.to_string());
            println!("{}", serde_json::to_string_pretty(&output).unwrap());
            std::process::exit(1);
        }
        Ok(output) => {
            println!("{}", output);
        }
    }
}

// TODO move to command module
async fn run(command: Command, url: String) -> Result<String> {
    let output = match command {
        Command::Account(Account::New) => {
            let path = account::Credentials::new()?.save()?;
            format!("Saved credentials to {}", path.to_string_lossy())
        }
        Command::Program(Program::Deploy { path }) => {
            let transaction = generate_deployment(&path)?;
            broadcast_to_blockchain(&transaction, &url).await?;
            transaction.json()
        }
        Command::Program(Program::Execute {
            path,
            function,
            inputs,
        }) => {
            let credentials =
                account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;

            let transaction = generate_execution(&path, function, &inputs, &credentials)?;
            broadcast_to_blockchain(&transaction, &url).await?;
            transaction.json()
        }
        Command::Get(Get {
            transaction_id,
            decrypt,
        }) => {
            let transaction = get_transaction(&transaction_id, &url).await?;

            if !decrypt {
                transaction.json()
            } else {
                let credentials = account::Credentials::load()?;
                let records = transaction
                    .output_records()
                    .iter()
                    .filter(|record| record.is_owner(&credentials.address, &credentials.view_key))
                    .filter_map(|record| record.decrypt(&credentials.view_key).ok())
                    .collect();

                serde_json::to_string_pretty(&GetDecryptionResponse {
                    execution: transaction,
                    decrypted_records: records,
                })?
            }
        }
        _ => todo!("Command has not been implemented yet"),
    };
    Ok(output)
}

async fn get_transaction(tx_id: &str, url: &str) -> Result<Transaction> {
    let client = HttpClient::new(url)?;
    // todo: this index key might have to be a part of the shared lib so that both the CLI and the ABCI can be in sync
    let query = Query::contains("app.tx_id", tx_id);

    let response = client
        .tx_search(query, false, 1, 1, Order::Ascending)
        .await?;

    // early return with error if no transaction has been indexed for that tx id
    ensure!(
        response.total_count > 0,
        "Transaction ID {} is invalid or has not yet been committed to the blockchain",
        tx_id
    );

    let tx_bytes: Vec<u8> = response.txs.into_iter().next().unwrap().tx.into();
    let transaction: Transaction = bincode::deserialize(&tx_bytes)?;

    Ok(transaction)
}

fn generate_deployment(path: &Path) -> Result<Transaction> {
    let program_string = fs::read_to_string(path).unwrap();
    debug!("Deploying program {}", program_string);

    let mut rng = thread_rng();

    let deployment = vm::generate_deployment(&program_string, &mut rng)?;

    // using a uuid for txid, just to skip having to use an additional fee record which now is necessary to run
    // Transaction::from_deployment
    let id = uuid::Uuid::new_v4().to_string();
    Ok(Transaction::Deployment {
        id,
        deployment: Box::new(deployment),
    })
}

fn generate_execution(
    path: &Path,
    function_name: vm::Identifier,
    inputs: &[vm::Value],
    credentials: &account::Credentials,
) -> Result<Transaction> {
    let rng = &mut rand::thread_rng();
    let program_string = fs::read_to_string(path).unwrap();

    let execution = vm::generate_execution(
        &program_string,
        function_name,
        inputs,
        &credentials.private_key,
        rng,
    )?;

    // using uuid here too for consistency, although in the case of Transaction::from_execution the additional fee is optional
    let id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Execution { id, execution })
}

async fn broadcast_to_blockchain(transaction: &Transaction, url: &str) -> Result<()> {
    let transaction_serialized = bincode::serialize(&transaction).unwrap();

    let client = HttpClient::new(url).unwrap();

    let tx: tendermint::abci::Transaction = transaction_serialized.into();

    let response = client.broadcast_tx_sync(tx).await?;

    debug!("Response from CheckTx: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(()),
        tendermint::abci::Code::Err(code) => {
            bail!("Error executing transaction {}: {}", code, response.log)
        }
    }
}
