use crate::vm::{self};
use anyhow::{ensure, Result};
use log::debug;
use rand;
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
        let verifying_keys = vm::generate_verifying_keys(&program)?;
        // using a uuid for txid, just to skip having to use an additional fee record which now is necessary to run
        // Transaction::from_deployment
        let id = uuid::Uuid::new_v4().to_string();
        let fee = Self::execute_fee(private_key, fee, 0)?;
        Ok(Transaction::Deployment {
            id,
            fee,
            program: Box::new(program),
            verifying_keys,
        })
    }

    // Used to generate an execution of a program in path or an execution of the credits program
    pub fn execution(
        path: &Path,
        function_name: vm::Identifier,
        inputs: &[vm::Value],
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
    ) -> Result<Self> {
        let program_string = fs::read_to_string(path).unwrap();
        let program: vm::Program = vm::generate_program(&program_string)?;
        let rng = &mut rand::thread_rng();
        let (proving_key, _) = vm::synthesize_keys(&program, rng, &function_name)?;

        let rng = &mut rand::thread_rng();

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

        let id = uuid::Uuid::new_v4().to_string();

        Ok(Self::Execution { id, transitions })
    }

    pub fn credits_execution(
        function_name: vm::Identifier,
        inputs: &[vm::Value],
        private_key: &vm::PrivateKey,
        requested_fee: Option<(u64, vm::Record)>,
    ) -> Result<Self> {
        let (proving_key, _) = vm::get_credits_key(&function_name)?;
        let program = vm::Program::credits()?;

        let rng = &mut rand::thread_rng();

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

        let id = uuid::Uuid::new_v4().to_string();
        Ok(Self::Execution { id, transitions })
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

    /// If the transaction is an execution, return the list of input record origins
    /// (in case they are record commitments).
    pub fn origin_commitments(&self) -> Vec<vm::Field> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.origins())
            .filter_map(|origin| {
                if let vm::Origin::Commitment(commitment) = origin {
                    Some(*commitment)
                } else {
                    None
                }
            })
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
            let fee_function = vm::Identifier::from_str("fee")?;
            let inputs = [
                vm::Value::Record(record),
                vm::Value::from_str(&format!("{gates}u64"))?,
            ];

            let rng = &mut rand::thread_rng();

            let (proving_key, _) = vm::get_credits_key(&fee_function)?;
            let program = vm::Program::credits()?;
            let transitions = vm::execution(
                program,
                fee_function,
                &inputs,
                private_key,
                rng,
                proving_key,
            )?;
            Ok(Some(transitions.first().unwrap().clone()))
        } else {
            Ok(None)
        }
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
