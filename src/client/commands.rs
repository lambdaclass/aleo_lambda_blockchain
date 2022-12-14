use crate::{account, tendermint};
use anyhow::{anyhow, Result};
use clap::Parser;
use itertools::Itertools;
use lib::query::AbciQuery;
use lib::transaction::Transaction;
use lib::vm;
use log::debug;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::vec;

#[derive(Debug, Parser)]
pub enum Command {
    #[clap(subcommand)]
    Account(Account),
    #[clap(subcommand)]
    Credits(Credits),
    #[clap(subcommand)]
    Program(Program),
    #[clap(name = "get")]
    Get(Get),
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
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::Value>,
    },
    /// Split input record by amount
    Split {
        #[clap(value_parser=parse_input_record)]
        input_record: vm::Value,
        #[clap(value_parser=parse_input_value)]
        amount: vm::Value,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::Value>,
    },
    /// Combine two records into one
    Combine {
        #[clap(value_parser=parse_input_record)]
        first_record: vm::Value,
        #[clap(value_parser=parse_input_record)]
        second_record: vm::Value,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::Value>,
    },
}

/// Commands to manage program transactions.
#[derive(Debug, Parser)]
pub enum Program {
    /// Builds and sends a deployment transaction to the Blockchain, returning the Transaction ID
    Deploy {
        /// Path where the aleo program file resides.
        #[clap(value_parser)]
        path: PathBuf,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::Value>,
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
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::Value>,
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
                Command::Program(Program::Deploy {
                    path,
                    fee,
                    fee_record,
                }) => {
                    let fee = choose_fee_record(&credentials, &url, &fee, &fee_record, &[]).await?;
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
                    fee_record,
                }) => {
                    let fee =
                        choose_fee_record(&credentials, &url, &fee, &fee_record, &inputs).await?;
                    let transaction = Transaction::execution(
                        &path,
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
                    let (fee_amount, fee_record) = credits.fee();
                    let fee =
                        choose_fee_record(&credentials, &url, &fee_amount, &fee_record, &inputs)
                            .await?;
                    let transaction = Transaction::credits_execution(
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

    pub fn fee(&self) -> (Option<u64>, Option<vm::Value>) {
        match self {
            Credits::Transfer {
                fee, fee_record, ..
            } => (*fee, fee_record.clone()),
            Credits::Split {
                fee, fee_record, ..
            } => (*fee, fee_record.clone()),
            Credits::Combine {
                fee, fee_record, ..
            } => (*fee, fee_record.clone()),
        }
    }
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

/// Given a desired amount of fee to pay, find the record on this account with the biggest
/// amount of gates that can be used to pay the fee, and that isn't already being used as
/// an execution input. If a record is already provided, use that, otherwise select a default
/// record from the account.
async fn choose_fee_record(
    credentials: &account::Credentials,
    url: &str,
    amount: &Option<u64>,
    record: &Option<vm::Value>,
    inputs: &[vm::Value],
) -> Result<Option<(u64, vm::Record)>> {
    if amount.is_none() {
        return Ok(None);
    }
    let amount = amount.unwrap();

    if let Some(vm::Value::Record(record)) = record {
        return Ok(Some((amount, record.clone())));
    }

    let account_records: Vec<vm::Record> = get_records(credentials, url)
        .await?
        .into_iter()
        .map(|(_, _, record)| record)
        .collect();

    select_default_fee_record(amount, inputs, &account_records).map(|record| Some((amount, record)))
}

/// Select one of the records to be used to pay the requested fee,
/// that is not already being used as input to the execution.
/// The biggest record is chosen as the default under the assumption
/// that choosing the best fit would lead to record fragmentation.
fn select_default_fee_record(
    amount: u64,
    inputs: &[vm::Value],
    account_records: &[vm::Record],
) -> Result<vm::Record> {
    // save the input records to make sure that we don't use one of the other execution inputs as the fee
    let input_records: HashSet<String> = inputs
        .iter()
        .filter_map(|value| {
            if let vm::Value::Record(record) = value {
                Some(record.to_string())
            } else {
                None
            }
        })
        .collect();

    account_records
        .iter()
        .sorted_by_key(|record|
                       // negate to get bigger records first
                       -(vm::gates(record) as i64))
        .find(|record| {
            // note that here we require that the amount of the record be more than the requested fee
            // even though there may be implicit fees in the execution that make the actual amount to be subtracted
            // less that that amount, but since we don't have the execution transitions yet, we can't know at this point
            // so we make this stricter requirement.
            !input_records.contains(&record.to_string()) && vm::gates(record) >= amount
        })
        .ok_or_else(|| {
            anyhow!("there are not records with enough credits for a {amount} gates fee")
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_default_record() {
        let private_key = vm::PrivateKey::new(&mut rand::thread_rng()).unwrap();
        let view_key = vm::ViewKey::try_from(&private_key).unwrap();
        let address = vm::Address::try_from(&view_key).unwrap();

        let record10 = mint_record(&address, &view_key, 10);
        let record5 = mint_record(&address, &view_key, 5);
        let record6 = mint_record(&address, &view_key, 6);

        // if no records in account, fail
        let error = select_default_fee_record(10, &[], &[]).unwrap_err();
        assert_eq!(
            "there are not records with enough credits for a 10 gates fee",
            error.to_string()
        );

        // if several records but none big enough, fail
        let error =
            select_default_fee_record(10, &[], &[record5.clone(), record6.clone()]).unwrap_err();
        assert_eq!(
            "there are not records with enough credits for a 10 gates fee",
            error.to_string()
        );

        // if one record no input, choose it
        let result = select_default_fee_record(5, &[], &[record6.clone()]).unwrap();
        assert_eq!(record6, result);

        // if one record but also input, fail
        let error =
            select_default_fee_record(5, &[vm::Value::Record(record6.clone())], &[record6.clone()])
                .unwrap_err();
        assert_eq!(
            "there are not records with enough credits for a 5 gates fee",
            error.to_string()
        );

        // if several records, choose the biggest one
        let result = select_default_fee_record(
            5,
            &[],
            &[record5.clone(), record10.clone(), record6.clone()],
        )
        .unwrap();
        assert_eq!(record10, result);

        let result = select_default_fee_record(
            5,
            &[vm::Value::Record(record10.clone())],
            &[record5, record10, record6.clone()],
        )
        .unwrap();
        assert_eq!(record6, result);
    }

    fn mint_record(address: &vm::Address, view_key: &vm::ViewKey, amount: u64) -> vm::Record {
        vm::mint_credits(address, amount, 123)
            .unwrap()
            .1
            .decrypt(view_key)
            .unwrap()
    }
}
