.PHONY: tendermint reset abci build cli genesis tendermint_config testnet tendermint_install

OS := $(shell uname | tr '[:upper:]' '[:lower:]')

ifeq ($(shell uname -p), arm)
ARCH=arm64
else
ARCH=amd64
endif

TENDERMINT_HOME=~/.tendermint/
VM_FEATURE=snarkvm_backend

# Build the client program and put it in bin/aleo
cli:
	mkdir -p bin && cargo build --release --features $(VM_FEATURE) && cp target/release/client bin/aleo

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
	cargo run --bin genesis --release --features $(VM_FEATURE) -- $(TENDERMINT_HOME)

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
testnet: HOMEDIR:=testnet
testnet: bin/tendermint cli
	rm -rf $(HOMEDIR)/
	bin/tendermint testnet --v $(VALIDATORS) --o ./$(HOMEDIR) --starting-ip-address $(ADDRESS)
	for node in $(HOMEDIR)/*/ ; do \
	  ALEO_HOME=$$node bin/aleo account new ; \
          make tendermint_config TENDERMINT_HOME=$$node ; \
	done
	cargo run --bin genesis --release --features $(VM_FEATURE) -- $(HOMEDIR)/*

# Initialize the tendermint configuration for a localnet of the given amount of validators
localnet: VALIDATORS:=4
localnet: ADDRESS:=127.0.0.1
localnet: HOMEDIR:=localnet
localnet: bin/tendermint cli
	rm -rf $(HOMEDIR)/
	bin/tendermint testnet --v $(VALIDATORS) --o ./$(HOMEDIR) --starting-ip-address $(ADDRESS)
	for n in $$(seq 0 $$(($(VALIDATORS)-1))) ; do \
	    ALEO_HOME=$(HOMEDIR)/node$$n bin/aleo account new ; \
        make localnet_config TENDERMINT_HOME=$(HOMEDIR)/node$$n NODE=$$n VALIDATORS=$(VALIDATORS); \
		mkdir $(HOMEDIR)/node$$n/abci ; \
	done
	cargo run --bin genesis --release --features $(VM_FEATURE) -- $(HOMEDIR)/*
.PHONY: localnet

localnet_config:
	sed -i.bak 's/max_body_bytes = 1000000/max_body_bytes = 12000000/g' $(TENDERMINT_HOME)/config/config.toml
	sed -i.bak 's/max_tx_bytes = 1048576/max_tx_bytes = 10485770/g' $(TENDERMINT_HOME)/config/config.toml
	for n in $$(seq 0 $$(($(VALIDATORS)-1))) ; do \
	    eval "sed -i.bak 's/127.0.0.$$(($${n}+1)):26656/127.0.0.1:26$${n}56/g' $(TENDERMINT_HOME)/config/config.toml" ;\
	done
	sed -i.bak 's#laddr = "tcp://0.0.0.0:26656"#laddr = "tcp://0.0.0.0:26$(NODE)56"#g' $(TENDERMINT_HOME)/config/config.toml
	sed -i.bak 's#laddr = "tcp://127.0.0.1:26657"#laddr = "tcp://0.0.0.0:26$(NODE)57"#g' $(TENDERMINT_HOME)/config/config.toml
	sed -i.bak 's#proxy_app = "tcp://127.0.0.1:26658"#proxy_app = "tcp://127.0.0.1:26$(NODE)58"#g' $(TENDERMINT_HOME)/config/config.toml
.PHONY: localnet_config

# run both the abci application and the tendermint node
# assumes config for each node has been done previously
localnet_start: NODE:=0
localnet_start: HOMEDIR:=localnet
localnet_start:
	bin/tendermint node --home ./$(HOMEDIR)/node$(NODE) --consensus.create_empty_blocks_interval="90s" &
	cd ./$(HOMEDIR)/node$(NODE)/abci; cargo run --release --bin snarkvm_abci --features $(VM_FEATURE) -- --port 26$(NODE)58
.PHONY: localnet_start

# remove the blockchain data
reset: bin/tendermint
	rm -rf ~/.tendermint
	rm -rf *.db/
	rm -f abci.*
	bin/tendermint unsafe_reset_all

# run the snarkvm tendermint application
abci:
	cargo run --release --bin snarkvm_abci  --features $(VM_FEATURE)

# run tests on release mode (default VM backend) to ensure there is no extra printing to stdout
test: FEATURE:=snarkvm_abci
test:
	RUST_BACKTRACE=full cargo test --release --features $(VM_FEATURE) -- --nocapture --test-threads=2


dockernet-build-abci:
	docker build -t snarkvm_abci .
.PHONY: dockernet-build-abci

# Run a 4-node testnet locally
dockernet-start: HOMEDIR:=dockernet
dockernet-start: dockernet-stop 
	make testnet HOMEDIR=$(HOMEDIR)
	make tendermint_install OS=linux ARCH=amd64
	mv tendermint-install/tendermint $(HOMEDIR)/ && rm -rf tendermint-install
	docker-compose up
.PHONY: dockernet-start

# Stop testnet
dockernet-stop:
	docker-compose down
.PHONY: dockernet-stop

# Reset the testnet data
dockernet-reset: HOMEDIR:=dockernet
dockernet-reset:
	rm -Rf $(HOMEDIR)
.PHONY: dockernet-reset
