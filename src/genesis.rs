/// Simple binary that loads the local aleo account and generates a credits record for it.
/// printing it's ciphertext for standard output. This is intended to generate genesis credits
/// for testnets.
pub mod account;
use std::str::FromStr;

use anyhow::{anyhow, Result};
use lib::{
    vm::{EncryptedRecord, Field, Identifier, ProgramID, Value},
    GenesisState,
};
use snarkvm::{
    circuit::AleoV0,
    prelude::{Process, Testnet3},
};

fn main() -> Result<()> {
    let mut rng = rand::thread_rng();
    let credentials = account::Credentials::load().map_err(|_| anyhow!("credentials not found"))?;

    // FIXME remove reliance on process
    let process = Process::<Testnet3>::load()?;
    let program_id = ProgramID::from_str("credits.aleo").unwrap();
    let function_name = Identifier::from_str("genesis").unwrap();
    let address = Value::from_str(&credentials.address.to_string()).unwrap();
    let args: Vec<String> = std::env::args().collect();
    let default = "1000".to_string();
    let amount = args.get(1).unwrap_or(&default);
    let amount = Value::from_str(&format!("{}u64", amount)).unwrap();
    let inputs = vec![address, amount];

    let authorization = process.authorize::<AleoV0, _>(
        &credentials.private_key,
        &program_id,
        function_name,
        &inputs,
        &mut rng,
    )?;
    let (_response, execution) = process.execute::<AleoV0, _>(authorization, &mut rng)?;
    let transition = execution.peek().unwrap();
    let outputs: Vec<(Field, EncryptedRecord)> = transition
        .output_records()
        .map(|(c, r)| (*c, r.clone()))
        .collect();
    let genesis = GenesisState { records: outputs };
    println!("{}", serde_json::to_string(&genesis).unwrap());

    Ok(())
}
