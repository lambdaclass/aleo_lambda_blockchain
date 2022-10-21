use anyhow::Result;
use clap::Parser;
use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use snarkvm::{
    circuit::AleoV0,
    file::VerifierFile,
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

    let execution = generate_execution(&cli.path, cli.function, &cli.inputs)?;

    // Executions can be both printed as and serialized to JSON
    println!("{}", execution);

    verify_execution(&cli.path, cli.function, &execution)?;

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

/// Function that a verifier node would use to verify incoming JSON transfer requests
fn verify_execution(
    path: &Path,
    function: Identifier<Testnet3>,
    execution_json: &str,
) -> Result<()> {
    let execution =
        FromStr::from_str(std::str::from_utf8(execution_json.as_bytes()).unwrap()).unwrap();

    let package: Package<Testnet3> = Package::open(path).unwrap();
    package.build::<AleoV0>(None)?;

    let build_dir = package.build_directory();
    let process = package.get_process()?;

    let verifier = VerifierFile::open(build_dir.as_path(), &function)?;

    let program_id = package.program_id();
    process.insert_verifying_key(program_id, &function, verifier.verifying_key().clone())?;

    process.verify_execution(&execution).unwrap();

    Ok(())
}
