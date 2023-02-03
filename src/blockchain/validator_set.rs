use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use lib::vm;
use log::{debug, error, warn};

use anyhow::{anyhow, Result};
use lib::validator::{Address, Stake, Validator, VotingPower};

type Fee = u64;

/// There's a baseline for the credits distributed among validators, in addition to fees.
/// For now it's constant, but it could be made to decrease based on height to control inflation.
const BASELINE_BLOCK_REWARD: Fee = 100;
/// The portion of the total block rewards that is given to the block proposer. The rest is distributed
/// among voters weighted by their voting power.
const PROPOSER_REWARD_PERCENTAGE: u64 = 50;

/// Tracks the network validator set, particularly how the tendermint addresses map to
/// aleo account addresses needed to assign credits records for validator rewards.
/// The ValidatorSet exposes methods to collect fees and has logic to distribute them
/// (in addition to a baseline reward), based on block proposer and voting power.
/// There are also methods to apply voting power changes on staking transactions.
#[derive(Debug)]
pub struct ValidatorSet {
    /// Path to the file used to persist the currently known validator list of validator, so the app works across restarts.
    path: PathBuf,
    /// The currently known validator set, including the terndermint pub key/address to aleo account mapping
    /// and their last known voting power.
    validators: HashMap<Address, Validator>,
    /// The fees collected for the current block.
    fees: Fee,
    /// The proposer of the current block.
    current_proposer: Option<Address>,
    /// The previous round block votes, to be considered to distribute this block's rewards.
    current_votes: HashMap<Address, VotingPower>,
    /// The current block's height, used as a seed to generate reward records deterministically across nodes.
    current_height: u64,
    /// The list of validators that had voting power changes during the current block, including added or removed ones.
    updated_validators: HashSet<Address>,
}

impl ValidatorSet {
    /// Create a new validator set. If a previous validators file is found, populate the set with its contents,
    /// otherwise start with an empty one.
    pub fn load_or_create(path: &Path) -> Self {
        let validators = if let Ok(json) = std::fs::read_to_string(path) {
            serde_json::from_str::<Vec<Validator>>(&json)
                .expect("validators file content is invalid")
                .into_iter()
                .map(|validator| {
                    debug!("loading validator {}", validator);
                    (validator.address(), validator)
                })
                .collect()
        } else {
            HashMap::new()
        };

        Self {
            path: path.into(),
            validators,
            current_height: 0,
            fees: 0,
            current_proposer: None,
            current_votes: HashMap::new(),
            updated_validators: HashSet::new(),
        }
    }

    pub fn replace(&mut self, validators: Vec<Validator>) {
        self.validators = validators
            .into_iter()
            .map(|validator| (validator.address(), validator))
            .collect()
    }

    /// Updates state based on previous commit votes, to know how awards should be assigned.
    pub fn begin_block(
        &mut self,
        proposer: &Address,
        votes: HashMap<Address, VotingPower>,
        height: u64,
    ) {
        if !self.validators.contains_key(proposer) {
            error!(
                "received unknown address as proposer {}",
                hex::encode_upper(proposer)
            );
        }

        for voter in votes.keys() {
            if !self.validators.contains_key(voter) {
                error!(
                    "received unknown address as voter {}",
                    hex::encode_upper(voter)
                );
            }
        }

        self.updated_validators = HashSet::new();
        self.current_height = height;
        self.current_proposer = Some(proposer.to_vec());
        // note that we rely on voting power for a given round as informed by tendermint as opposed to
        // using the one tracked in self.validators. This is because the voting power on the informed round
        // may not be the same as the last known one (e.g. there could be staking changes already applied
        // to self.validators that will take some rounds before affecting the consensus voting).
        self.current_votes = votes;
        self.fees = BASELINE_BLOCK_REWARD;
    }

    /// Return whether is valid to apply the given validator update, e.g.
    /// there's enough voting power to unstake and the tendermint and aleo addresses
    /// the known mappings. This takes into account pending updates if any, so it's safe
    /// to use both during lightweight mempool checks (check_tx) and transaction delivery (deliver_tx).
    pub fn validate(&self, update: &Stake) -> Result<()> {
        if let Some(validator) = self.validators.get(&update.validator_address()) {
            // this is an already known validator, try to apply the staking update and see if it succeeds
            validator.clone().apply(update)?;
        } else {
            // this is a new validator
            Validator::from_stake(update)?;
        };
        Ok(())
    }

    /// Add or update the given validator and its voting power.
    /// Assumes this update has been validated previously with is_valid_update.
    pub fn apply(&mut self, update: Stake) {
        // mark as updated so its included in the pending updates result
        self.updated_validators.insert(update.validator_address());

        // note that this could leave a validator with zero voting power, which will instruct
        // tendermint to remove it, but we still need to keep it around since we can receive
        // votes from that validator on subsequent rounds.
        self.validators
            .entry(update.validator_address())
            .and_modify(|validator| {
                validator
                    .apply(&update)
                    .expect("attempted to apply an invalid update")
            })
            .or_insert_with(|| {
                Validator::from_stake(&update).expect("attempted to apply an invalid update")
            });
    }

    /// Add the given amount to the current block collected fees.
    pub fn collect(&mut self, fee: u64) {
        self.fees += fee;
    }

    /// Return the list of validators that have been updated by transactions in the current block.
    pub fn pending_updates(&self) -> Vec<Validator> {
        self.updated_validators
            .iter()
            .fold(Vec::new(), |mut acc, address| {
                acc.push(
                    self.validators
                        .get(address)
                        .expect("missing updated validator")
                        .clone(),
                );
                acc
            })
    }

    /// Distributes the sum of the block fees plus some baseline block credits
    /// according to some rule, e.g. 50% for the proposer and 50% for validators
    /// weighted by their voting power (which is assumed to be proportional to its stake).
    /// If there are credits left because of rounding errors when dividing by voting power,
    /// they are assigned to the proposer.
    pub fn block_rewards(&self) -> Vec<(vm::Field, vm::EncryptedRecord)> {
        if let Some(proposer) = &self.current_proposer {
            // first calculate which part of the total belongs to voters
            let voter_reward_percentage = 100 - PROPOSER_REWARD_PERCENTAGE;
            let total_voter_reward = (self.fees * voter_reward_percentage) / 100;
            let total_voting_power = self
                .current_votes
                .iter()
                .fold(0, |accum, (_address, power)| accum + power);
            debug!(
                "total block rewards: {}, total voting power: {}, total voter rewards: {}",
                self.fees, total_voting_power, total_voter_reward
            );

            // calculate how much belongs to each validator, proportional to its voting power
            let mut remaining_fees = self.fees;
            let mut rewards = HashMap::new();
            for (address, voting_power) in &self.current_votes {
                let credits = (*voting_power * total_voter_reward) / total_voting_power;
                remaining_fees -= credits;
                rewards.insert(address, credits);
            }

            // What's left of the fees, goes to the proposer.
            // This should be roughly PROPOSER_REWARD_PERCENTAGE plus some leftover because
            // of rounding errors when distributing based on voting power above
            debug!(
                "{} is current round proposer",
                self.validators
                    .get(proposer)
                    .expect("proposer not found in address map")
            );
            *rewards.entry(proposer).or_default() += remaining_fees;

            assert_eq!(
                self.fees,
                rewards.values().sum::<u64>(),
                "the sum of rewarded credits is different than the fees: {rewards:?}"
            );

            // generate credits records based on the rewards
            let mut output_records = Vec::new();
            for (address, credits) in rewards {
                let validator = self
                    .validators
                    .get(address)
                    .expect("validator address not found");

                debug!(
                    "Assigning {credits} credits to {validator} (voting power {})",
                    self.current_votes.get(address).unwrap_or(&0)
                );

                let record = vm::mint_record(
                    "credits.aleo",
                    "credits",
                    &validator.aleo_address,
                    credits,
                    self.current_height,
                )
                .expect("Couldn't mint credit records for reward");

                output_records.push(record);
            }

            output_records
        } else {
            warn!("no proposer on this round, skipping rewards");
            Vec::new()
        }
    }

    /// Saves the currently known list of validators to disk.
    pub fn commit(&mut self) -> Result<()> {
        let validators_vec: Vec<Validator> = self.validators.values().cloned().collect();
        let json = serde_json::to_string(&validators_vec).expect("couldn't serialize validators");
        std::fs::write(&self.path, json)
            .map_err(|e| anyhow!("failed to write validators file {:?} {e}", self.path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::NamedTempFile;
    use lib::vm;

    #[test]
    fn generate_rewards() {
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let tmint3 = "TtJ9B7yGXANFIJqH2LJO8JN6M2WOn2w7sRN0HHi14UE=";
        let tmint4 = "uHC9buPyVi5GT8dohO1OQ+HlfKQ1HwUHAyv3AjKKsZQ=";

        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let aleo3 = account_keys();
        let aleo4 = account_keys();

        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 1).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 1).unwrap();
        let validator3 = Validator::from_str(tmint3, &aleo3.1.to_string(), 1).unwrap();
        let validator4 = Validator::from_str(tmint4, &aleo4.1.to_string(), 1).unwrap();

        // create validator set, set validators with voting power
        let tempfile = NamedTempFile::new("validators").unwrap();
        let mut set = ValidatorSet::load_or_create(tempfile.path());
        set.replace(vec![
            validator1.clone(),
            validator2.clone(),
            validator3.clone(),
            validator4.clone(),
        ]);

        // tmint1 is proposer, tmint3 doesn't vote
        let mut votes = HashMap::new();
        votes.insert(validator1.address(), 10);
        votes.insert(validator2.address(), 15);
        votes.insert(validator3.address(), 25);
        let voting_power = 10 + 15 + 25;
        set.begin_block(&validator1.address(), votes, 1);

        // add fees
        set.collect(20);
        set.collect(35);
        let fees = 20 + 35;

        // get rewards
        let records = set.block_rewards();
        let rewards1 = decrypt_rewards(&aleo1, &records);
        let rewards2 = decrypt_rewards(&aleo2, &records);
        let rewards3 = decrypt_rewards(&aleo3, &records);
        let rewards4 = decrypt_rewards(&aleo4, &records);

        // check proposer gets 50% and the rest is distributed according to vote power
        let total_rewards = BASELINE_BLOCK_REWARD + fees;
        let voter_rewards = total_rewards * PROPOSER_REWARD_PERCENTAGE / 100;

        // ensure the no credits are lost in the process
        assert_eq!(total_rewards, rewards1 + rewards2 + rewards3);

        // non-proposers receive credits proportional to their voting power
        assert_eq!(voter_rewards * 15 / voting_power, rewards2);
        assert_eq!(voter_rewards * 25 / voting_power, rewards3);
        assert_eq!(0, rewards4);

        // proposer gets PROPOSER_REWARD_PERCENTAGE + a part proportional to their voting power + what's left because of rounding
        // so, basically, all the rest
        assert_eq!(total_rewards - rewards2 - rewards3, rewards1);

        // run another block with different votes, rewards start from scratch
        let mut votes = HashMap::new();
        votes.insert(validator4.address(), 10);
        set.begin_block(&validator4.address(), votes, 2);
        set.collect(10);

        let records = set.block_rewards();
        let rewards1 = decrypt_rewards(&aleo1, &records);
        let rewards2 = decrypt_rewards(&aleo2, &records);
        let rewards3 = decrypt_rewards(&aleo3, &records);
        let rewards4 = decrypt_rewards(&aleo4, &records);
        assert_eq!(0, rewards1);
        assert_eq!(0, rewards2);
        assert_eq!(0, rewards3);
        assert_eq!(BASELINE_BLOCK_REWARD + 10, rewards4);
    }

    #[test]
    fn current_proposer_hadnt_vote() {
        // the current round proposer for some reason may not have voted on the previous round
        // we've seen this happening at cluster start. This test exercises that case to make
        // sure we don't rely on the proposer address being included in the current votes

        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 1).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 1).unwrap();

        // create validator set, set validators with voting power
        let tempfile = NamedTempFile::new("validators").unwrap();
        let mut set = ValidatorSet::load_or_create(tempfile.path());
        set.replace(vec![validator1.clone(), validator2.clone()]);

        // tmint1 is proposer and didn't vote
        let mut votes = HashMap::new();
        votes.insert(validator2.address(), 15);
        let voting_power = 15;
        set.begin_block(&validator1.address(), votes, 1);

        // add fees
        set.collect(35);
        let fees = 35;

        // get rewards
        let records = set.block_rewards();
        let rewards1 = decrypt_rewards(&aleo1, &records);
        let rewards2 = decrypt_rewards(&aleo2, &records);

        // check proposer gets 50% and the rest is distributed according to vote power
        let total_rewards = BASELINE_BLOCK_REWARD + fees;
        let voter_rewards = total_rewards * PROPOSER_REWARD_PERCENTAGE / 100;

        // ensure the no credits are lost in the process
        assert_eq!(total_rewards, rewards1 + rewards2);

        // non-proposers receive credits proportional to their voting power
        assert_eq!(voter_rewards * 15 / voting_power, rewards2);
        assert_eq!(total_rewards - rewards2, rewards1);
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn rewards_are_deterministic() {
        // create 2 different validators with the same amounts
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 1).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 1).unwrap();
        let validators = vec![validator1.clone(), validator2.clone()];

        let tempfile1 = NamedTempFile::new("validators").unwrap();
        let tempfile2 = NamedTempFile::new("validators").unwrap();
        let mut set1 = ValidatorSet::load_or_create(tempfile1.path());
        let mut set2 = ValidatorSet::load_or_create(tempfile2.path());
        set1.replace(validators.clone());
        set2.replace(validators);

        let mut votes = HashMap::new();
        votes.insert(validator1.address(), 10);
        votes.insert(validator2.address(), 15);
        set1.begin_block(&validator1.address(), votes.clone(), 1);
        set2.begin_block(&validator1.address(), votes.clone(), 1);
        set1.collect(100);
        set2.collect(100);

        let mut records11 = set1.block_rewards();
        let mut records21 = set2.block_rewards();
        records11.sort_by_key(|k| k.0.clone());
        records21.sort_by_key(|k| k.0.clone());

        // check that the records generated by both validators are the same
        // regardless of the nonce component of the records
        assert_eq!(records11, records21);

        // prepare another block with the same fees, verify that even though
        // the record amounts are the same, the records themselves are not
        set1.begin_block(&validator1.address(), votes.clone(), 2);
        set2.begin_block(&validator1.address(), votes.clone(), 2);
        set1.collect(100);
        set2.collect(100);

        let mut records12 = set1.block_rewards();
        let mut records22 = set2.block_rewards();
        records12.sort_by_key(|k| k.0.clone());
        records22.sort_by_key(|k| k.0.clone());

        // both validators see the same for this round
        assert_eq!(records12, records22);
        // but the records are not equal to the previous one
        assert_ne!(records11, records12);
        assert_ne!(records21, records22);

        // the gates inside the records are the same
        let rewards111 = decrypt_rewards(&aleo1, &records11);
        let rewards121 = decrypt_rewards(&aleo1, &records12);
        assert_eq!(rewards111, rewards121);
    }

    #[test]
    fn genesis_rewards() {
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 1).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 1).unwrap();

        // create validator set, set validators with voting power
        let tempfile = NamedTempFile::new("validators").unwrap();
        let mut set = ValidatorSet::load_or_create(tempfile.path());
        set.replace(vec![validator1.clone(), validator2]);

        // in genesis there won't be any previous block votes
        let votes = HashMap::new();
        set.begin_block(&validator1.address(), votes, 1);

        set.collect(20);
        set.collect(35);
        let fees = 20 + 35;

        let records = set.block_rewards();
        let rewards1 = decrypt_rewards(&aleo1, &records);
        let rewards2 = decrypt_rewards(&aleo2, &records);
        let total_rewards = BASELINE_BLOCK_REWARD + fees;

        // proposer takes all
        assert_eq!(total_rewards, rewards1);
        assert_eq!(0, rewards2);
    }

    #[test]
    fn add_update_validators() {
        // create set and setup initial 2 validators
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let tmint3 = "TtJ9B7yGXANFIJqH2LJO8JN6M2WOn2w7sRN0HHi14UE=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let aleo3 = account_keys();
        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 1).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 1).unwrap();

        // create validator set, set validators with voting power
        let tempfile = NamedTempFile::new("validators").unwrap();
        let mut set = ValidatorSet::load_or_create(tempfile.path());
        set.replace(vec![validator1.clone(), validator2]);

        // votes/begin block/commit
        let mut votes = HashMap::new();
        votes.insert(validator1.address(), 15);
        set.begin_block(&validator1.address(), votes, 1);
        // no updates on this round (should ignore default ones from before begin block)
        assert_eq!(0, set.pending_updates().len());
        let _records = set.block_rewards();
        set.commit().unwrap();

        // votes/begin block
        let mut votes = HashMap::new();
        votes.insert(validator1.address(), 15);
        set.begin_block(&validator1.address(), votes, 1);

        // add a new validator, update voting power of a previous one
        let stake3 = Stake::new(tmint3, aleo3.1, 1).unwrap();
        let stake2 = Stake::new(tmint2, aleo2.1, 5).unwrap();
        set.apply(stake3.clone());
        set.apply(stake2.clone());

        // pending updates includes the two given
        let mut updates = set.pending_updates();
        updates.sort_by_key(|v| v.voting_power);
        assert_eq!(2, updates.len());
        assert_eq!(stake3.validator_address(), updates[0].address());
        assert_eq!(1, updates[0].voting_power);
        assert_eq!(stake2.validator_address(), updates[1].address());
        assert_eq!(6, updates[1].voting_power);

        let _records = set.block_rewards();
        set.commit().unwrap();
    }

    #[test]
    fn remove_validators() {
        // create set and setup initial 2 validators
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 5).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 5).unwrap();

        let tempfile = NamedTempFile::new("validators").unwrap();
        let mut set = ValidatorSet::load_or_create(tempfile.path());
        set.replace(vec![validator1, validator2.clone()]);

        // votes/begin block
        let mut votes = HashMap::new();
        votes.insert(validator2.address(), 5);
        set.begin_block(&validator2.address(), votes, 1);

        // remove stake but not enough to remove validator
        let stake2 = Stake::new(tmint2, aleo2.1, -3).unwrap();
        set.apply(stake2.clone());

        // pending updates includes the updated
        let updates = set.pending_updates();
        assert_eq!(1, updates.len());
        assert_eq!(stake2.validator_address(), updates[0].address());
        assert_eq!(2, updates[0].voting_power);

        let _records = set.block_rewards();
        set.commit().unwrap();

        // votes/begin block
        let mut votes = HashMap::new();
        votes.insert(validator2.address(), 5);
        set.begin_block(&validator2.address(), votes, 1);

        // remove remaining stake
        let stake2 = Stake::new(tmint2, aleo2.1, -2).unwrap();
        set.apply(stake2.clone());

        // pending updates includes the removed
        let updates = set.pending_updates();
        assert_eq!(1, updates.len());
        assert_eq!(stake2.validator_address(), updates[0].address());
        assert_eq!(0, updates[0].voting_power);

        // get rewards check as expected, include removed
        let _records = set.block_rewards();
        set.commit().unwrap();

        // votes/begin block, shouldn't fail even if it includes votes from removed one
        let mut votes = HashMap::new();
        votes.insert(validator2.address(), 5);
        set.begin_block(&validator2.address(), votes, 1);
        assert_eq!(0, set.pending_updates().len());
        let _records = set.block_rewards();
        set.commit().unwrap();
    }

    #[test]
    fn validators_update_validations() {
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let validator1 = Validator::from_str(tmint1, &aleo1.1.to_string(), 5).unwrap();
        let validator2 = Validator::from_str(tmint2, &aleo2.1.to_string(), 5).unwrap();

        let tempfile = NamedTempFile::new("validators").unwrap();
        let mut set = ValidatorSet::load_or_create(tempfile.path());
        let validators = vec![validator1, validator2];
        set.replace(validators);

        // invalid when aleo address doesn't match previously known one
        let aleo2_fake = account_keys();
        let validator2_fake = Stake::new(tmint2, aleo2_fake.1, 5).unwrap();
        let error = set.validate(&validator2_fake).unwrap_err();
        assert!(error
            .to_string()
            .contains("attempted to apply a staking update on a different aleo account"));

        // invalid on new one and negative voting
        let tmint3 = "TtJ9B7yGXANFIJqH2LJO8JN6M2WOn2w7sRN0HHi14UE=";
        let aleo3 = account_keys();
        let validator3 = Stake::new(tmint3, aleo3.1, -5).unwrap();
        let error = set.validate(&validator3).unwrap_err();
        assert_eq!(
            "cannot create a validator with negative voting power",
            error.to_string()
        );

        // invalid on zero power new
        let error = Stake::new(tmint3, aleo3.1, 0).unwrap_err();
        assert_eq!("can't stake zero credits", error.to_string());

        // invalid on negative voting more than available
        let validator2 = Stake::new(tmint2, aleo2.1, -6).unwrap();
        let error = set.validate(&validator2).unwrap_err();
        assert!(error
            .to_string()
            .contains("attempted to unstake more voting power than available"));
    }

    pub fn account_keys() -> (vm::ViewKey, vm::Address) {
        let private_key = vm::PrivateKey::new(&mut rand::thread_rng()).unwrap();
        let view_key = vm::ViewKey::try_from(&private_key).unwrap();
        let address = vm::Address::try_from(&view_key).unwrap();
        (view_key, address)
    }

    fn decrypt_rewards(
        owner: &(vm::ViewKey, vm::Address),
        rewards: &[(vm::Field, vm::EncryptedRecord)],
    ) -> u64 {
        rewards
            .iter()
            .filter(|(_, record)| record.is_owner(&owner.1, &owner.0))
            .fold(0, |acc, (_, record)| {
                let decrypted = record.decrypt(&owner.0).unwrap();
                #[cfg(feature = "snarkvm_backend")]
                let gates = ***decrypted.gates();
                #[cfg(feature = "lambdavm_backend")]
                let gates = decrypted.gates;
                acc + gates
            })
    }
}
