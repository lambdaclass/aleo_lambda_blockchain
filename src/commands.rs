use clap::Parser;
use snarkvm::prelude::{Address, Identifier, PrivateKey, Testnet3, Value, ViewKey};
use std::path::PathBuf;

/// Commands to manage accounts.
#[derive(Debug, Parser)]
pub enum Account {
    /// Generates a new account.
    New {
        /// Seed the RNG with a numeric value.
        #[clap(value_parser, short, long)]
        seed: Option<u64>,
    },
    /// Fetches the records owned by the given account.
    Records {
        /// The view key of the account from which the records are fetched.
        #[clap(short, long)]
        view_key: ViewKey<Testnet3>,
    },
    /// Fetches the records owned by the given account and calculates the final credits balance.
    Balance {
        /// The view key of the account from which the records are fetched
        #[clap(short, long)]
        view_key: ViewKey<Testnet3>,
    },
    /// Commits an execution transaction to send a determined amount of credits to another account.
    Transfer {
        /// Account to which the credits will be transferred.
        #[clap(short, long)]
        recipient_public_key: Address<Testnet3>,
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
        function: Identifier<Testnet3>,
        /// The function inputs.
        #[clap(value_parser)]
        inputs: Vec<Value<Testnet3>>,
        /// Account private key necessary to authorize the execution transaction
        #[clap(value_parser)]
        private_key: PrivateKey<Testnet3>,
    },
}

/// Return the status of a Transaction: Type, whether it is committed to the ledger, and the program name.
/// In the case of execution transactions, it also outputs the function's inputs and outputs.
#[derive(Debug, Parser)]
pub struct Get {
    /// Transaction ID from which to retrieve information
    #[clap(value_parser)]
    transaction_id: PathBuf,
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
