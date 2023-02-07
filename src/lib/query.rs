use crate::vm::{self, ProgramID};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum AbciQuery {
    /// Returns all records's ciphertexts from the blockchain
    GetRecords,
    /// Returns all spent records's serial numbers
    GetSpentSerialNumbers,
    /// Returns the program struct given its id
    GetProgram { program_id: ProgramID },
    /// Returns a valid merkle path for a record
    GetMerklePath { ciphertext: vm::Field },
}

impl From<AbciQuery> for Vec<u8> {
    fn from(q: AbciQuery) -> Vec<u8> {
        // bincoding an enum should not fail ever so unwrap() here should be fine
        bincode::serialize(&q).unwrap()
    }
}
