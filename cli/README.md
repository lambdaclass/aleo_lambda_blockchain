implement a minimal rust binary that has snarkVM as a dependency and:

- takes an aleo instructions program file
- builds it (which generates proving and verifier files)
- runs the verification function using the verifying key from the produced verifier file


Usage:

    cargo run --release -- ../hello hello 1u32 1u32
