# Aleo Consensus

MVP for an aleo blockchain. The current implementation uses a thin wrapper around some layers of [this SnarkVM fork](https://github.com/Entropy1729/snarkVM) and [Tendermint](https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html) for the blockchain and consensus.

## Project structure

* [aleo/](./aleo): example aleo instruction programs
* [src/cli.rs](./src/cli.rs): CLI program to interacto with the VM and the blockchain (e.g. create an acocut, deploy and execute programs)
* [src/snarkvm_abci/](./src/snarkvm_abci): Implements the [Application Blockchain Interface](https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview) (ABCI) to connect the aleo specific logic (e.g. program proof verification) to the Tendermint Core infrastructure.

## Running a single-node blockchain

Requires Rust and `jq` to be installed.

Run `snarkvm_abci`:

```shell
make abci
```

This will have our ABCI app running and ready to connect to a tendermint node:

```
2022-11-07T20:32:21.577768Z  INFO ThreadId(01) ABCI server running at 127.0.0.1:26658
```

In another terminal run the tendermint node:

```shell
make node
```

This will download and install tendermint if necessary. Alternatively, [these instructions](https://github.com/tendermint/tendermint/blob/main/docs/introduction/install.md) can be used.
Be sure to checkout version `0.34.x` as current rust abci implementation does not support `0.35.0` (`0.35.0` has not been released yet).

At this point, both terminals should start to exchange messages and `Commited height` should start to grow.

## Sending deploys and executions to the blockchain

On another terminal, compile the command-line client:

    make cli

Create an aleo account:

    bin/aleo account new

This will generate an account address and the credentials necessary to generate execution proofs. Now run the client app to deploy an aleo program:

```shell
bin/aleo program deploy aleo/hello.aleo
```

That should take some time to create the deployment transaction and send it to tendermint. In the client terminal you should see something like:

```
2022-11-07T20:37:19.377Z INFO [client] Deploying program program hello.aleo;

function hello:
    input r0 as u32.public;
    input r1 as u32.private;
    add r0 r1 into r2;
    output r2 as u32.private;

 • Loaded universal setup (in 1793 ms)
 • Built 'hello' (in 6363 ms)
 • Certified 'hello': 147 ms
2022-11-07T20:37:32.973Z DEBUG [tendermint_rpc::client::transport::http::sealed] Outgoing request: {
  "jsonrpc": "2.0",
  "id": "c45e1f52-a9f1-414e-ab2c-8b02746ee349",
  "method": "broadcast_tx_sync",
  "params": {
    "tx": "AAAAACQAAAAAAAAANTZhZ...
```
And a very long encoded transaction. Finally something like

```
2022-11-07T20:37:33.922Z DEBUG [tendermint_rpc::client::transport::http::sealed] Incoming response: {
  "jsonrpc": "2.0",
  "id": "c45e1f52-a9f1-414e-ab2c-8b02746ee349",
  "result": {
    "code": 0,
    "data": "",
    "log": "",
    "codespace": "",
    "hash": "6A58F82922F436ECF8765F7AEF90AC79BE8091A7D5AF14C121326DB5533A9339"
  }
}
```

With a code 0 meaning the program was successfully deployed. You should also see the transaction being received in the ABCI terminal with some message like:

```
2022-11-07T20:36:49.862776Z  INFO ThreadId(65) Check Tx
2022-11-07T20:36:49.868743Z  INFO ThreadId(65) Verifying Execution: {"edition":0,"transitions":[{"id":"as1n0tlsr9rglamwr9tcqxf60ndpgkvhu83py79t78808w2vx95tv9q2ataeg","program":"hello.aleo","function":"hello","inputs":[{"type":"public","id":"1478829010713049050956129212113341334476706503997215127720201268298504260669field","value":"1u32"},{"type":"private","id":"375755831552960522697901416301536744612216033684938328198044587457082215962field","value":"ciphertext1qyq0lmjlsmwjwxxuxft5vw24pqj70fv76pj2q8a96m37mpyqregxzrge629d3"}],"outputs":[{"type":"private","id":"567543593766656021073803866365676200751324568935625558453747354077101654776field","value":"ciphertext1qyq265hx4fqu0edg8rdlwl44vatns7jwtn4hksfpylma3h3nm7rrxpcew67q6"}],"proof":"proof1qqqqzqqqqqqqqqqqm2uje400mwxc56umrwqj8jfefxnfnplgtcl7gc9kq68rxwnfzk...
```
and the rest of the transaction.


Finally to execute a program (locally) and send the execution transaction (with its proof) run in client terminal:

```shell
bin/aleo program execute aleo/hello.aleo hello 1u32 1u32
```

The command above will run the program and send the execution to the blockchain:

```
2022-11-07T20:44:07.702Z INFO [client] executing program hello.aleo function hello inputs [1u32, 1u32]
🚀 Executing 'hello.aleo/hello'...

 • Executing 'hello.aleo/hello'...
 • Executed 'hello' (in 1151 ms)
2022-11-07T20:44:15.817Z INFO [client] outputs [2u32]
2022-11-07T20:44:15.817Z DEBUG [tendermint_rpc::client::transport::http::sealed] Outgoing request: {
  "jsonrpc": "2.0",
  "id": "3a45b4de-db1a-4d8a-a23c-dbe6180003f5",
  "method": "broadcast_tx_sync",
  "params": {
    "tx": "AQAAACQAAAAAAAAAY2RkNTNlZjktM2I5Zi00N...
```
and finally
```
2022-11-07T20:44:15.830Z DEBUG [tendermint_rpc::client::transport::http::sealed] Incoming response: {
  "jsonrpc": "2.0",
  "id": "3a45b4de-db1a-4d8a-a23c-dbe6180003f5",
  "result": {
    "code": 0,
    "data": "",
    "log": "",
    "codespace": "",
    "hash": "51943117E73DC0521E0502795ADD8DC1A40856342E5A9F6516E0ECCDC66E0B13"
  }
}
```
with the success response.

After each execution, tendermint node may be left in an invalid state. If that's the case run:

```shell
make reset
```

to restore the initial state.

## Running multiple nodes on docker compose

Follow [Tendermint Core instructions](https://docs.tendermint.com/v0.34/networks/docker-compose.html) to build the tendermint binary and the tendermint/localnode image

Copy the tendermint binary into a `build` directory in the root of this project.

Then build the `snarkvm_abci` image:

```
make localnet-build-abci
```

And to start the test net run:

```
make localnet-start
```

Note that each node will require more than 2Gb to run so docker should be configured to use 10Gb or more in order to work with the default 4 nodes.

To modify the configuartion you should edit `docker-compose.yml` file

The configuration mounts some volumes in the `build/node{_}/` directories, and in case the tendermint nodes state needs to be reset, just run

```
make localnet-reset
```

or delete all the `node{_}` dirs to remove local `snarkvm_abci` data (it will require to download all the parameters on next run)

## Design

The diagram below describes the current architecture of the system:

![This is an image](doc/architecture.png)

* The blockchain nodes run in a peer to peer network where each node contains a Tendermint core and an application process.
* Tendermint core handles the basic functions of a blockchain: p2p networking, receiving transactions and relying them to peers, running a consensus algorithm to propose and vote for blocks and keeping a ledger of committed transactions.
* The application tracks application-specific logic and state. The state is derived from the transactions seen by the node (in our case, the set of spent and unspent records, and the deployed program certificates). The logic includes validating the execution transactions by verifying their proofs.
* The application is isolated from the outer world and communicates exclusively with the tendermint process through specific hooks of the Application Blockchain Interface (abci). For example: the `CheckTx` hook is used to validate transactions before putting them in the local mempool and relaying them to the peers, the `DeliverTx` writes application state changes derived from transactions included in a block and the `Commit` hook applies those changes when the block is committed to the ledger.

These interactions between tendermint core and the application are depicted below:

<img src="doc/abci.png" width="640">

For a diagram of the the consensus protocol check the [tendermint documentation](https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview).

Below are sequence diagrams of deployment and execution transactions.

TODO deploy sequence diagram
TODO execution sequence diagram

## Implementation notes

* [ABCI overview](https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview)
* [About why app hash is needed](https://github.com/tendermint/tendermint/issues/1179). Also [this](https://github.com/tendermint/tendermint/blob/v0.34.x/spec/abci/apps.md#query-proofs).
