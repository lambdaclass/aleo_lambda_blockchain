/// Library for interfacing with the VM, and generating Transactions
///
use std::{ops::Deref, str::FromStr, sync::Arc};

use anyhow::{anyhow, bail, ensure, Result};
use indexmap::IndexMap;
use log::debug;
use parking_lot::{lock_api::RwLock, RawRwLock};
use rand::{rngs::ThreadRng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use snarkvm::{
    circuit::AleoV0,
    console::types::string::Integer,
    prelude::{
        Balance, CallStack, Environment, Itertools, Literal, Network, One, Owner, Plaintext,
        Testnet3, ToBits, ToField, Uniform, I64,
    },
};

mod stack;

pub type Address = snarkvm::prelude::Address<Testnet3>;
pub type Identifier = snarkvm::prelude::Identifier<Testnet3>;
pub type Value = snarkvm::prelude::Value<Testnet3>;
pub type Program = snarkvm::prelude::Program<Testnet3>;
pub type Ciphertext = snarkvm::prelude::Ciphertext<Testnet3>;
pub type Record = snarkvm::prelude::Record<Testnet3, snarkvm::prelude::Plaintext<Testnet3>>;
type Execution = snarkvm::prelude::Execution<Testnet3>;
pub type EncryptedRecord = snarkvm::prelude::Record<Testnet3, Ciphertext>;
pub type ViewKey = snarkvm::prelude::ViewKey<Testnet3>;
pub type PrivateKey = snarkvm::prelude::PrivateKey<Testnet3>;
pub type Field = snarkvm::prelude::Field<Testnet3>;
pub type Origin = snarkvm::prelude::Origin<Testnet3>;
pub type Output = snarkvm::prelude::Output<Testnet3>;
pub type ProgramID = snarkvm::prelude::ProgramID<Testnet3>;
pub type VerifyingKey = snarkvm::prelude::VerifyingKey<Testnet3>;
pub type ProvingKey = snarkvm::prelude::ProvingKey<Testnet3>;
pub type Deployment = snarkvm::prelude::Deployment<Testnet3>;
pub type Transition = snarkvm::prelude::Transition<Testnet3>;
pub type VerifyingKeyMap = IndexMap<Identifier, VerifyingKey>;
pub type KeyPairMap = IndexMap<Identifier, (ProvingKey, VerifyingKey)>;

/// Basic deployment validations
pub fn verify_deployment(program: &Program, verifying_keys: VerifyingKeyMap) -> Result<()> {
    // Ensure the deployment contains verifying keys.
    let program_id = program.id();
    ensure!(
        !verifying_keys.is_empty(),
        "No verifying keys present in the deployment for program '{program_id}'"
    );

    // Ensure the number of verifying keys matches the number of program functions.
    if verifying_keys.len() != program.functions().len() {
        bail!("The number of verifying keys does not match the number of program functions");
    }

    // Ensure the program functions are in the same order as the verifying keys.
    for ((function_name, function), candidate_name) in
        program.functions().iter().zip_eq(verifying_keys.keys())
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

pub fn verify_execution(transition: &Transition, verifying_keys: &VerifyingKeyMap) -> Result<()> {
    // Verify each transition.
    log::debug!(
        "Verifying transition for {}/{}...",
        transition.program_id(),
        transition.function_name()
    );

    // this check also rules out coinbase executions (e.g. credits genesis function)
    ensure!(
        *transition.fee() >= 0,
        "The execution fee is negative, cannot create credits"
    );

    // Ensure the transition ID is correct.
    ensure!(
        **transition.id() == transition.to_root()?,
        "The transition ID is incorrect"
    );
    // Ensure the number of inputs is within the allowed range.
    ensure!(
        transition.inputs().len() <= Testnet3::MAX_INPUTS,
        "Transition exceeded maximum number of inputs"
    );
    // Ensure the number of outputs is within the allowed range.
    ensure!(
        transition.outputs().len() <= Testnet3::MAX_INPUTS,
        "Transition exceeded maximum number of outputs"
    );
    // Ensure each input is valid.
    if transition
        .inputs()
        .iter()
        .enumerate()
        .any(|(index, input)| !input.verify(transition.tcm(), index))
    {
        bail!("Failed to verify a transition input")
    }
    // Ensure each output is valid.
    let num_inputs = transition.inputs().len();
    if transition
        .outputs()
        .iter()
        .enumerate()
        .any(|(index, output)| !output.verify(transition.tcm(), num_inputs + index))
    {
        bail!("Failed to verify a transition output")
    }
    // Compute the x- and y-coordinate of `tpk`.
    let (tpk_x, tpk_y) = transition.tpk().to_xy_coordinate();
    // [Inputs] Construct the verifier inputs to verify the proof.
    let mut inputs = vec![
        <Testnet3 as Environment>::Field::one(),
        *tpk_x,
        *tpk_y,
        **transition.tcm(),
    ];
    // [Inputs] Extend the verifier inputs with the input IDs.
    inputs.extend(
        transition
            .inputs()
            .iter()
            .flat_map(|input| input.verifier_inputs()),
    );

    // [Inputs] Extend the verifier inputs with the output IDs.
    inputs.extend(
        transition
            .outputs()
            .iter()
            .flat_map(|output| output.verifier_inputs()),
    );
    // [Inputs] Extend the verifier inputs with the fee.
    inputs.push(*I64::<Testnet3>::new(*transition.fee()).to_field()?);

    log::debug!(
        "Transition public inputs ({} elements): {:#?}",
        inputs.len(),
        inputs
    );

    // Retrieve the verifying key.
    let verifying_key = verifying_keys
        .get(transition.function_name())
        .ok_or_else(|| anyhow!("missing verifying key"))?;
    // Ensure the proof is valid.
    ensure!(
        verifying_key.verify(transition.function_name(), &inputs, transition.proof()),
        "Transition is invalid"
    );
    Ok(())
}

/// Generate proving and verifying keys for each function in the given program,
/// and return them in a function name -> (proving key, verifying key) map.
pub fn synthesize_program_keys(program: &Program) -> Result<KeyPairMap> {
    let mut verifying_keys = IndexMap::new();

    for function_name in program.functions().keys() {
        let rng = &mut rand::thread_rng();
        verifying_keys.insert(
            *function_name,
            synthesize_function_keys(program, rng, function_name)?,
        );
    }

    Ok(verifying_keys)
}

/// Generate proving and verifying keys for the given function.
pub fn synthesize_function_keys(
    program: &Program,
    rng: &mut ThreadRng,
    function_name: &Identifier,
) -> Result<(ProvingKey, VerifyingKey)> {
    let stack = stack::new_init(program)?;
    stack.synthesize_key::<AleoV0, _>(function_name, rng)?;
    let proving_key = stack.proving_keys.read().get(function_name).cloned();
    let proving_key = proving_key.ok_or_else(|| anyhow!("proving key not found for identifier"))?;

    let verifying_key = stack.verifying_keys.read().get(function_name).cloned();
    let verifying_key =
        verifying_key.ok_or_else(|| anyhow!("verifying key not found for identifier"))?;

    Ok((proving_key, verifying_key))
}

// Generates a program deployment for source transactions
pub fn generate_program(program_string: &str) -> Result<Program> {
    // Verify program is valid by parsing it and returning it
    Program::from_str(program_string)
}

pub fn execution(
    program: Program,
    function_name: Identifier,
    inputs: &[Value],
    private_key: &PrivateKey,
    rng: &mut ThreadRng,
    key: ProvingKey,
) -> Result<Vec<Transition>> {
    ensure!(
        !Program::is_coinbase(program.id(), &function_name),
        "Coinbase functions cannot be called"
    );

    ensure!(
        program.contains_function(&function_name),
        "Function '{function_name}' does not exist."
    );

    debug!(
        "executing program {} function {} inputs {:?}",
        program, function_name, inputs
    );

    let stack = stack::new_init(&program)?;

    stack.insert_proving_key(&function_name, key)?;

    let authorization = stack.authorize::<AleoV0, _>(private_key, function_name, inputs, rng)?;
    let execution: Arc<RwLock<RawRwLock, _>> = Arc::new(RwLock::new(Execution::new()));

    // Execute the circuit.
    let _ = stack.execute_function::<AleoV0, _>(
        CallStack::execute(authorization, execution.clone())?,
        rng,
    )?;

    let execution = execution.read().clone();

    Ok(execution.into_transitions().collect())
}

/// Generate a record for a specific program with the given attributes,
/// by using the given seed to deterministically generate a nonce.
/// This could be replaced by a more user-friendly record constructor.
pub fn mint_record(
    program_id: &str,
    record_name: &str,
    owner_address: &Address,
    gates: u64,
    seed: u64,
) -> Result<(Field, EncryptedRecord)> {
    // TODO have someone verify/audit this, probably it's unsafe or breaks cryptographic assumptions

    let owner = Owner::Private(Plaintext::Literal(
        Literal::Address(*owner_address),
        Default::default(),
    ));
    let amount = Integer::new(gates);
    let gates = Balance::Private(Plaintext::Literal(Literal::U64(amount), Default::default()));
    let empty_data = IndexMap::new();

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let randomizer = Uniform::rand(&mut rng);
    let nonce = Testnet3::g_scalar_multiply(&randomizer);

    let public_record = Record::from_plaintext(owner, gates, empty_data, nonce)?;
    let record_name = Identifier::from_str(record_name)?;
    let program_id = ProgramID::from_str(program_id)?;
    let commitment = public_record.to_commitment(&program_id, &record_name)?;
    let encrypted_record = public_record.encrypt(randomizer)?;
    Ok((commitment, encrypted_record))
}

/// Extract the record gates (the minimal credits unit) as a u64 integer, instead of a snarkvm internal type.
pub fn gates(record: &Record) -> u64 {
    *record.gates().deref().deref()
}

/// A helper method to derive the serial number from the private key and commitment.
pub fn compute_serial_number(private_key: PrivateKey, commitment: Field) -> Result<Field> {
    // Compute the generator `H` as `HashToGroup(commitment)`.
    let h = Testnet3::hash_to_group_psd2(&[Testnet3::serial_number_domain(), commitment])?;
    // Compute `gamma` as `sk_sig * H`.
    let gamma = h * private_key.sk_sig();
    // Compute `sn_nonce` as `Hash(COFACTOR * gamma)`.
    let sn_nonce = Testnet3::hash_to_scalar_psd2(&[
        Testnet3::serial_number_domain(),
        gamma.mul_by_cofactor().to_x_coordinate(),
    ])?;
    // Compute `serial_number` as `Commit(commitment, sn_nonce)`.
    Testnet3::commit_bhp512(
        &(Testnet3::serial_number_domain(), commitment).to_bits_le(),
        &sn_nonce,
    )
}

// Note this is a very hacky ad hoc helper for a specific purpose. Adding it like this
// because it's general purpose equivalent would be a lot of code which we won't plan to use
// short term. If you find adding more cases here consider implementing it properly
pub fn u64_from_output(output: &Output) -> Result<u64> {
    if let Output::Public(_, Some(Plaintext::Literal(Literal::U64(value), _))) = output {
        return Ok(*value.deref());
    };
    bail!("output type extraction not supported");
}

// same as above
pub fn address_from_output(output: &Output) -> Result<Address> {
    if let Output::Public(_, Some(Plaintext::Literal(Literal::Address(value), _))) = output {
        return Ok(*value);
    };
    bail!("output type extraction not supported");
}
