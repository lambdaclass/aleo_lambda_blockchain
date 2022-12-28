use std::str::FromStr;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::vm;

pub type VotingPower = i64;
pub type Address = Vec<u8>;

// FIXME this is being used to represent both a validator with its current voting power
// and a voting power update to be applied to one such validator.
// separate those in different entities to make the distinction more obvious
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Validator {
    pub aleo_address: vm::Address,
    pub pub_key: tendermint::PublicKey,
    pub voting_power: VotingPower,
}

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(vm::Field, vm::EncryptedRecord)>,
    pub validators: Vec<Validator>,
}

impl Validator {
    /// Construct a new validator update from a base64 encoded ed25519 public key string (as it appears in tendermint JSON files)
    /// And an Aleo address string.
    pub fn from_str(pub_key: &str, aleo_address: &str, voting_power: VotingPower) -> Result<Self> {
        let aleo_address = vm::Address::from_str(aleo_address)?;
        let pub_key = tendermint::PublicKey::from_raw_ed25519(&base64::decode(pub_key)?)
            .ok_or_else(|| anyhow!("failed to generate tendermint public key"))?;
        Ok(Self::new(pub_key, aleo_address, voting_power))
    }

    fn new(
        pub_key: tendermint::PublicKey,
        aleo_address: vm::Address,
        voting_power: VotingPower,
    ) -> Self {
        Self {
            pub_key,
            aleo_address,
            voting_power,
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
