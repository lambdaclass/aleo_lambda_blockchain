use std::str::FromStr;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::vm;

pub type VotingPower = u64;
pub type Address = Vec<u8>;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Validator {
    pub aleo_address: vm::Address,
    pub aleo_view_key: vm::ViewKey,
    pub_key: tendermint::PublicKey,
    voting_power: VotingPower,
}

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(vm::Field, vm::EncryptedRecord)>,
    pub validators: Vec<Validator>,
}

impl Validator {
    /// Construct a new validator from a base64 encoded ed25519 public key string (as it appears in tendermint JSON files)
    /// And an Aleo address string.
    pub fn from_str(pub_key: &str, aleo_address: &str, aleo_view_key: &str) -> Result<Self> {
        let aleo_address = vm::Address::from_str(aleo_address)?;
        let aleo_view_key = vm::ViewKey::from_str(aleo_view_key)?;
        let pub_key = tendermint::PublicKey::from_raw_ed25519(&base64::decode(pub_key)?)
            .ok_or_else(|| anyhow!("failed to generate tendermint public key"))?;
        Ok(Self::new(pub_key, aleo_address, aleo_view_key))
    }

    fn new(
        pub_key: tendermint::PublicKey,
        aleo_address: vm::Address,
        aleo_view_key: vm::ViewKey,
    ) -> Self {
        Self {
            pub_key,
            aleo_address,
            aleo_view_key,
            voting_power: 0,
        }
    }

    /// Return the tendermint validator address (which is derived from its public key) as bytes.
    pub fn address(&self) -> Address {
        tendermint::account::Id::from(self.pub_key.ed25519().expect("unsupported public key type"))
            .as_bytes()
            .to_vec()
    }

    pub fn voting_power(&self) -> VotingPower {
        self.voting_power
    }

    /// Add the given amount, which can be negative, to the validator's current voting power.
    /// Return an error if more than available is attempted to be subtracted.
    pub fn add_voting_power(&mut self, diff: i64) -> Result<VotingPower> {
        let new_power = self.voting_power as i64 + diff;
        if new_power < 0 {
            bail!("can't set a negative voting power")
        }
        self.voting_power = new_power as u64;
        Ok(self.voting_power)
    }
}

impl std::hash::Hash for Validator {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write(&self.address())
    }
}

impl std::fmt::Display for Validator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}",
            hex::encode_upper(self.address()),
            self.aleo_address
        )
    }
}
