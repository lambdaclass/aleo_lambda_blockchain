/// Binary that walks a list of tendermint node directories (like the default ~/.tendermint or a testnet generated node dir),
/// assuming they also contain an aleo account credentials file, and updates their genesis files to include the genesis state
/// expected by our abci app.
use std::{collections::HashMap, path::PathBuf, str::FromStr};

use anyhow::Result;
use clap::Parser;
use lib::{GenesisState, jaleo};

/// Takes a list of node directories and updates the genesis files on each of them
/// to include records to assign default credits to each validator and a mapping
/// of tendermint validator address to aleo account address.
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

    // for each node in the testnet, map its tendermint address to its aleo account address
    // and generate records for initial validator credits
    let mut address_map = HashMap::new();
    let mut genesis_records = Vec::new();
    for node_dir in cli.node_dirs.clone() {
        println!("processing {}", node_dir.to_string_lossy());

        let aleo_account_path = node_dir.join("account.json");
        let aleo_account: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(aleo_account_path)?)?;
        let aleo_address = jaleo::Address::from_str(aleo_account["address"].as_str().unwrap())?;

        let tmint_account_path = node_dir.join("config/priv_validator_key.json");
        let tmint_account: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmint_account_path)?)?;
        let tmint_address = tmint_account["address"].as_str().unwrap();

        address_map.insert(tmint_address.to_string(), aleo_address);

        println!("Generating record for {aleo_address}");
        // NOTE: using a hardcoded seed, not for production!
        let record = jaleo::mint_credits(&aleo_address, cli.amount)?;
        genesis_records.push(record);
    }

    // update the genesis JSON with the calculated app state
    let genesis_path = cli
        .node_dirs
        .first()
        .expect("need at least one directory")
        .join("config/genesis.json");
    let mut genesis: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(genesis_path)?)?;
    let genesis_state = GenesisState {
        records: genesis_records,
        validators: address_map,
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
