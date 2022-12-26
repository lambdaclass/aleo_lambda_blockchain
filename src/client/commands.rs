use crate::{account, tendermint};
use anyhow::{anyhow, bail, Result};
use clap::Parser;
use itertools::Itertools;
use lib::program_file::ProgramFile;
use lib::query::AbciQuery;
use lib::transaction::Transaction;
use lib::vm;
use lib::vm::{EncryptedRecord, ProgramID};
use log::debug;
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

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
        input_record: vm::UserInputValueType,
        #[clap(value_parser=parse_input_value)]
        recipient_address: vm::UserInputValueType,
        #[clap(value_parser=parse_input_value)]
        amount: u64,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::UserInputValueType>,
    },
    /// Split input record by amount
    Split {
        #[clap(value_parser=parse_input_record)]
        input_record: vm::UserInputValueType,
        #[clap(value_parser=parse_input_value)]
        amount: u64,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::UserInputValueType>,
    },
    /// Combine two records into one
    Combine {
        #[clap(value_parser=parse_input_record)]
        first_record: vm::UserInputValueType,
        #[clap(value_parser=parse_input_record)]
        second_record: vm::UserInputValueType,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::UserInputValueType>,
    },
    /// Take credits out from a credits record and stake them as a blockchain validator. This will execute a program and output a
    /// stake record that can be later used to reclaim the staked credits.
    Stake {
        /// The amount of gates to stake.
        #[clap()]
        amount: u64,
        /// The credits record to subtract the staked amount from.
        #[clap(value_parser=parse_input_record)]
        record: vm::UserInputValueType,
        /// The tendermint address of the validator that will stake the credits.
        #[clap()]
        validator: String,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::UserInputValueType>,
    },
    /// Take credits out of a stake record, reducing the voting power of the validator.
    Unstake {
        /// The amount of gates to unstake. Should at most what this validator has already staked.
        #[clap()]
        amount: u64,
        /// The stake record to recover the staked amount from.
        #[clap(value_parser=parse_input_record)]
        record: vm::UserInputValueType,
        /// The tendermint address of the validator that is staking the credits.
        #[clap()]
        validator: String,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::UserInputValueType>,
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
        fee_record: Option<vm::UserInputValueType>,
    },
    /// Runs locally and sends an execution transaction to the blockchain, returning the Transaction ID
    Execute {
        /// Program to execute (path or program_id).
        #[clap(value_parser)]
        program: String,
        /// The function name.
        #[clap(value_parser)]
        function: vm::Identifier,
        /// The function inputs.
        #[clap(value_parser=parse_input_value)]
        inputs: Vec<vm::UserInputValueType>,
        /// Amount of gates to pay as fee for this execution. If omitted not fee is paid.
        #[clap(long)]
        fee: Option<u64>,
        /// The record to use to subtract the fee amount. If omitted, the record with most gates in the account is used.
        #[clap(long, value_parser=parse_input_record)]
        fee_record: Option<vm::UserInputValueType>,
        /// Run the input code locally, generating the execution proof but without sending it over to the blockchain. Displays execution and decrypted records.
        #[clap(long, short, default_value_t = false)]
        dry_run: bool,
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
                    program,
                    function,
                    inputs,
                    fee,
                    fee_record,
                    dry_run,
                }) => {
                    let fee =
                        choose_fee_record(&credentials, &url, &fee, &fee_record, &inputs).await?;
                    let program = match get_program(&url, &program).await? {
                        Some(program) => program,
                        None => bail!("Could not find program {}", program),
                    };
                    let transaction = Transaction::execution(
                        program,
                        function,
                        &inputs,
                        &credentials.private_key,
                        fee,
                    )?;

                    let mut transaction_json = json!(transaction);
                    if !dry_run {
                        let transaction_serialized = bincode::serialize(&transaction).unwrap();
                        tendermint::broadcast(transaction_serialized, &url).await?;
                    } else {
                        let records = Self::decrypt_records(&transaction, credentials);

                        if !records.is_empty() {
                            transaction_json
                                .as_object_mut()
                                .unwrap()
                                .insert("decrypted_records".to_string(), json!(records));
                        }
                    }
                    json!(transaction_json)
                }
                Command::Program(Program::Build { path }) => {
                    let program_source = std::fs::read_to_string(&path)?;
                    let program_file = ProgramFile::build(&program_source)?;
                    let output_path = path.with_extension("avm");
                    program_file.save(&output_path)?;
                    json!({ "path": output_path })
                }
                Command::Credits(Credits::Transfer {
                    input_record,
                    recipient_address,
                    amount,
                    fee,
                    fee_record,
                }) => {
                    let inputs = [
                        input_record.clone(),
                        recipient_address.clone(),
                        u64_to_value(amount),
                    ];
                    run_credits_command(
                        &credentials,
                        &url,
                        "transfer",
                        &inputs,
                        None,
                        &fee,
                        &fee_record,
                    )
                    .await?
                }
                Command::Credits(Credits::Combine {
                    first_record,
                    second_record,
                    fee,
                    fee_record,
                }) => {
                    let inputs = [first_record.clone(), second_record.clone()];
                    run_credits_command(
                        &credentials,
                        &url,
                        "combine",
                        &inputs,
                        None,
                        &fee,
                        &fee_record,
                    )
                    .await?
                }
                Command::Credits(Credits::Split {
                    input_record,
                    amount,
                    fee,
                    fee_record,
                }) => {
                    let inputs = [input_record.clone(), u64_to_value(amount)];
                    run_credits_command(
                        &credentials,
                        &url,
                        "split",
                        &inputs,
                        None,
                        &fee,
                        &fee_record,
                    )
                    .await?
                }
                Command::Credits(Credits::Stake {
                    amount,
                    record,
                    validator,
                    fee,
                    fee_record,
                }) => {
                    let inputs = [record.clone(), u64_to_value(amount)];
                    run_credits_command(
                        &credentials,
                        &url,
                        "stake",
                        &inputs,
                        Some(validator),
                        &fee,
                        &fee_record,
                    )
                    .await?
                }
                Command::Credits(Credits::Unstake {
                    amount,
                    record,
                    validator,
                    fee,
                    fee_record,
                }) => {
                    let inputs = [record.clone(), u64_to_value(amount)];
                    run_credits_command(
                        &credentials,
                        &url,
                        "unstake",
                        &inputs,
                        Some(validator),
                        &fee,
                        &fee_record,
                    )
                    .await?
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
                        let records = Self::decrypt_records(&transaction, credentials);

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

    fn decrypt_records(
        transaction: &Transaction,
        credentials: account::Credentials,
    ) -> Vec<vm::Record> {
        transaction
            .output_records()
            .iter()
            .filter(|(_commitment, record)| {
                // The above turns a snarkVM address into an address that is
                // useful for the vm. This should change a little when we support
                // our own addresses.
                let address = vm::to_address(credentials.address.to_string());
                record.is_owner(&address, &credentials.view_key)
            })
            .filter_map(|(_commitment, record)| record.decrypt(&credentials.view_key).ok())
            .collect()
    }
}

fn u64_to_value(amount: u64) -> vm::UserInputValueType {
    vm::UserInputValueType::U64(amount)
}

async fn run_credits_command(
    credentials: &account::Credentials,
    url: &str,
    function: &str,
    inputs: &[vm::UserInputValueType],
    validator: Option<String>,
    fee_amount: &Option<u64>,
    fee_record: &Option<vm::UserInputValueType>,
) -> Result<serde_json::Value> {
    let fee = choose_fee_record(credentials, url, fee_amount, fee_record, inputs).await?;
    let function_identifier = vm::Identifier::from_str(function)?;
    let transaction = Transaction::credits_execution(
        function_identifier,
        inputs,
        &credentials.private_key,
        fee,
        validator,
    )?;
    let transaction_serialized = bincode::serialize(&transaction).unwrap();
    tendermint::broadcast(transaction_serialized, url).await?;
    Ok(json!(transaction))
}

/// Extends the snarkvm's default argument parsing to support using record ciphertexts as record inputs
fn parse_input_value(input: &str) -> Result<vm::UserInputValueType> {
    // try parsing an encrypted record string
    if input.starts_with("record") {
        return parse_input_record(input);
    }

    // %account is a syntactic sugar for current user address
    if input == "%account" {
        let credentials = account::Credentials::load()?;
        let address = credentials.address.to_string();
        return vm::UserInputValueType::try_from(address);
    }

    // try parsing a jsonified plaintext record
    if let Ok(record) = serde_json::from_str::<vm::Record>(input) {
        return Ok(vm::UserInputValueType::Record(vm::Record {
            owner: record.owner,
            gates: record.gates,
            data: record.data,
            nonce: record.nonce,
        }));
    }
    // otherwise fallback to parsing a snarkvm literal
    vm::UserInputValueType::try_from(input.to_string())
}

pub fn parse_input_record(input: &str) -> Result<vm::UserInputValueType> {
    let ciphertext: EncryptedRecord = serde_json::from_str(input)?;
    let credentials = account::Credentials::load()?;
    ciphertext
        .decrypt(&credentials.view_key)
        .map(vm::UserInputValueType::Record)
}

/// Retrieves all records from the blockchain, and only those that are correctly decrypted
/// (i.e, are owned by the passed credentials) and have not been spent are returned
async fn get_records(
    credentials: &account::Credentials,
    url: &str,
) -> Result<Vec<(vm::Field, vm::EncryptedRecord, vm::Record)>> {
    let get_records_response = tendermint::query(AbciQuery::GetRecords.into(), url).await?;
    let get_spent_records_response =
        tendermint::query(AbciQuery::GetSpentSerialNumbers.into(), url).await?;

    let records: Vec<(vm::Field, vm::EncryptedRecord)> =
        bincode::deserialize(&get_records_response)?;
    let spent_records: HashSet<vm::Field> = bincode::deserialize(&get_spent_records_response)?;

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
    record: &Option<vm::UserInputValueType>,
    inputs: &[vm::UserInputValueType],
) -> Result<Option<(u64, vm::Record)>> {
    if amount.is_none() {
        return Ok(None);
    }
    let amount = amount.unwrap();

    if let Some(vm::UserInputValueType::Record(vm::Record {
        owner,
        gates,
        data,
        nonce,
    })) = record
    {
        return Ok(Some((
            amount,
            vm::Record::new(*owner, *gates, data.clone(), Some(*nonce)),
        )));
    }

    let account_records: Vec<vm::Record> = get_records(credentials, url)
        .await?
        .into_iter()
        .map(|(_, _, record)| record)
        .collect();

    select_default_fee_record(amount, inputs, &account_records).map(|record| Some((amount, record)))
}

async fn get_program(url: &str, program: &str) -> Result<Option<vm::Program>> {
    match fs::read_to_string(PathBuf::from(program)) {
        Ok(program_string) => vm::generate_program(&program_string).map(Some),
        Err(_) => get_program_from_blockchain(url, ProgramID::from_str(program)?).await,
    }
}

async fn get_program_from_blockchain(
    url: &str,
    program_id: vm::ProgramID,
) -> Result<Option<vm::Program>> {
    let result = tendermint::query(AbciQuery::GetProgram { program_id }.into(), url).await?;
    let program: Option<vm::Program> = bincode::deserialize(&result)?;
    Ok(program)
}

/// Select one of the records to be used to pay the requested fee,
/// that is not already being used as input to the execution.
/// The biggest record is chosen as the default under the assumption
/// that choosing the best fit would lead to record fragmentation.
fn select_default_fee_record(
    amount: u64,
    inputs: &[vm::UserInputValueType],
    account_records: &[vm::Record],
) -> Result<vm::Record> {
    // save the input records to make sure that we don't use one of the other execution inputs as the fee
    let input_records: HashSet<String> = inputs
        .iter()
        .filter_map(|value| {
            if let vm::UserInputValueType::Record(vm::Record {
                owner,
                gates,
                data,
                nonce,
            }) = value
            {
                Some(vm::Record::new(*owner, *gates, data.clone(), Some(*nonce)).to_string())
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
        let error = select_default_fee_record(
            5,
            &[vm::UserInputValueType::Record(vm::Record {
                owner: record6.owner,
                gates: record6.gates,
                data: record6.data.clone(),
                nonce: record6.nonce,
            })],
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
            &[vm::UserInputValueType::Record(vm::Record {
                owner: record10.owner,
                gates: record10.gates,
                data: record10.data.clone(),
                nonce: record10.nonce,
            })],
            &[record5, record10, record6.clone()],
        )
        .unwrap();
        assert_eq!(record6, result);
    }

    fn mint_record(address: &vm::Address, view_key: &vm::ViewKey, amount: u64) -> vm::Record {
        vm::mint_credits(address, amount)
            .unwrap()
            .1
            .decrypt(view_key)
            .unwrap()
    }
}
