use std::path::Path;

use crate::vm;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// This helper struct provides methods to dump programs and their proving/verifying keys into
/// files to support vm "built-in" programs, i.e. programs that come already built and can be
/// shared between the network and clients without extra work, like the credits program.
#[derive(Serialize, Deserialize, Debug)]
pub struct ProgramFile {
    pub program: vm::Program,
    pub keys: vm::ProgramBuild,
}

impl ProgramFile {
    pub fn build(program_source: &str) -> Result<Self> {
        let (program, keys) = vm::build_program(&program_source)?;

        Ok(Self { program, keys })
    }

    pub fn save(&self, output_path: &Path) -> Result<()> {
        let json = serde_json::to_string(self)?;
        std::fs::write(output_path, json).map_err(|e| anyhow!(e))
    }

    pub fn load(path: &Path) -> Result<(vm::Program, vm::ProgramBuild)> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("couldn't find stored program: {e}"))?;
        let stored: Self = serde_json::from_str(&json)?;
        Ok((stored.program, stored.keys))
    }
}
