/// This module includes helper functions initially taken from SnarkVM's Stack struct.
/// The goal is to progressively remove the dependency on that struct.
use std::{str::FromStr, sync::Arc};

use super::{Identifier, Program, UniversalSRS};
use anyhow::{bail, ensure, Result};
use snarkvm::prelude::{CallOperator, FromBytes, Instruction, RegisterTypes, Testnet3, ToBytes};

pub type Stack = snarkvm::prelude::Stack<Testnet3>;

/// This function creates and initializes a `Stack` struct for a given program on the fly, providing functionality
/// related to Programs (deploy, executions, key synthesis) without the need of a `Process`. It essentially combines
/// Stack::new() and Stack::init()
pub fn new_init(program: &Program, universal_srs: Arc<UniversalSRS>) -> Result<Stack> {
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
    }
    // Return the stack.
    Ok(stack)
}

/// Determine the number of function calls in this function (including the function itself).
/// NOTE: we are not considering external function calls (from an imported program) here.
pub fn count_function_calls(program: &Program, function_name: &Identifier) -> Result<usize> {
    let mut num_function_calls = 1;
    let function = program.get_function(function_name)?;
    for instruction in function.instructions() {
        if let Instruction::Call(call) = instruction {
            num_function_calls += match call.operator() {
                CallOperator::Locator(_locator) => {
                    bail!("external function calls are not supported")
                }
                CallOperator::Resource(resource) => count_function_calls(program, resource)?,
            };
        }
    }

    Ok(num_function_calls)
}
