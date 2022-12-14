use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub mod program_file;
pub mod query;
pub mod transaction;

pub use vmtropy as vm;
pub use vm::jaleo;

#[derive(Deserialize, Serialize)]
pub struct GenesisState {
    pub records: Vec<(jaleo::Field, jaleo::JAleoRecord)>,
    pub validators: HashMap<String, jaleo::Address>,
}
