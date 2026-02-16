# Incan Programming Language - Makefile
# =====================================

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

.PHONY: build  ## build - Debug build (fast compile)
build:
	@echo "\033[1mBuilding (debug)...\033[0m"
	@cargo build

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

.PHONY: check  ## quality - Run all quality checks (fmt + lint)
check: fmt-check lint
	@echo "\033[32m✓ All checks passed\033[0m"

.PHONY: udeps  ## quality - Check for unused dependencies (requires nightly + cargo-udeps)
udeps:
	@echo "\033[1mChecking for unused dependencies...\033[0m"
	@cargo +nightly udeps --quiet 2>/dev/null || echo "\033[33m⚠ cargo-udeps skipped (requires cargo-udeps + nightly rustc 1.85+. Run `rustup update nightly` if needed.)\033[0m"

.PHONY: pre-commit  ## quality - Fast pre-commit: fmt, lint, and test (no release build, no udeps)
pre-commit: fmt lint
	@echo "\033[1mRunning tests...\033[0m"
	@cargo nextest run --all 2>/dev/null || cargo test --all --quiet
	@echo "\033[32m✓ Pre-commit checks passed\033[0m"

.PHONY: pre-commit-full  ## quality - Full CI check: fmt, lint, udeps, test, and release build
pre-commit-full: fmt lint udeps
	@echo "\033[1mRunning tests...\033[0m"
	@cargo nextest run --all 2>/dev/null || cargo test --all --quiet
	@echo "\033[1mBuilding release...\033[0m"
	@cargo build --release --quiet
	@echo "\033[32m✓ Full pre-commit checks passed\033[0m"

# =============================================================================
# Testing
# =============================================================================

.PHONY: test  ## test - Run all tests
test:
	@echo "\033[1mRunning tests...\033[0m"
	@cargo nextest run --all 2>/dev/null || cargo test --all --verbose

.PHONY: examples  ## test - Smoke test examples (check all, run entrypoints with timeout)
examples: release
	@echo "\033[1mRunning examples...\033[0m"
	@INCAN_NO_BANNER=1 INCAN_EXAMPLES_TIMEOUT=$${INCAN_EXAMPLES_TIMEOUT:-5} bash scripts/run_examples.sh

.PHONY: benchmarks  ## test - Run benchmark suite (requires hyperfine)
benchmarks: release
	@echo "\033[1mRunning benchmarks...\033[0m"
	@INCAN_NO_BANNER=1 bash benchmarks/run_all.sh

.PHONY: benchmarks-rust  ## test - Run benchmarks (Incan vs Rust only; no Python)
benchmarks-rust: release
	@echo "\033[1mRunning benchmarks (Incan vs Rust; no Python)...\033[0m"
	@INCAN_NO_BANNER=1 SKIP_PYTHON=true bash benchmarks/run_all.sh

.PHONY: benchmarks-incan  ## test - Smoke-check benchmark .incn files (build only; no Python/Rust runs)
benchmarks-incan: release
	@echo "\033[1mChecking benchmarks (Incan build only)...\033[0m"
	@INCAN_NO_BANNER=1 bash benchmarks/check_incan.sh

.PHONY: smoke-test-core
smoke-test-core:
	@$(MAKE) release
	@echo "\033[1mRunning Incan assertion canary...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan test tests/fixtures/test_assert_canary.incn
	@echo "\033[32m✓ Incan assertion canary passed\033[0m"
	@echo "\033[1mBuilding web example (build-only)...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan build examples/web/hello_web.incn
	@echo "\033[32m✓ Web example built\033[0m"
	@echo "\033[1mBuilding nested_project example (build-only)...\033[0m"
	@INCAN_NO_BANNER=1 ./target/release/incan build examples/advanced/nested_project/src/main.incn
	@echo "\033[32m✓ Nested project example built\033[0m"
	@echo "\033[1mRunning examples...\033[0m"
	@INCAN_NO_BANNER=1 INCAN_EXAMPLES_TIMEOUT=$${INCAN_EXAMPLES_TIMEOUT:-5} bash scripts/run_examples.sh
	@echo "\033[1mChecking benchmarks (Incan build only)...\033[0m"
	@INCAN_NO_BANNER=1 bash benchmarks/check_incan.sh

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

.PHONY: verify  ## test - Recommended local gate: pre-commit + smoke-test-fast
verify:
	@$(MAKE) pre-commit
	@$(MAKE) smoke-test-fast

.PHONY: test-verbose  ## test - Run tests with output
test-verbose:
	@echo "\033[1mRunning tests (verbose)...\033[0m"
	@cargo nextest run --all --no-capture 2>/dev/null || cargo test -- --nocapture

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
	@cargo build --release --bin incan-lsp
	@echo "\033[32m✓ LSP server built: target/release/incan-lsp\033[0m"

.PHONY: install-lsp  ## tool - Install incan-lsp to ~/.cargo/bin
install-lsp:
	@echo "\033[1mInstalling incan-lsp...\033[0m"
	@cargo install --path . --bin incan-lsp --force
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
	@cd editors/vscode && npm ci
	@cd editors/vscode && npm run compile
	@cd editors/vscode && npx @vscode/vsce package
	@echo "\033[32m✓ Extension packaged\033[0m"

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
