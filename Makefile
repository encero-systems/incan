# Incan development commands: formatting, quality gates and docs.
#
# Nothing here runs Cargo or builds the compiler. Oven (workspaces/oven, DD-0008) builds and tests it; during dev.8
# the frozen dev.7 builder bakes the Oven and the compiler.

.DEFAULT_GOAL := help

INCAN ?= incan

# Workspace Rust sources rustfmt owns; fixture crates keep the shape their tests assert and vendored crates their upstream form.
RUST_SOURCES = git ls-files -z 'loaves/*.rs' ':!:loaves/**/fixtures/**' ':!:loaves/third_party/**'

# =============================================================================
# Help
# =============================================================================

.PHONY: help
help:  ## Display this help message
	@echo "\033[1mBuild:\033[0m"
	@grep -E '^.PHONY: .*?## build - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## build - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mCode Quality:\033[0m"
	@grep -E '^.PHONY: .*?## quality - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## quality - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mDocs:\033[0m"
	@grep -E '^.PHONY: .*?## docs - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## docs - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'

# =============================================================================
# Build
# =============================================================================

.PHONY: examples  ## build - Run examples and check declared frontend-only examples with installed incan
examples:
	@bash scripts/run_examples.sh

.PHONY: install-hooks  ## build - Point git at the repository's commit hooks
install-hooks:
	@git config core.hooksPath "$(CURDIR)/.githooks"
	@chmod +x "$(CURDIR)/.githooks/"* 2>/dev/null || true
	@echo "\033[32m✓ core.hooksPath -> $(CURDIR)/.githooks\033[0m"

# =============================================================================
# Code quality
# =============================================================================

.PHONY: fmt  ## quality - Format workspace Rust sources with nightly rustfmt
fmt:
	@$(RUST_SOURCES) | xargs -0 rustfmt +nightly --edition 2024 --config skip_children=true

.PHONY: fmt-check  ## quality - Check workspace Rust formatting without writing
fmt-check:
	@$(RUST_SOURCES) | xargs -0 rustfmt +nightly --edition 2024 --config skip_children=true --check

.PHONY: rustdoc-gate  ## quality - Require rustdoc on changed Rust functions/methods
rustdoc-gate:
	@echo "\033[1mChecking rustdoc coverage for changed Rust functions/methods...\033[0m"
	@$(INCAN) test scripts/test_check_changed_rustdocs.incn
	@$(INCAN) run scripts/check_changed_rustdocs.incn

.PHONY: rustdoc-gate-ci
rustdoc-gate-ci:
	@$(INCAN) run scripts/check_changed_rustdocs.incn

.PHONY: doc-paths  ## quality - Check repository paths named in contributor documentation
doc-paths:
	@$(INCAN) run scripts/check_doc_paths.incn

.PHONY: us-english-check  ## quality - Fail on UK spellings in tracked text (US English is the standard); us-english-fix rewrites them
us-english-check:
	@$(INCAN) run scripts/check_us_english.incn

.PHONY: us-english-fix  ## quality - Rewrite UK spellings as US English in place, prose and identifiers alike
us-english-fix:
	@$(INCAN) run scripts/check_us_english.incn -- --fix

.PHONY: reference-contract-check  ## quality - Fail when a changed docs-site reference page states more than its contract
reference-contract-check:
	@$(INCAN) run scripts/check_reference_contract.incn

.PHONY: agents-doc-sync  ## quality - Check AGENTS.md's skill table matches .agents/skills/
agents-doc-sync:
	@echo "\033[1mChecking AGENTS.md skill table against .agents/skills/...\033[0m"
	@$(INCAN) run scripts/check_agents_doc_sync.incn

.PHONY: agents-doc-sync-ci
agents-doc-sync-ci:
	@$(INCAN) run scripts/check_agents_doc_sync.incn

.PHONY: pre-commit-fast  ## quality - Fast local gate: formatting, spelling, rustdoc and the skill table (doc paths return with the dev.8 docs pass)
pre-commit-fast: fmt-check us-english-check rustdoc-gate agents-doc-sync

# =============================================================================
# Docs
# =============================================================================

.PHONY: docs  ## docs - Build and serve the documentation site locally
docs:
	@$(MAKE) -C workspaces/docs-site docs

.PHONY: docs-install  ## docs - Install docs site dependencies (MkDocs + Material)
docs-install:
	@$(MAKE) -C workspaces/docs-site docs-install

.PHONY: docs-check-components  ## docs - Validate Incapunk component and asset contracts
docs-check-components:
	@$(MAKE) -C workspaces/docs-site docs-check-components

.PHONY: docs-check-learning  ## docs - Validate learning routes and canonical tutorial contracts
docs-check-learning:
	@$(MAKE) -C workspaces/docs-site docs-check-learning

.PHONY: docs-build  ## docs - Build docs site (MkDocs strict)
docs-build:
	@$(MAKE) -C workspaces/docs-site docs-build

.PHONY: docs-serve  ## docs - Serve docs site locally (MkDocs)
docs-serve:
	@$(MAKE) -C workspaces/docs-site docs-serve

.PHONY: docs-lint  ## docs - Lint markdown docs (markdownlint-cli2 via npx)
docs-lint:
	@$(MAKE) -C workspaces/docs-site docs-lint
