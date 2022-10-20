use anyhow::Result;
use std::path::PathBuf;
use clap::Parser;

use snarkvm::{package::Package, prelude::{Testnet3, Identifier}, circuit::AleoV0, prelude::Value};

#[derive(Debug, Parser)]
pub struct Cli {
    #[clap(value_parser)]
    path: PathBuf,

    #[clap(value_parser)]
    function: Identifier<Testnet3>,

    #[clap(value_parser)]
    inputs: Vec<Value<Testnet3>>,
}

fn main() -> Result<()>{
    let cli = Cli::parse();

    let package: Package<Testnet3> = Package::open(cli.path.as_path()).unwrap();
    package.build::<AleoV0>(None)?;

    // is this necessary?
    let rng = &mut rand::thread_rng();

    let (_response, _transition) = package.run::<AleoV0, _>(
        None,
        package.manifest_file().development_private_key(),
        cli.function,
        &cli.inputs,
        rng,
    )?;

    Ok(())
}
