use serde::{Deserialize, Serialize};
use snarkvm::prelude::{Deployment, Execution, Testnet3};

#[derive(Serialize, Deserialize, Debug)]
pub enum Transaction {
    Deployment {
        id: String,
        deployment: Deployment<Testnet3>,
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

    // FIXME the output of a deployment is inconveniently big, fix that
    // and try to remove this function in favor of standard traits
    // we probably want standard serde serialization for transport
    // and a pretty printed json for human display and logging
    pub fn json(&self) -> String {
        // consider https://crates.io/crates/attrsets
        serde_json::to_string_pretty(self).unwrap()
    }
}
