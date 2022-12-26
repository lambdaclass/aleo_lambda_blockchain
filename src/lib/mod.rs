use std::{path::PathBuf, str::FromStr};

pub mod program_file;
pub mod query;
pub mod transaction;
pub mod validator;
pub mod vm;

/// Directory to store aleo related files (e.g. account, cached programs). Typically ~/.aleo/
pub fn aleo_home() -> PathBuf {
    std::env::var("ALEO_HOME")
        .map(|path| PathBuf::from_str(&path).unwrap())
        .unwrap_or_else(|_| dirs::home_dir().unwrap().join(".aleo"))
}

/// Get the credits program. This is a special built-in program of the system, which contains
/// functions to move aleo money. Since it's required for most uses in clients and servers, it's
/// cached to only be built once.
pub fn load_credits() -> (vm::Program, vm::KeyPairMap) {
    // try to fetch from cache
    let cache_path = aleo_home().join("cache/credits.avm");
    if let Ok(program) = program_file::ProgramFile::load(&cache_path) {
        log::debug!("found credits program in {cache_path:?}");
        return program;
    }

    // else build keys and cache for future use
    log::debug!("cached credits not found, building and saving to {cache_path:?}");
    let source = include_str!("../../aleo/credits.aleo");
    let file = program_file::ProgramFile::build(source).expect("couldn't build credits program");
    std::fs::create_dir_all(aleo_home().join("cache")).expect("couldn't create cache dir");
    file.save(&cache_path)
        .expect("couldn't save credits program");

    (file.program, file.keys)
}
