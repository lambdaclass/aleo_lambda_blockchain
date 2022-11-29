/// Library for interfacing with the VM, and generating Transactions
///
use std::{str::FromStr, sync::Arc};

use anyhow::{anyhow, bail, ensure, Result};
use log::debug;
use parking_lot::{lock_api::RwLock, RawRwLock};
use rand::rngs::ThreadRng;
use snarkvm::{
    circuit::{AleoV0, IndexMap},
    prelude::{CallStack, Environment, Network, Testnet3, ToField, I64},
};

use snarkvm::prelude::One;

mod stack;

pub type Address = snarkvm::prelude::Address<Testnet3>;
pub type Identifier = snarkvm::prelude::Identifier<Testnet3>;
pub type Deployment = snarkvm::prelude::Deployment<Testnet3>;
pub type Value = snarkvm::prelude::Value<Testnet3>;
pub type Program = snarkvm::prelude::Program<Testnet3>;
pub type Ciphertext = snarkvm::prelude::Ciphertext<Testnet3>;
type Execution = snarkvm::prelude::Execution<Testnet3>;
pub type Record = snarkvm::prelude::Record<Testnet3, snarkvm::prelude::Plaintext<Testnet3>>;
pub type EncryptedRecord = snarkvm::prelude::Record<Testnet3, Ciphertext>;
pub type ViewKey = snarkvm::prelude::ViewKey<Testnet3>;
pub type PrivateKey = snarkvm::prelude::PrivateKey<Testnet3>;
pub type Field = snarkvm::prelude::Field<Testnet3>;
pub type Origin = snarkvm::prelude::Origin<Testnet3>;
pub type Output = snarkvm::prelude::Output<Testnet3>;
pub type ProgramID = snarkvm::prelude::ProgramID<Testnet3>;
pub type Certificate = snarkvm::prelude::Certificate<Testnet3>;
pub type VerifyingKey = snarkvm::prelude::VerifyingKey<Testnet3>;
pub type Transition = snarkvm::prelude::Transition<Testnet3>;

pub type VerifyingKeyMap = IndexMap<Identifier, (VerifyingKey, Certificate)>;

/// Ensure the verifying keys are well-formed and the certificates are valid.
pub fn verify_deployment(deployment: &Deployment, rng: &mut ThreadRng) -> Result<()> {
    let stack = stack::new_init(deployment.program())?;
    stack.verify_deployment::<AleoV0, _>(deployment, rng)
}

pub fn verify_execution(
    transitions: &Vec<Transition>,
    verifying_keys: &IndexMap<Identifier, (VerifyingKey, Certificate)>,
) -> Result<()> {
    // Ensure the number of transitions matches the program function.
    ensure!(
        !transitions.is_empty(),
        "There are no transitions in the execution"
    );

    // Verify each transition.
    for transition in transitions {
        debug!(
            "Verifying transition for {}/{}...",
            transition.program_id(),
            transition.function_name()
        );
        // Ensure an external execution isn't attempting to create credits
        // The assumption at this point is that credits can only be created in the genesis block
        // We may revisit if we add validator rewards, at which point some credits may be minted, although
        // still not by external function calls
        ensure!(
            !Program::is_coinbase(transition.program_id(), transition.function_name()),
            "Coinbase functions cannot be called"
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
        debug!(
            "Transition public inputs ({} elements): {:#?}",
            inputs.len(),
            inputs
        );

        // Retrieve the verifying key.
        let (verifying_key, _) = verifying_keys
            .get(transition.function_name())
            .ok_or_else(|| anyhow!("missing verifying key"))?;
        // Ensure the proof is valid.
        ensure!(
            verifying_key.verify(transition.function_name(), &inputs, transition.proof()),
            "Transition is invalid"
        );
    }
    Ok(())
}

// these struct-level functions should probably not be in the Vm level
pub fn generate_deployment(program_string: &str, rng: &mut ThreadRng) -> Result<Deployment> {
    let program = snarkvm::prelude::Program::from_str(program_string).unwrap();

    let stack = stack::new_init(&program)?;
    // Return the deployment.
    stack.deploy::<AleoV0, _>(rng)
}

pub fn generate_execution(
    program_string: &str,
    function_name: Identifier,
    inputs: &[Value],
    private_key: &PrivateKey,
    rng: &mut ThreadRng,
) -> Result<Vec<Transition>> {
    let program: Program = snarkvm::prelude::Program::from_str(program_string).unwrap();
    execute(program, function_name, inputs, private_key, rng)
}

pub fn credits_execution(
    function_name: Identifier,
    inputs: &[Value],
    private_key: &PrivateKey,
    rng: &mut ThreadRng,
) -> Result<Execution> {
    let credits_program = Program::credits()?;
    execute(credits_program, function_name, inputs, private_key, rng)
}

fn execute(
    program: Program,
    function_name: Identifier,
    inputs: &[Value],
    private_key: &PrivateKey,
    rng: &mut ThreadRng,
) -> Result<Execution> {
    ensure!(
        program.contains_function(&function_name),
        "Function '{function_name}' does not exist."
    );
    // we check this on the verify side (which will run in the blockchain)
    // repeating here just to fail early
    ensure!(
        !Program::is_coinbase(program.id(), &function_name),
        "Coinbase functions cannot be called"
    );

    let stack = stack::new_init(&program)?;

    // Synthesize each proving and verifying key.
    for function_name in program.functions().keys() {
        stack.synthesize_key::<AleoV0, _>(function_name, rng)?
    }

    debug!(
        "executing program {} function {} inputs {:?}",
        program, function_name, inputs
    );

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
