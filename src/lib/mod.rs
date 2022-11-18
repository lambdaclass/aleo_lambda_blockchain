use serde::{Deserialize, Serialize};

pub mod vm;

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

    /// Decrypts any available records and consumes the transaction object
    pub fn decrypt_records(self, address: &vm::Address, view_key: &vm::ViewKey) -> Vec<vm::Record> {
        if let Transaction::Execution { execution, .. } = self {
            execution
                .iter()
                .flat_map(|transition| transition.output_records())
                .filter(|(_, record)| record.is_owner(address, view_key))
                .filter_map(|(_, record)| record.decrypt(view_key).ok())
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

// this is here just to use it in tests, consider moving it
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GetDecryptionResponse {
    pub execution: Transaction,
    pub decrypted_records: Vec<vm::Record>,
}
