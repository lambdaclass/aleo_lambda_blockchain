use anyhow::{anyhow, bail, ensure, Result};
use clap::Parser;
use commands::{Account, Command, Get, Program};
use lib::{query::AbciQuery, transaction::Transaction, vm};
use log::debug;
use rand::thread_rng;
use serde_json::json;
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
    #[clap(short, long, global = false, default_value_t = false)]
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

    let (exit_code, output) = match run(cli.command, cli.url).await {
        Ok(output) => (0, output),
        Err(err) => (1, json!({"error": err.to_string()})),
    };
    println!("{output:#}");
    std::process::exit(exit_code);
}

// TODO move to command module
async fn run(command: Command, url: String) -> Result<serde_json::Value> {
    let output = match command {
        Command::Account(Account::New) => {
            let credentials = account::Credentials::new()?;
            let path = credentials.save()?;

            json!({"path": path, "account": credentials})
        }
        Command::Account(Account::Balance) => {
            let credentials =
                account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;

            let records = get_records(credentials.address, credentials.view_key, &url).await?;
            let balance = records
                .iter()
                .fold(0, |acc, (_, _, record)| acc + ***record.gates());

            json!({ "balance": balance })
        }
        Command::Account(Account::Records) => {
            let credentials =
                account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;

            let records = get_records(credentials.address, credentials.view_key, &url).await?;
            let records: Vec<serde_json::Value> = records
                .iter()
                .map(|(commitment, ciphertext, plaintext)| {
                    json!({
                        "commitment": commitment,
                        "ciphertext": ciphertext,
                        "record": plaintext
                    })
                })
                .collect();
            json!(&records)
        }
        Command::Program(Program::Deploy {
            path,
            compile_remotely,
        }) => {
            let transaction = if compile_remotely {
                generate_program(&path)?
            } else {
                generate_deployment(&path)?
            };
            broadcast_to_blockchain(&transaction, &url).await?;
            json!(transaction)
        }
        Command::Program(Program::Execute {
            path,
            function,
            inputs,
        }) => {
            let credentials =
                account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;

            let transaction = generate_execution(&path, function, &inputs, &credentials)?;
            println!("{transaction}");
            broadcast_to_blockchain(&transaction, &url).await?;
            json!(transaction)
        }
        Command::Credits(credits) => {
            let credentials =
                account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;
            let transaction =
                generate_credits_execution(credits.identifier()?, credits.inputs(), &credentials)?;
            broadcast_to_blockchain(&transaction, &url).await?;
            json!(transaction)
        }
        Command::Get(Get {
            transaction_id,
            decrypt,
        }) => {
            let transaction = get_transaction(&transaction_id, &url).await?;

            if !decrypt {
                json!(transaction)
            } else {
                let credentials = account::Credentials::load()?;
                let records: Vec<vm::Record> = transaction
                    .output_records()
                    .iter()
                    .filter(|record| record.is_owner(&credentials.address, &credentials.view_key))
                    .filter_map(|record| record.decrypt(&credentials.view_key).ok())
                    .collect();

                json!({
                    "execution": transaction,
                    "decrypted_records": records
                })
            }
        }
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
    let program_string = fs::read_to_string(path)?;
    debug!("Deploying program {}", program_string);

    let mut rng = thread_rng();

    let program = vm::generate_program(&program_string)?;

    let verifying_keys = vm::generate_verifying_keys(&program, &mut rng)?;

    // using a uuid for txid, just to skip having to use an additional fee record which now is necessary to run
    // Transaction::from_deployment
    let id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Deployment {
        id,
        program: Box::new(program),
        verifying_keys,
    })
}

fn generate_program(path: &Path) -> Result<Transaction> {
    let program_string = fs::read_to_string(path)?;
    debug!("Deploying non-compiled program {}", program_string);

    let program = vm::generate_program(&program_string)?;

    let id = uuid::Uuid::new_v4().to_string();
    Ok(Transaction::Source {
        id,
        program: Box::new(program),
    })
}

fn generate_execution(
    path: &Path,
    function_name: vm::Identifier,
    inputs: &[vm::UserInputValueType],
    credentials: &account::Credentials,
) -> Result<Transaction> {
    let rng = &mut rand::thread_rng();
    let program_string = fs::read_to_string(path)?;

    let transitions = vm::generate_execution(
        &program_string,
        function_name,
        inputs,
        &credentials.private_key,
        rng,
    )?;

    // using uuid here too for consistency, although in the case of Transaction::from_execution the additional fee is optional
    let id = uuid::Uuid::new_v4().to_string();
    Ok(Transaction::Execution { id, transitions })
}

fn generate_credits_execution(
    function_name: vm::Identifier,
    inputs: Vec<vm::UserInputValueType>,
    credentials: &account::Credentials,
) -> Result<Transaction> {
    let rng = &mut rand::thread_rng();

    let transitions = vm::credits_execution(function_name, &inputs, &credentials.private_key, rng)?;

    // using uuid here too for consistency, although in the case of Transaction::from_execution the additional fee is optional
    let id = uuid::Uuid::new_v4().to_string();

    Ok(Transaction::Execution { id, transitions })
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

async fn get_records(
    address: vm::Address,
    view_key: vm::ViewKey,
    url: &str,
) -> Result<Vec<(vm::Field, vm::EncryptedRecord, vm::Record)>> {
    let query = AbciQuery::RecordsUnspentOwned { address, view_key };
    let response = query_blockchain(&query, url).await?;
    let records: Vec<(vm::Field, vm::EncryptedRecord)> = bincode::deserialize(&response)?;
    debug!("Records: {:?}", records);
    let records = records
        .into_iter()
        .map(|(commitment, ciphertext)| {
            let record = ciphertext.decrypt(&view_key).unwrap();
            (commitment, ciphertext, record)
        })
        .collect();
    Ok(records)
}

async fn query_blockchain(query: &AbciQuery, url: &str) -> Result<Vec<u8>> {
    let client = HttpClient::new(url).unwrap();

    let query_serialized = bincode::serialize(&query).unwrap();

    let response = client
        .abci_query(None, query_serialized, None, true)
        .await?;

    debug!("Response from Query: {:?}", response);
    match response.code {
        tendermint::abci::Code::Ok => Ok(response.value),
        tendermint::abci::Code::Err(code) => {
            bail!("Error executing transaction {}: {}", code, response.log)
        }
    }
}
