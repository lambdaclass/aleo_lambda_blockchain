# Aleo Consensus
MVPs for an aleo blockchain

## Prover and Verifier

You can run a `Verifier` server that listens on port 6200 for executions, deserializes them and verifies them with

```
cd verifier
cargo run --release
```

With the Verifier running, there is a `Prover` CLI that executes a program and sends the resulting execution to the Verifier. To run it:

```
cd prover
cargo run --release -- ../hello hello 1u32 1u32
```

Currently the program and function being executed are hardcoded to be the `hello` function in `hello.aleo`.
