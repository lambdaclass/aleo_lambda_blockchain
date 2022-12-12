use crate::vm::{self};
use anyhow::Result;
use log::debug;
use rand;
use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Ciphertext, Record, Testnet3};
use std::fs;
use std::path::Path;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        program: Box<vm::Program>,
        verifying_keys: vm::VerifyingKeyMap,
    },
    Execution {
        id: String,
        transitions: Vec<vm::Transition>,
    },
}

impl Transaction {
    // Used to generate deployment of a new program in path
    pub fn deployment(path: &Path) -> Result<Self> {
        let program_string = fs::read_to_string(path)?;
        let mut rng = rand::thread_rng();
        debug!("Deploying program {}", program_string);
        let program = vm::generate_program(&program_string)?;
        let verifying_keys = vm::generate_verifying_keys(&program, &mut rng)?;
        // using a uuid for txid, just to skip having to use an additional fee record which now is necessary to run
        // Transaction::from_deployment
        let id = uuid::Uuid::new_v4().to_string();
        Ok(Transaction::Deployment {
            id,
            program: Box::new(program),
            verifying_keys,
        })
    }

    // Used to generate an execution of a program in path or an execution of the credits program
    pub fn execution(
        path: Option<&Path>,
        function_name: vm::Identifier,
        inputs: &[vm::Value],
        private_key: &vm::PrivateKey,
    ) -> Result<Self> {
        let rng = &mut rand::thread_rng();

        let transitions = if let Some(path) = path {
            let program_string = fs::read_to_string(path)?;

            vm::generate_execution(&program_string, function_name, inputs, private_key, rng)?
        } else {
            vm::credits_execution(function_name, inputs, private_key, rng)?
        };

        let id = uuid::Uuid::new_v4().to_string();

        Ok(Self::Execution { id, transitions })
    }

    pub fn id(&self) -> &str {
        match self {
            Transaction::Deployment { id, .. } => id,
            Transaction::Execution { id, .. } => id,
        }
    }

    pub fn output_records(&self) -> Vec<&Record<Testnet3, Ciphertext<Testnet3>>> {
        if let Transaction::Execution { transitions, .. } = self {
            transitions
                .iter()
                .flat_map(|transition| transition.output_records())
                .map(|(_, record)| record)
                .collect()
        } else {
            Vec::new()
        }
    }

    /// If the transaction is an execution, return the list of input record origins
    /// (in case they are record commitments).
    pub fn origin_commitments(&self) -> Vec<&vm::Field> {
        if let Transaction::Execution {
            ref transitions, ..
        } = self
        {
            transitions
                .iter()
                .flat_map(|transition| transition.origins())
                .filter_map(|origin| {
                    if let vm::Origin::Commitment(commitment) = origin {
                        Some(commitment)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
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
