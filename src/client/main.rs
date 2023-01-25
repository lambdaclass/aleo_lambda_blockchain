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
    //let cli = Cli::parse_from(["","credits", "stake", "1", "recordb566035acf34e4a19d562e1d9c1c56698de726edbb9a8f190b1ef26ede27fd0b430dcab9fc29acb49309144a747dd3700d98359aab829b1b8d0aa390d5cb9859086d9fbf8badc8e026a41939d00b6e5b6db26f27b7c2be1c1108be359cfda5896140a77d43e41a1af459c202fae36fea579c68e02314196bb676c491acea30ddcf4c6ed8fc7326e99c95b368ccf816fe004c6f55f2839b09189dc470bbd3c264fff81669307b08ad0221eec6649c00ccc218a89ccd692de3f3e74f4380add90bb849bdc40b18fa687477f7634ec619896b1b0e9b9d9745dd0f5641ab8a95265fc339aed233622e2343f498ad66dd3ec75c6157f32bb9e366e08f40a54e24", "QsClAYVnRRQHxlMQW/ENsBWirlMfZQBowehbfpQxyyU="]);

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
