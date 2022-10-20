use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

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

fn main() -> Result<()> {
    let cli = Cli::parse();

    let package: Package<Testnet3> = Package::open(cli.path.as_path()).unwrap();
    package.build::<AleoV0>(None)?;

    // is this necessary?
    let rng = &mut rand::thread_rng();

    let (response, execution) = package.run::<AleoV0, _>(
        None,
        package.manifest_file().development_private_key(),
        cli.function,
        &cli.inputs,
        rng,
    )?;

    println!("outputs {:?}", response.outputs());

    // verify the execution
    let process = package.get_process()?;
    process.verify_execution(&execution)?;

    // TODO once we get the above working, we need to do the same without assuming local files
    // e.g. the way a node would have to run it if all the necessary context was received in a request

    Ok(())
}
