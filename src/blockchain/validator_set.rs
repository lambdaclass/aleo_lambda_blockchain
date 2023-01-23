use std::collections::HashMap;

use lib::vm;
use log::{debug, error, warn};

use lib::validator::{Address, Validator, VotingPower};

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
#[derive(Debug)]
pub struct ValidatorSet {
    path: &'static str,
    validators: HashMap<Address, Validator>,
    fees: Fee,
    current_proposer: Option<Address>,
    current_votes: HashMap<Address, VotingPower>,
    current_height: u64,
}

impl ValidatorSet {
    /// Create a new validator set. If a previous validators file is found, populate the set with its contents,
    /// otherwise start with an empty one.
    pub fn new(path: &'static str) -> Self {
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
            path,
            validators,
            current_height: 0,
            fees: 0,
            current_proposer: None,
            current_votes: HashMap::new(),
        }
    }

    /// Replace the entire validator set with the given tendermint pubkey to aleo address mapping.
    /// The mapping is stored to a validators file to pick up across node restarts.
    // FIXME redundant name, reconsider
    pub fn set_validators(&mut self, validators: Vec<Validator>) {
        std::fs::write(self.path, serde_json::to_string(&validators).unwrap()).unwrap();
        let addresses = validators
            .into_iter()
            .map(|validator| {
                debug!("loading validator {}", validator);
                (validator.address(), validator)
            })
            .collect();
        self.validators = addresses;
    }

    /// Updates state based on previous commit votes, to know how awards should be assigned.
    pub fn prepare(
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

        self.current_height = height;
        self.current_proposer = Some(proposer.to_vec());
        self.current_votes = votes;
        self.fees = BASELINE_BLOCK_REWARD;
    }

    /// Add the given amount to the current block collected fees.
    pub fn add(&mut self, fee: u64) {
        self.fees += fee;
    }

    /// Distributes the sum of the block fees plus some baseline block credits
    /// according to some rule, e.g. 50% for the proposer and 50% for validators
    /// weighted by their voting power (which is assumed to be proportional to its stake).
    /// If there are credits left because of rounding errors when dividing by voting power,
    /// they are assigned to the proposer.
    pub fn rewards(&self) -> Vec<(vm::Field, vm::EncryptedRecord)> {
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
                let credits = (voting_power * total_voter_reward) / total_voting_power;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use lib::vm;

    #[ctor::dtor]
    fn shutdown() {
        std::fs::remove_file("abci.validators.test.1").unwrap_or_default();
        std::fs::remove_file("abci.validators.test.2").unwrap_or_default();
        std::fs::remove_file("abci.validators.test.3").unwrap_or_default();
    }

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

        let validator1 =
            Validator::from_str(tmint1, &aleo1.1.to_string(), &aleo1.0.to_string()).unwrap();
        let validator2 =
            Validator::from_str(tmint2, &aleo2.1.to_string(), &aleo2.0.to_string()).unwrap();
        let validator3 =
            Validator::from_str(tmint3, &aleo3.1.to_string(), &aleo3.0.to_string()).unwrap();
        let validator4 =
            Validator::from_str(tmint4, &aleo4.1.to_string(), &aleo4.0.to_string()).unwrap();

        // create validator set, set validators with voting power
        let mut set = ValidatorSet::new("abci.validators.test.1");

        let validators = vec![
            validator1.clone(),
            validator2.clone(),
            validator3.clone(),
            validator4.clone(),
        ];
        set.set_validators(validators);

        // tmint1 is proposer, tmint3 doesn't vote
        let mut votes = HashMap::new();
        votes.insert(validator1.address(), 10);
        votes.insert(validator2.address(), 15);
        votes.insert(validator3.address(), 25);
        let voting_power = 10 + 15 + 25;
        set.prepare(&validator1.address(), votes, 1);

        // add fees
        set.add(20);
        set.add(35);
        let fees = 20 + 35;

        // get rewards
        let records = set.rewards();
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
        set.prepare(&validator4.address(), votes, 2);
        set.add(10);

        let records = set.rewards();
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
        let validator1 =
            Validator::from_str(tmint1, &aleo1.1.to_string(), &aleo1.0.to_string()).unwrap();
        let validator2 =
            Validator::from_str(tmint2, &aleo2.1.to_string(), &aleo2.0.to_string()).unwrap();

        // create validator set, set validators with voting power
        let mut set = ValidatorSet::new("abci.validators.test.1");

        let validators = vec![validator1.clone(), validator2.clone()];
        set.set_validators(validators);

        // tmint1 is proposer and didn't vote
        let mut votes = HashMap::new();
        votes.insert(validator2.address(), 15);
        let voting_power = 15;
        set.prepare(&validator1.address(), votes, 1);

        // add fees
        set.add(35);
        let fees = 35;

        // get rewards
        let records = set.rewards();
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
    #[ignore = "reason"]
    #[allow(clippy::clone_on_copy)]
    fn rewards_are_deterministic() {
        // create 2 different validators with the same amounts
        let tmint1 = "vM+mkdPMvplfxO7wM57z4FXy0TlBC2Onb+MaqcXE8ig=";
        let tmint2 = "2HWbuGk04WQm/CrI/0HxoEtjGY0DXp8oMY6RsyrWwbU=";
        let aleo1 = account_keys();
        let aleo2 = account_keys();
        let validator1 =
            Validator::from_str(tmint1, &aleo1.1.to_string(), &aleo1.0.to_string()).unwrap();
        let validator2 =
            Validator::from_str(tmint2, &aleo2.1.to_string(), &aleo2.0.to_string()).unwrap();
        let validators = vec![validator1.clone(), validator2.clone()];

        let mut set1 = ValidatorSet::new("abci.validators.test.2");
        let mut set2 = ValidatorSet::new("abci.validators.test.3");
        set1.set_validators(validators.clone());
        set2.set_validators(validators);

        let mut votes = HashMap::new();
        votes.insert(validator1.address(), 10);
        votes.insert(validator2.address(), 15);
        set1.prepare(&validator1.address(), votes.clone(), 1);
        set2.prepare(&validator1.address(), votes.clone(), 1);
        set1.add(100);
        set2.add(100);

        let mut records11 = set1.rewards();
        let mut records21 = set2.rewards();
        records11.sort_by_key(|(commitment, _value)| commitment.clone());
        records21.sort_by_key(|(commitment, _value)| commitment.clone());

        // check that the records generated by both validators are the same
        // regardless of the nonce component of the records
        assert_eq!(records11, records21);

        // prepare another block with the same fees, verify that even though
        // the record amounts are the same, the records themselves are not
        set1.prepare(&validator1.address(), votes.clone(), 2);
        set2.prepare(&validator1.address(), votes.clone(), 2);
        set1.add(100);
        set2.add(100);

        let mut records12 = set1.rewards();
        let mut records22 = set2.rewards();
        records12.sort_by_key(|(commitment, _value)| commitment.clone());
        records22.sort_by_key(|(commitment, _value)| commitment.clone());

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
        let validator1 =
            Validator::from_str(tmint1, &aleo1.1.to_string(), &aleo1.0.to_string()).unwrap();
        let validator2 =
            Validator::from_str(tmint2, &aleo2.1.to_string(), &aleo2.0.to_string()).unwrap();

        // create validator set, set validators with voting power
        let mut set = ValidatorSet::new("abci.validators.test.1");

        let validators = vec![validator1.clone(), validator2];
        set.set_validators(validators);

        // in genesis there won't be any previous block votes
        let votes = HashMap::new();
        set.prepare(&validator1.address(), votes, 1);

        set.add(20);
        set.add(35);
        let fees = 20 + 35;

        let records = set.rewards();
        let rewards1 = decrypt_rewards(&aleo1, &records);
        let rewards2 = decrypt_rewards(&aleo2, &records);
        let total_rewards = BASELINE_BLOCK_REWARD + fees;

        // proposer takes all
        assert_eq!(total_rewards, rewards1);
        assert_eq!(0, rewards2);
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
                #[cfg(feature = "vmtropy_backend")]
                let gates = decrypted.gates;
                acc + gates
            })
    }
}
