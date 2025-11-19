.PHONY: install install_node install_jobc chaos kill_node node job clean build build_debug clippy fmt test pre_commit

SHELL  			:= /bin/bash
REACTOR_GIT     ?= https://github.com/satyamjay-iitd/reactor.git
REACTOR_BRANCH  ?= master
NCTRL_CRATE     ?= reactor_nctrl
JCTRL_CRATE     ?= reactor_jctrl
TARGET_SO = ./target/release/lib_epaxos.so
LOG             ?= run.txt
LOGFILE         ?= logs/$(LOG)

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

build_debug:
	cargo build --features verbose

kill_node:
	@echo "Killing process on port 3000 if any..."
	@lsof -ti :3000 | xargs --no-run-if-empty kill

node: kill_node install_node build
	reactor_nctrl --port 3000 target/release
	
node_debug: kill_node install_node build_debug
	reactor_nctrl --port 3000 target/debug | grep --line-buffered "epaxos::"

node_log:
	@mkdir -p logs
	@stdbuf -oL -eL make node | \
	    tee >(stdbuf -oL sed 's/\x1b\[[0-9;]*m//g' > "$(LOGFILE)")

node_log_debug:
	@mkdir -p logs
	@stdbuf -oL -eL make node_debug | \
	    tee >(stdbuf -oL sed 's/\x1b\[[0-9;]*m//g' > "$(LOGFILE)")

job: install_jobc
	reactor_jctrl ./epaxos.toml

clippy:
	cargo clippy -- -D warnings

fmt:
	cargo fmt --all

test:
	cargo test

pre_commit: fmt clippy build_debug build test

clean:
	cargo uninstall reactor_nctrl || true
	cargo uninstall reactor_jctrl || true
	cargo clean	
