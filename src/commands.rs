use crate::account;
use anyhow::Result;
use clap::Parser;
use lib::vm::{EncryptedRecord, Identifier, Record, Value};
use std::path::PathBuf;
use std::str::FromStr;
use std::vec;

/// Commands to manage accounts.
#[derive(Debug, Parser)]
pub enum Account {
    New,
    /// Fetches the records owned by the given account.
    Records,
    /// Fetches the records owned by the given account and calculates the final credits balance.
    Balance,
    Decrypt {
        /// Value to decrypt
        #[clap(short, long)]
        value: String,
    },
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

/// Extends the snarkvm's default argument parsing to support using record ciphertexts as record inputs
pub fn parse_input_value(input: &str) -> Result<Value> {
    // try parsing an encrypted record string
    if input.starts_with("record") {
        let credentials = account::Credentials::load()?;
        let ciphertext = EncryptedRecord::from_str(input)?;
        let record = ciphertext.decrypt(&credentials.view_key)?;
        return Ok(Value::Record(record));
    }

    // %account is a syntactic sugar for cuerren user address
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
