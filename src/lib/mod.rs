use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub mod program_file;
pub mod query;
pub mod transaction;
pub mod vm;

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(vm::Field, vm::EncryptedRecord)>,
    pub validators: HashMap<String, vm::Address>,
}
