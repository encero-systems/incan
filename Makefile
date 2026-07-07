# Incan Programming Language - Makefile
# =====================================

NEXTEST := $(shell command -v cargo-nextest 2>/dev/null)
TEST_VERBOSE ?= 0

ifeq ($(strip $(NEXTEST)),)
ifeq ($(TEST_VERBOSE),1)
TEST_CMD = cargo test --all --features lsp --verbose
else
TEST_CMD = cargo test --all --features lsp
endif
else
ifeq ($(TEST_VERBOSE),1)
TEST_CMD = cargo nextest run --all --features lsp --status-level all
else
TEST_CMD = cargo nextest run --all --features lsp --status-level slow --final-status-level slow
endif
endif

# After `make build` / `make build-fast`, symlink ~/.cargo/bin/incan → target/debug/incan so `incan` on PATH (IDE run,
# other repos) matches this checkout. When `incan-lsp` was built (`make build` uses --features lsp), also symlink
# ~/.cargo/bin/incan-lsp so the editor LSP matches without `cargo install`. Off when CI is set; opt out with
# INCAN_SKIP_CARGO_BIN_LINK=1.
ifneq ($(CI),)
INCAN_LINK_CARGO_BIN ?= 0
else
INCAN_LINK_CARGO_BIN ?= 1
endif

.PHONY: help
help: build-quiet  ## Display this help message
	@INCAN_NO_BANNER=1 ./target/debug/incan --version
	@echo ""
	@echo "\033[1mBuild:\033[0m"
	@grep -E '^.PHONY: .*?## build - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## build - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mCode Quality:\033[0m"
	@grep -E '^.PHONY: .*?## quality - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## quality - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mTesting:\033[0m"
	@grep -E '^.PHONY: .*?## test - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## test - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mDocs:\033[0m"
	@grep -E '^.PHONY: .*?## docs - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## docs - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mTooling:\033[0m"
	@grep -E '^.PHONY: .*?## tool - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## tool - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
	@echo ""
	@echo "\033[1mMiscellaneous:\033[0m"
	@grep -E '^.PHONY: .*?## misc - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## misc - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
	@echo ""

# =============================================================================
# Build
# =============================================================================

.PHONY: _incan_link_debug_to_cargo_bin
_incan_link_debug_to_cargo_bin:
	@if [ "$(INCAN_LINK_CARGO_BIN)" != "1" ] || [ "$(INCAN_SKIP_CARGO_BIN_LINK)" = "1" ]; then exit 0; fi
	@if [ ! -f "$(CURDIR)/target/debug/incan" ]; then echo "incan: expected $(CURDIR)/target/debug/incan after build"; exit 1; fi
	@mkdir -p "$(HOME)/.cargo/bin"
	@ln -sf "$(CURDIR)/target/debug/incan" "$(HOME)/.cargo/bin/incan"
	@echo "\033[32m✓ Linked ~/.cargo/bin/incan -> $(CURDIR)/target/debug/incan\033[0m"
	@if [ -f "$(CURDIR)/target/debug/incan-lsp" ]; then \
		ln -sf "$(CURDIR)/target/debug/incan-lsp" "$(HOME)/.cargo/bin/incan-lsp"; \
		echo "\033[32m✓ Linked ~/.cargo/bin/incan-lsp -> $(CURDIR)/target/debug/incan-lsp\033[0m"; \
	fi

.PHONY: build  ## build - Debug build (compiler + LSP); links ~/.cargo/bin/incan + incan-lsp locally
build:
	@echo "\033[1mBuilding (debug)...\033[0m"
	@cargo build --features lsp
	@$(MAKE) _incan_link_debug_to_cargo_bin

.PHONY: build-fast  ## build - Debug build (compiler only); links ~/.cargo/bin/incan locally
build-fast:
	@echo "\033[1mBuilding compiler only (debug)...\033[0m"
	@cargo build
	@$(MAKE) _incan_link_debug_to_cargo_bin

.PHONY: build-quiet
build-quiet:
	@cargo build --quiet 2>/dev/null || cargo build --quiet

.PHONY: release  ## build - Release build (optimized)
release:
	@echo "\033[1mBuilding (release)...\033[0m"
	@cargo build --release

.PHONY: install  ## build - Install to ~/.cargo/bin
install:
	@echo "\033[1mInstalling incan...\033[0m"
	@cargo install --path .
	@echo "\033[32m✓ Installed to ~/.cargo/bin/incan\033[0m"

# =============================================================================
# Code Quality
# =============================================================================

.PHONY: fmt  ## quality - Format Rust code
fmt:
	@echo "\033[1mFormatting code...\033[0m"
	@cargo +nightly fmt --version >/dev/null 2>&1 || ( \
		echo "\033[33m⚠ nightly rustfmt is required for this project formatting config.\033[0m"; \
		echo "\033[33m  Install it via: rustup toolchain install nightly --component rustfmt\033[0m"; \
		exit 1; \
	)
	@cargo +nightly fmt --all
	@echo "\033[32m✓ Code formatted\033[0m"

.PHONY: fmt-check  ## quality - Check formatting without changes
fmt-check:
	@echo "\033[1mChecking formatting...\033[0m"
	@cargo +nightly fmt --version >/dev/null 2>&1 || ( \
		echo "\033[33m⚠ nightly rustfmt is required for this project formatting config.\033[0m"; \
		echo "\033[33m  Install it via: rustup toolchain install nightly --component rustfmt\033[0m"; \
		exit 1; \
	)
	@cargo +nightly fmt --all -- --check

.PHONY: lint  ## quality - Run clippy linter
lint:
	@echo "\033[1mRunning clippy...\033[0m"
	@cargo clippy --all-targets --all-features -- -D warnings

.PHONY: lint-fast  ## quality - Run faster clippy profile (workspace + all-features)
lint-fast:
	@echo "\033[1mRunning clippy (fast profile)...\033[0m"
	@cargo clippy --workspace --all-features -- -D warnings

.PHONY: fmt-check-ci
fmt-check-ci:
	@cargo +nightly fmt --version >/dev/null 2>&1 || ( \
		echo "\033[33m⚠ nightly rustfmt is required for this project formatting config.\033[0m"; \
		echo "\033[33m  Install it via: rustup toolchain install nightly --component rustfmt\033[0m"; \
		exit 1; \
	)
	@cargo +nightly fmt --all -- --check

.PHONY: lint-fast-ci
lint-fast-ci:
	@cargo clippy --workspace --all-features -- -D warnings

.PHONY: rustdoc-gate  ## quality - Require rustdoc on changed Rust functions/methods
rustdoc-gate:
	@echo "\033[1mChecking rustdoc coverage for changed Rust functions/methods...\033[0m"
	@python3 scripts/check_changed_rustdocs.py

.PHONY: rustdoc-gate-ci
rustdoc-gate-ci:
	@python3 scripts/check_changed_rustdocs.py

.PHONY: cargo-deny  ## quality - Run cargo-deny policy checks
cargo-deny:
	@echo "\033[1mRunning cargo-deny...\033[0m"
	@cargo deny check

.PHONY: cargo-deny-ci
cargo-deny-ci:
	@cargo deny check

.PHONY: check-fast-ci
check-fast-ci:
	@cargo check --workspace --all-features

.PHONY: check  ## quality - Run all quality checks (fmt + lint)
check: fmt-check lint
	@echo "\033[32m✓ All checks passed\033[0m"

.PHONY: udeps  ## quality - Check for unused dependencies (requires nightly + cargo-udeps)
udeps:
	@echo "\033[1mChecking for unused dependencies...\033[0m"
	@cargo +nightly udeps --quiet 2>/dev/null || echo "\033[33m⚠ cargo-udeps skipped (requires cargo-udeps + nightly rustc 1.85+. Run `rustup update nightly` if needed.)\033[0m"

.PHONY: pre-commit-fast  ## quality - Fast local gate: fmt-check + cargo check with phase timing
pre-commit-fast:
	@set -e; \
	start=$$(date +%s); \
	printf "\033[1mChecking formatting...\033[0m "; \
	$(MAKE) -s fmt-check-ci; \
	echo "\033[32mDONE\033[0m"; \
	t1=$$(date +%s); \
	printf "\033[1mChecking rustdoc coverage...\033[0m "; \
	$(MAKE) -s rustdoc-gate-ci; \
	echo "\033[32mDONE\033[0m"; \
	t2=$$(date +%s); \
	echo "\033[1mRunning cargo check (fast gate)...\033[0m"; \
	$(MAKE) -s check-fast-ci; \
	echo "\033[32mDONE\033[0m"; \
	t3=$$(date +%s); \
	echo "\033[32m✓ Pre-commit checks passed (fast)\033[0m"; \
	echo "\033[36mPhase timing:\033[0m fmt-check=$$((t1-start))s, rustdoc=$$((t2-t1))s, check=$$((t3-t2))s, total=$$((t3-start))s"

.PHONY: pre-commit-full-gate  ## quality - Full local gate core: fmt-check + tests + clippy + cargo-deny with phase timing
pre-commit-full-gate:
	@set -e; \
	start=$$(date +%s); \
	printf "\033[1mChecking formatting...\033[0m "; \
	$(MAKE) -s fmt-check-ci; \
	echo "\033[32mDONE\033[0m"; \
	t1=$$(date +%s); \
	printf "\033[1mChecking rustdoc coverage...\033[0m "; \
	$(MAKE) -s rustdoc-gate-ci; \
	echo "\033[32mDONE\033[0m"; \
	t2=$$(date +%s); \
	echo "\033[1mRunning tests...\033[0m"; \
	$(TEST_CMD); \
	echo "\033[32mDONE\033[0m"; \
	t3=$$(date +%s); \
	echo "\033[1mRunning clippy...\033[0m"; \
	$(MAKE) -s lint-fast-ci; \
	echo "\033[32mDONE\033[0m"; \
	t4=$$(date +%s); \
	echo "\033[1mRunning cargo-deny...\033[0m"; \
	$(MAKE) -s cargo-deny-ci; \
	echo "\033[32mDONE\033[0m"; \
	t5=$$(date +%s); \
	echo "\033[32m✓ Pre-commit checks passed (full)\033[0m"; \
	echo "\033[36mPhase timing:\033[0m fmt-check=$$((t1-start))s, rustdoc=$$((t2-t1))s, tests=$$((t3-t2))s, lint=$$((t4-t3))s, deny=$$((t5-t4))s, total=$$((t5-start))s"

.PHONY: pre-commit  ## quality - Full local gate: pre-commit-full-gate + smoke-test-fast
pre-commit:
	@echo "\033[1mRunning pre-commit (full local gate)...\033[0m"
	@$(MAKE) pre-commit-full-gate
	@$(MAKE) smoke-test-fast
	@echo "\033[32m✓ Pre-commit passed\033[0m"

.PHONY: ci-full  ## quality - Full CI check: fmt, lint, udeps, test, and release build
ci-full: fmt lint udeps
	@echo "\033[1mRunning tests...\033[0m"
	@$(TEST_CMD)
	@echo "\033[1mBuilding release...\033[0m"
	@cargo build --release --quiet
	@echo "\033[32m✓ Full CI checks passed\033[0m"

# =============================================================================
# Testing
# =============================================================================

.PHONY: test  ## test - Run all tests
test:
	@echo "\033[1mRunning tests...\033[0m"
	@$(TEST_CMD)

.PHONY: test-rust-inspect  ## test - Run focused rust-inspect regression tests
test-rust-inspect:
	@echo "\033[1mRunning rust-inspect focused tests...\033[0m"
	@cargo test --lib --features rust_inspect frontend::typechecker::tests::test_rust_inspect_unavailable_stays_permissive_for_method_calls
	@cargo test --lib --features rust_inspect frontend::typechecker::tests::test_rusttype_return_coercion_recorded_for_generic_newtype_method_call

.PHONY: generated-rust-audit-gate  ## test - Run deterministic generated Rust audit helper checks
generated-rust-audit-gate:
	@echo "\033[1mRunning generated Rust audit helper checks...\033[0m"
	@cargo test --test generated_rust_audit_tests
	@python3 scripts/generated_rust_audit.py --format json --fail-on-missing \
		--artifact program-main=tests/fixtures/generated_rust_audit/main.rs \
		--artifact stdlib-copy=tests/fixtures/generated_rust_audit/nested >/dev/null
	@echo "\033[32m✓ Generated Rust audit helper checks passed\033[0m"

.PHONY: examples  ## test - Smoke test examples (check all, run entrypoints with timeout)
examples: release
	@echo "\033[1mRunning examples...\033[0m"
	@INCAN_NO_BANNER=1 INCAN_EXAMPLES_TIMEOUT=$${INCAN_EXAMPLES_TIMEOUT:-30} bash scripts/run_examples.sh

.PHONY: benchmarks  ## test - Run benchmark suite (requires hyperfine)
benchmarks: release
	@echo "\033[1mRunning benchmarks...\033[0m"
	@INCAN_NO_BANNER=1 bash workspaces/benchmarks/run_all.sh

.PHONY: benchmarks-rust  ## test - Run benchmarks (Incan vs Rust only; no Python)
benchmarks-rust: release
	@echo "\033[1mRunning benchmarks (Incan vs Rust; no Python)...\033[0m"
	@INCAN_NO_BANNER=1 SKIP_PYTHON=true bash workspaces/benchmarks/run_all.sh

.PHONY: benchmarks-incan  ## test - Smoke-check benchmark .incn files (build only; no Python/Rust runs)
benchmarks-incan: release
	@echo "\033[1mChecking benchmarks (Incan build only)...\033[0m"
	@INCAN_NO_BANNER=1 bash workspaces/benchmarks/check_incan.sh

.PHONY: smoke-test-release
smoke-test-release:
	@$(MAKE) release

.PHONY: smoke-test-require-release-bin
smoke-test-require-release-bin:
	@if [ ! -x "$(CURDIR)/target/release/incan" ]; then \
		echo "incan: expected $(CURDIR)/target/release/incan; run make smoke-test-release first"; \
		exit 1; \
	fi

.PHONY: smoke-test-canary
smoke-test-canary:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mRunning Incan assertion canary...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan test tests/fixtures/test_assert_canary.incn
	@echo "\033[32m✓ Incan assertion canary passed\033[0m"

.PHONY: smoke-test-web-example
smoke-test-web-example:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mBuilding web example (build-only)...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan build examples/web/hello_web.incn
	@echo "\033[32m✓ Web example built\033[0m"

.PHONY: smoke-test-nested-project-example
smoke-test-nested-project-example:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mBuilding nested_project example (build-only)...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan build examples/advanced/nested_project/src/main.incn
	@echo "\033[32m✓ Nested project example built\033[0m"

.PHONY: smoke-test-examples
smoke-test-examples:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mRunning examples...\033[0m"
	@INCAN_NO_BANNER=1 INCAN_EXAMPLES_TIMEOUT=$${INCAN_EXAMPLES_TIMEOUT:-30} bash scripts/run_examples.sh

.PHONY: smoke-test-benchmarks-incan
smoke-test-benchmarks-incan:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mChecking benchmarks (Incan build only)...\033[0m"
	@INCAN_NO_BANNER=1 bash workspaces/benchmarks/check_incan.sh

.PHONY: smoke-test-core
smoke-test-core:
	@$(MAKE) smoke-test-release
	@$(MAKE) smoke-test-canary
	@$(MAKE) smoke-test-web-example
	@$(MAKE) smoke-test-nested-project-example
	@$(MAKE) smoke-test-examples
	@$(MAKE) smoke-test-benchmarks-incan

.PHONY: smoke-test  ## test - Full smoke test: tests + release canary + examples + benchmarks-incan
smoke-test:
	@echo "\033[1mRunning smoke-test...\033[0m"
	@$(MAKE) test
	@$(MAKE) smoke-test-core
	@echo "\033[32m✓ Smoke-test passed\033[0m"

.PHONY: smoke-test-fast  ## test - Fast smoke test for after pre-commit (skips duplicate unit test suite)
smoke-test-fast:
	@echo "\033[1mRunning smoke-test-fast...\033[0m"
	@$(MAKE) smoke-test-core
	@echo "\033[32m✓ Smoke-test-fast passed\033[0m"

.PHONY: verify  ## test - Compatibility alias to pre-commit
verify:
	@$(MAKE) pre-commit

.PHONY: test-verbose  ## test - Run tests with output
test-verbose:
	@echo "\033[1mRunning tests (verbose)...\033[0m"
	@cargo nextest run --all --no-capture 2>/dev/null || cargo test --all -- --nocapture

.PHONY: test-diagnose  ## test - Run tests with live output (use if pre-commit hangs to find culprit)
test-diagnose:
	@echo "\033[1mRunning tests with live output (Ctrl+C when stuck to see last test)...\033[0m"
	@cargo test --all --no-fail-fast -- --nocapture --test-threads=1

.PHONY: test-timings  ## test - Generate cargo compile-timing report (target/cargo-timings)
test-timings:
	@echo "\033[1mGenerating cargo timing report for test build...\033[0m"
	@cargo test --all --no-run --timings
	@echo "\033[32m✓ Timing report generated in target/cargo-timings\033[0m"

.PHONY: test-one  ## test - Run specific test (TEST=name)
test-one:
ifdef TEST
	@echo "\033[1mRunning test: $(TEST)\033[0m"
	@cargo nextest run -E "test($(TEST))" --no-capture 2>/dev/null || cargo test $(TEST) -- --nocapture
else
	@echo "Usage: \033[36mmake test-one TEST=test_name\033[0m"
	@echo "Example: make test-one TEST=test_run_c_import_this"
endif

# =============================================================================
# Tooling
# =============================================================================

.PHONY: lsp  ## tool - Build the LSP server
lsp:
	@echo "\033[1mBuilding LSP server...\033[0m"
	@cargo build --release --features lsp --bin incan-lsp
	@echo "\033[32m✓ LSP server built: target/release/incan-lsp\033[0m"

.PHONY: install-lsp  ## tool - Install incan-lsp to ~/.cargo/bin
install-lsp:
	@echo "\033[1mInstalling incan-lsp...\033[0m"
	@cargo install --path . --features lsp --bin incan-lsp --force
	@echo "\033[32m✓ Installed to ~/.cargo/bin/incan-lsp\033[0m"
	@echo "\033[33mℹ Ensure ~/.cargo/bin is on your PATH\033[0m"

.PHONY: test-incan-canary  ## test - End-to-end Incan test canary (assertion codegen)
test-incan-canary: release
	@echo "\033[1mRunning Incan assertion canary...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan test tests/fixtures/test_assert_canary.incn
	@echo "\033[32m✓ Incan assertion canary passed\033[0m"

.PHONY: examples-web-build  ## test - Build-only web example (no run)
examples-web-build: release
	@echo "\033[1mBuilding web example (build-only)...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan build examples/web/hello_web.incn
	@echo "\033[32m✓ Web example built\033[0m"

.PHONY: examples-nested-project-build  ## test - Build-only nested_project example (multi-module imports)
examples-nested-project-build: release
	@echo "\033[1mBuilding nested_project example (build-only)...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan build examples/advanced/nested_project/src/main.incn
	@echo "\033[32m✓ Nested project example built\033[0m"

.PHONY: vscode-package  ## tool - Package VS Code extension
vscode-package:
	@echo "\033[1mPackaging VS Code extension...\033[0m"
	@cd workspaces/ide/vscode && npm ci
	@cd workspaces/ide/vscode && npm run compile
	@cd workspaces/ide/vscode && npx @vscode/vsce package
	@echo "\033[32m✓ Extension packaged\033[0m"

.PHONY: toolchain-release-build  ## tool - Build toolchain release binaries (compiler + LSP)
toolchain-release-build:
	@echo "\033[1mBuilding toolchain release binaries...\033[0m"
	@cargo build --locked --release --features lsp --bin incan --bin incan-lsp
	@echo "\033[32m✓ toolchain release binaries built\033[0m"

.PHONY: toolchain-release-package  ## tool - Package local toolchain archive (TOOLCHAIN_DIST=/private/tmp/incan-local-test)
toolchain-release-package: toolchain-release-build
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh package

.PHONY: toolchain-release-assets  ## tool - Write local toolchain manifest/install assets
toolchain-release-assets:
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh assets

.PHONY: toolchain-release-smoke-direct  ## tool - Smoke local toolchain installer directly
toolchain-release-smoke-direct:
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh direct

.PHONY: toolchain-release-smoke-npm  ## tool - Smoke npm thin installer from local toolchain assets
toolchain-release-smoke-npm:
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh npm

.PHONY: toolchain-release-smoke-pip  ## tool - Smoke pip thin installer from local toolchain assets
toolchain-release-smoke-pip:
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh pip

.PHONY: toolchain-release-smoke-homebrew  ## tool - Render and syntax-check local Homebrew formula
toolchain-release-smoke-homebrew:
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh homebrew

.PHONY: toolchain-release-smoke  ## tool - Full local toolchain release smoke (direct + npm + pip + Homebrew syntax)
toolchain-release-smoke: toolchain-release-build
	@TOOLCHAIN_DIST="$${TOOLCHAIN_DIST:-/private/tmp/incan-local-test}" bash workspaces/release/toolchain/local_smoke.sh all

.PHONY: watch  ## tool - Watch for changes and rebuild (requires cargo-watch)
watch:
	@echo "\033[1mWatching for changes...\033[0m"
	@cargo watch -x build

# =============================================================================
# Miscellaneous
# =============================================================================

.PHONY: run  ## misc - Build and run (debug mode)
run:
	@cargo run --

.PHONY: zen  ## misc - Print the Zen of Incan
zen:
	@cargo build --release -q 2>/dev/null
	@INCAN_NO_BANNER=1 ./target/release/incan run -c "import this"

.PHONY: clean  ## misc - Clean build artifacts
clean:
	@echo "\033[1mCleaning...\033[0m"
	@cargo clean
	@rm -rf target/incan/
	@echo "\033[32m✓ Clean\033[0m"

.PHONY: docs  ## docs - Build and serve the documentation site locally
docs:
	@$(MAKE) -C workspaces/docs-site docs

.PHONY: docs-install  ## docs - Install docs site dependencies (MkDocs + Material)
docs-install:
	@$(MAKE) -C workspaces/docs-site docs-install

.PHONY: docs-build  ## docs - Build docs site (MkDocs strict)
docs-build:
	@$(MAKE) -C workspaces/docs-site docs-build

.PHONY: docs-serve  ## docs - Serve docs site locally (MkDocs)
docs-serve:
	@$(MAKE) -C workspaces/docs-site docs-serve

.PHONY: docs-lint  ## docs - Lint markdown docs (markdownlint-cli2 via npx)
docs-lint:
	@$(MAKE) -C workspaces/docs-site docs-lint

.PHONY: version  ## misc - Show version info
version:
	@echo "\033[1mIncan version:\033[0m"
	@cargo pkgid | cut -d# -f2
	@echo ""
	@echo "\033[1mRust version:\033[0m"
	@rustc --version
