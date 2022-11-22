use crate::vm::{self};
use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Ciphertext, Record, Testnet3};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        deployment: Box<vm::Deployment>,
    },
    Execution {
        id: String,
        execution: vm::Execution,
    },
}

impl Transaction {
    pub fn id(&self) -> &str {
        match self {
            Transaction::Deployment { id, .. } => id,
            Transaction::Execution { id, .. } => id,
        }
    }

    pub fn output_records(&self) -> Vec<&Record<Testnet3, Ciphertext<Testnet3>>> {
        if let Transaction::Execution { execution, .. } = self {
            execution
                .iter()
                .flat_map(|transition| transition.output_records())
                .map(|(_, record)| record)
                .collect()
        } else {
            Vec::new()
        }
    }

    // FIXME the output of a deployment is inconveniently big, fix that
    // and try to remove this function in favor of standard traits
    // we probably want standard serde serialization for transport
    // and a pretty printed json for human display and logging
    pub fn json(&self) -> String {
        // consider https://crates.io/crates/attrsets
        serde_json::to_string_pretty(self).unwrap()
    }

    /// If the transaction is an execution, return the list of input record origins
    /// (in case they are record commitments).
    pub fn origin_commitments(&self) -> Vec<&vm::Field> {
        if let Transaction::Execution { ref execution, .. } = self {
            execution
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
            Transaction::Execution { id, execution } => {
                let transition = execution.peek().unwrap();
                let program_id = transition.program_id();
                write!(f, "Execution({},{})", program_id, id)
            }
        }
    }
}
