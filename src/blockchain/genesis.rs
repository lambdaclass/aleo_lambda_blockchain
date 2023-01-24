/// Binary that walks a list of tendermint node directories (like the default ~/.tendermint or a testnet generated node dir),
/// assuming they also contain an aleo account credentials file, and updates their genesis files to include the genesis state
/// expected by our abci app.
use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use lib::{validator, vm};

/// Takes a list of node directories and updates the genesis files on each of them
/// to include records to assign default credits to each validator and a mapping
/// of tendermint validator pubkey to aleo account address.
#[derive(Debug, Parser)]
#[clap()]
pub struct Cli {
    /// List of node directories.
    /// Each one is expected to contain a config/genesis.json (with a tendermint genesis)
    /// a config/priv_validator_key.json (with tendermint validator credentials)
    /// and a account.json (with aleo credentials)
    #[clap()]
    node_dirs: Vec<PathBuf>,

    /// The amount of gates to assign to each validator
    #[clap(long, default_value = "1000")]
    amount: u64,
}

fn main() -> Result<()> {
    let cli: Cli = Cli::parse();

    // update the genesis JSON with the calculated app state
    let genesis_path = cli
        .node_dirs
        .first()
        .expect("need at least one directory")
        .join("config/genesis.json");
    let mut genesis: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(genesis_path)?)?;
    let voting_powers: HashMap<String, u64> = genesis["validators"]
        .as_array()
        .unwrap()
        .iter()
        .map(|validator| {
            (
                validator["pub_key"]["value"].as_str().unwrap().to_string(),
                validator["power"].as_str().unwrap().parse().unwrap(),
            )
        })
        .collect();

    // for each node in the testnet, map its tendermint pubkey to its aleo account address
    // and generate records for initial validator credits
    let mut validators = Vec::new();
    let mut genesis_records = Vec::new();
    for node_dir in cli.node_dirs.clone() {
        println!("processing {}", node_dir.to_string_lossy());

        let aleo_account_path = node_dir.join("account.json");
        let aleo_account: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(aleo_account_path)?)?;
        let aleo_address = aleo_account["address"].as_str().unwrap();
        let aleo_view_key = aleo_account["view_key"].as_str().unwrap();

        let tmint_account_path = node_dir.join("config/priv_validator_key.json");
        let tmint_account: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmint_account_path)?)?;
        let tmint_pubkey = tmint_account["pub_key"]["value"]
            .as_str()
            .expect("couldn't extract pubkey from json");
        let voting_power = *voting_powers.get(tmint_pubkey).unwrap();
        let validator = validator::Validator::from_str(tmint_pubkey, aleo_address, voting_power)?;

        println!("Generating record for {aleo_address}");
        // NOTE: using a hardcoded seed, not for production!
        #[allow(unused_mut)]
        let mut record = vm::mint_record(
            "credits.aleo",
            "credits",
            &validator.aleo_address,
            cli.amount,
            1234,
        )?;

        genesis_records.push(record);
        validators.push(validator);
    }

    // update the genesis JSON with the calculated app state
    let genesis_state = validator::GenesisState {
        records: genesis_records,
        validators,
    };
    genesis.as_object_mut().unwrap().insert(
        "app_state".to_string(),
        serde_json::to_value(genesis_state)?,
    );
    let genesis_json = serde_json::to_string_pretty(&genesis)?;

    // set the same genesis file in all nodes of the testnet
    for node_dir in cli.node_dirs {
        let node_genesis_path = node_dir.join("config/genesis.json");
        println!("Writing genesis to {}", node_genesis_path.to_string_lossy());
        std::fs::write(node_genesis_path, &genesis_json)?;
    }
    Ok(())
}
