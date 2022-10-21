use anyhow::Result;
use clap::Parser;
use std::{path::PathBuf, str::FromStr};

use snarkvm::{
    circuit::AleoV0,
    file::VerifierFile,
    package::Package,
    prelude::Execution,
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

    let rng = &mut rand::thread_rng();

    let (response, execution) = package.run::<AleoV0, _>(
        None,
        package.manifest_file().development_private_key(),
        cli.function,
        &cli.inputs,
        rng,
    )?;

    println!("outputs {:?}", response.outputs());

    let build_dir = package.build_directory();
    let process = package.get_process()?;
    let verifier = VerifierFile::open(build_dir.as_path(), &cli.function)?;

    let program_id = package.program_id();
    process.insert_verifying_key(program_id, &cli.function, verifier.verifying_key().clone())?;

    // Executions can be both printed as and serialized to JSON
    println!("{}", execution);

    // std::fs::write("./execution", execution.to_string()).unwrap();

    // let execution_json = std::fs::read("./execution").unwrap();
    // let execution = FromStr::from_str(std::str::from_utf8(&execution_json).unwrap()).unwrap();

    process.verify_execution(&execution)?;

    // TODO once we get the above working, we need to do the same without assuming local files
    // e.g. the way a node would have to run it if all the necessary context was received in a request

    Ok(())
}

/// Function that a verifier node would use to verify incoming JSON transfer requests
fn verify_execution(execution_json: &str) -> Result<()> {
    let execution =
        FromStr::from_str(std::str::from_utf8(execution_json.as_bytes()).unwrap()).unwrap();

    let credits_path = std::path::Path::new("./credits");
    let package: Package<Testnet3> = Package::open(credits_path).unwrap();
    package.build::<AleoV0>(None)?;

    let build_dir = package.build_directory();
    let process = package.get_process()?;

    let function_identifier: Identifier<Testnet3> = "transfer".try_into().unwrap();
    let verifier = VerifierFile::open(build_dir.as_path(), &function_identifier)?;

    let program_id = package.program_id();
    process.insert_verifying_key(
        program_id,
        &function_identifier,
        verifier.verifying_key().clone(),
    )?;

    process.verify_execution(&execution).unwrap();

    Ok(())
}
