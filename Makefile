.PHONY: tendermint reset abci build cli genesis tendermint_config testnet tendermint_install

OS := $(shell uname | tr '[:upper:]' '[:lower:]')

ifeq ($(shell uname -p), arm)
ARCH=arm64
else
ARCH=amd64
endif

TENDERMINT_HOME=~/.tendermint/

# Build the client program and put it in bin/aleo
cli:
	mkdir -p bin && cargo build --release && cp target/release/client bin/aleo

# Installs tendermint for current OS and puts it in bin/
bin/tendermint:
	make tendermint_install
	mv tendermint-install/tendermint bin/ && rm -rf tendermint-install

# Internal phony target to install tendermint for an arbitrary OS
tendermint_install:
	mkdir -p tendermint-install bin && cd tendermint-install &&\
	wget https://github.com/tendermint/tendermint/releases/download/v0.34.22/tendermint_0.34.22_$(OS)_$(ARCH).tar.gz &&\
	tar -xzvf tendermint_0.34.22_$(OS)_$(ARCH).tar.gz

# initialize tendermint and write a genesis file for a local testnet.
genesis: bin/tendermint cli
	test -f $(TENDERMINT_HOME)/account.json || ALEO_HOME=$(TENDERMINT_HOME) bin/aleo account new
	bin/tendermint init
	cargo run --bin genesis --release -- $(TENDERMINT_HOME)

# Run a tendermint node, installing it if necessary
node: genesis tendermint_config
	bin/tendermint node --consensus.create_empty_blocks_interval="8s"

# Override a tendermint node's default configuration. NOTE: we should do something more declarative if we need to update more settings.
tendermint_config:
	sed -i.bak 's/max_body_bytes = 1000000/max_body_bytes = 12000000/g' $(TENDERMINT_HOME)/config/config.toml
	sed -i.bak 's/max_tx_bytes = 1048576/max_tx_bytes = 10485770/g' $(TENDERMINT_HOME)/config/config.toml
	sed -i.bak 's#laddr = "tcp://127.0.0.1:26657"#laddr = "tcp://0.0.0.0:26657"#g' $(TENDERMINT_HOME)/config/config.toml

# Initialize the tendermint configuration for a testnet of the given amount of validators
testnet: VALIDATORS:=4
testnet: ADDRESS:=192.167.10.2
testnet: bin/tendermint cli
	rm -rf testnet/
	bin/tendermint testnet --v $(VALIDATORS) --o ./testnet --starting-ip-address $(ADDRESS)
	for node in testnet/*/ ; do \
	  ALEO_HOME=$$node bin/aleo account new ; \
          make tendermint_config TENDERMINT_HOME=$$node ; \
	done
	cargo run --bin genesis --release -- testnet/*

# remove the blockchain data
reset: bin/tendermint
	rm -rf ~/.tendermint
	rm -rf *.db/
	rm -f abci.*
	bin/tendermint unsafe_reset_all

# run the snarkvm tendermint application
abci:
	cargo run --release --bin snarkvm_abci

# run tests on release mode to ensure there is no extra printing to stdout
test:
	RUST_BACKTRACE=full cargo test --release -- --nocapture --test-threads=4

localnet-build-abci:
	docker build -t snarkvm_abci .
.PHONY: localnet-build-abci

# Run a 4-node testnet locally
localnet-start: localnet-stop testnet
	make tendermint_install OS=linux ARCH=amd64
	mv tendermint-install/tendermint testnet/ && rm -rf tendermint-install
	docker-compose up
.PHONY: localnet-start

# Stop testnet
localnet-stop:
	docker-compose down
.PHONY: localnet-stop

# Reset the testnet data
localnet-reset:
	rm -Rf testnet
.PHONY: localnet-reset
