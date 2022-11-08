use anyhow::{anyhow, Result};
use log::info;
use serde::{Deserialize, Serialize};
use snarkvm::prelude::Testnet3;
use snarkvm::prelude::{Address, PrivateKey, ViewKey};
use std::fs;
use std::path::PathBuf;

/// File that stores the public and private keys associated with an account
#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub private_key: PrivateKey<Testnet3>,
    pub view_key: ViewKey<Testnet3>,
    pub address: Address<Testnet3>,
}

impl Credentials {
    pub fn new() -> Result<Self> {
        let private_key = PrivateKey::<Testnet3>::new(&mut rand::thread_rng())?;
        let view_key = ViewKey::try_from(&private_key)?;
        let address = Address::try_from(&view_key)?;
        Ok(Self {
            private_key,
            view_key,
            address,
        })
    }

    pub fn save(&self, file: Option<PathBuf>) -> Result<()> {
        let file = file.unwrap_or_else(Self::default_file);
        let dir = file.parent().unwrap();
        fs::create_dir_all(dir)?;
        info!("Saving credentials to {}", file.to_string_lossy());
        let account_json = serde_json::to_string(&self)?;
        fs::write(file, account_json)?;
        Ok(())
    }

    pub fn load(file: Option<PathBuf>) -> Result<Self> {
        let account_json = fs::read_to_string(file.unwrap_or_else(Self::default_file))?;
        serde_json::from_str(&account_json).map_err(|e| anyhow!(e))
    }

    fn default_file() -> PathBuf {
        dirs::home_dir().unwrap().join(".aleo/account.json")
    }
}
