.PHONY: install install_node install_jobc chaos kill_node node job clean build
	
REACTOR_GIT     ?= https://github.com/satyamjay-iitd/reactor.git
REACTOR_BRANCH  ?= master
NCTRL_CRATE     ?= reactor_nctrl
JCTRL_CRATE     ?= reactor_jctrl
TARGET_SO = ./target/release/lib_epaxos.so

install_node:
	cargo install \
	    --git $(REACTOR_GIT) \
	    --branch $(REACTOR_BRANCH) \
	    $(NCTRL_CRATE) \
	    --features jaeger

install_jobc:
	cargo install \
	    --git $(REACTOR_GIT) \
	    --branch $(REACTOR_BRANCH) \
	    $(JCTRL_CRATE)

# Needed eventually for local testing
# install_node: ../MTP/reactor/generic_nctrl
# 	cargo install --path ../MTP/reactor/generic_nctrl --features jaeger

# install_jobc: ../MTP/reactor/generic_jctrl
# 	cargo install --path ../MTP/reactor/generic_jctrl

build:
	cargo build --release --features verbose

kill_node:
	@echo "Killing process on port 3000 if any..."
	@lsof -ti :3000 | xargs --no-run-if-empty kill

node: kill_node install_node build
	reactor_nctrl --port 3000 target/release
	
job: install_jobc
	reactor_jctrl ./epaxos.toml

clippy:
	cargo clippy -- -D warnings

fmt:
	cargo fmt --all

test:
	cargo test

pre_commit: fmt clippy build test

clean:
	cargo uninstall reactor_nctrl || true
	cargo uninstall reactor_jctrl || true
	cargo clean	
