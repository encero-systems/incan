# The repository entry points: format, lint, contracts, examples, docs and the local gate.

.DEFAULT_GOAL := help

INCAN ?= incan
INCAN_LSP ?= incan-lsp
CONTRACT_SCRATCH ?= target/contracts
CONTRACT_JOBS ?= 12

# Workspace Rust sources rustfmt owns; fixture crates keep the shape their tests assert.
RUST_SOURCES = git ls-files -z 'loaves/*.rs' ':!:loaves/**/fixtures/**'
# Every Loaf whose `[rust]` table declares a library or procedural macro, wherever `[rust.source]` roots it;
# `make lint` checks each with clippy through `incan lint`.
RUST_FACETS = git ls-files 'loaves/**/loaf.toml' ':!:loaves/**/fixtures/**' | xargs awk 'FNR == 1 { rust = 0 } /^\[/ { rust = ($$0 == "[rust]") } rust && /^type *= *"(lib|proc-macro)"/ { name = FILENAME; sub(/\/loaf\.toml$$/, "", name); print name }'

.PHONY: help
help:
	@echo "make fmt | lint | test | examples | docs | pre-commit"

.PHONY: examples
examples:
	@INCAN_BIN="$${INCAN_BIN:-$(INCAN)}" $(INCAN) run scripts/run_examples.incn

.PHONY: test
test:
	@INCAN_LSP="$(INCAN_LSP)" $(INCAN) run workspaces/contracts/src/main.incn -- $(INCAN) workspaces/contracts/fixtures --scratch "$(CONTRACT_SCRATCH)" --jobs "$(CONTRACT_JOBS)" behavior check library interop testing project format graph

.PHONY: lint
lint:
	@status=0; for loaf in $$($(RUST_FACETS)); do $(INCAN) lint "$$loaf" || status=1; done; exit $$status

.PHONY: fmt
fmt:
	@$(RUST_SOURCES) | xargs -0 rustfmt +nightly --edition 2024 --config skip_children=true

.PHONY: docs
docs:
	@INCAN_BIN="$${INCAN_BIN:-$(INCAN)}" $(INCAN) run scripts/check_docs_examples.incn
	@$(MAKE) -C workspaces/docs-site docs-build

.PHONY: pre-commit
pre-commit:
	@$(RUST_SOURCES) | xargs -0 rustfmt +nightly --edition 2024 --config skip_children=true --check
	@$(INCAN) run scripts/check_us_english.incn
	@$(INCAN) test scripts/test_check_changed_rustdocs.incn
	@$(INCAN) run scripts/check_changed_rustdocs.incn
	@$(INCAN) run scripts/check_agents_doc_sync.incn
	@$(INCAN) run scripts/check_doc_paths.incn
	@$(INCAN) run scripts/check_reference_contract.incn
