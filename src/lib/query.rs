use crate::vm::ProgramID;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum AbciQuery {
    /// Returns all records's ciphertexts from the blockchain
    GetRecords,
    /// Returns all spent records's serial numbers
    GetSpentSerialNumbers,
    /// Returns the program struct given it's id
    GetProgram { program_id: ProgramID },
}

impl From<AbciQuery> for Vec<u8> {
    fn from(q: AbciQuery) -> Vec<u8> {
        // bincoding an enum should not fail ever so unwrap() here should be fine
        bincode::serialize(&q).unwrap()
    }
}
