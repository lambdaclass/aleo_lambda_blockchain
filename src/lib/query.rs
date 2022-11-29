use crate::vm::{Address, ViewKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum AbciQuery {
    RecordsUnspentOwned { address: Address, view_key: ViewKey },
}
