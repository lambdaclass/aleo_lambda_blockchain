use crate::load_credits;
use crate::validator;
use crate::vm;
use anyhow::{anyhow, ensure, Result};
use itertools::Itertools;
use log::debug;
use rand;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        program: Box<vm::Program>,
        verifying_keys: vm::VerifyingKeyMap,
        fee: Option<vm::Transition>,
    },
    Execution {
        id: String,
        transitions: Vec<vm::Transition>,
    },
}

impl Transaction {
    // Used to generate deployment of a new program in path
    pub fn deployment(
        path: &Path,
        private_key: &vm::PrivateKey,
        fee: Option<(u64, vm::Record)>,
    ) -> Result<Self> {
        let program_string = fs::read_to_string(path)?;
        debug!("Deploying program {}", program_string);
        let program = vm::generate_program(&program_string)?;

        // generate program keys (proving and verifying) and keep the verifying one for the deploy
        let verifying_keys = vm::synthesize_program_keys(&program)?
            .into_iter()
            .map(|(i, keys)| (i, keys.1))
            .collect();

        let fee = Self::execute_fee(private_key, fee, 0)?;

        Transaction::Deployment {
            id: "not known yet".to_string(),
            fee,
            program: Box::new(program),
            verifying_keys,
        }
        .set_hashed_id()
    }

    // Used to generate an execution of a program in path or an execution of the credits program
    pub fn execution(
        program: vm::Program,
        function_name: vm::Identifier,
        inputs: &[vm::Value],
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
    ) -> Result<Self> {
        let rng = &mut rand::thread_rng();

        let (proving_key, _) = vm::synthesize_function_keys(&program, rng, &function_name)?;
        let mut transitions = vm::execution(
            program,
            function_name,
            inputs,
            private_key,
            rng,
            proving_key,
        )?;

        // some amount of fees may be implicit if the execution drops credits. in that case, those credits are
        // subtracted from the fees that were requested to be paid.
        let implicit_fees = transitions.iter().map(|transition| transition.fee()).sum();
        if let Some(transition) = Self::execute_fee(private_key, requested_fee, implicit_fees)? {
            transitions.push(transition);
        }

        Self::Execution {
            id: "not known yet".to_string(),
            transitions,
        }
        .set_hashed_id()
    }

    pub fn credits_execution(
        function_name: &str,
        inputs: &[vm::Value],
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
    ) -> Result<Self> {
        let mut transitions = Self::execute_credits(function_name, inputs, private_key)?;

        // some amount of fees may be implicit if the execution drops credits. in that case, those credits are
        // subtracted from the fees that were requested to be paid.
        let implicit_fees = transitions.iter().map(|transition| transition.fee()).sum();
        if let Some(transition) = Self::execute_fee(private_key, requested_fee, implicit_fees)? {
            transitions.push(transition);
        }

        Self::Execution {
            id: "not known yet".to_string(),
            transitions,
        }
        .set_hashed_id()
    }

    pub fn id(&self) -> &str {
        match self {
            Transaction::Deployment { id, .. } => id,
            Transaction::Execution { id, .. } => id,
        }
    }

    pub fn output_records(&self) -> Vec<(vm::Field, vm::EncryptedRecord)> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.output_records())
            .map(|(commitment, record)| (*commitment, record.clone()))
            .collect()
    }

    /// If the transaction is an execution, return the list of input record serial numbers
    pub fn record_serial_numbers(&self) -> Vec<vm::Field> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.serial_numbers().cloned())
            .collect()
    }

    fn transitions(&self) -> Vec<vm::Transition> {
        match self {
            Transaction::Deployment { fee, .. } => {
                if let Some(transition) = fee {
                    vec![transition.clone()]
                } else {
                    vec![]
                }
            }
            Transaction::Execution { transitions, .. } => transitions.clone(),
        }
    }

    /// Return the sum of the transition fees contained in this transition.
    /// For deployments it's the fee of the fee specific transition, if present.
    /// For executions, it's the sum of the fees of all the execution transitions.
    pub fn fees(&self) -> i64 {
        match self {
            Transaction::Deployment { fee, .. } => {
                fee.as_ref().map_or(0, |transition| *transition.fee())
            }
            Transaction::Execution { transitions, .. } => transitions
                .iter()
                .fold(0, |acc, transition| acc + transition.fee()),
        }
    }

    /// Extract a list of validator updates that result from the current execution.
    /// This will return a non-empty vector in case some of the transitions are of the
    /// stake or unstake functions in the credits program.
    pub fn stake_updates(&self) -> Result<Vec<validator::Stake>> {
        let mut result = Vec::new();
        if let Self::Execution { transitions, .. } = self {
            for transition in transitions {
                if transition.program_id().to_string() == "credits.aleo" {
                    let extract_output = |index: usize| {
                        transition
                            .outputs()
                            .get(index)
                            .ok_or_else(|| anyhow!("couldn't find staking output in transition"))
                    };

                    let amount = match transition.function_name().to_string().as_str() {
                        "stake" => vm::int_from_output::<u64>(extract_output(2)?)? as i64,
                        "unstake" => -(vm::int_from_output::<u64>(extract_output(2)?)? as i64),
                        _ => continue,
                    };

                    // TODO: Factor out the following extraction and test it as with the original conversion

                    let validator_key: [u64; 4] = [
                        vm::int_from_output(extract_output(4)?)?,
                        vm::int_from_output(extract_output(5)?)?,
                        vm::int_from_output(extract_output(6)?)?,
                        vm::int_from_output(extract_output(7)?)?,
                    ];

                    let validator = Transaction::validator_key_from_u64s(&validator_key)?;

                    let aleo_address = vm::address_from_output(extract_output(3)?)?;
                    let validator = validator::Stake::new(&validator, aleo_address, amount)?;

                    result.push(validator);
                }
            }
        }
        Ok(result)
    }

    /// If there is some required fee, return the transition resulting of executing
    /// the fee function of the credits program for the requested amount.
    /// The fee function just burns the desired amount of credits, so its effect is just
    /// to produce a difference between the input/output records of its transition.
    fn execute_fee(
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
        implicit_fee: i64,
    ) -> Result<Option<vm::Transition>> {
        if let Some((gates, record)) = requested_fee {
            ensure!(
                implicit_fee >= 0,
                "execution produced a negative fee, cannot create credits"
            );

            if implicit_fee > gates as i64 {
                // already covered by implicit fee, don't spend the record
                return Ok(None);
            }

            let gates = gates as i64 - implicit_fee;
            let inputs = [
                vm::Value::Record(record),
                vm::Value::from_str(&format!("{gates}u64"))?,
            ];

            let transitions = Self::execute_credits("fee", &inputs, private_key)?;
            Ok(Some(transitions.first().unwrap().clone()))
        } else {
            Ok(None)
        }
    }

    fn execute_credits(
        function: &str,
        inputs: &[vm::Value],
        private_key: &vm::PrivateKey,
    ) -> Result<Vec<vm::Transition>> {
        let rng = &mut rand::thread_rng();
        let function = vm::Identifier::from_str(function)?;
        let (program, keys) = load_credits();
        let (proving_key, _) = keys
            .get(&function)
            .ok_or_else(|| anyhow!("credits function not found"))?;

        vm::execution(
            program,
            function,
            inputs,
            private_key,
            rng,
            proving_key.clone(),
        )
    }

    /// Verify that the transaction id is consistent with its contents, by checking it's sha256 hash.
    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.id() == self.hash()?,
            "Corrupted transaction: Inconsistent transaction id"
        );

        Ok(())
    }

    /// Hash the contents of the given enum and return it with the hash as its id.
    fn set_hashed_id(mut self) -> Result<Self> {
        let new_id = self.hash()?;
        match self {
            Transaction::Deployment { ref mut id, .. } => *id = new_id,
            Transaction::Execution { ref mut id, .. } => *id = new_id,
        };
        Ok(self)
    }

    /// Calculate a sha256 hash of the contents of the transaction (dependent on the transaction type)
    fn hash(&self) -> Result<String> {
        let mut hasher = Sha256::new();

        let variant_code: u8 = match self {
            Transaction::Deployment { .. } => 0,
            Transaction::Execution { .. } => 1,
        };
        hasher.update(variant_code.to_be_bytes());

        match self {
            Transaction::Deployment {
                id: _id,
                program,
                verifying_keys,
                fee,
            } => {
                hasher.update(program.id().to_string());

                for (key, value) in verifying_keys.into_iter() {
                    hasher.update(key.to_string());
                    hasher.update(serde_json::to_string(value)?);
                }

                if let Some(fee) = fee {
                    hasher.update(fee.to_string());
                }
            }
            Transaction::Execution {
                id: _id,
                transitions,
            } => {
                for transition in transitions.iter() {
                    hasher.update(serde_json::to_string(transition)?);
                }
            }
        }

        let hash = hasher.finalize().as_slice().to_owned();
        Ok(hex::encode(hash))
    }

    // TODO: Move this to validator set/use tendermint-rs structs for pub keys?
    pub fn validator_key_as_u128s(bytes: &[u8]) -> Result<(u128, u128)> {
        ensure!(
            bytes.len() == 32,
            "Input validator address is not 32 bytes long"
        );
        let high_part: [u8; 16] = bytes[0..16].try_into()?;
        let low_part: [u8; 16] = bytes[16..].try_into()?;

        Ok((
            u128::from_be_bytes(high_part),
            u128::from_be_bytes(low_part),
        ))
    }

    pub fn validator_key_from_u128s(higher: u128, lower: u128) -> Result<String> {
        let mut address = higher.to_be_bytes().to_vec();

        address.append(&mut lower.to_be_bytes().to_vec());
        Ok(base64::encode(address))
    }

    /// Returns a slice of 32 bytes (the size of a Tendermint Public Key) as 4 sections of `u64`s
    /// where the order of the `u64s` is from the most significant to the least significant
    pub fn validator_key_as_u64s(bytes: &[u8]) -> Result<Vec<u64>> {
        ensure!(
            bytes.len() == 32,
            "Input validator address is not 32 bytes long"
        );

        let sections: Vec<u64> = bytes
            .chunks_exact(8)
            .map(|x| u64::from_be_bytes(x.try_into().expect("error converting address into u64s")))
            .collect();

        ensure!(
            sections.len() == 4,
            "Input validator address was incorrectly converted"
        );

        Ok(sections)
    }

    /// Returns a Tendermint Public Key from a slice of 4 `u64`s, where the first `u64`
    /// corresponds to the most significant section of bytes and the order of significance
    /// is descending
    pub fn validator_key_from_u64s(sections: &[u64]) -> Result<String> {
        ensure!(
            sections.len() == 4,
            "Input validator address does not have 4 sections"
        );

        let sections = sections
            .into_iter()
            .flat_map(|x| x.to_be_bytes())
            .collect_vec();
        Ok(base64::encode(&sections))
    }
}

impl std::fmt::Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transaction::Deployment { id, program, .. } => {
                write!(f, "Deployment({},{})", id, program.id())
            }
            Transaction::Execution { id, transitions } => {
                let transition = transitions.first().unwrap();
                let program_id = transition.program_id();
                write!(f, "Execution({program_id},{id})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::transaction::Transaction;

    #[test]
    fn convert_validator_address_u128() {
        let pub_key = "KvYujhwQVoCOH1B3FrmtjSN5GgKUjarOKDNIbWfA8hc=";
        let key_encoded = base64::decode(pub_key).unwrap();
        let (h, l) = Transaction::validator_key_as_u128s(&key_encoded).unwrap();
        assert!(h == 57105825100092210844007095251039268237u128);
        assert!(l == 47151775319435836265997973510082851351u128);

        assert!(Transaction::validator_key_from_u128s(h, l).unwrap() == pub_key);
    }

    #[test]
    fn convert_validator_address_u64() {
        let pub_key = "KvYujhwQVoCOH1B3FrmtjSN5GgKUjarOKDNIbWfA8hc=";
        let key_encoded = base64::decode(pub_key).unwrap();
        let key_sections = Transaction::validator_key_as_u64s(&key_encoded).unwrap();

        let expected_slice = [
            3095712981754861184u64,
            10240992550076394893u64,
            2556102861894036174u64,
            2896738620058694167u64,
        ];

        assert_eq!(key_sections, expected_slice);
        assert!(Transaction::validator_key_from_u64s(&key_sections).unwrap() == pub_key);
    }
}
