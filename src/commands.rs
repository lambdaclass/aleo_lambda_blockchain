use clap::Parser;
use lib::vm::{Address, Identifier, Value};
use std::path::PathBuf;

/// Commands to manage accounts.
#[derive(Debug, Parser)]
pub enum Account {
    /// Generates a new account.
    New,
    /// Fetches the records owned by the given account.
    Records,
    /// Fetches the records owned by the given account and calculates the final credits balance.
    Balance,
    /// Commits an execution transaction to send a determined amount of credits to another account.
    Transfer {
        /// Account to which the credits will be transferred.
        #[clap(short, long)]
        recipient_public_key: Address,
        /// Amount of credits to transfer
        #[clap(value_parser, short, long)]
        credits: u64,
    },
    Decrypt {
        /// Value to decrypt
        #[clap(short, long)]
        value: String,
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
        #[clap(value_parser)]
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
}
