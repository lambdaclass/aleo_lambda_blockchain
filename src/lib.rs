use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Deployment, Execution, Testnet3};

#[derive(Serialize, Deserialize)]
pub enum Transaction {
    Deployment(String, Deployment<Testnet3>),
    Execution(String, Execution<Testnet3>),
}
