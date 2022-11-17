use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Deployment, Execution, Field, Origin, Plaintext, Record, Testnet3};

pub mod account;
pub mod vm;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        deployment: Box<Deployment<Testnet3>>,
    },
    Execution {
        id: String,
        execution: Execution<Testnet3>,
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
    pub fn decrypt_records(
        self,
        credentials: &account::Credentials,
    ) -> Result<Vec<Record<Testnet3, Plaintext<Testnet3>>>> {
        match self {
            Transaction::Execution { execution, .. } => {
                let mut decrypted_records = Vec::new();

                // Only decrypt the records owned by the current user.
                for record in execution
                    .iter()
                    .flat_map(|transition| transition.output_records())
                    .filter_map(|(_, record)| {
                        if record.is_owner(&credentials.address, &credentials.view_key) {
                            Some(record)
                        } else {
                            None
                        }
                    })
                {
                    let record = record.decrypt(&credentials.view_key)?;
                    decrypted_records.push(record);
                }

                Ok(decrypted_records)
            }
            _ => bail!("Transaction is not an execution so it does not have records to decrypt"),
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
    pub fn origin_commitments(&self) -> Vec<&Field<Testnet3>> {
        if let Transaction::Execution { ref execution, .. } = self {
            execution
                .iter()
                .flat_map(|transition| transition.origins())
                .filter_map(|origin| {
                    if let Origin::Commitment(commitment) = origin {
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

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GetDecryptionResponse {
    pub execution: Transaction,
    pub decrypted_records: Vec<Record<Testnet3, Plaintext<Testnet3>>>,
}
