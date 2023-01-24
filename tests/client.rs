use assert_cmd::{
    assert::{Assert, AssertError},
    Command,
};
use assert_fs::NamedTempFile;

use rand::Rng;
use retry::{self, delay::Fixed};
use serde::de::DeserializeOwned;
use std::str;
use std::{collections::HashMap, fs};

const HELLO_PROGRAM: &str = "hello";
const UNKNOWN_PROGRAM: &str = "unknown";
const TOKEN_PROGRAM: &str = "token";
const MINT_FUNCTION: &str = "mint";
const TRANSFER_FUNCTION: &str = "transfer_amount";
const CONSUME_FUNCTION: &str = "consume";

const CURRENT_ACCOUNT: &str = "%account";

#[test]
fn basic_program() {
    let (_tempfile, home_path, _) = &new_account();

    // deploy a program
    let (_program_file, program_path, _) = load_program(HELLO_PROGRAM);
    let transaction = client_command(home_path, &["program", "deploy", &program_path]).unwrap();
    let transaction_id = get_transaction_id(&transaction).unwrap();

    // get deployment tx, need to retry until it gets committed
    retry_command(home_path, &["get", transaction_id]).unwrap();

    // execute the program, save txid
    let transaction =
        execute_program(home_path, &program_path, "hello", &["1u32", "1u32"]).unwrap();

    let transaction_id = get_transaction_id(&transaction).unwrap();

    // get execution tx, assert expected output
    let transaction = retry_command(home_path, &["get", transaction_id]).unwrap();
    let value = transaction
        .pointer("/Execution/transitions/0/outputs/0/value")
        .unwrap()
        .as_str()
        .unwrap();

    // check the output of the execution is the sum of the inputs
    assert_eq!("2u32", value);
}

#[test]
fn program_validations() {
    let (_tempfile, home_path, _) = &new_account();
    let (_program_file, program_path, program_id) = load_program(HELLO_PROGRAM);

    // fail on execute non deployed command
    let error =
        execute_program(home_path, &program_path, HELLO_PROGRAM, &["1u32", "1u32"]).unwrap_err();
    assert!(error.contains("Error executing transaction 1: Could not verify transaction"));

    // not fail on dry-running non-deployed program ()
    execute_program(
        home_path,
        &program_path,
        HELLO_PROGRAM,
        &["1u32", "1u32", "--dry-run"],
    )
    .unwrap();

    // deploy a program
    client_command(home_path, &["program", "deploy", &program_path]).unwrap();

    // fail on already deployed compiled locally
    let error = client_command(home_path, &["program", "deploy", &program_path]).unwrap_err();

    assert!(error.contains("Internal error: tx already exists in cache"));

    // execute the program, retrieving it from the blockchain, using it's id
    execute_program(home_path, &program_id, "hello", &["1u32", "1u32"]).unwrap();

    // fail on program execution with an invalid id
    let error =
        execute_program(home_path, "inexistent_id.aleo", "hello", &["1u32", "1u32"]).unwrap_err();
    assert!(error.contains("Could not find program"));

    // fail on unknown function
    let error =
        execute_program(home_path, &program_path, UNKNOWN_PROGRAM, &["1u32", "1u32"]).unwrap_err();
    assert!(error.contains("does not exist"));

    // fail on missing parameter
    let error = execute_program(home_path, &program_path, HELLO_PROGRAM, &["1u32"]).unwrap_err();
    assert!(error.contains("expects 2 inputs"));
}

#[test]
fn decrypt_records() {
    let (_acc_file, home_path, credentials) = &new_account();
    let (_program_file, program_path, _) = load_program(TOKEN_PROGRAM);

    // deploy a program, save txid
    client_command(home_path, &["program", "deploy", &program_path]).unwrap();

    // get address
    let address = credentials.get("address").unwrap();

    // execute mint
    let transaction = execute_program(
        home_path,
        &program_path,
        MINT_FUNCTION,
        &["1u64", CURRENT_ACCOUNT],
    )
    .unwrap();

    let transaction_id = get_transaction_id(&transaction).unwrap();

    // test successful decryption of records (same credentials)
    let transaction = retry_command(home_path, &["get", transaction_id, "-d"]).unwrap();
    let (owner, gates, amount) = get_decrypted_record(&transaction);

    assert_eq!(amount.to_string(), "1u64.private");
    assert_eq!(gates.to_string(), "0u64.private");
    assert_eq!(owner.to_string(), format!("{address}.private"));

    // dry run contains decrypted records
    let output = execute_program(
        home_path,
        &program_path,
        MINT_FUNCTION,
        &["1u64", CURRENT_ACCOUNT, "--dry-run"],
    )
    .unwrap();

    output
        .pointer("/decrypted_records")
        .unwrap()
        .as_array()
        .unwrap();

    let (_acc_file, home_path, _) = &new_account();

    // should fail to decrypt records (different credentials)
    let transaction = retry_command(home_path, &["get", transaction_id, "-d"]).unwrap();

    let decrypted_records = transaction
        .pointer("/decrypted_records")
        .unwrap()
        .as_array()
        .unwrap();

    assert!(decrypted_records.is_empty());
}

#[test]
fn token_transaction() {
    // Create two accounts: Alice and Bob
    let (_tempfile_alice, alice_home, alice_credentials) = &new_account();
    let (_tempfile_bob, bob_home, bob_credentials) = &new_account();

    // Load token program with Alice credentials
    let (_program_file, program_path, _) = load_program("token");

    // Deploy the token program to the blockchain
    client_command(alice_home, &["program", "deploy", &program_path]).unwrap();

    // Mint 10 tokens into an Alice Record
    let transaction = execute_program(
        alice_home,
        &program_path,
        MINT_FUNCTION,
        &["10u64", CURRENT_ACCOUNT],
    )
    .unwrap();

    let transaction_id = get_transaction_id(&transaction).unwrap();

    // Get and decrypt te mint output record
    let transaction = retry_command(alice_home, &["get", transaction_id]).unwrap();

    let record = get_encrypted_record(&transaction);

    // Transfer 5 tokens from Alice to Bob
    let transaction = execute_program(
        alice_home,
        &program_path,
        TRANSFER_FUNCTION,
        &[record, bob_credentials.get("address").unwrap(), "5u64"],
    )
    .unwrap();
    let transfer_transaction_id = get_transaction_id(&transaction).unwrap();

    // Get, decrypt and assert correctness of Alice output record: Should have 5u64.private in the amount variable
    let transaction = retry_command(alice_home, &["get", transfer_transaction_id, "-d"]).unwrap();
    let (owner, _gates, amount) = get_decrypted_record(&transaction);

    assert_eq!(
        owner,
        format!("{}.private", alice_credentials.get("address").unwrap())
    );
    assert_eq!(amount, "5u64.private");

    // Get, decrypt and assert correctness of Bob output record: Should have 5u64.private in the amount variable
    let transaction = retry_command(bob_home, &["get", transfer_transaction_id, "-d"]).unwrap();
    let (owner, _gates, amount) = get_decrypted_record(&transaction);

    assert_eq!(
        owner,
        format!("{}.private", bob_credentials.get("address").unwrap())
    );
    assert_eq!(amount, "5u64.private");
}

#[test]
fn consume_records() {
    // new account41
    let (_acc_file, home_path, _) = &new_account();

    // load "records" program
    let (_program_file, program_path, _) = load_program("records");

    // deploy "records" program
    client_command(home_path, &["program", "deploy", &program_path]).unwrap();

    // execute mint
    let transaction = execute_program(
        home_path,
        &program_path,
        MINT_FUNCTION,
        &["10u64", CURRENT_ACCOUNT],
    )
    .unwrap();

    let transaction_id = transaction
        .pointer("/Execution/id")
        .unwrap()
        .as_str()
        .unwrap();

    // Get the mint record
    let transaction = retry_command(home_path, &["get", transaction_id]).unwrap();
    let record = get_encrypted_record(&transaction);

    // execute consume with output record
    execute_program(home_path, &program_path, CONSUME_FUNCTION, &[record]).unwrap();

    // execute consume with same output record, execution fails, no double spend
    let error = execute_program(home_path, &program_path, CONSUME_FUNCTION, &[record]).unwrap_err();

    assert!(error.contains("is unknown or already spent"));

    // create a fake record
    let (_new_acc_file, _new_home_path, credentials) = &new_account();

    let address = credentials.get("address").unwrap();

    let record = format!(
        "{{owner: {}.private,gates: 0u64.private,amount: 10u64.public,_nonce:{}}}",
        address,
        random_nonce()
    );

    // execute with made output record, execution fails, no use unknown record
    let error =
        execute_program(home_path, &program_path, CONSUME_FUNCTION, &[&record]).unwrap_err();

    assert!(error.contains("must belong to the signer") || error.contains("Invalid value"));
}

#[test]
fn try_create_credits() {
    let (_tempfile, home_path, _) = &new_account();

    let (_program_file, program_path, _) = load_program("records");
    client_command(home_path, &["program", "deploy", &program_path]).unwrap();
    let output = execute_program(
        home_path,
        &program_path,
        "mint_credits",
        &["100u64", "%account"],
    )
    .err()
    .unwrap();
    assert!(output.contains("is not satisfied on the given inputs"));
}

#[test]
fn transfer_credits() {
    let validator_home = validator_account_path();

    // assuming the first record has more than 10 credits
    let record = client_command(&validator_home, &["account", "records"])
        .unwrap()
        .pointer("/0/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    let (_tempfile, receiver_home, credentials) = &new_account();

    client_command(
        &validator_home,
        &[
            "credits",
            "transfer",
            &record,
            credentials.get("address").unwrap(),
            "10",
        ],
    )
    .unwrap();

    // check the the account received the balance
    // (validator balance can't be checked because it could receive a reward while the test is running)
    assert_balance(receiver_home, 10).unwrap();
}

#[test]
fn transaction_fees() {
    // create a test account
    let (_tempfile, receiver_home, credentials) = &new_account();

    // try to run a deployment with a fee but no credits available, should fail
    let (_program_file, program_path, _) = load_program(HELLO_PROGRAM);
    let output = client_command(
        receiver_home,
        &["program", "deploy", &program_path, "--fee", "100"],
    )
    .unwrap_err();
    assert!(output.contains("there are not records with enough credits for a 100 gates fee"));

    // transfer a known amount of credits to the test account
    let validator_home = validator_account_path();
    let record = client_command(&validator_home, &["account", "records"])
        .unwrap()
        .pointer("/1/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    client_command(
        &validator_home,
        &[
            "credits",
            "transfer",
            &record,
            credentials.get("address").unwrap(),
            "10",
        ],
    )
    .unwrap();

    assert_balance(receiver_home, 10).unwrap();

    // try to run a deployment with a fee of more credits than available, should fail
    let output = client_command(
        receiver_home,
        &["program", "deploy", &program_path, "--fee", "100"],
    )
    .unwrap_err();
    assert!(output.contains("there are not records with enough credits for a 100 gates fee"));

    // run a deployment with fee
    client_command(
        receiver_home,
        &["program", "deploy", &program_path, "--fee", "2"],
    )
    .unwrap();
    assert_balance(receiver_home, 8).unwrap();

    // run an execution with fee
    client_command(
        receiver_home,
        &[
            "program",
            "execute",
            &program_path,
            "hello",
            "1u32",
            "1u32",
            "--fee",
            "2",
        ],
    )
    .unwrap();
    assert_balance(receiver_home, 6).unwrap();

    // run a credits execution with a fee, should account for the implicit fees
    let record = client_command(receiver_home, &["account", "records"])
        .unwrap()
        .pointer("/0/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // at this point there's a single record in the account, since we want to run a credits program
    // below and also pay a separate fee, we'll have to split it first
    let transaction = client_command(receiver_home, &["credits", "split", &record, "3"]).unwrap();

    // request the transaction until it's committed before moving on, to ensure records are available
    let transaction_id = get_transaction_id(&transaction).unwrap();
    retry_command(receiver_home, &["get", transaction_id]).unwrap();

    let record = client_command(receiver_home, &["account", "records"])
        .unwrap()
        .pointer("/0/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // using credits fee as a regular execution because it's the only handy function we have
    // to generate a transition that spends credits on its own.
    client_command(
        receiver_home,
        &[
            "program",
            "execute",
            "aleo/credits.aleo",
            "fee",
            &record,
            "2u64",
            "--fee",
            "3",
        ],
    )
    .unwrap();

    // it had 2 records of 3 credits each. executed the fee function with 2 of input and requested total 3 of fee
    // the execution has an implicit fee of 2 so another 1 is payed from the other record to reach the requested 3 of fee
    // so there should be another 3 remaining
    assert_balance(receiver_home, 3).unwrap();

    // run another command specifying which record to use
    let record = client_command(receiver_home, &["account", "records"])
        .unwrap()
        .pointer("/0/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    client_command(
        receiver_home,
        &[
            "program",
            "execute",
            &program_path,
            "hello",
            "1u32",
            "1u32",
            "--fee",
            "1",
            "--fee-record",
            &record,
        ],
    )
    .unwrap();
    assert_balance(receiver_home, 2).unwrap();
}

#[test]
fn staking() {
    // create a test account
    let (_tempfile, receiver_home, credentials) = &new_account();

    // transfer a known amount of credits to the test account
    let validator_home = validator_account_path();
    let tendermint_validator = validator_address(&validator_home);
    let record = client_command(&validator_home, &["account", "records"])
        .unwrap()
        .pointer("/2/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    client_command(
        &validator_home,
        &[
            "credits",
            "transfer",
            &record,
            credentials.get("address").unwrap(),
            "50",
        ],
    )
    .unwrap();

    assert_balance(receiver_home, 50).unwrap();

    let user_record = client_command(receiver_home, &["account", "records"])
        .unwrap()
        .pointer("/0/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // try to stake more than available, fail
    let error = client_command(
        receiver_home,
        &[
            "credits",
            "stake",
            "60",
            &user_record,
            &tendermint_validator,
        ],
    )
    .unwrap_err();
    
    // FIXME currently this results in an unexpected failure because of how snarkvm handles integer overflow errors
    // this should be improved to properly handle execution errors internally and showing a clear error message in the CLI
    assert!(error.contains("Integer subtraction failed"));

    // TODO add check: try to stake for an unexistent validator, fail

    // stake all available, but fail because this is not the expected aleo account
    let error = client_command(
        receiver_home,
        &[
            "credits",
            "stake",
            "50",
            &user_record,
            &tendermint_validator,
        ],
    )
    .unwrap_err();
    assert!(error.contains("attempted to apply a staking update on a different aleo account"));

    let validator_record = client_command(&validator_home, &["account", "records"])
        .unwrap()
        .pointer("/0/ciphertext")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    // stake some credits from the validator account
    let transaction = client_command(
        &validator_home,
        &[
            "credits",
            "stake",
            "5",
            &validator_record,
            &tendermint_validator,
        ],
    )
    .unwrap();

    let staked_credits_record = transaction
        .pointer("/Execution/transitions/0/outputs/1/value")
        .unwrap()
        .as_str()
        .unwrap();

    // try to unstake more than available, fail
    let error = client_command(
        &validator_home,
        &["credits", "unstake", "15", staked_credits_record],
    )
    .unwrap_err();
    // FIXME currently this results in an unexpected failure because of how snarkvm handles integer overflow errors
    // this should be improved to properly handle execution errors internally and showing a clear error message in the CLI
    assert!(error.contains("Integer subtraction failed"));

    // unstake all available
    client_command(
        &validator_home,
        &["credits", "unstake", "5", staked_credits_record],
    )
    .unwrap();

    // TODO: Test to see if the validator_set file actually gets updated with staking updates
}

// HELPERS

/// Retries iteratively to get a transaction until something returns
/// If `home_path` is Some(val), it uses the val as the credentials file in order to get the required credentials to attempt decryption
fn retry_command(
    home_path: &str,
    args: &[&str],
) -> Result<serde_json::Value, retry::Error<AssertError>> {
    retry::retry(Fixed::from_millis(1000).take(10), || {
        let mut command = &mut Command::cargo_bin("client").unwrap();
        command = command.env("ALEO_HOME", home_path);
        command.args(args).assert().try_success()
    })
    .map(parse_output)
}

fn random_nonce() -> String {
    const CHARSET: &[u8] = b"0123456789";
    const NONCE_LENGTH: usize = 80;

    let mut rng = rand::thread_rng();

    let nonce: String = (0..NONCE_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    format!("{nonce}group.public")
}

/// Generate a tempfile with account credentials and return it along with the aleo home path.
/// The file will be removed when it goes out of scope.
fn new_account() -> (NamedTempFile, String, HashMap<String, String>) {
    let tempfile = NamedTempFile::new(".aleo/account.json").unwrap();
    let aleo_path = tempfile
        .path()
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

    let credentials = client_command(&aleo_path, &["account", "new"]).unwrap();

    let credentials: HashMap<String, String> =
        serde_json::from_value(credentials.pointer("/account").unwrap().clone()).unwrap();

    (tempfile, aleo_path, credentials)
}

/// Load the source code from the given example file, randomize it's name, and return a tempfile
/// with the same source code but with the new name, along with its path and the new id.
/// The file will be removed when it goes out of scope.
fn load_program(program_name: &str) -> (NamedTempFile, String, String) {
    let program_file = NamedTempFile::new(program_name).unwrap();
    let path = program_file.path().to_string_lossy().to_string();
    // FIXME hardcoded path
    let source = fs::read_to_string(format!("aleo/{program_name}.aleo")).unwrap();
    // randomize the name so it's different on every test
    let program_id = format!("{}{}.aleo", program_name, unique_id());
    let source = source.replace(&format!("{program_name}.aleo"), &program_id);
    fs::write(&path, source).unwrap();
    (program_file, path, program_id)
}

fn unique_id() -> String {
    uuid::Uuid::new_v4()
        .to_string()
        .split('-')
        .last()
        .unwrap()
        .to_string()
}

/// Extract the command assert output and deserialize it as json
fn parse_output<T: DeserializeOwned>(result: Assert) -> T {
    let output = String::from_utf8(result.get_output().stdout.to_vec()).unwrap();
    serde_json::from_str(&output).unwrap()
}

fn get_transaction_id(transaction: &serde_json::Value) -> Option<&str> {
    if let Some(value) = transaction.pointer("/Execution/id") {
        return value.as_str();
    }
    transaction.pointer("/Deployment/id").unwrap().as_str()
}

fn get_decrypted_record(transaction: &serde_json::Value) -> (&str, &str, &str) {
    let amount = transaction
        .pointer("/decrypted_records/0/data/amount")
        .unwrap()
        .as_str()
        .unwrap();
    let gates = transaction
        .pointer("/decrypted_records/0/gates")
        .unwrap()
        .as_str()
        .unwrap();
    let owner = transaction
        .pointer("/decrypted_records/0/owner")
        .unwrap()
        .as_str()
        .unwrap();

    (owner, gates, amount)
}

fn get_encrypted_record(transaction: &serde_json::Value) -> &str {
    transaction
        .pointer("/Execution/transitions/0/outputs/0/value")
        .unwrap()
        .as_str()
        .unwrap()
}

fn assert_balance(path: &str, expected: u64) -> Result<(), retry::Error<String>> {
    retry::retry(Fixed::from_millis(1000).take(10), || {
        let balance = client_command(path, &["account", "balance"])
            .unwrap()
            .pointer("/balance")
            .unwrap()
            .as_u64()
            .unwrap();

        if balance == expected {
            Ok(())
        } else {
            Err(format!("expected {expected} found {balance}"))
        }
    })
}

fn execute_program(
    user_path: &str,
    program_path: &str,
    fun: &str,
    inputs: &[&str],
) -> Result<serde_json::Value, String> {
    let args = [&["program", "execute", program_path, fun], inputs].concat();
    client_command(user_path, &args)
}

fn client_command(user_path: &str, args: &[&str]) -> Result<serde_json::Value, String> {
    let mut command = &mut Command::cargo_bin("client").unwrap();

    command = command.env("ALEO_HOME", user_path);

    command
        .args(args)
        .assert()
        .try_success()
        .map(parse_output)
        .map_err(|e| e.to_string())
}

fn validator_account_path() -> String {
    dirs::home_dir()
        .unwrap()
        .join(".tendermint")
        .to_string_lossy()
        .into()
}

fn validator_address(tendermint_home: &str) -> String {
    let key_path = std::path::Path::new(tendermint_home).join("config/priv_validator_key.json");
    let json = std::fs::read_to_string(key_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    json["pub_key"]["value"].as_str().unwrap().to_string()
}
