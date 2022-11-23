/// Library for interfacing with the VM, and generating Transactions
///
use std::{str::FromStr, sync::Arc};

use anyhow::{bail, ensure, Result};
use log::debug;
use rand::rngs::ThreadRng;
use snarkvm::{
    circuit::AleoV0,
    prelude::{
        Environment, FinalizeTypes, FromBytes, Instruction, Network, RegisterTypes, Testnet3,
        ToBytes, ToField, I64,
    },
};

use snarkvm::prelude::One;

pub type Address = snarkvm::prelude::Address<Testnet3>;
pub type Identifier = snarkvm::prelude::Identifier<Testnet3>;
pub type Deployment = snarkvm::prelude::Deployment<Testnet3>;
pub type Value = snarkvm::prelude::Value<Testnet3>;
pub type Program = snarkvm::prelude::Program<Testnet3>;
pub type Ciphertext = snarkvm::prelude::Ciphertext<Testnet3>;
pub type Stack = snarkvm::prelude::Stack<Testnet3>;
pub type Process = snarkvm::prelude::Process<Testnet3>;
pub type Execution = snarkvm::prelude::Execution<Testnet3>;
pub type UniversalSRS = snarkvm::prelude::UniversalSRS<Testnet3>;
pub type Record = snarkvm::prelude::Record<Testnet3, snarkvm::prelude::Plaintext<Testnet3>>;
pub type EncryptedRecord = snarkvm::prelude::Record<Testnet3, Ciphertext>;
pub type ViewKey = snarkvm::prelude::ViewKey<Testnet3>;
pub type PrivateKey = snarkvm::prelude::PrivateKey<Testnet3>;
pub type Field = snarkvm::prelude::Field<Testnet3>;
pub type Origin = snarkvm::prelude::Origin<Testnet3>;
pub type Output = snarkvm::prelude::Output<Testnet3>;
pub type ProgramID = snarkvm::prelude::ProgramID<Testnet3>;

// TODO: keeping Process here as a parameter mainly for the ABCI to use, but it has to be removed
// in favor of a more general program store
pub fn verify_deployment(
    deployment: &Deployment,
    process: &Process,
    rng: &mut ThreadRng,
) -> Result<()> {
    // Retrieve the program ID.
    let program_id = deployment.program().id();
    // Ensure the program does not already exist in the process.
    ensure!(
        !process.contains_program(program_id),
        "Program '{program_id}' already exists"
    );
    // Ensure the program is well-formed, by computing the stack.
    let stack = Stack::new(process, deployment.program())?;
    // Ensure the verifying keys are well-formed and the certificates are valid.
    stack.verify_deployment::<AleoV0, _>(deployment, rng)
}

// TODO: keeping Process here as a parameter mainly for the ABCI to use, but it has to be removed
// in favor of a more general program store
pub fn verify_execution(execution: &Execution, process: &Process) -> Result<()> {
    // Retrieve the edition.
    let edition = execution.edition();
    // Ensure the edition matches.
    ensure!(
        edition == Testnet3::EDITION,
        "Executed the wrong edition (expected '{}', found '{edition}').",
        Testnet3::EDITION
    );

    // Ensure the execution contains transitions.
    ensure!(
        !execution.is_empty(),
        "There are no transitions in the execution"
    );

    // Ensure the number of transitions matches the program function.
    {
        // Retrieve the transition (without popping it).
        let transition = execution.peek()?;
        // Retrieve the stack.
        let stack = process.get_stack(transition.program_id())?;
        // Ensure the number of calls matches the number of transitions.
        let number_of_calls = stack.get_number_of_calls(transition.function_name())?;
        ensure!(
                number_of_calls == execution.len(),
                "The number of transitions in the execution is incorrect. Expected {number_of_calls}, but found {}",
                execution.len()
            );
    }

    // Replicate the execution stack for verification.
    let mut queue = execution.clone();

    // Verify each transition.
    while let Ok(transition) = queue.pop() {
        #[cfg(debug_assertions)]
        println!(
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

        // Retrieve the stack.
        let stack = process.get_stack(transition.program_id())?;
        // Retrieve the function from the stack.
        let function = stack.get_function(transition.function_name())?;
        // Determine the number of function calls in this function.
        let mut num_function_calls = 0;
        for instruction in function.instructions() {
            if let Instruction::Call(call) = instruction {
                // Determine if this is a function call.
                if call.is_function_call(stack)? {
                    num_function_calls += 1;
                }
            }
        }
        // If there are function calls, append their inputs and outputs.
        if num_function_calls > 0 {
            // This loop takes the last `num_function_call` transitions, and reverses them
            // to order them in the order they were defined in the function.
            for transition in (*queue).iter().rev().take(num_function_calls).rev() {
                // [Inputs] Extend the verifier inputs with the input IDs of the external call.
                inputs.extend(
                    transition
                        .inputs()
                        .iter()
                        .flat_map(|input| input.verifier_inputs()),
                );
                // [Inputs] Extend the verifier inputs with the output IDs of the external call.
                inputs.extend(transition.output_ids().map(|id| **id));
            }
        }

        // [Inputs] Extend the verifier inputs with the output IDs.
        inputs.extend(
            transition
                .outputs()
                .iter()
                .flat_map(|output| output.verifier_inputs()),
        );

        // [Inputs] Extend the verifier inputs with the fee.
        inputs.push(*I64::<Testnet3>::new(*transition.fee()).to_field()?);

        #[cfg(debug_assertions)]
        debug!(
            "Transition public inputs ({} elements): {:#?}",
            inputs.len(),
            inputs
        );

        // Retrieve the verifying key.
        let verifying_key =
            process.get_verifying_key(transition.program_id(), transition.function_name())?;
        // Ensure the proof is valid.
        ensure!(
            verifying_key.verify(transition.function_name(), &inputs, transition.proof()),
            "Transition is invalid"
        );
    }
    Ok(())
}

// TODO: keeping Process here as a parameter mainly for the ABCI to use, but it has to be removed
// in favor of a more general program store
pub fn finalize_deployment(deployment: &Deployment, process: &mut Process) -> Result<()> {
    // Compute the program stack.
    let stack = Stack::new(process, deployment.program())?;
    // Insert the verifying keys.
    for (function_name, (verifying_key, _)) in deployment.verifying_keys() {
        stack.insert_verifying_key(function_name, verifying_key.clone())?;
    }

    // Add the stack to the process.
    process.stacks.insert(*deployment.program_id(), stack);
    Ok(())
}

// these struct-level functions should probably not be in the Vm level
pub fn generate_deployment(program_string: &str, rng: &mut ThreadRng) -> Result<Deployment> {
    let program = snarkvm::prelude::Program::from_str(program_string).unwrap();

    let universal_srs = Arc::new(UniversalSRS::load()?);

    // NOTE: we're skipping the part of imported programs
    // https://github.com/Entropy1729/snarkVM/blob/2c4e282df46ed71c809fd4b49738fd78562354ac/vm/package/deploy.rs#L149

    let stack = new_init_stack(&program, universal_srs)?;
    // Return the deployment.
    stack.deploy::<AleoV0, _>(rng)
}

pub fn generate_execution(
    program_string: &str,
    function_name: Identifier,
    inputs: &[Value],
    private_key: &PrivateKey,
    rng: &mut ThreadRng,
) -> Result<Execution> {
    let program: Program = snarkvm::prelude::Program::from_str(program_string).unwrap();
    let program_id = program.id();

    ensure!(
        program.contains_function(&function_name),
        "Function '{function_name}' does not exist."
    );

    // we check this on the verify side (which will run in the blockchain)
    // repeating here just to fail early
    ensure!(
        !Program::is_coinbase(program_id, &function_name),
        "Coinbase functions cannot be called"
    );

    let mut process = Process::load()?;
    if program_id.to_string() != "credits.aleo" {
        process.add_program(&program).unwrap();
    }

    let universal_srs = Arc::new(UniversalSRS::load()?);

    // Compute the program stack.
    let stack = new_init_stack(&program, universal_srs)?;

    // Synthesize each proving and verifying key.
    for function_name in program.functions().keys() {
        stack.synthesize_key::<AleoV0, _>(function_name, rng)?
    }

    debug!(
        "executing program {} function {} inputs {:?}",
        program, function_name, inputs
    );

    // Execute the circuit.
    let authorization =
        process.authorize::<AleoV0, _>(private_key, program_id, function_name, inputs, rng)?;
    let (response, execution) = process.execute::<AleoV0, _>(authorization, rng)?;

    debug!("outputs {:?}", response.outputs());

    Ok(execution)
}

/// This function creates and initializes a `Stack` struct for a given program on the fly, providing functionality
/// related to Programs (deploy, executions, key synthesis) without the need of a `Process`. It essentially combines
/// Stack::new() and Stack::init()
fn new_init_stack(program: &Program, universal_srs: Arc<UniversalSRS>) -> Result<Stack> {
    // Retrieve the program ID.
    let program_id = program.id();
    // Ensure the program network-level domain (NLD) is correct.
    ensure!(
        program_id.is_aleo(),
        "Program '{program_id}' has an incorrect network-level domain (NLD)"
    );

    // Ensure the program contains functions.
    ensure!(
        !program.functions().is_empty(),
        "No functions present in the deployment for program '{program_id}'"
    );

    // Serialize the program into bytes.
    let program_bytes = program.to_bytes_le()?;
    // Ensure the program deserializes from bytes correctly.
    ensure!(
        program == &Program::from_bytes_le(&program_bytes)?,
        "Program byte serialization failed"
    );

    // Serialize the program into string.
    let program_string = program.to_string();
    // Ensure the program deserializes from a string correctly.
    ensure!(
        program == &Program::from_str(&program_string)?,
        "Program string serialization failed"
    );

    // Return the stack.
    // Construct the stack for the program.
    let mut stack = Stack {
        program: program.clone(),
        external_stacks: Default::default(),
        register_types: Default::default(),
        finalize_types: Default::default(),
        universal_srs,
        proving_keys: Default::default(),
        verifying_keys: Default::default(),
    };

    // TODO: Handle imports (see comment in generate_deployment())

    // Add the program closures to the stack.
    for closure in program.closures().values() {
        // Add the closure to the stack.
        // Retrieve the closure name.
        let name = closure.name();
        // Ensure the closure name is not already added.
        ensure!(
            stack.get_register_types(name).is_err(),
            "Closure '{name}' already exists"
        );

        // Compute the register types.
        let register_types = RegisterTypes::from_closure(&stack, closure)?;
        // Add the closure name and register types to the stack.
        stack.register_types.insert(*name, register_types);
        // Return success.
        // Retrieve the closure name.
        let name = closure.name();
        // Ensure the closure name is not already added.
        ensure!(
            !stack.register_types.contains_key(name),
            "Closure '{name}' already exists"
        );

        // Compute the register types.
        let register_types = RegisterTypes::from_closure(&stack, closure)?;
        // Add the closure name and register types to the stack.
        stack.register_types.insert(*name, register_types);
    }
    // Add the program functions to the stack.
    for function in program.functions().values() {
        let name = function.name();
        // Ensure the function name is not already added.
        ensure!(
            !stack.register_types.contains_key(name),
            "Function '{name}' already exists"
        );

        // Compute the register types.
        let register_types = RegisterTypes::from_function(&stack, function)?;
        // Add the function name and register types to the stack.
        stack.register_types.insert(*name, register_types);

        // If the function contains a finalize, insert it.
        if let Some((_, finalize)) = function.finalize() {
            // Compute the finalize types.
            let finalize_types = FinalizeTypes::from_finalize(&stack, finalize)?;
            // Add the finalize name and finalize types to the stack.
            stack.finalize_types.insert(*name, finalize_types);
        }
    }
    // Return the stack.
    Ok(stack)
}
