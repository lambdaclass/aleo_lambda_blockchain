.PHONY: tendermint reset abci build

OS := $(shell uname | tr '[:upper:]' '[:lower:]')

ifeq ($(shell uname -p), arm)
ARCH=arm64
else
ARCH=amd64
endif

# Build the client program and put it in bin/aleo
cli:
	mkdir -p bin && cargo build --release && cp target/release/client bin/aleo

# Installs tendermint on linux or mac.
bin/tendermint:
	mkdir -p tendermint-install bin && cd tendermint-install &&\
	wget https://github.com/tendermint/tendermint/releases/download/v0.34.22/tendermint_0.34.22_$(OS)_$(ARCH).tar.gz &&\
	tar -xzvf tendermint_0.34.22_$(OS)_$(ARCH).tar.gz &&\
	cd .. && mv tendermint-install/tendermint bin/ && rm -rf tendermint-install

# Run a tendermint node, installing it if necessary
node: bin/tendermint
	bin/tendermint init
	bin/tendermint node --consensus.create_empty_blocks_interval="8s"

# remove the blockchain data
reset: bin/tendermint
	bin/tendermint unsafe_reset_all

# run the snarkvm tendermint application
abci:
	cargo run --release --bin snarkvm_abci
