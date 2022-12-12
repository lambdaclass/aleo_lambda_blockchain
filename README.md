# Aleo Consensus

MVP for an aleo blockchain. The current implementation uses a thin wrapper around some layers of [this SnarkVM fork](https://github.com/Entropy1729/snarkVM) and [Tendermint](https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html) for the blockchain and consensus.

## Project structure

* [aleo/](./aleo): example aleo instruction programs
* [src/client/](./src/client/): CLI program to interact with the VM and the blockchain (e.g. create an account, deploy and execute programs)
* [src/snarkvm_abci/](./src/snarkvm_abci): Implements the [Application Blockchain Interface](https://docs.tendermint.com/v0.34/introduction/what-is-tendermint.html#abci-overview) (ABCI) to connect the aleo specific logic (e.g. program proof verification) to the Tendermint Core infrastructure.
* [src/genesis.rs](./src/genesis.rs): Implements a helper program that generates JSON files that represent the genesis state for the ABCI app (which Tendermint requires).
* [src/lib/](./src/lib/): Shared library used by the CLI and the ABCI.


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

This will download and install tendermint if necessary. Alternatively, [these instructions](https://github.com/tendermint/tendermint/blob/main/docs/introduction/install.md) can be used. This will also generate the genesis file for the starting state of the blockchain.
Be sure to checkout version `0.34.x` as current rust abci implementation does not support `0.35.0` (`0.34` is the latest public release).

At this point, both terminals should start to exchange messages and `Commited height` should start to grow.

## Sending deploys and executions to the blockchain

On another terminal, compile the command-line client:

    make cli

Create an aleo account:

    bin/aleo account new

This will generate an account address and the credentials necessary to generate execution proofs, located by default on `~/.aleo/account.json`. This path can be overridden by setting the env var `ALEO_HOME`. Now run the client app to deploy an aleo program:

```shell
bin/aleo program deploy aleo/hello.aleo
```

That should take some time to create the deployment transaction and send it to the Tendermint network. In the client terminal you should see a JSON response similar to the following one:

```
{
  "Deployment": {
    "deployment": {
      "edition": 0,
      "program": "program hello.aleo;\n\nfunction hello:\n    input r0 as u32.public;\n    input r1 as u32.private;\n    add r0 r1 into r2;\n    output r2 as u32.public;\n",
      "verifying_keys": {
        "hello": [
          "verifier1qqqpqqqqqqqqqqqqs57qqqqqqqqqppfuqqqqqqqqqz32sqqqqqqqqqqrmuqqqqqqqqqzeyqqqqqqqqqqpsqqqqqqqqqqpw8vnp60tq25uf470rmxydj0xjwgwnrkmsvr9s02se3dj4kpawjjxhrx42x28zfa5ayu7ypjvj5zqrsnx8kalm6fh4498er5me7jhdd29l5fplnc4mtawyfjlfldjvzz8q3p..."
        ]
      }
    },
    "id": "7999aa60-ad74-45d2-aa57-f75cb01ac653"
  }
}
```
 You should also see the transaction being received in the ABCI terminal with some message like:

```
2022-12-06T19:13:22.456235Z  INFO ThreadId(06) Check Tx
2022-12-06T19:13:22.691738Z  INFO ThreadId(06) Transaction Deployment(7999aa60-ad74-45d2-aa57-f75cb01ac653,hello.aleo) verification successful
2022-12-06T19:13:22.695360Z  INFO ThreadId(07) Committing height 2
2022-12-06T19:13:22.696073Z  INFO ThreadId(06) Check Tx
2022-12-06T19:13:22.876382Z  INFO ThreadId(06) Transaction Deployment(7999aa60-ad74-45d2-aa57-f75cb01ac653,hello.aleo) verification successful
2022-12-06T19:13:23.857425Z  INFO ThreadId(07) Deliver Tx
2022-12-06T19:13:24.066973Z  INFO ThreadId(07) Transaction Deployment(7999aa60-ad74-45d2-aa57-f75cb01ac653,hello.aleo) verification successful
```

This means that the program was deployed succesfully and stored on the blockchain. If you tried to redeploy it, you would see the following error message:

```shell
{
  "error": "Error executing transaction 1: Could not verify transaction: Program already exists"
}
```

Notice that transaction JSON includes an `id` field which you can retrieve by running `bin/aleo get {transaction_id}`. It will retrieve the same JSON from the blockchain if you run it.

Finally to execute a program (locally) and send the execution transaction (with its proof) run in client terminal:

```shell
bin/aleo program execute aleo/hello.aleo hello 1u32 1u32
```

The command above will run the program and send the execution to the blockchain:

```
{
  "Execution": {
    "id": "15499c5b-b0b7-46eb-87da-366f38cc485c",
    "transitions": [
      {
        "fee": 0,
        "function": "hello",
        "id": "as1f0y0080cuvfnz5rwdm4yk90ny7pghqfxa42kg37jv403ccpvzqfquftums",
        "inputs": [
          {
            "id": "525052707127338880162170750843371169229438004982085783899427530567050481836field",
            "type": "public",
            "value": "1u32"
          },
          {
            "id": "4717481540194483200902154787515383856547973389698044767020383141679731034593field",
            "type": "private",
            "value": "ciphertext1qyq87903rnzu2va44zq985y0cqltr4uhkftwnqlvtsdfzwxd8cr5syqcnmfum"
          }
        ],
        "outputs": [
          {
            "id": "6029120002360365362314373657798705508848679817835900143138007626549923986692field",
            "type": "public",
            "value": "2u32"
          }
        ],
        "program": "hello.aleo",
        "proof": "proof1qqqqzqqqqqqqqqqqm0t60gjwk8k9.....",
        "tcm": "2845314139032602383675815349790318009604197052650910442608790130653723629069field",
        "tpk": "5997679097582981876126538929314854897856654620144767419232361193165432492135group"
      }
    ]
  }
}
```
Again, we see the transaction (of type `Execution`) and its ID, which means the execution was sent out to the network sucesfully.

After each execution, tendermint node may be left in an invalid state. If that's the case run:

```shell
make reset
```

to restore the initial state. Notice that this will delete the databases that store the programs and the records, so previous deployments and account records will be deleted.

### Debugging the client/ABCI

By default, the CLI will output no more data than a JSON response. To enable verbose output, you can pass the `-v` flag to see logs up to the level that you have set on the env var `RUST_LOG`. The same applies for the ABCI.

### Setting the blockchain endpoint

By default, the CLI client sends every transaction to `http://127.0.0.1:26657`, which is the local port for the ABCI application. In order to override this, you can set the env var `BLOCKCHAIN_URL` or alternatively, you can pass `-url {blockchain_url}` in the commands.

### See available CLI parameters
In order to see all different commands and parameters that the CLI can take, you can run `bin/aleo --help`.

## Running tests

In order to run tests, make sure the ABCI and the Tendermint Node are currently (`make abci` and `make node` respectively) running locally, and run `make test`.

## Working with records

In order to work with records, there are some things to keep in mind. As an example, we can use the `aleo/token.aleo` program. Deploy the program by running `bin/aleo program deploy aleo/token.aleo` and then do:

```shell
program execute aleo/token.aleo mint 12u64 {address}
```

````
{
  "Execution": {
    "id": "13a6e12f-c1be-46ce-b88e-9f3d74c7f9f5",
    "transitions": [
      {
        "fee": 0,
        "function": "mint",
        "id": "as1cektpje7mvrpgx4hawwdgfdgtw3yssfka282mpxaka8wcwmmhuyq8r3tf8",
        "inputs": [
          {
            "id": "1903280603122117322278162275809747610855764299760927153918643421395521073298field",
            "type": "private",
            "value": "ciphertext1qyqfajapluev4msjwzzptsymq4l0wl329krz6jqspmxz22leek7ljzsnq8nuh"
          },
          {
            "id": "3522127154863327477692820168234451494875264297697436450178817817644445199141field",
            "type": "private",
            "value": "ciphertext1qgqfmm7w6xztayz66ua54f7se899gusalx7540tavay508gcxndsjq5us4ncksafshdl4kfcwdgana8sp6ttnaj27t7m4t4pkq7tpvw4qgz20wda"
          }
        ],
        "outputs": [
          {
            "checksum": "662823924611211467835113438946492963566923769790927177001971226066013541277field",
            "id": "7219462448732480752053359377653342974759175543229460809950854209290642964685field",
            "type": "record",
            "value": "record1qyqsqtve8kg9afk6vzva3cpar5jztamahh38l75v6fzjee0te72xdfs0qyqsp0yrkua473w430zkrdls9ndreg8ucg7swph8zref9hem6e7pmdg8qyqqvctdda6kuaprqqpqzqq78dw4y06ax8l0fs49txvf0n0azx7ue6guhld7c5ecxtxexujjzymnlltl8hac9cy0vr0d6xd6fc6gqhfn3znfa4vcz22jrtyqax2s2gkz2mv"
          }
        ],
        "program": "token.aleo",
        "proof": "proof1qqqqzqqqqqqqqqqq0t0lajnnm",
        "tcm": "3029182936031307181714700392868610826830807174425109386666072696640639657186field",
        "tpk": "3762006807277732897697753651971484341155898222134947549896413876819808587029group"
      }
    ]
  }
}
````

You can use your address from the account creation here. You can see the output contains a record, which you can use in further executions such as the function `transfer_amount` from the same aleo program by passing the value `record1qyqsqtve8kg9afk6vzva3cpar5jztamahh38l75v6fzjee0te72xdfs0qyqsp0yrkua473w430zkrdls9ndreg8ucg7swph8zref9hem6e7pmdg8qyqqvctdda6kuaprqqpqzqq78dw4y06ax8l0fs49txvf0n0azx7ue6guhld7c5ecxtxexujjzymnlltl8hac9cy0vr0d6xd6fc6gqhfn3znfa4vcz22jrtyqax2s2gkz2mv` as the parameter.

## Initialize validators

In order to initialize the necessary files that would be required on a testnet, you can run:

````
make VALIDATORS=3 testnet
````

This will create subdirectories in `/mytestnet/` for each of the validators (defaults to 4 if it's not passed as a parameter). This means that there are files for the private validator keys, account info and genesis state. This way the nodes are able to translate a tendermint validator address to an aleo account, which in turn are used to generate reward records.

## Running multiple nodes on docker compose

This requires having docker (with docker-compose) installed.

Then build the `snarkvm_abci` image:

```
make localnet-build-abci
```

And to start the test net run:

```
make localnet-start
```

Note that each node will require more than 2Gb to run so docker should be configured to use 10Gb or more in order to work with the default 4 nodes.

To modify the configuration you should edit `docker-compose.yml` file

The configuration mounts some volumes in the `testnet/node{_}/` directories, and in case the tendermint nodes state needs to be reset, just run:

```
make localnet-reset
```

or delete all the `node{_}` dirs to remove local `snarkvm_abci` data (it will require to download all the parameters on next run).

You will find an `account.json` file in each `testnet/node{_}/` directory, with the aleo credentials of the validators (usable to run commands with the credits of the validators). On a MacOS docker deploy, each of the 4 testnet nodes will be exposed on ports 26657, 26660, 26663, 26666 of localhost.

Thus, you can interact with the network from the host like this:

```
ALEO_HOME=testnet/node1/ bin/aleo --url http://127.0.0.1:26657 account balance
```

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
