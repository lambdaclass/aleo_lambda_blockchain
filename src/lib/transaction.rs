use crate::vm::{self};
use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Ciphertext, Record, Testnet3};

// Until we settle on one of the alternatives depending on performance, we have 2 variants for deployment transactions:
// Transaction::Deployment generates verifying keys offline and sends them to the network along with the program
// Transaction::Source just sends the program after being validated, and keys are generated on-chain
#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        deployment: Box<vm::Deployment>,
    },
    Source {
        id: String,
        program: Box<vm::Program>,
    },
    Execution {
        id: String,
        transitions: Vec<vm::Transition>,
    },
}

impl Transaction {
    pub fn id(&self) -> &str {
        match self {
            Transaction::Deployment { id, .. } => id,
            Transaction::Execution { id, .. } => id,
            Transaction::Source { id, .. } => id,
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
            Transaction::Deployment { id, deployment } => {
                write!(f, "Deployment({},{})", id, deployment.program_id())
            }
            Transaction::Source { id, program } => {
                write!(f, "Source({},{})", id, program.id())
            }
            Transaction::Execution { id, transitions } => {
                let transition = transitions.first().unwrap();
                let program_id = transition.program_id();
                write!(f, "Execution({},{})", program_id, id)
            }
        }
    }
}
