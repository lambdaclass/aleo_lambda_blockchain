use crate::{account, tendermint};
use anyhow::{anyhow, bail, Result};
use clap::Parser;
use itertools::Itertools;
use lib::program_file::ProgramFile;
use lib::query::AbciQuery;
use lib::transaction::Transaction;
use lib::{jaleo, vm};
use log::debug;
use serde_json::json;
use std::collections::HashSet;
use std::path::PathBuf;

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
        input_record: vm::SimpleworksValueType,
        #[clap(value_parser=parse_input_value)]
        recipient_address: vm::SimpleworksValueType,
        #[clap(value_parser=parse_input_value)]
        amount: vm::SimpleworksValueType,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::SimpleworksValueType>,
    },
    /// Split input record by amount
    Split {
        #[clap(value_parser=parse_input_record)]
        input_record: vm::SimpleworksValueType,
        #[clap(value_parser=parse_input_value)]
        amount: vm::SimpleworksValueType,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::SimpleworksValueType>,
    },
    /// Combine two records into one
    Combine {
        #[clap(value_parser=parse_input_record)]
        first_record: vm::SimpleworksValueType,
        #[clap(value_parser=parse_input_record)]
        second_record: vm::SimpleworksValueType,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::SimpleworksValueType>,
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
        fee_record: Option<vm::SimpleworksValueType>,
    },
    /// Runs locally and sends an execution transaction to the Blockchain, returning the Transaction ID
    Execute {
        /// Path where the package resides.
        #[clap(value_parser)]
        path: PathBuf,
        /// The function name.
        #[clap(value_parser)]
        function: jaleo::Identifier,
        /// The function inputs.
        #[clap(value_parser=parse_input_value)]
        inputs: Vec<vm::SimpleworksValueType>,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::SimpleworksValueType>,
    },
    /// Builds an .aleo program's keys and saves them to an .avm file
    Build {
        /// Path to the .aleo program to build
        #[clap(value_parser)]
        path: PathBuf,
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
                Command::Account(Account::New) => {
                    bail!("this shouldn't be reachable, the account new is a special case handled elsewhere")
                }
                Command::Account(Account::Balance) => {
                    let balance = get_records(&credentials, &url)
                        .await?
                        .iter()
                        .fold(0, |acc, (_, _, record)| acc + record.gates);

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
                Command::Program(Program::Build { path }) => {
                    let program_file = ProgramFile::build(&path)?;
                    let output_path = path.with_extension("avm");
                    program_file.save(&output_path)?;
                    json!({ "path": output_path })
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
                        let records: Vec<jaleo::JAleoRecord> = transaction
                            .output_records()
                            .iter()
                            .filter(|record| {
                                let mut address = [0_u8; 63];
                                for (address_byte, primitive_address_byte) in address
                                    .iter_mut()
                                    .zip(credentials.address.to_string().as_bytes())
                                {
                                    *address_byte = *primitive_address_byte;
                                }
                                record.is_owner(&address, &credentials.view_key)
                            })
                            .filter_map(|record| record.decrypt(&credentials.view_key).ok())
                            .collect();

                        json!({
                            "execution": transaction,
                            "decrypted_records": records
                        })
                    }
                }
            }
        };

        Ok(output)
    }
}

impl Credits {
    pub fn inputs(&self) -> Vec<vm::SimpleworksValueType> {
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

    pub fn identifier(&self) -> Result<jaleo::Identifier> {
        match self {
            Credits::Combine { .. } => jaleo::Identifier::try_from("combine"),
            Credits::Split { .. } => jaleo::Identifier::try_from("split"),
            Credits::Transfer { .. } => jaleo::Identifier::try_from("transfer"),
        }
    }

    pub fn fee(&self) -> (Option<u64>, Option<vm::SimpleworksValueType>) {
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
fn parse_input_value(input: &str) -> Result<vm::SimpleworksValueType> {
    // try parsing an encrypted record string
    if input.starts_with("record") {
        return parse_input_record(input);
    }

    // %account is a syntactic sugar for current user address
    if input == "%account" {
        let credentials = account::Credentials::load()?;
        let address = credentials.address.to_string();
        return vm::SimpleworksValueType::try_from(address);
    }

    // try parsing a jsonified plaintext record
    if let Ok(record) = serde_json::from_str::<jaleo::JAleoRecord>(input) {
        return Ok(vm::SimpleworksValueType::Record {
            owner: record.owner,
            gates: record.gates,
            entries: record.entries,
            nonce: record.nonce,
        });
    }
    // otherwise fallback to parsing a snarkvm literal
    vm::SimpleworksValueType::try_from(input.to_string())
}

pub fn parse_input_record(input: &str) -> Result<vm::SimpleworksValueType> {
    // let ciphertext = vm::EncryptedRecord::from_str(input)?;
    // let credentials = account::Credentials::load()?;
    // ciphertext
    //     .decrypt(&credentials.view_key)
    //     .map(vm::SimpleworksValueType::Record)

    vm::SimpleworksValueType::try_from(input.to_string())
}

/// Retrieves all records from the blockchain, and only those that are correctly decrypted
/// (i.e, are owned by the passed credentials) and have not been spent are returned
async fn get_records(
    credentials: &account::Credentials,
    url: &str,
) -> Result<Vec<(jaleo::Field, jaleo::JAleoRecord, jaleo::JAleoRecord)>> {
    let get_records_response = tendermint::query(AbciQuery::GetRecords.into(), url).await?;
    let get_spent_records_response =
        tendermint::query(AbciQuery::GetSpentSerialNumbers.into(), url).await?;

    let records: Vec<(jaleo::Field, jaleo::JAleoRecord)> =
        bincode::deserialize(&get_records_response)?;
    let spent_records: HashSet<jaleo::Field> = bincode::deserialize(&get_spent_records_response)?;

    debug!("Records: {:?}", records);
    let records = records
        .into_iter()
        .filter_map(|(commitment, ciphertext)| {
            ciphertext
                .decrypt(&credentials.view_key)
                .map(|decrypted_record| (commitment, ciphertext, decrypted_record))
                .ok()
                .filter(|(_, _, decrypted_record)| {
                    let serial_number = decrypted_record.serial_number(&credentials.private_key);
                    serial_number.is_ok() & spent_records.contains(&serial_number.unwrap())
                })
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
    record: &Option<vm::SimpleworksValueType>,
    inputs: &[vm::SimpleworksValueType],
) -> Result<Option<(u64, jaleo::JAleoRecord)>> {
    if amount.is_none() {
        return Ok(None);
    }
    let amount = amount.unwrap();

    if let Some(vm::SimpleworksValueType::Record {
        owner,
        gates,
        entries,
        nonce,
    }) = record
    {
        return Ok(Some((
            amount,
            jaleo::JAleoRecord::new(*owner, *gates, entries.clone(), Some(*nonce)),
        )));
    }

    let account_records: Vec<jaleo::JAleoRecord> = get_records(credentials, url)
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
    inputs: &[vm::SimpleworksValueType],
    account_records: &[jaleo::JAleoRecord],
) -> Result<jaleo::JAleoRecord> {
    // save the input records to make sure that we don't use one of the other execution inputs as the fee
    let input_records: HashSet<String> = inputs
        .iter()
        .filter_map(|value| {
            if let vm::SimpleworksValueType::Record {
                owner,
                gates,
                entries,
                nonce,
            } = value
            {
                Some(
                    jaleo::JAleoRecord::new(*owner, *gates, entries.clone(), Some(*nonce))
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect();

    account_records
        .iter()
        .sorted_by_key(|record|
                       // negate to get bigger records first
                       -(record.gates as i64))
        .find(|record| {
            // note that here we require that the amount of the record be more than the requested fee
            // even though there may be implicit fees in the execution that make the actual amount to be subtracted
            // less that that amount, but since we don't have the execution transitions yet, we can't know at this point
            // so we make this stricter requirement.
            !input_records.contains(&record.to_string()) && record.gates >= amount
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
        let private_key = jaleo::PrivateKey::new(&mut rand::thread_rng()).unwrap();
        let view_key = jaleo::ViewKey::try_from(&private_key).unwrap();
        let address = jaleo::Address::try_from(&view_key).unwrap();

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
        let error = select_default_fee_record(
            5,
            &[vm::SimpleworksValueType::Record {
                owner: record6.owner,
                gates: record6.gates,
                entries: record6.entries.clone(),
                nonce: record6.nonce,
            }],
            &[record6.clone()],
        )
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
            &[vm::SimpleworksValueType::Record {
                owner: record10.owner,
                gates: record10.gates,
                entries: record10.entries.clone(),
                nonce: record10.nonce,
            }],
            &[record5, record10, record6.clone()],
        )
        .unwrap();
        assert_eq!(record6, result);
    }

    fn mint_record(
        address: &jaleo::Address,
        view_key: &jaleo::ViewKey,
        amount: u64,
    ) -> jaleo::JAleoRecord {
        jaleo::mint_credits(address, amount)
            .unwrap()
            .1
            .decrypt(view_key)
            .unwrap()
    }
}
