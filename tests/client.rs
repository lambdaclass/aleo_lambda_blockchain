use assert_cmd::{
    assert::{Assert, AssertError},
    Command,
};
use assert_fs::NamedTempFile;
use lib::{
    transaction::Transaction,
    vm::{Identifier, Output},
    GetDecryptionResponse,
};
use rand::Rng;
use retry::{delay::Fixed, Error};
use serde::de::DeserializeOwned;
use std::{collections::HashMap, fs};

#[test]
fn basic_program() {
    let (_tempfile, home_path) = &new_account();

    // deploy a program, save txid
    let (_program_file, program_path) = load_program("hello");
    let result = Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "deploy", &program_path])
        .assert()
        .success();
    let transaction: Transaction = parse_output(result);

    // get deployment tx, need to retry until it gets committed
    retry_command(home_path, &["get", transaction.id()])
        .unwrap()
        .success();

    // execute the program, save txid
    let result = Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "execute", &program_path, "hello", "1u32", "1u32"])
        .assert()
        .success();
    let transaction: Transaction = parse_output(result);

    // get execution tx, assert expected output
    let result = retry_command(home_path, &["get", transaction.id()]);
    let transaction: Transaction = parse_output(result.unwrap());

    // check the output of the execution is the sum of the inputs
    if let Transaction::Execution { execution, .. } = transaction {
        let transition = execution.peek().unwrap();
        let output = transition.outputs();

        if let Output::Public(_, Some(ref value)) = output[0] {
            assert_eq!("2u32", value.to_string());
        } else {
            panic!("expected a public output");
        }
    } else {
        panic!("expected an execution");
    }
}

#[test]
fn program_validations() {
    let (_tempfile, home_path) = &new_account();
    let (_program_file, program_path) = load_program("hello");

    // fail on execute non deployed command
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "execute", &program_path, "hello", "1u32", "1u32"])
        .assert()
        .failure();

    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "deploy", &program_path])
        .assert()
        .success();

    // fail on already deployed
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "deploy", &program_path])
        .assert()
        .failure();

    // fail on unknown function
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args([
            "program",
            "execute",
            &program_path,
            "goodbye",
            "1u32",
            "1u32",
        ])
        .assert()
        .failure();

    // fail on missing parameter
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "execute", &program_path, "hello", "1u32"])
        .assert()
        .failure();
}

#[test]
fn decrypt_records() {
    let (acc_file, home_path) = &new_account();
    let (_program_file, program_path) = load_program("token");

    let _ = Command::cargo_bin("client")
        .unwrap()
        .args(["program", "deploy", &program_path])
        .env("ALEO_HOME", home_path)
        .assert()
        .success();

    let account: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(acc_file).unwrap()).unwrap();
    let address = account.get("address").unwrap();

    let result = Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args(["program", "execute", &program_path, "mint", "1u64", address])
        .assert()
        .success();

    let transaction: Transaction = parse_output(result);

    // test successful decryption of records (same credentials)
    let result = retry_command(home_path, &["get", transaction.id(), "-d"])
        .unwrap()
        .success();

    let output: GetDecryptionResponse = parse_output(result);
    let owner_address = format!("{}.private", address);
    let record = &output.decrypted_records[0];
    assert_eq!(record.owner().to_string(), owner_address);
    let value = record
        .data()
        .get(&Identifier::try_from("amount").unwrap())
        .unwrap();
    assert_eq!(value.to_string(), "1u64.private");

    let (_acc_file, home_path) = &new_account();

    // should fail to decrypt records (different credentials)
    let result = retry_command(home_path, &["get", transaction.id(), "-d"])
        .unwrap()
        .success();
    let output: GetDecryptionResponse = parse_output(result);
    assert!(output.decrypted_records.is_empty());
}

#[test]
fn token_transaction() {
    // Create two accounts: Alice and Bob
    let (tempfile_alice, alice_home) = &new_account();
    let (tempfile_bob, bob_home) = &new_account();

    // Load token program with Alice credentials
    let (_program_file, program_path) = load_program("token");

    // Deploy the token program to the blockchain
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", alice_home)
        .args(["program", "deploy", &program_path])
        .assert()
        .success();

    // Load Alice and Bob credentials
    let alice_credentials: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(tempfile_alice).unwrap()).unwrap();
    let bob_credentials: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(tempfile_bob).unwrap()).unwrap();

    // Mint 10 tokens into an Alice Record
    let result = Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", alice_home)
        .args([
            "program",
            "execute",
            &program_path,
            "mint",
            "10u64",
            alice_credentials.get("address").unwrap(),
        ])
        .assert()
        .success();

    // parse mint transacction output
    let transaction: Transaction = parse_output(result);

    // Get and decrypt te mint output record
    let result = retry_command(alice_home, &["get", transaction.id()])
        .unwrap()
        .success();

    let tx: Transaction = parse_output(result);

    // Transfer 5 tokens from Alice to Bob
    let output = Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", alice_home)
        .args([
            "program",
            "execute",
            &program_path,
            "transfer_amount",
            &tx.output_records()[0].to_string(),
            bob_credentials.get("address").unwrap(),
            "5u64",
        ])
        .assert()
        .success();

    // parse transfer transacction output
    let transaction: Transaction = parse_output(output);

    // Get, decrypt and assert correctness of Alice output record: Should have 5u64.private in the amount variable
    let result = retry_command(alice_home, &["get", transaction.id(), "-d"])
        .unwrap()
        .success();
    let output: GetDecryptionResponse = parse_output(result);
    let record = &output.decrypted_records[0];
    let alice_address = format!("{}.private", alice_credentials.get("address").unwrap());
    assert_eq!(record.owner().to_string(), alice_address);
    let value = record
        .data()
        .get(&Identifier::try_from("amount").unwrap())
        .unwrap();
    assert_eq!(value.to_string(), "5u64.private");

    // Get, decrypt and assert correctness of Bob output record: Should have 5u64.private in the amount variable
    let result = retry_command(bob_home, &["get", transaction.id(), "-d"])
        .unwrap()
        .success();
    let output: GetDecryptionResponse = parse_output(result);
    let record = &output.decrypted_records[0];
    let bob_address = format!("{}.private", bob_credentials.get("address").unwrap());
    assert_eq!(record.owner().to_string(), bob_address);
    let value = record
        .data()
        .get(&Identifier::try_from("amount").unwrap())
        .unwrap();
    assert_eq!(value.to_string(), "5u64.private");
}

#[test]
fn consume_records() {
    // new account41
    let (acc_file, home_path) = &new_account();

    // load "records" program
    let (_program_file, program_path) = load_program("records");

    // deploy "records" program
    Command::cargo_bin("client")
        .unwrap()
        .args(["program", "deploy", &program_path])
        .env("ALEO_HOME", home_path)
        .assert()
        .success();

    // get account address
    let account: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(acc_file).unwrap()).unwrap();

    let address = account.get("address").unwrap();

    // execute mint
    let output = Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args([
            "program",
            "execute",
            &program_path,
            "mint",
            "10u64",
            address,
        ])
        .assert()
        .success();

    // parse tx
    let transaction: Transaction = parse_output(output);

    // Get and decrypt te mint output record
    let result = retry_command(home_path, &["get", transaction.id()])
        .unwrap()
        .success();

    let tx: Transaction = parse_output(result);

    // execute consume with output record
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args([
            "program",
            "execute",
            &program_path,
            "consume",
            &tx.output_records()[0].to_string(),
        ])
        .assert()
        .success();

    // execute consume with same output record, execution fails, no double spend
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", home_path)
        .args([
            "program",
            "execute",
            &program_path,
            "consume",
            &tx.output_records()[0].to_string(),
        ])
        .assert()
        .failure();

    // create a fake record
    let (new_acc_file, new_home_path) = &new_account();

    let new_account: HashMap<String, String> =
        serde_json::from_str(&fs::read_to_string(new_acc_file).unwrap()).unwrap();

    let address = new_account.get("address").unwrap();

    let record = format!(
        "{{owner: {}.private,gates: 0u64.private,amount: 10u64.public,_nonce:{}}}",
        address,
        random_nonce()
    );

    // execute with made output record, execution fails, no use unknown record
    Command::cargo_bin("client")
        .unwrap()
        .env("ALEO_HOME", new_home_path)
        .args(["program", "execute", &program_path, "consume", &record])
        .assert()
        .failure();
}

// HELPERS

/// Retries iteratively to get a transaction until something returns
/// If `home_path` is Some(val), it uses the val as the credentials file in order to get the required credentials to attempt decryption
fn retry_command(home_path: &str, args: &[&str]) -> Result<Assert, Error<AssertError>> {
    retry::retry(Fixed::from_millis(1000).take(5), || {
        Command::cargo_bin("client")
            .unwrap()
            .env("ALEO_HOME", home_path)
            .args(args)
            .assert()
            .try_success()
    })
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
