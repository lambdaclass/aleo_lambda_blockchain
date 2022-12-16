use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub mod program_file;
pub mod query;
pub mod transaction;

pub use vm::jaleo;
pub use vmtropy as vm;

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(jaleo::Field, jaleo::EncryptedRecord)>,
    pub validators: HashMap<String, jaleo::Address>,
}
