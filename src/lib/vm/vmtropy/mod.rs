/// Library for interfacing with the VM, and generating Transactions
///
use std::str::FromStr;

use anyhow::{anyhow, bail, ensure, Result};
use log::debug;
pub use vmtropy::build_program;
pub use vmtropy::helpers::to_address;
pub use vmtropy::jaleo::{get_credits_key, mint_credits};
pub use vmtropy::jaleo::{Itertools, UserInputValueType};
use vmtropy::VariableType;

const MAX_INPUTS: usize = 8;
const MAX_OUTPUTS: usize = 8;

// pub type Address = snarkvm::prelude::Address<Testnet3>;
pub type Address = vmtropy::jaleo::Address;
pub type Identifier = vmtropy::jaleo::Identifier;
// pub type Value = snarkvm::prelude::Value<Testnet3>;
pub type Program = vmtropy::jaleo::Program;
pub type ProgramBuild = vmtropy::ProgramBuild;
// pub type Ciphertext = snarkvm::prelude::Ciphertext<Testnet3>;
pub type Record = vmtropy::jaleo::Record;
// type Execution = snarkvm::prelude::Execution<Testnet3>;
pub type EncryptedRecord = vmtropy::jaleo::EncryptedRecord;
pub type ViewKey = vmtropy::jaleo::ViewKey;
pub type PrivateKey = vmtropy::jaleo::PrivateKey;
pub type Field = vmtropy::jaleo::Field;
// pub type Origin = snarkvm::prelude::Origin<Testnet3>;
// pub type Output = snarkvm::prelude::Output<Testnet3>;
pub type ProgramID = vmtropy::jaleo::ProgramID;
pub type VerifyingKey = vmtropy::jaleo::VerifyingKey;
pub type ProvingKey = vmtropy::jaleo::ProvingKey;
pub type Deployment = vmtropy::jaleo::Deployment;
pub type Transition = vmtropy::jaleo::Transition;
pub type VerifyingKeyMap = vmtropy::jaleo::VerifyingKeyMap;
// pub type VerifyingKeyMap = IndexMap<Identifier, VerifyingKey>;

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
        if &candidate_name.to_string() != &function.name().to_string() {
            bail!(
                "The verifier key is '{candidate_name}', but the function name is '{}'",
                function.name()
            )
        }
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
        !program_is_coinbase(&transition.program_id, &transition.function_name),
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
    let proof = vmtropy::jaleo::deserialize_proof(proof_bytes)?;

    let inputs: Vec<UserInputValueType> = transition
        .inputs
        .iter()
        .filter_map(|i| match i {
            vmtropy::VariableType::Public(value) => Some(value.clone()),
            _ => None,
        })
        .collect();

    // Ensure the proof is valid.
    ensure!(
        vmtropy::verify_proof(verifying_key.clone(), &inputs, &proof)?,
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

// The _rng and _key arguments are here just to be compliant with the snarkVM API, we don't actually use them.
pub fn execution(
    program: Program,
    function_name: Identifier,
    inputs: &[UserInputValueType],
    private_key: &PrivateKey,
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

    let (compiled_function_variables, proof) = vmtropy::execute_function(&function, inputs)?;

    let inputs = vmtropy::jaleo::process_circuit_inputs(
        &function,
        &compiled_function_variables,
        private_key,
    )?;
    let outputs = vmtropy::jaleo::process_circuit_outputs(&function, &compiled_function_variables)?;

    let bytes_proof = vmtropy::jaleo::serialize_proof(proof)?;
    let encoded_proof = hex::encode(bytes_proof);

    let transition = Transition {
        program_id: program.id().to_string(),
        function_name: function_name.to_string(),
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
