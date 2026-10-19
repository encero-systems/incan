# Incan development commands.
#
# Nothing here runs Cargo. The compiler builds and its tests run through the Oven (workspaces/compiler-bootstrap) with a
# stage-zero compiler, from loaf.toml manifests and incan.pub Loaves; scripts/cargo-guard fails any Cargo invocation.
#
# Native inputs: the build reads the incan.pub index checkout and the source archive directory from
# INCAN_SDK_NATIVE_INDEX and INCAN_SDK_NATIVE_BLOBS; the compiler-suite graph is selected per host from the checkout.

.DEFAULT_GOAL := help

TARGET_DIR ?= $(CURDIR)/target
# Stage zero builds this checkout's compiler. During dev.8 that is the frozen dev.7 compiler.
INCAN_STAGE_ZERO ?= incan
INCAN_RUST_TOOLCHAIN ?= 1.98.0
INCAN_OVEN_STORE ?= $(TARGET_DIR)/compiler-development/store
INCAN_NATIVE_COMPILER_GRAPH ?= $(shell sh "$(CURDIR)/scripts/select_native_compiler_graph.sh" "$(CURDIR)")
# Directories whose executables own the C toolchain a recorded native-link fact names (the macOS Command Line Tools).
INCAN_OVEN_LINK_OWNERS ?= $(if $(filter Darwin,$(shell uname -s)),/Library/Developer/CommandLineTools,)
TEST_ROOT ?=
TEST_EXACT ?=
INCAN_TEST_ONE_REPORT ?=

DEV_ENV = INCAN_SDK_NATIVE_COMPILER_GRAPH="$(INCAN_NATIVE_COMPILER_GRAPH)" \
	INCAN_SOURCE_ROOT="$(CURDIR)" \
	INCAN_STDLIB="$(CURDIR)/loaves/stdlib" \
	INCAN_STDLIB_DIR="$(CURDIR)/loaves/stdlib" \
	INCAN_OVEN_LINK_OWNERS="$(INCAN_OVEN_LINK_OWNERS)"
COMPILER_DEVELOPMENT = $(DEV_ENV) sh "$(CURDIR)/scripts/compiler_development.sh" \
	"$(CURDIR)" "$(TARGET_DIR)" "$(INCAN_STAGE_ZERO)" "$(INCAN_RUST_TOOLCHAIN)" "$(INCAN_OVEN_STORE)"

# Workspace Rust sources rustfmt owns; fixture crates keep the shape their tests assert.
RUST_SOURCES = git ls-files -z 'loaves/*.rs' ':!:loaves/**/fixtures/**'

# =============================================================================
# Help
# =============================================================================

.PHONY: help
help:  ## Display this help message
	@echo "\033[1mBuild:\033[0m"
	@grep -E '^.PHONY: .*?## build - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## build - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mTesting:\033[0m"
	@grep -E '^.PHONY: .*?## test - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## test - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mCode Quality:\033[0m"
	@grep -E '^.PHONY: .*?## quality - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## quality - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mDocs:\033[0m"
	@grep -E '^.PHONY: .*?## docs - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## docs - "}; {printf "  \033[36m%-24s\033[0m %s\n", $$2, $$3}'

# =============================================================================
# Build
# =============================================================================

.PHONY: native-inputs-check
native-inputs-check:
	@test -n "$$INCAN_SDK_NATIVE_INDEX" -a -n "$$INCAN_SDK_NATIVE_BLOBS" || { \
		echo "set INCAN_SDK_NATIVE_INDEX (an incan.pub index checkout) and INCAN_SDK_NATIVE_BLOBS (source archives)" >&2; \
		exit 2; }

.PHONY: build  ## build - Build the compiler (debug) through the Oven with the stage-zero compiler
build: native-inputs-check
	@echo "\033[1mBuilding the compiler through the Oven...\033[0m"
	@$(COMPILER_DEVELOPMENT) build "$(TARGET_DIR)/debug/incan"

.PHONY: build-quiet
build-quiet: native-inputs-check
	@$(COMPILER_DEVELOPMENT) quiet "$(TARGET_DIR)/debug/incan"

.PHONY: install-hooks  ## build - Point git at the repository's commit hooks
install-hooks:
	@git config core.hooksPath "$(CURDIR)/.githooks"
	@chmod +x "$(CURDIR)/.githooks/"* 2>/dev/null || true
	@echo "\033[32m✓ core.hooksPath -> $(CURDIR)/.githooks\033[0m"

# =============================================================================
# Testing
# =============================================================================

.PHONY: test-one  ## test - Run one authored test root, or one exact test in it, through the Oven
test-one: native-inputs-check
	@test -n "$(TEST_ROOT)" || { echo "usage: make test-one TEST_ROOT=loaves/compiler/incan_frontend/src/lib.rs [TEST_EXACT=module::test_name]" >&2; exit 2; }
	@echo "\033[1mRunning $(TEST_ROOT)$(if $(TEST_EXACT), ($(TEST_EXACT)),) through the Oven...\033[0m"
	@$(COMPILER_DEVELOPMENT) test "$(TEST_ROOT)" "$(TEST_EXACT)" "$(INCAN_TEST_ONE_REPORT)"

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
	@cd scripts && python3 -m unittest -q test_check_changed_rustdocs
	@python3 scripts/check_changed_rustdocs.py

.PHONY: rustdoc-gate-ci
rustdoc-gate-ci:
	@python3 scripts/check_changed_rustdocs.py

.PHONY: doc-paths  ## quality - Check repository paths named in contributor documentation
doc-paths:
	@python3 scripts/check_doc_paths.py

.PHONY: us-english-check  ## quality - Fail on UK spellings in tracked text (US English is the standard); us-english-fix rewrites them
us-english-check:
	@python3 scripts/check_us_english.py

.PHONY: us-english-fix  ## quality - Rewrite UK spellings as US English in place, prose and identifiers alike
us-english-fix:
	@python3 scripts/check_us_english.py --fix

.PHONY: reference-contract-check  ## quality - Fail when a changed docs-site reference page states more than its contract
reference-contract-check:
	@python3 scripts/check_reference_contract.py

.PHONY: agents-doc-sync  ## quality - Check AGENTS.md's skill table matches .agents/skills/
agents-doc-sync:
	@echo "\033[1mChecking AGENTS.md skill table against .agents/skills/...\033[0m"
	@python3 scripts/check_agents_doc_sync.py

.PHONY: agents-doc-sync-ci
agents-doc-sync-ci:
	@python3 scripts/check_agents_doc_sync.py

.PHONY: behavior-roots  ## quality - Regenerate the behavior roots (one libtest case per behavior fixture) from their areas
behavior-roots:
	@INCAN_WRITE_BEHAVIOR_ROOTS=1 INCAN_INTERNAL_TEST_SOURCE_ROOT="$(CURDIR)" $(MAKE) test-one \
		TEST_ROOT=loaves/compiler/incan_test_support/src/lib.rs \
		TEST_EXACT=behavior_roots::tests::behavior_roots_match_their_areas

.PHONY: pre-commit-fast  ## quality - Fast local gate: formatting, spelling, rustdoc, doc paths and the skill table
pre-commit-fast: fmt-check us-english-check rustdoc-gate doc-paths agents-doc-sync

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
