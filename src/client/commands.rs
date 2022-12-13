use crate::{account, tendermint};
use anyhow::{anyhow, Result};
use clap::Parser;
use itertools::Itertools;
use lib::query::AbciQuery;
use lib::transaction::Transaction;
use lib::vm;
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
                    let balance = get_records(&credentials, &url)
                        .await?
                        .iter()
                        .fold(0, |acc, (_, _, record)| acc + ***record.gates());

                    json!({ "balance": balance })
                }
                Command::Account(Account::Records) => {
                    let records: Vec<serde_json::Value> = get_records(&credentials, &url)
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
                Command::Program(Program::Deploy { path, fee }) => {
                    let fee = find_fee_record(&credentials, &url, &fee, &[]).await?;
                    let transaction =
                        Transaction::deployment(&path, &credentials.private_key, fee)?;
                    let transaction_serialized = bincode::serialize(&transaction).unwrap();
                    tendermint::broadcast(transaction_serialized, &url).await?;
                    json!(transaction)
                }
                Command::Program(Program::Execute {
                    path,
                    function,
                    inputs,
                    fee,
                }) => {
                    let fee = find_fee_record(&credentials, &url, &fee, &inputs).await?;
                    let transaction = Transaction::execution(
                        Some(&path),
                        function,
                        &inputs,
                        &credentials.private_key,
                        fee,
                    )?;
                    let transaction_serialized = bincode::serialize(&transaction).unwrap();
                    tendermint::broadcast(transaction_serialized, &url).await?;
                    json!(transaction)
                }
                Command::Credits(credits) => {
                    let inputs = credits.inputs();
                    let fee = find_fee_record(&credentials, &url, &credits.fee(), &inputs).await?;
                    let transaction = Transaction::execution(
                        None,
                        credits.identifier()?,
                        &inputs,
                        &credentials.private_key,
                        fee,
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
                        let records: Vec<vm::Record> = transaction
                            .output_records()
                            .iter()
                            .filter(|(_, record)| {
                                record.is_owner(&credentials.address, &credentials.view_key)
                            })
                            .filter_map(|(_, record)| record.decrypt(&credentials.view_key).ok())
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
        #[clap(value_parser=parse_input_record)]
        input_record: vm::Value,
        #[clap(value_parser=parse_input_value)]
        recipient_address: vm::Value,
        #[clap(value_parser=parse_input_value)]
        amount: vm::Value,
        /// Amount of gates to pay as fee for this execution. Will be subtracted from one of the account's records.
        #[clap(short, long)]
        fee: Option<u64>,
    },
    /// Split input record by amount
    Split {
        #[clap(value_parser=parse_input_record)]
        input_record: vm::Value,
        #[clap(value_parser=parse_input_value)]
        amount: vm::Value,
        /// Amount of gates to pay as fee for this execution. Will be subtracted from one of the account's records.
        #[clap(short, long)]
        fee: Option<u64>,
    },
    /// Combine two records into one
    Combine {
        #[clap(value_parser=parse_input_record)]
        first_record: vm::Value,
        #[clap(value_parser=parse_input_record)]
        second_record: vm::Value,
        /// Amount of gates to pay as fee for this execution. Will be subtracted from one of the account's records.
        #[clap(short, long)]
        fee: Option<u64>,
    },
}

impl Credits {
    pub fn inputs(&self) -> Vec<vm::Value> {
        match self {
            Credits::Transfer {
                input_record,
                recipient_address,
                amount,
                ..
            } => vec![
                input_record.clone(),
                recipient_address.clone(),
                amount.clone(),
            ],
            Credits::Combine {
                first_record,
                second_record,
                ..
            } => vec![first_record.clone(), second_record.clone()],
            Credits::Split {
                input_record,
                amount,
                ..
            } => vec![input_record.clone(), amount.clone()],
        }
    }

    pub fn identifier(&self) -> Result<vm::Identifier> {
        match self {
            Credits::Combine { .. } => vm::Identifier::try_from("combine"),
            Credits::Split { .. } => vm::Identifier::try_from("split"),
            Credits::Transfer { .. } => vm::Identifier::try_from("transfer"),
        }
    }

    pub fn fee(&self) -> Option<u64> {
        match self {
            Credits::Transfer { fee, .. } => *fee,
            Credits::Split { fee, .. } => *fee,
            Credits::Combine { fee, .. } => *fee,
        }
    }
}

// TODO move these enums to the top of the file (so the commands are visible before the impl methods)
/// Commands to manage program transactions.
#[derive(Debug, Parser)]
pub enum Program {
    /// Builds and sends a deployment transaction to the Blockchain, returning the Transaction ID
    Deploy {
        /// Path where the aleo program file resides.
        #[clap(value_parser)]
        path: PathBuf,
        /// Amount of gates to pay as fee for this deployment. Will be subtracted from one of the account's records.
        #[clap(short, long)]
        fee: Option<u64>,
    },
    /// Runs locally and sends an execution transaction to the Blockchain, returning the Transaction ID
    Execute {
        /// Path where the package resides.
        #[clap(value_parser)]
        path: PathBuf,
        /// The function name.
        #[clap(value_parser)]
        function: vm::Identifier,
        /// The function inputs.
        #[clap(value_parser=parse_input_value)]
        inputs: Vec<vm::Value>,
        /// Amount of gates to pay as fee for this execution. Will be subtracted from one of the account's records.
        #[clap(short, long)]
        fee: Option<u64>,
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

async fn get_records(
    credentials: &account::Credentials,
    url: &str,
) -> Result<Vec<(vm::Field, vm::EncryptedRecord, vm::Record)>> {
    let abci_query = AbciQuery::RecordsUnspentOwned {
        address: credentials.address,
        view_key: credentials.view_key,
    };
    let query_serialized = bincode::serialize(&abci_query).unwrap();
    let response = tendermint::query(query_serialized, url).await?;
    let records: Vec<(vm::Field, vm::EncryptedRecord)> = bincode::deserialize(&response)?;
    debug!("Records: {:?}", records);
    let records = records
        .into_iter()
        .map(|(commitment, ciphertext)| {
            let record = ciphertext.decrypt(&credentials.view_key).unwrap();
            (commitment, ciphertext, record)
        })
        .collect();
    Ok(records)
}

/// Given a desired amount of fee to pay, find the record on this account with the smallest
/// amount of gates that can be used to pay the fee, and that isn't already being used as
/// an execution input.
async fn find_fee_record(
    credentials: &account::Credentials,
    url: &str,
    amount: &Option<u64>,
    inputs: &[vm::Value],
) -> Result<Option<(u64, vm::Record)>> {
    if amount.is_none() {
        return Ok(None);
    }
    let amount = amount.unwrap();

    // save the input records to make sure that we don't use one of the other execution inputs as the fee
    // this should be a HashSet instead, but Record doesn't implement hash
    let input_records: Vec<vm::Record> = inputs
        .iter()
        .filter_map(|value| {
            if let vm::Value::Record(record) = value {
                Some(record.clone())
            } else {
                None
            }
        })
        .collect();

    let record = get_records(credentials, url)
        .await?
        .into_iter()
        .sorted_by_key(|(_, _, record)| -(vm::gates(record) as i64))
        .find(|(_, _, record)| {
            // note that here we require that the amount of the record be more than the requested fee
            // even though there may be implicit fees in the execution that make the actual amount to be subtracted
            // less that that amount, but since we don't have the execution transitions yet, we can't know at this point
            // so we make this stricter requirement.
            !input_records.contains(record) && vm::gates(record) >= amount
        })
        .map(|(_, _, record)| record)
        .ok_or_else(|| {
            anyhow!("there are not records with enough credits for a {amount} gates fee")
        })?;

    Ok(Some((amount, record)))
}

/// Extends the snarkvm's default argument parsing to support using record ciphertexts as record inputs
fn parse_input_value(input: &str) -> Result<vm::Value> {
    // try parsing an encrypted record string
    if input.starts_with("record") {
        return parse_input_record(input);
    }

    // %account is a syntactic sugar for current user address
    if input == "%account" {
        let credentials = account::Credentials::load()?;
        let address = credentials.address.to_string();
        return vm::Value::from_str(&address);
    }

    // try parsing a jsonified plaintext record
    if let Ok(record) = serde_json::from_str::<vm::Record>(input) {
        return Ok(vm::Value::Record(record));
    }
    // otherwise fallback to parsing a snarkvm literal
    vm::Value::from_str(input)
}

pub fn parse_input_record(input: &str) -> Result<vm::Value> {
    let ciphertext = vm::EncryptedRecord::from_str(input)?;
    let credentials = account::Credentials::load()?;
    ciphertext
        .decrypt(&credentials.view_key)
        .map(vm::Value::Record)
}
