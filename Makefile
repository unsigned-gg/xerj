# XERJ workspace Makefile
# Enforces host throttling (-j 2) and delegates Rust engine gates to engine/crates/xtask.

.DEFAULT_GOAL := help

JOBS ?= 2
CARGO_BUILD_JOBS ?= $(JOBS)
export JOBS CARGO_BUILD_JOBS

XTASK := cd engine && cargo run -q -j $(JOBS) -p xtask --

.PHONY: help check xwork hygiene fmt fmt-check clippy test doc-test deny typos machete ci install

help: ## Show this help message
	@awk 'BEGIN { FS = ":.*##"; printf "Usage: make \033[36m<target>\033[0m\n" } \
		/^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5); next } \
		/^[a-zA-Z_0-9-]+:.*##/ { printf "  \033[36m%-24s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

##@ Quality & Gates

check: hygiene fmt-check clippy test ## Run hygiene, formatting, clippy (-D warnings), and test gates (-j 2)

xwork: ## Run topological verification pipeline from projectfile.yaml
	@$(XTASK) xwork

hygiene: ## Validate toolchain, projectfile.yaml, stages.rs, and workflow timeouts
	@$(XTASK) toolchain-check
	@$(XTASK) projectfile-check
	@$(XTASK) stages-check
	@$(XTASK) workflow-timeout-check

fmt: ## Format Rust engine workspace sources
	@$(XTASK) fmt

fmt-check: ## Check Rust engine formatting without modifying files
	@$(XTASK) fmt-check

clippy: ## Run Rust clippy linter with warnings denied (-j 2)
	@$(XTASK) clippy

test: ## Run Rust workspace test suite (-j 2)
	@$(XTASK) test

doc-test: ## Run Rust documentation tests (-j 2)
	@$(XTASK) doc-test

deny: ## Audit Rust dependencies with cargo-deny
	@$(XTASK) deny

typos: ## Check repository spelling with typos
	@$(XTASK) typos

machete: ## Check unused Cargo dependencies with cargo-machete
	@$(XTASK) machete

ci: ## Execute full SPEC-0004 xtask CI gate sequence
	@$(XTASK) ci

##@ Build

install: ## Run CI gates and build release engine binaries (-j 2)
	@$(XTASK) install
