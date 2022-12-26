use crate::load_credits;
use crate::vm::{self};
use anyhow::{ensure, Result};
use indexmap::IndexMap;
use log::debug;
use serde::{Deserialize, Serialize};
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

        /// The pub key of the tendermint validator that staked credits go to,
        /// in case this is a staking/unstaking execution.
        // NOTE: for most purposes the stake/unstake transactions are just another type of/ execution.
        // we could handle most of the special properties as methods in Transaction, but since the vm
        // doesn't have support for a type where we could fit the validator address, we need to treat it
        // as a special case and pass it separately (which also prevents us to include that address in the
        // execution proof).
        // also NOTE: having the address out of the record and as a separate field means that we can't ensure
        // the voting power gets removed from the same tendermint validator that did the staking in the first place.
        // we may work around that but ideally having the tendermint address in the record would be preferable
        validator: Option<String>,
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

        // generate program keys (proving and verifying) and keep the verifying one for the deploy
        let (program, program_build) = vm::build_program(&program_string)?;

        let verifying_keys: IndexMap<String, crate::vm::VerifyingKey> = program_build
            .map
            .into_iter()
            .map(|(i, keys)| (i, keys.1))
            .collect();

        let id = uuid::Uuid::new_v4().to_string();
        let fee = Self::execute_fee(private_key, fee, 0)?;
        Ok(Transaction::Deployment {
            id,
            fee,
            program: Box::new(program),
            verifying_keys: crate::vm::VerifyingKeyMap {
                map: verifying_keys,
            },
        })
    }

    // Used to generate an execution of a program in path or an execution of the credits program
    pub fn execution(
        program: vm::Program,
        function_name: vm::Identifier,
        inputs: &[vm::UserInputValueType],
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
    ) -> Result<Self> {
        let mut transitions = vm::execution(program, function_name, inputs, private_key)?;

        // some amount of fees may be implicit if the execution drops credits. in that case, those credits are
        // subtracted from the fees that were requested to be paid.
        let implicit_fees = transitions.iter().map(|transition| transition.fee).sum();
        if let Some(transition) = Self::execute_fee(private_key, requested_fee, implicit_fees)? {
            transitions.push(transition);
        }

        let id = uuid::Uuid::new_v4().to_string();

        Ok(Self::Execution {
            id,
            transitions,
            validator: None,
        })
    }

    pub fn credits_execution(
        function_name: vm::Identifier,
        inputs: &[vm::UserInputValueType],
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
        validator: Option<String>,
    ) -> Result<Self> {
        let program = vm::Program::credits()?;

        let mut transitions = vm::execution(program, function_name, inputs, private_key)?;

        // some amount of fees may be implicit if the execution drops credits. in that case, those credits are
        // subtracted from the fees that were requested to be paid.
        let implicit_fees = transitions.iter().map(|transition| transition.fee).sum();
        if let Some(transition) = Self::execute_fee(private_key, requested_fee, implicit_fees)? {
            transitions.push(transition);
        }

        let id = uuid::Uuid::new_v4().to_string();
        Ok(Self::Execution {
            id,
            transitions,
            validator,
        })
    }

    pub fn id(&self) -> &str {
        match self {
            Transaction::Deployment { id, .. } => id,
            Transaction::Execution { id, .. } => id,
        }
    }

    pub fn output_records(&self) -> Vec<vm::EncryptedRecord> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.output_records())
            .collect()
    }

    /// If the transaction is an execution, return the list of input record serial numbers
    pub fn record_serial_numbers(&self) -> Vec<vm::Field> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.serial_numbers())
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
                fee.as_ref().map_or(0, |transition| transition.fee)
            }
            Transaction::Execution { transitions, .. } => transitions
                .iter()
                .fold(0, |acc, transition| acc + transition.fee),
        }
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
                vm::UserInputValueType::Record(crate::vm::Record {
                    owner: record.owner,
                    gates: record.gates,
                    entries: record.entries,
                    nonce: record.nonce,
                }),
                // TODO: Revisit the cast below.
                vm::UserInputValueType::U64(gates as u64),
            ];

            let transitions = Self::execute_credits("fee", &inputs, private_key)?;
            Ok(Some(transitions.first().unwrap().clone()))
        } else {
            Ok(None)
        }
    }

    fn execute_credits(
        function: &str,
        inputs: &[vm::UserInputValueType],
        private_key: &vm::PrivateKey,
    ) -> Result<Vec<vm::Transition>> {
        let function = vm::Identifier::from_str(function)?;
        let (program, _keys) = load_credits();

        vm::execution(program, function, inputs, private_key)
    }
}

impl std::fmt::Display for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transaction::Deployment { id, program, .. } => {
                write!(f, "Deployment({},{})", id, program.id())
            }
            Transaction::Execution {
                id,
                transitions,
                validator: None,
            } => {
                let transition = transitions.first().unwrap();
                write!(f, "Execution({},{id})", transition.program_id)
            }
            Transaction::Execution {
                id,
                validator: Some(validator),
                ..
            } => {
                write!(f, "StakingExecution({id},{validator})")
            }
        }
    }
}
