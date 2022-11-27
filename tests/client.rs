use assert_cmd::{
    assert::{Assert, AssertError},
    Command,
};
use assert_fs::NamedTempFile;
use rand::Rng;
use retry::{self, delay::Fixed};
use serde::de::DeserializeOwned;
use std::{collections::HashMap, fs};

const HELLO_PROGRAM: &str = "hello";
const UNKNOWN_PROGRAM: &str = "unknown";
const TOKEN_PROGRAM: &str = "token";
const MINT: &str = "mint";
const GET: &str = "get";
const TRANSFER: &str = "transfer_amount";
const CONSUME: &str = "consume";

const CURRENT_ACCOUNT: &str = "%account";

#[test]
fn basic_program() {
    let (_tempfile, home_path) = &new_account();

    // deploy a program, save txid
    let (_program_file, program_path) = load_program(HELLO_PROGRAM);
    let transaction = deploy_program(home_path, &program_path).unwrap();
    let transaction_id = get_transaction_id(&transaction).unwrap();

    // get deployment tx, need to retry until it gets committed
    retry_command(home_path, &[GET, transaction_id]).unwrap();

    // execute the program, save txid
    let transaction =
        execute_program(home_path, &program_path, HELLO_PROGRAM, &["1u32", "1u32"]).unwrap();

    let transaction_id = get_transaction_id(&transaction).unwrap();

    // get execution tx, assert expected output
    let transaction = retry_command(home_path, &[GET, transaction_id]).unwrap();
    let value = transaction
        .pointer("/Execution/execution/transitions/0/outputs/0/value")
        .unwrap()
        .as_str()
        .unwrap();

    // check the output of the execution is the sum of the inputs
    assert_eq!("2u32", value);
}

#[test]
fn program_validations() {
    let (_tempfile, home_path) = &new_account();
    let (_program_file, program_path) = load_program(HELLO_PROGRAM);

    // fail on execute non deployed command
    execute_program(home_path, &program_path, HELLO_PROGRAM, &["1u32", "1u32"]).unwrap_err();

    // deploy a program, save txid
    deploy_program(home_path, &program_path).unwrap();

    // fail on already deployed
    deploy_program(home_path, &program_path).unwrap_err();

    // fail on unknown function
    execute_program(home_path, &program_path, UNKNOWN_PROGRAM, &["1u32", "1u32"]).unwrap_err();

    // fail on missing parameter
    execute_program(home_path, &program_path, HELLO_PROGRAM, &["1u32"]).unwrap_err();
}

#[test]
fn decrypt_records() {
    let (acc_file, home_path) = &new_account();
    let (_program_file, program_path) = load_program(TOKEN_PROGRAM);

    // deploy a program, save txid
    deploy_program(home_path, &program_path).unwrap();

    // get address
    let account: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(acc_file).unwrap()).unwrap();
    let address = account.get("address").unwrap();

    // execute mint
    let transaction =
        execute_program(home_path, &program_path, MINT, &["1u64", CURRENT_ACCOUNT]).unwrap();

    let transaction_id = get_transaction_id(&transaction).unwrap();

    // test successful decryption of records (same credentials)
    let transaction = retry_command(home_path, &[GET, transaction_id, "-d"]).unwrap();
    let (owner, gates, amount) = get_decrypted_record(&transaction);

    assert_eq!(amount.to_string(), "1u64.private");
    assert_eq!(gates.to_string(), "0u64.private");
    assert_eq!(owner.to_string(), format!("{}.private", address));

    let (_acc_file, home_path) = &new_account();

    // // should fail to decrypt records (different credentials)
    let transaction = retry_command(home_path, &[GET, transaction_id, "-d"]).unwrap();

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
    let (tempfile_alice, alice_home) = &new_account();
    let (tempfile_bob, bob_home) = &new_account();

    // Load token program with Alice credentials
    let (_program_file, program_path) = load_program("token");

    // Deploy the token program to the blockchain
    deploy_program(alice_home, &program_path).unwrap();

    // Load Alice and Bob credentials
    let alice_credentials: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(tempfile_alice).unwrap()).unwrap();
    let bob_credentials: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(tempfile_bob).unwrap()).unwrap();

    // Mint 10 tokens into an Alice Record
    let transaction =
        execute_program(alice_home, &program_path, MINT, &["10u64", CURRENT_ACCOUNT]).unwrap();

    let transaction_id = get_transaction_id(&transaction).unwrap();

    // Get and decrypt te mint output record
    let transaction = retry_command(alice_home, &[GET, transaction_id]).unwrap();

    let record = get_encrypted_record(&transaction);

    // Transfer 5 tokens from Alice to Bob
    let transaction = execute_program(
        alice_home,
        &program_path,
        TRANSFER,
        &[record, bob_credentials.get("address").unwrap(), "5u64"],
    )
    .unwrap();
    let transfer_transaction_id = get_transaction_id(&transaction).unwrap();

    // Get, decrypt and assert correctness of Alice output record: Should have 5u64.private in the amount variable
    let transaction = retry_command(alice_home, &[GET, transfer_transaction_id, "-d"]).unwrap();
    let (owner, _gates, amount) = get_decrypted_record(&transaction);

    assert_eq!(
        owner,
        format!("{}.private", alice_credentials.get("address").unwrap())
    );
    assert_eq!(amount, "5u64.private");

    // Get, decrypt and assert correctness of Bob output record: Should have 5u64.private in the amount variable
    let transaction = retry_command(bob_home, &[GET, transfer_transaction_id, "-d"]).unwrap();
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
    let (_acc_file, home_path) = &new_account();

    // load "records" program
    let (_program_file, program_path) = load_program("records");

    // deploy "records" program
    deploy_program(home_path, &program_path).unwrap();

    // execute mint
    let transaction =
        execute_program(home_path, &program_path, MINT, &["10u64", CURRENT_ACCOUNT]).unwrap();

    let transaction_id = transaction
        .pointer("/Execution/id")
        .unwrap()
        .as_str()
        .unwrap();

    // Get the mint record
    let transaction = retry_command(home_path, &[GET, transaction_id]).unwrap();
    let record = get_encrypted_record(&transaction);

    // execute consume with output record
    execute_program(home_path, &program_path, CONSUME, &[record]).unwrap();

    // execute consume with same output record, execution fails, no double spend
    execute_program(home_path, &program_path, CONSUME, &[record]).unwrap_err();

    // create a fake record
    let (new_acc_file, _new_home_path) = &new_account();

    let new_account: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(new_acc_file).unwrap()).unwrap();

    let address = new_account.get("address").unwrap();

    let record = format!(
        "{{owner: {}.private,gates: 0u64.private,amount: 10u64.public,_nonce:{}}}",
        address,
        random_nonce()
    );

    // execute with made output record, execution fails, no use unknown record
    execute_program(home_path, &program_path, CONSUME, &[&record]).unwrap_err();
}

#[test]
fn validate_credits() {
    let (_tempfile, home_path) = &new_account();

    let credits_path = "aleo/credits.aleo";

    // test that executing the mint function fails
    let output = execute_program(home_path, credits_path, MINT, &["%account", "100u64"])
        .err()
        .unwrap();
    assert!(output.contains("Coinbase functions cannot be called"));

    // test that executing the genesis function fails
    let output = execute_program(home_path, credits_path, "genesis", &["%account", "100u64"])
        .err()
        .unwrap();
    assert!(output.contains("Coinbase functions cannot be called"));

    let (_program_file, program_path) = load_program("credits");
    deploy_program(home_path, &program_path).unwrap();
    let output = execute_program(home_path, &program_path, MINT, &["%account", "100u64"])
        .err()
        .unwrap();
    assert!(output.contains("is not satisfied on the given inputs"));
}

#[test]
fn transfer_credits() {
    // TODO implement this when the get records feature is implemented
    // (so we know what local account records are available for transfer)

    // (this assumes the blockchain is running with minted tokens for the local account)
    // create a test account
    // using the local environment ALEO_HOME, execute credits transfer to the test account
    // check output record with ALEO_HOME account, verify it has moved credits out
    // check output record with test account, verify it has received credits
}

// HELPERS

/// Retries iteratively to get a transaction until something returns
/// If `home_path` is Some(val), it uses the val as the credentials file in order to get the required credentials to attempt decryption
fn retry_command(
    home_path: &str,
    args: &[&str],
) -> Result<serde_json::Value, retry::Error<AssertError>> {
    retry::retry(Fixed::from_millis(1000).take(5), || {
        Command::cargo_bin("client")
            .unwrap()
            .env("ALEO_HOME", home_path)
            .args(args)
            .assert()
            .try_success()
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

    format!("{}group.public", nonce)
}

/// Generate a tempfile with account credentials and return it along with the aleo home path.
/// The file will be removed when it goes out of scope.
fn new_account() -> (NamedTempFile, String) {
    let tempfile = NamedTempFile::new(".aleo/account.json").unwrap();
    let aleo_path = tempfile
        .path()
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string();

    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", aleo_path.clone())
        .args(["account", "new"])
        .assert()
        .success();

    (tempfile, aleo_path)
}

/// Load the source code from the given example file, and return a tempfile along with its path,
/// with the same source code but a randomized name.
/// The file will be removed when it goes out of scope.
fn load_program(program_name: &str) -> (NamedTempFile, String) {
    let program_file = NamedTempFile::new(program_name).unwrap();
    let path = program_file.path().to_string_lossy().to_string();
    // FIXME hardcoded path
    let source = fs::read_to_string(format!("aleo/{}.aleo", program_name)).unwrap();
    // randomize the name so it's different on every test
    let source = source.replace(
        &format!("{}.aleo", program_name),
        &format!("{}{}.aleo", program_name, unique_id()),
    );
    fs::write(&path, source).unwrap();
    (program_file, path)
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

fn deploy_program(user_path: &str, program_path: &str) -> Result<serde_json::Value, String> {
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", user_path)
        .args(["program", "deploy", program_path])
        .assert()
        .try_success()
        .map(parse_output)
        .map_err(|e| e.to_string())
}

fn execute_program(
    user_path: &str,
    program_path: &str,
    fun: &str,
    inputs: &[&str],
) -> Result<serde_json::Value, String> {
    let args = [&["program", "execute", program_path, fun], inputs].concat();
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", user_path)
        .args(args)
        .assert()
        .try_success()
        .map(parse_output)
        .map_err(|e| e.to_string())
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
        .pointer("/Execution/execution/transitions/0/outputs/0/value")
        .unwrap()
        .as_str()
        .unwrap()
}
