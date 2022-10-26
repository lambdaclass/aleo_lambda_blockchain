use anyhow::Result;
use bytes::Bytes;
use lib::network::ReliableSender;
use std::net::SocketAddr;

use std::path::{Path, PathBuf};

use clap::Parser;

use snarkvm::{
    circuit::AleoV0,
    package::Package,
    prelude::Value,
    prelude::{Identifier, Testnet3},
};

#[derive(Debug, Parser)]
pub struct Cli {
    #[clap(value_parser)]
    path: PathBuf,

    #[clap(value_parser)]
    function: Identifier<Testnet3>,

    #[clap(value_parser)]
    inputs: Vec<Value<Testnet3>>,
}

#[tokio::main()]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let execution = generate_execution(&cli.path, cli.function, &cli.inputs)?;
    let execution_serialized = bincode::serialize(&execution.to_string()).unwrap();
    let execution_bytes = Bytes::from_iter(execution_serialized);

    let mut sender = ReliableSender::new();
    let address = "127.0.0.1:6200".parse::<SocketAddr>().unwrap();

    let reply_handler = sender.send(address, execution_bytes).await;
    let _response = reply_handler.await?;

    Ok(())
}

fn generate_execution(
    path: &Path,
    function: Identifier<Testnet3>,
    inputs: &[Value<Testnet3>],
) -> Result<String> {
    let package: Package<Testnet3> = Package::open(path).unwrap();
    package.build::<AleoV0>(None)?;

    let rng = &mut rand::thread_rng();

    let (response, execution) = package.run::<AleoV0, _>(
        None,
        package.manifest_file().development_private_key(),
        function,
        inputs,
        rng,
    )?;

    println!("outputs {:?}", response.outputs());

    Ok(execution.to_string())
}
