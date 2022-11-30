use serde::{Deserialize, Serialize};

pub mod query;
pub mod transaction;
pub mod vm;

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(vm::Field, vm::EncryptedRecord)>,
}
