use clap::Parser;
use serde_json::json;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

mod account;
mod commands;
mod tendermint;

/// Default tendermint url
const LOCAL_BLOCKCHAIN_URL: &str = "http://127.0.0.1:26657";

#[derive(Debug, Parser)]
#[clap()]
pub struct Cli {
    /// Specify a subcommand.
    #[clap(subcommand)]
    pub command: commands::Command,

    /// Output log lines to stdout based on the desired log level (RUST_LOG env var).
    #[clap(short, long, global = false, default_value_t = false)]
    pub verbose: bool,

    /// tendermint node url
    #[clap(short, long, env = "BLOCKCHAIN_URL", default_value = LOCAL_BLOCKCHAIN_URL)]
    pub url: String,
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

    let (exit_code, output) = match cli.command.run(cli.url).await {
        Ok(output) => (0, output),
        Err(err) => (1, json!({"error": err.to_string()})),
    };
    
    println!("{output:#}");
    std::process::exit(exit_code);
}
