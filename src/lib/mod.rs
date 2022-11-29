use serde::{Deserialize, Serialize};

pub mod query;
pub mod transaction;
pub mod vm;

// this is here just to use it in tests, consider moving it
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct GetDecryptionResponse {
    pub execution: transaction::Transaction,
    pub decrypted_records: Vec<vm::Record>,
}

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(vm::Field, vm::EncryptedRecord)>,
}