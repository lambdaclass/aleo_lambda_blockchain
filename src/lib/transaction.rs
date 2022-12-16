use crate::vm::{self, jaleo};
use anyhow::{ensure, Result};
use indexmap::IndexMap;
use log::debug;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::str::FromStr;
use vm::jaleo::{Record, VerifyingKey, VerifyingKeyMap};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        program: Box<jaleo::Program>,
        verifying_keys: jaleo::VerifyingKeyMap,
        fee: Option<jaleo::Transition>,
    },
    Execution {
        id: String,
        transitions: Vec<jaleo::Transition>,
    },
}

impl Transaction {
    // Used to generate deployment of a new program in path
    pub fn deployment(
        path: &Path,
        private_key: &jaleo::PrivateKey,
        fee: Option<(u64, jaleo::Record)>,
    ) -> Result<Self> {
        let program_string = fs::read_to_string(path)?;
        debug!("Deploying program {}", program_string);

        // generate program keys (proving and verifying) and keep the verifying one for the deploy
        let (program, program_build) = vm::build_program(&program_string)?;

        let verifying_keys: IndexMap<String, VerifyingKey> = program_build
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
            verifying_keys: VerifyingKeyMap {
                map: verifying_keys,
            },
        })
    }

    // Used to generate an execution of a program in path or an execution of the credits program
    pub fn execution(
        path: &Path,
        function_name: jaleo::Identifier,
        inputs: &[jaleo::UserInputValueType],
        private_key: &jaleo::PrivateKey,
        requested_fee: Option<(u64, jaleo::Record)>,
    ) -> Result<Self> {
        let program_string = fs::read_to_string(path).unwrap();
        let program = jaleo::generate_program(&program_string)?;
        let rng = &mut vm::generate_rand();
        let mut transitions = jaleo::execution(&program, &function_name, inputs, private_key, rng)?;

        // some amount of fees may be implicit if the execution drops credits. in that case, those credits are
        // subtracted from the fees that were requested to be paid.
        let implicit_fees = transitions.iter().map(|transition| transition.fee).sum();
        if let Some(transition) = Self::execute_fee(private_key, requested_fee, implicit_fees)? {
            transitions.push(transition);
        }

        let id = uuid::Uuid::new_v4().to_string();

        Ok(Self::Execution { id, transitions })
    }

    pub fn credits_execution(
        function_name: jaleo::Identifier,
        inputs: &[jaleo::UserInputValueType],
        private_key: &jaleo::PrivateKey,
        requested_fee: Option<(u64, jaleo::Record)>,
    ) -> Result<Self> {
        let program = jaleo::Program::credits()?;

        let rng = &mut vm::generate_rand();

        let mut transitions = jaleo::execution(&program, &function_name, inputs, private_key, rng)?;

        // some amount of fees may be implicit if the execution drops credits. in that case, those credits are
        // subtracted from the fees that were requested to be paid.
        let implicit_fees = transitions.iter().map(|transition| transition.fee).sum();
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

    pub fn output_records(&self) -> Vec<jaleo::EncryptedRecord> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.output_records())
            .collect()
    }

    /// If the transaction is an execution, return the list of input record serial numbers
    pub fn record_serial_numbers(&self) -> Vec<jaleo::Field> {
        self.transitions()
            .iter()
            .flat_map(|transition| transition.serial_numbers())
            .collect()
    }

    fn transitions(&self) -> Vec<jaleo::Transition> {
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
        private_key: &jaleo::PrivateKey,
        requested_fee: Option<(u64, jaleo::Record)>,
        implicit_fee: i64,
    ) -> Result<Option<jaleo::Transition>> {
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
            let fee_function = jaleo::Identifier::from_str("fee")?;
            let inputs = [
                jaleo::UserInputValueType::Record(Record {
                    owner: record.owner,
                    gates: record.gates,
                    entries: record.entries,
                    nonce: record.nonce,
                }),
                // TODO: Revisit the cast below.
                jaleo::UserInputValueType::U64(gates as u64),
            ];

            let rng = &mut vm::generate_rand();

            let program = jaleo::Program::credits()?;
            let transitions = jaleo::execution(&program, &fee_function, &inputs, private_key, rng)?;
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
                write!(f, "Execution({},{id})", transition.program_id)
            }
        }
    }
}
