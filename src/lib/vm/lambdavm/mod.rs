/// Library for interfacing with the VM, and generating Transactions
///
use std::str::FromStr;

use anyhow::{anyhow, bail, ensure, Result};
pub use lambdavm::build_program;
pub use lambdavm::jaleo::{get_credits_key, mint_credits};
pub use lambdavm::jaleo::{Itertools, UserInputValueType};
use lambdavm::VariableType;
use log::debug;
use sha3::{Digest, Sha3_256};

const MAX_INPUTS: usize = 8;
const MAX_OUTPUTS: usize = 8;

pub type Address = lambdavm::jaleo::Address;
pub type Identifier = lambdavm::jaleo::Identifier;
pub type Program = lambdavm::jaleo::Program;
pub type ProgramBuild = lambdavm::ProgramBuild;
pub type Record = lambdavm::jaleo::Record;
pub type EncryptedRecord = lambdavm::jaleo::EncryptedRecord;
pub type ViewKey = lambdavm::jaleo::ViewKey;
pub type PrivateKey = lambdavm::jaleo::PrivateKey;
pub type Field = lambdavm::jaleo::Field;
pub type ProgramID = lambdavm::jaleo::ProgramID;
pub type VerifyingKey = lambdavm::jaleo::VerifyingKey;
pub type ProvingKey = lambdavm::jaleo::ProvingKey;
pub type Deployment = lambdavm::jaleo::Deployment;
pub type Transition = lambdavm::jaleo::Transition;
pub type VerifyingKeyMap = lambdavm::jaleo::VerifyingKeyMap;

/// Basic deployment validations
pub fn verify_deployment(program: &Program, verifying_keys: VerifyingKeyMap) -> Result<()> {
    // Ensure the deployment contains verifying keys.
    let program_id = program.id();
    ensure!(
        !verifying_keys.map.is_empty(),
        "No verifying keys present in the deployment for program '{program_id}'"
    );

    // Ensure the number of verifying keys matches the number of program functions.
    if verifying_keys.map.len() != program.functions().len() {
        bail!("The number of verifying keys does not match the number of program functions");
    }

    // Ensure the program functions are in the same order as the verifying keys.
    for ((function_name, function), candidate_name) in
        program.functions().iter().zip_eq(verifying_keys.map.keys())
    {
        // Ensure the function name is correct.
        if function_name != function.name() {
            bail!(
                "The function key is '{function_name}', but the function name is '{}'",
                function.name()
            )
        }
        // Ensure the function name with the verifying key is correct.
        if candidate_name != function.name() {
            bail!(
                "The verifier key is '{candidate_name}', but the function name is '{}'",
                function.name()
            )
        }
    }
    Ok(())
}

pub fn ensure_srs_file_exists() -> Result<()> {
    let (_, srs_file_path) = lambdavm::universal_srs::get_universal_srs_dir_and_filepath()?;
    if let Err(_) = std::fs::File::open(srs_file_path) {
        let _ = lambdavm::universal_srs::generate_universal_srs_and_write_to_file()?;
    }
    Ok(())
}

pub fn verify_execution(
    transition: &Transition,
    verifying_key_map: &VerifyingKeyMap,
) -> Result<()> {
    // Verify each transition.
    log::debug!(
        "Verifying transition for {}/{}...",
        transition.program_id,
        transition.function_name
    );

    // this check also rules out coinbase executions (e.g. credits genesis function)
    ensure!(
        transition.fee >= 0,
        "The execution fee is negative, cannot create credits"
    );

    // Ensure an external execution isn't attempting to create credits
    // The assumption at this point is that credits can only be created in the genesis block
    // We may revisit if we add validator rewards, at which point some credits may be minted, although
    // still not by external function calls
    ensure!(
        !program_is_coinbase(
            &transition.program_id.to_string(),
            &transition.function_name.to_string()
        ),
        "Coinbase functions cannot be called"
    );
    // // Ensure the transition ID is correct.
    // ensure!(
    //     **transition == transition.to_root()?,
    //     "The transition ID is incorrect"
    // );
    // Ensure the number of inputs is within the allowed range.
    ensure!(
        transition.inputs.len() <= MAX_INPUTS,
        "Transition exceeded maximum number of inputs"
    );
    // Ensure the number of outputs is within the allowed range.
    ensure!(
        transition.outputs.len() <= MAX_OUTPUTS,
        "Transition exceeded maximum number of outputs"
    );
    // // Ensure each input is valid.
    // if transition
    //     .inputs
    //     .iter()
    //     .enumerate()
    //     .any(|(index, input)| !input.verify(transition.tcm(), index))
    // {
    //     bail!("Failed to verify a transition input")
    // }
    // // Ensure each output is valid.
    // let num_inputs = transition.inputs.len();
    // if transition
    //     .outputs
    //     .iter()
    //     .enumerate()
    //     .any(|(index, output)| !output.verify(transition.tcm(), num_inputs + index))
    // {
    //     bail!("Failed to verify a transition output")
    // }

    // Retrieve the verifying key.
    let verifying_key = verifying_key_map
        .map
        .get(&transition.function_name)
        .ok_or_else(|| anyhow!("missing verifying key"))?;
    // Decode and deserialize the proof.
    let proof_bytes = hex::decode(&transition.proof)?;

    // TODO: Fix this by making proofs derive the deserialize trait instead of this.
    let proof = lambdavm::jaleo::deserialize_proof(proof_bytes)?;

    let inputs: Vec<UserInputValueType> = transition
        .inputs
        .iter()
        .filter_map(|i| match i {
            lambdavm::VariableType::Public(value) => Some(value.clone()),
            _ => None,
        })
        .collect();

    // Ensure the proof is valid.
    ensure!(
        lambdavm::verify_proof(verifying_key.clone(), &inputs, &proof)?,
        "Transition is invalid"
    );

    Ok(())
}

pub fn program_is_coinbase(program_id: &str, function_name: &str) -> bool {
    (function_name == "mint" || function_name == "genesis") && program_id == "credits.aleo"
}

// Generates a program deployment for source transactions
pub fn generate_program(program_string: &str) -> Result<Program> {
    // Verify program is valid by parsing it and returning it
    Program::from_str(program_string)
}

pub fn execution(
    program: Program,
    function_name: Identifier,
    inputs: &[UserInputValueType],
    private_key: &PrivateKey,
    _proving_key: Option<ProvingKey>,
) -> Result<Vec<Transition>> {
    ensure!(
        !program_is_coinbase(&program.id().to_string(), &function_name.to_string()),
        "Coinbase functions cannot be called"
    );

    debug!(
        "executing program {} function {} inputs {:?}",
        program, function_name, inputs
    );

    let function = program
        .get_function(&function_name)
        .map_err(|e| anyhow!("{}", e))?;

    let (compiled_function_variables, proof) =
        lambdavm::execute_function(&program, &function_name.to_string(), inputs)?;

    let inputs = lambdavm::jaleo::process_circuit_inputs(
        &function,
        &compiled_function_variables,
        private_key,
    )?;
    let outputs =
        lambdavm::jaleo::process_circuit_outputs(&function, &compiled_function_variables)?;

    let bytes_proof = lambdavm::jaleo::serialize_proof(proof)?;
    let encoded_proof = hex::encode(bytes_proof);

    let transition = Transition {
        program_id: *program.id(),
        function_name,
        inputs: inputs.into_values().collect::<Vec<VariableType>>(),
        outputs: outputs.into_values().collect::<Vec<VariableType>>(),
        proof: encoded_proof,
        fee: 0,
    };

    Ok(vec![transition])
}

/// Extract the record gates (the minimal credits unit) as a u64 integer, instead of a snarkvm internal type.
pub fn gates(record: &Record) -> u64 {
    record.gates
}

/// This is temporary. We should be using the `serial_number` method in the Record struct, but
/// we are doing this to conform to the current API.
pub fn compute_serial_number(_private_key: PrivateKey, commitment: Field) -> Result<Field> {
    Ok(sha3_hash(&hex::decode(commitment)?))
}

fn sha3_hash(input: &[u8]) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(input);
    let bytes = hasher.finalize().to_vec();
    hex::encode(bytes)
}

/// Generate a record for a specific program with the given attributes,
/// by using the given seed to deterministically generate a nonce.
/// This could be replaced by a more user-friendly record constructor.
pub fn mint_record(
    _program_id: &str,
    _record_name: &str,
    owner_address: &Address,
    gates: u64,
    seed: u64,
) -> Result<(Field, EncryptedRecord)> {
    // For now calling mint_credits is enough; on the snarkVM backend the program_id
    // and record_name are just used to calculate the commitment, but we don't do things that way
    // The seed is used for instantiating a randomizer, which is used to generate the nonce
    // and encrypt the record. Once again, we don't really do things that way for now.

    mint_credits(owner_address, gates, seed)
}

/// Matches types of literals (that we know are numbers) and turns them into u128 before trying to downcast to the desired type
// TODO: Once https://trello.com/c/vtHu588B/77-handle-inputs-and-outputs-visibility-encryption is merged, fix this
pub fn int_from_output<T: std::convert::TryFrom<u128>>(output: &VariableType) -> Result<T>
where
    <T as TryFrom<u128>>::Error: std::fmt::Debug,
{
    match output {
        VariableType::Private(user_input_value_type)
        | VariableType::Public(user_input_value_type) => {
            let value = match user_input_value_type {
                UserInputValueType::U8(v) => *v as u128,
                UserInputValueType::U16(v) => *v as u128,
                UserInputValueType::U32(v) => *v as u128,
                UserInputValueType::U64(v) => *v as u128,
                UserInputValueType::U128(v) => *v,
                _ => todo!(),
            };
            Ok(T::try_from(value).expect("issue casting literal to desired type"))
        }
        _ => {
            bail!("output type extraction not supported");
        }
    }
}

// same as above
pub fn address_from_output(output: &VariableType) -> Result<Address> {
    if let VariableType::Public(UserInputValueType::Address(address)) = output {
        let address_string = std::str::from_utf8(address)?;
        let address = Address::from_str(address_string)?;
        return Ok(address);
    };

    if let VariableType::Private(UserInputValueType::Address(address)) = output {
        let address_string = std::str::from_utf8(address)?;
        let address = Address::from_str(address_string)?;
        return Ok(address);
    };

    bail!("output type extraction not supported");
}

pub fn u64_to_value(amount: u64) -> UserInputValueType {
    UserInputValueType::from_str(&format!("{amount}u64")).expect("couldn't parse amount")
}
