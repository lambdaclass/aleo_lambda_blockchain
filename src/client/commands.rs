use crate::{account, tendermint};
use anyhow::{anyhow, Result};
use clap::Parser;
use lib::query::AbciQuery;
use lib::transaction::Transaction;
use lib::vm::{self, EncryptedRecord, Identifier, Record, Value};
use log::debug;
use serde_json::json;
use std::path::PathBuf;
use std::str::FromStr;
use std::vec;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(subcommand)]
    Account(Account),
    #[clap(subcommand)]
    Program(Program),
    #[clap(name = "get")]
    Get(Get),
    #[clap(subcommand)]
    Credits(Credits),
}

impl Command {
    pub async fn run(self, url: String) -> Result<serde_json::Value> {
        let output = if let Command::Account(Account::New) = self {
            let credentials = account::Credentials::new()?;
            let path = credentials.save()?;

            json!({"path": path, "account": credentials})
        } else {
            let credentials =
                account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;

            match self {
                Command::Account(Account::Balance) => {
                    let balance = get_records(credentials.address, credentials.view_key, &url)
                        .await?
                        .iter()
                        .fold(0, |acc, (_, _, record)| acc + ***record.gates());

                    json!({ "balance": balance })
                }
                Command::Account(Account::Records) => {
                    let records: Vec<serde_json::Value> =
                        get_records(credentials.address, credentials.view_key, &url)
                            .await?
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
                        Transaction::from_source(&path)?
                    } else {
                        Transaction::deployment(&path)?
                    };
                    let transaction_serialized = bincode::serialize(&transaction).unwrap();
                    tendermint::broadcast(transaction_serialized, &url).await?;
                    json!(transaction)
                }
                Command::Program(Program::Execute {
                    path,
                    function,
                    inputs,
                }) => {
                    let transaction = Transaction::execution(
                        Some(&path),
                        function,
                        &inputs,
                        &credentials.private_key,
                    )?;
                    let transaction_serialized = bincode::serialize(&transaction).unwrap();
                    tendermint::broadcast(transaction_serialized, &url).await?;
                    json!(transaction)
                }
                Command::Credits(credits) => {
                    let transaction = Transaction::execution(
                        None,
                        credits.identifier()?,
                        &credits.inputs(),
                        &credentials.private_key,
                    )?;
                    let transaction_serialized = bincode::serialize(&transaction).unwrap();
                    tendermint::broadcast(transaction_serialized, &url).await?;
                    json!(transaction)
                }
                Command::Get(Get {
                    transaction_id,
                    decrypt,
                }) => {
                    let transaction = tendermint::get_transaction(&transaction_id, &url).await?;
                    let transaction: Transaction = bincode::deserialize(&transaction)?;

                    if !decrypt {
                        json!(transaction)
                    } else {
                        let records: Vec<Record> = transaction
                            .output_records()
                            .iter()
                            .filter(|record| {
                                record.is_owner(&credentials.address, &credentials.view_key)
                            })
                            .filter_map(|record| record.decrypt(&credentials.view_key).ok())
                            .collect();

                        json!({
                            "execution": transaction,
                            "decrypted_records": records
                        })
                    }
                }
                _ => json!("unknown command"),
            }
        };

        Ok(output)
    }
}

/// Commands to manage accounts.
#[derive(Debug, Parser)]
pub enum Account {
    New,
    /// Fetches the unspent records owned by the given account.
    Records,
    /// Fetches the unspent records owned by the given account and calculates the final credits balance.
    Balance,
}

#[derive(Debug, Parser)]
pub enum Credits {
    /// Transfer credtis to recipient_address from address that owns the input record
    Transfer {
        #[clap(value_parser=parse_input_value)]
        input_record: Value,
        #[clap(value_parser=parse_input_value)]
        recipient_address: Value,
        #[clap(value_parser=parse_input_value)]
        amount: Value,
    },
    /// Split input record by amount
    Split {
        #[clap(value_parser=parse_input_value)]
        input_record: Value,
        #[clap(value_parser=parse_input_value)]
        amount: Value,
    },
    /// Combine two records into one
    Combine {
        #[clap(value_parser=parse_input_value)]
        first_record: Value,
        #[clap(value_parser=parse_input_value)]
        second_record: Value,
    },
}

impl Credits {
    pub fn inputs(self) -> Vec<Value> {
        match self {
            Credits::Transfer {
                input_record,
                recipient_address,
                amount,
            } => vec![input_record, recipient_address, amount],
            Credits::Combine {
                first_record,
                second_record,
            } => vec![first_record, second_record],
            Credits::Split {
                input_record,
                amount,
            } => vec![input_record, amount],
        }
    }

    pub fn identifier(&self) -> Result<Identifier> {
        match self {
            Credits::Combine { .. } => Identifier::try_from("combine"),
            Credits::Split { .. } => Identifier::try_from("split"),
            Credits::Transfer { .. } => Identifier::try_from("transfer"),
        }
    }
}

/// Commands to manage program transactions.
#[derive(Debug, Parser)]
pub enum Program {
    /// Builds and sends a deployment transaction to the Blockchain, returning the Transaction ID
    Deploy {
        /// Path where the aleo program file resides.
        #[clap(value_parser)]
        path: PathBuf,
        /// Compile remotely and send the synthesized keys along with the program.
        #[clap(short, long, default_value_t = false)]
        compile_remotely: bool,
    },
    /// Runs locally and sends an execution transaction to the Blockchain, returning the Transaction ID
    Execute {
        /// Path where the package resides.
        #[clap(value_parser)]
        path: PathBuf,
        /// The function name.
        #[clap(value_parser)]
        function: Identifier,
        /// The function inputs.
        #[clap(value_parser=parse_input_value)]
        inputs: Vec<Value>,
    },
}

/// Return the status of a Transaction: Type, whether it is committed to the ledger, and the program name.
/// In the case of execution transactions, it also outputs the function's inputs and outputs.
#[derive(Debug, Parser)]
pub struct Get {
    /// Transaction ID from which to retrieve information
    #[clap(value_parser)]
    pub transaction_id: String,

    /// Whether to decrypt the incoming transaction private records
    #[clap(short, long, default_value_t = false)]
    pub decrypt: bool,
}

pub async fn get_records(
    address: vm::Address,
    view_key: vm::ViewKey,
    url: &str,
) -> Result<Vec<(vm::Field, vm::EncryptedRecord, vm::Record)>> {
    let abci_query = AbciQuery::RecordsUnspentOwned { address, view_key };
    let query_serialized = bincode::serialize(&abci_query).unwrap();
    let response = tendermint::query(query_serialized, url).await?;
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

/// Extends the snarkvm's default argument parsing to support using record ciphertexts as record inputs
pub fn parse_input_value(input: &str) -> Result<Value> {
    // try parsing an encrypted record string
    if input.starts_with("record") {
        let credentials = account::Credentials::load()?;
        let ciphertext = EncryptedRecord::from_str(input)?;
        let record = ciphertext.decrypt(&credentials.view_key)?;
        return Ok(Value::Record(record));
    }

    // %account is a syntactic sugar for current user address
    if input == "%account" {
        let credentials = account::Credentials::load()?;
        let address = credentials.address.to_string();
        return Value::from_str(&address);
    }

    // try parsing a jsonified plaintext record
    if let Ok(record) = serde_json::from_str::<Record>(input) {
        return Ok(Value::Record(record));
    }
    // otherwise fallback to parsing a snarkvm literal
    Value::from_str(input)
}
