use anyhow::{anyhow, Result};
use lib::vm;
use log::debug;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
/// File that stores the public and private keys associated with an account.
/// Stores it at $ALEO_HOME/account.json, with ~/.aleo as the default ALEO_HOME.
#[derive(Serialize, Deserialize)]
pub struct Credentials {
    pub private_key: vm::PrivateKey,
    pub view_key: vm::ViewKey,
    pub address: vm::Address,
}

impl Credentials {
    pub fn new() -> Result<Self> {
        let private_key = vm::PrivateKey::new(&mut rand::thread_rng())?;
        let view_key = vm::ViewKey::try_from(&private_key)?;
        let address = vm::Address::try_from(&view_key)?;
        Ok(Self {
            private_key,
            view_key,
            address,
        })
    }

    pub fn save(&self) -> Result<PathBuf> {
        let file = Self::path();
        let dir = file.parent().unwrap();
        fs::create_dir_all(dir)?;
        debug!("Saving credentials to {}", file.to_string_lossy());
        let account_json = serde_json::to_string(&self)?;
        fs::write(file.clone(), account_json)?;
        Ok(file)
    }

    pub fn load() -> Result<Self> {
        let account_json = fs::read_to_string(Self::path())?;
        serde_json::from_str(&account_json).map_err(|e| anyhow!(e))
    }

    fn path() -> PathBuf {
        lib::aleo_home().join("account.json")
    }
}
