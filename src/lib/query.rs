use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum AbciQuery {
    /// Returns all records's ciphertexts from the blockchain
    GetRecords,
    /// Returns all spent records's serial numbers
    GetSpentSerialNumbers,
}

impl From<AbciQuery> for Vec<u8> {
    fn from(q: AbciQuery) -> Vec<u8>{
        bincode::serialize(&q).unwrap()
    }
}