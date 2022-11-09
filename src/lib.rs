use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Deployment, Execution, Testnet3};

#[derive(Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment(String, Deployment<Testnet3>),
    Execution(String, Execution<Testnet3>),
}

impl Transaction {
    pub fn id(&self) -> &str {
        match self {
            Transaction::Deployment(id, _) => id,
            Transaction::Execution(id, _) => id,
        }
    }
}