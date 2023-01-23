use std::str::FromStr;

use anyhow::{anyhow, ensure, Result};
use log::debug;
use serde::{Deserialize, Serialize};

use crate::vm;

pub type VotingPower = u64;
pub type Address = Vec<u8>;

/// Represents a validator node in the blockchain with a given voting power for the consensus
/// protocol. Each validator has an associated tendermint public key and an aleo account.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Validator {
    pub aleo_address: vm::Address,
    pub pub_key: tendermint::PublicKey,
    pub voting_power: VotingPower,
}

/// Represents an amount of credits (positive or negative) that are staked on a specific validator.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Stake {
    aleo_address: vm::Address,
    pub_key: tendermint::PublicKey,
    gates_delta: i64,
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
        Ok(Self {
            pub_key: parse_pub_key(pub_key)?,
            aleo_address,
            voting_power,
        })
    }

    /// Instantiate a validator from the given initial stake, which should be positive.
    pub fn from_stake(stake: &Stake) -> Result<Self> {
        ensure!(
            stake.gates_delta > 0,
            "cannot create a validator with negative voting power"
        );
        Ok(Self {
            aleo_address: stake.aleo_address,
            pub_key: stake.pub_key,
            voting_power: stake.gates_delta as u64,
        })
    }

    /// Update the validator voting power based on the given change in stake.
    /// It will fail if the stake belongs to a different validator or if more stake than
    /// available is attempted to be removed.
    pub fn apply(&mut self, stake: &Stake) -> Result<()> {
        ensure!(
            self.address() == stake.validator_address(),
            "attempted to apply a staking update on a different validator. expected {} received {}",
            self,
            stake
        );

        ensure!(self.aleo_address == stake.aleo_address,
                "attempted to apply a staking update on a different aleo account. expected {} received {}",
                self.aleo_address, stake.aleo_address);

        let new_power = self.voting_power as i64 + stake.gates_delta;
        ensure!(
            new_power >= 0,
            "attempted to unstake more voting power than available for {self}"
        );
        self.voting_power = new_power as u64;

        Ok(())
    }

    /// Return the tendermint validator address (which is derived from its public key) as bytes.
    pub fn address(&self) -> Address {
        pub_key_to_address(&self.pub_key)
    }
}

impl Stake {
    /// Construct a stake of a given amount (positive or negative) for a specific validator.
    /// identified by its base64 encoded ed25519 public key string  and aleo address.
    pub fn new(pub_key: &str, aleo_address: vm::Address, gates_delta: i64) -> Result<Self> {
        ensure!(gates_delta != 0, "can't stake zero credits");
        Ok(Self {
            pub_key: parse_pub_key(pub_key)?,
            aleo_address,
            gates_delta,
        })
    }

    /// Return the tendermint validator address (which is derived from its public key) as bytes.
    pub fn validator_address(&self) -> Address {
        pub_key_to_address(&self.pub_key)
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

impl std::fmt::Display for Stake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}/{}",
            hex::encode_upper(self.validator_address()),
            self.aleo_address
        )
    }
}

fn parse_pub_key(key: &str) -> Result<tendermint::PublicKey> {
    debug!("key: {}", key);
    tendermint::PublicKey::from_raw_ed25519(&base64::decode(key)?)
        .ok_or_else(|| anyhow!("failed to generate tendermint public key"))
}

fn pub_key_to_address(key: &tendermint::PublicKey) -> Address {
    tendermint::account::Id::from(key.ed25519().expect("unsupported public key type"))
        .as_bytes()
        .to_vec()
}
