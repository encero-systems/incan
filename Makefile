# Incan Programming Language - Makefile
# =====================================

# Nested generated-project and named-publisher work is deliberately constrained so one local test command does not
# consume every core. Override the cap for a specific machine with `make test INCAN_TEST_CARGO_BUILD_JOBS=<n>`.
INCAN_TEST_CARGO_BUILD_JOBS ?= 2
INCAN_TEST_GENERATED_CARGO_TARGET_DIR ?= $(CURDIR)/target/incan_generated_shared_target
INCAN_TEST_SDK_PROVIDER_STORE ?= $(CURDIR)/target/incan_test_sdk_provider_store
INCAN_TEST_SDK_PROVIDER_PATH_FILE ?= $(CURDIR)/target/incan_test_sdk_provider_path
INCAN_TEST_OVEN_HOME ?= $(CURDIR)/target/incan_test_oven_home
INCAN_TEST_OVEN_LOAF_ROOT ?= $(CURDIR)/target/share/incan/oven/loafs
INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT ?= $(CURDIR)/target/oven-alpha-release-toolchain
INCAN_TEST_OVEN_RELEASE_COMPILER_BIN ?= $(CURDIR)/target/debug/incan
INCAN_TEST_OVEN_COMPILER_SUITE_STORE ?= $(CURDIR)/target/oven-compiler-suite-store
# Caller-owned compiler-suite outputs are one-use. `test-oven` creates a fresh directory below this root and removes
# it after reporting its physical disk use, so repeated local runs cannot reuse a stale test binary or accumulate it.
INCAN_TEST_OVEN_COMPILER_SUITE_OUTPUT_ROOT ?= $(CURDIR)/target
# Oven owns the release and compiler-suite storage profiles. Make supplies roots and deliberate test inputs only;
# refusal tests pass explicit tiny CLI limits rather than redefining production policy here.
INCAN_TEST_OVEN_BAKE_FORMAT ?= text
INCAN_TEST_OVEN_BAKE_REPORT ?=
# Optional caller-owned location for a completed compiler-suite JSON report. The default test target removes its
# one-use caller output after success; the case-timing target retains only this small evidence file for ranking.
INCAN_TEST_OVEN_COMPILER_SUITE_REPORT ?=
# Optional caller-owned location for a successful `test-one` compiler-suite JSON report. The focused command keeps
# its disposable output clean by default; a diagnostic caller can retain only the report for nested-command analysis.
INCAN_TEST_OVEN_TEST_ONE_REPORT ?=
# The pinned publisher Cargo supplies the unstable unit graph and the package-qualified Rust-inspection tests that
# exercise Cargo's nightly-only metadata flags. Loaf receipts and direct-rustc suite execution use the selected
# consumer toolchain, so the pinned Rust 1.98.0 gates prove their advertised compiler rather than nightly. The named
# publisher/test-fixture boundary remains explicit; normal Oven build/run/test remains direct-rustc.
INCAN_TEST_PREWARM_TOOLCHAIN ?= 1.98.0
INCAN_TEST_PUBLISHER_TOOLCHAIN ?= nightly-2026-03-24
INCAN_TEST_FIXTURE_CARGO_TOOLCHAIN ?= $(INCAN_TEST_PUBLISHER_TOOLCHAIN)
INCAN_TEST_LOAF_TOOLCHAIN ?= 1.98.0
INCAN_TEST_SUITE_TOOLCHAIN ?= 1.98.0
TEST_ENV = CARGO_BUILD_JOBS=$(INCAN_TEST_CARGO_BUILD_JOBS) \
	INCAN_GENERATED_CARGO_TARGET_DIR="$(INCAN_TEST_GENERATED_CARGO_TARGET_DIR)" \
	INCAN_INTERNAL_SDK_PROVIDER_STORE="$(INCAN_TEST_SDK_PROVIDER_STORE)" \
	INCAN_HOME="$(INCAN_TEST_OVEN_HOME)" \
	INCAN_SOURCE_ROOT="$(CURDIR)" \
	INCAN_STDLIB="$(CURDIR)/crates/incan_stdlib/stdlib" \
	INCAN_STDLIB_DIR="$(CURDIR)/crates/incan_stdlib/stdlib" \
	INCAN_TOOLCHAIN_CRATES_DIR="$(CURDIR)/crates"
TEST_RUNTIME_ENV = $(TEST_ENV) \
	INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE="$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)" \
	INCAN_SDK_INVENTORY="$$(cat "$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)")/sdk-inventory.json"
ifneq ($(strip $(INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS)),)
TEST_RUNTIME_ENV += INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS="$(INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS)"
endif
ifneq ($(strip $(INCAN_TEST_COMMAND_TIMINGS)),)
TEST_RUNTIME_ENV += INCAN_TEST_COMMAND_TIMINGS="$(INCAN_TEST_COMMAND_TIMINGS)"
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
	@echo "\033[1mRelease gates (local only):\033[0m"
	@grep -E '^.PHONY: .*?## gate - .*$$' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ".PHONY: |## gate - "}; {printf "  \033[36m%-18s\033[0m %s\n", $$2, $$3}'
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

.PHONY: lint-fast  ## quality - Run faster clippy profile (workspace + all targets + all-features)
lint-fast:
	@echo "\033[1mRunning clippy (fast profile)...\033[0m"
	@cargo clippy --workspace --all-targets --all-features -- -D warnings

.PHONY: fmt-check-ci
fmt-check-ci:
	@cargo +nightly fmt --version >/dev/null 2>&1 || ( \
		echo "\033[33m⚠ nightly rustfmt is required for this project formatting config.\033[0m"; \
		echo "\033[33m  Install it via: rustup toolchain install nightly --component rustfmt\033[0m"; \
		exit 1; \
	)
	@cargo +nightly fmt --all -- --check

# Matches hosted CI's clippy surface (.github/workflows/ci.yml): without --all-targets, test and bench targets are
# never linted locally, and feature-gated test-module violations (deny(clippy::expect_used)) surface only on CI.
.PHONY: lint-fast-ci
lint-fast-ci:
	@cargo clippy --workspace --all-targets --all-features -- -D warnings

.PHONY: rustdoc-gate  ## quality - Require rustdoc on changed Rust functions/methods
rustdoc-gate:
	@echo "\033[1mChecking rustdoc coverage for changed Rust functions/methods...\033[0m"
	@python3 scripts/check_changed_rustdocs.py

.PHONY: rustdoc-gate-ci
rustdoc-gate-ci:
	@python3 scripts/check_changed_rustdocs.py

.PHONY: version-gate  ## quality - Require hand-written version literals to match the workspace version
version-gate:
	@python3 scripts/check_release_version_consistency.py

.PHONY: agents-doc-sync  ## quality - Check AGENTS.md's skill table matches .agents/skills/ (local only, not CI)
agents-doc-sync:
	@echo "\033[1mChecking AGENTS.md skill table against .agents/skills/...\033[0m"
	@python3 scripts/check_agents_doc_sync.py

.PHONY: agents-doc-sync-ci
agents-doc-sync-ci:
	@python3 scripts/check_agents_doc_sync.py

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
	printf "\033[1mChecking version consistency...\033[0m "; \
	$(MAKE) -s version-gate; \
	echo "\033[32mDONE\033[0m"; \
	t2a=$$(date +%s); \
	printf "\033[1mChecking AGENTS.md skill table...\033[0m "; \
	$(MAKE) -s agents-doc-sync-ci; \
	echo "\033[32mDONE\033[0m"; \
	t2b=$$(date +%s); \
	echo "\033[1mRunning cargo check (fast gate)...\033[0m"; \
	$(MAKE) -s check-fast-ci; \
	echo "\033[32mDONE\033[0m"; \
	t3=$$(date +%s); \
	echo "\033[32m✓ Pre-commit checks passed (fast)\033[0m"; \
	echo "\033[36mPhase timing:\033[0m fmt-check=$$((t1-start))s, rustdoc=$$((t2-t1))s, version-gate=$$((t2a-t2))s, agents-doc-sync=$$((t2b-t2a))s, check=$$((t3-t2b))s, total=$$((t3-start))s"

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
	printf "\033[1mChecking AGENTS.md skill table...\033[0m "; \
	$(MAKE) -s agents-doc-sync-ci; \
	echo "\033[32mDONE\033[0m"; \
	t2b=$$(date +%s); \
	echo "\033[1mRunning tests...\033[0m"; \
	$(MAKE) -s test-oven; \
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
	echo "\033[36mPhase timing:\033[0m fmt-check=$$((t1-start))s, rustdoc=$$((t2-t1))s, agents-doc-sync=$$((t2b-t2))s, tests=$$((t3-t2b))s, lint=$$((t4-t3))s, deny=$$((t5-t4))s, total=$$((t5-start))s"

.PHONY: pre-commit  ## quality - Full local gate: pre-commit-full-gate + smoke-test-fast
pre-commit:
	@echo "\033[1mRunning pre-commit (full local gate)...\033[0m"
	@$(MAKE) pre-commit-full-gate
	@$(MAKE) smoke-test-fast
	@echo "\033[32m✓ Pre-commit passed\033[0m"

.PHONY: ci-full  ## quality - Full CI check: fmt, lint, udeps, test, and release build
ci-full: fmt lint udeps
	@echo "\033[1mRunning tests...\033[0m"
	@$(MAKE) -s test-oven
	@echo "\033[1mBuilding release...\033[0m"
	@cargo build --release --quiet
	@echo "\033[32m✓ Full CI checks passed\033[0m"

# =============================================================================
# Testing
# =============================================================================

.PHONY: fetch-locked-cargo-sources
fetch-locked-cargo-sources:
	@cargo fetch --locked
	@cargo fetch --manifest-path crates/incan_stdlib/stdlib/components/stdlib-interop/vocab_companion/Cargo.toml --locked

.PHONY: fetch-oven-loaf-sources
fetch-oven-loaf-sources:
	@cargo fetch --manifest-path tests/fixtures/oven_loaf_dependencies/Cargo.toml --locked

.PHONY: fetch-release-support-workspace-sources
fetch-release-support-workspace-sources:
	@bash workspaces/release/toolchain/fetch_release_support_workspace_sources.sh

.PHONY: test-oven
test-oven: test-prewarm-oven-loafs test-prewarm-oven-release-loafs
	@$(MAKE) --no-print-directory test-oven-replay

.PHONY: test-oven-partition  ## test - Replay one deterministic prewarmed Oven compiler-suite partition
# CI restores the complete compiler and release envelopes before invoking this target. Keep it replay-only: a
# partition must never silently publish or prewarm an authority that its receipt is supposed to consume.
test-oven-partition:
	@test -n "$(INCAN_TEST_OVEN_PARTITION_INDEX)" || { echo "INCAN_TEST_OVEN_PARTITION_INDEX is required" >&2; exit 2; }
	@test -n "$(INCAN_TEST_OVEN_PARTITION_COUNT)" || { echo "INCAN_TEST_OVEN_PARTITION_COUNT is required" >&2; exit 2; }
	@partition_display=$$(( $(INCAN_TEST_OVEN_PARTITION_INDEX) + 1 )); \
		echo "\033[1mRunning prepared Oven compiler-suite partition $$partition_display/$(INCAN_TEST_OVEN_PARTITION_COUNT)...\033[0m"
	@$(MAKE) --no-print-directory test-oven-replay \
		INCAN_TEST_OVEN_COMPILER_SUITE_PARTITION_ARGS='--partition-index $(INCAN_TEST_OVEN_PARTITION_INDEX) --partition-count $(INCAN_TEST_OVEN_PARTITION_COUNT)'

# Rust-analyzer fixture metadata creates nested Cargo lockfile copies. The Unix-only Oven suite therefore owns a
# short `/tmp` scratch directory instead of inheriting an arbitrarily deep worktree `TMPDIR`.
.PHONY: test-oven-replay
test-oven-replay:
	@echo "\033[1mRunning prepared compiler-suite replay through Oven...\033[0m"
	@set -e; \
		mkdir -p "$(INCAN_TEST_OVEN_COMPILER_SUITE_OUTPUT_ROOT)"; \
		suite_output="$$(mktemp -d "$(INCAN_TEST_OVEN_COMPILER_SUITE_OUTPUT_ROOT)/oven-compiler-suite-output.XXXXXX")"; \
		suite_tmp="$$(mktemp -d "/tmp/incan-oven-suite.XXXXXX")"; \
		suite_succeeded=false; \
		cleanup_suite_output() { \
			rm -rf -- "$$suite_tmp"; \
			if [ "$$suite_succeeded" = true ]; then \
				if [ -n "$(INCAN_TEST_OVEN_COMPILER_SUITE_REPORT)" ]; then \
					cp "$$suite_output/compiler-suite-report.json" "$(INCAN_TEST_OVEN_COMPILER_SUITE_REPORT)"; \
				fi; \
				rm -rf -- "$$suite_output"; \
			else echo "Oven suite failed; retaining caller output at $$suite_output" >&2; fi; \
		}; \
		trap cleanup_suite_output EXIT; \
		rustc_path="$$(rustup which --toolchain "$(INCAN_TEST_SUITE_TOOLCHAIN)" rustc)"; \
		fixture_cargo_path="$$(rustup which --toolchain "$(INCAN_TEST_FIXTURE_CARGO_TOOLCHAIN)" cargo)"; \
		mkdir -p "$$suite_output/cargo-guard"; \
		printf '%s\n' '#!/bin/sh' 'printf "%s\\n" "unexpected Cargo invocation: $$*" >> "$$INCAN_OVEN_CARGO_GUARD_LOG"' 'exit 97' \
			> "$$suite_output/cargo-guard/cargo"; \
		chmod +x "$$suite_output/cargo-guard/cargo"; \
		: > "$$suite_output/cargo-guard/invocations.log"; \
		PATH="$$suite_output/cargo-guard:$$PATH" \
			INCAN_OVEN_CARGO_GUARD_LOG="$$suite_output/cargo-guard/invocations.log" TMPDIR="$$suite_tmp" \
			$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" CARGO_NET_OFFLINE=true INCAN_NO_BANNER=1 \
			INCAN_INTERNAL_OVEN_NORMAL_CONSUMER_BIN="$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/bin/incan" \
			INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT="$(CURDIR)/target" \
			./target/debug/incan oven compiler-libtests \
				--compiler-root "$(CURDIR)" --rustc "$$rustc_path" --fixture-cargo "$$fixture_cargo_path" \
				--feature lsp --output "$$suite_output" \
				--store "$(INCAN_TEST_OVEN_COMPILER_SUITE_STORE)" \
				$(INCAN_TEST_OVEN_COMPILER_SUITE_PARTITION_ARGS) \
				--format text; \
		test ! -s "$$suite_output/cargo-guard/invocations.log"; \
		suite_succeeded=true

.PHONY: test-oven-case-timings  ## test - Run the complete Oven suite once and retain its top-25 case-timing report
test-oven-case-timings:
	@$(MAKE) --no-print-directory test-oven INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS=1 \
		INCAN_TEST_OVEN_COMPILER_SUITE_REPORT="$(CURDIR)/target/oven-compiler-suite-case-timings.json"

.PHONY: test  ## test - Run all compiler tests through bounded Oven direct-Rustc execution
test: test-oven

.PHONY: test-prewarm-sdk
test-prewarm-sdk:
	@echo "\033[1mPrewarming compiled SDK providers...\033[0m"
	@if [ "$(INCAN_TEST_COMPILER_ALREADY_BUILT)" = "1" ]; then \
		test -x "$(CURDIR)/target/debug/incan"; \
	else \
		$(TEST_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_PREWARM_TOOLCHAIN)" cargo build --features lsp; \
	fi
	@$(TEST_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_PREWARM_TOOLCHAIN)" CARGO_NET_OFFLINE=true INCAN_NO_BANNER=1 \
		INCAN_STDLIB="$(CURDIR)/crates/incan_stdlib/stdlib" \
		INCAN_STDLIB_DIR="$(CURDIR)/crates/incan_stdlib/stdlib" \
		INCAN_INTERNAL_SDK_PROVIDER_PATH_FILE="$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)" \
		./target/debug/incan check tests/fixtures/test_assert_canary.incn
	@test -s "$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)"
	@test -f "$$(cat "$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)")/sdk-inventory.json"

.PHONY: shadow-comparison-evidence  ## test - Stage Oven and prove the #1146 source-observable comparison actually ran
# The bounded #1146 comparison's legacy route is Oven-owned, so it needs a published direct-rustc plan. Without
# one the comparison is honestly unavailable and every corpus row stays non-green -- which a default test run
# cannot distinguish from a comparison that was never implemented. This target stages the exact plans, then runs the
# relevant suites with INCAN_SHADOW_REQUIRE_LEGACY_ROUTE=1 so an unstaged or failing comparison is a hard failure
# rather than a reported skip. Each distinct source-session provider closure gets its own exact receipt; the
# comparator selects from that ordered list rather than treating a broader or narrower closure as authority.
shadow-comparison-evidence: test-prewarm-sdk
	@echo "\033[1mStaging Oven and proving the #1146 source-observable comparison...\033[0m"
	@set -e; \
		if [ "$(INCAN_TEST_COMPILER_ALREADY_BUILT)" = "1" ]; then \
			test -x "$(CURDIR)/target/debug/incan"; \
		else \
			$(TEST_ENV) cargo build --bin incan; \
		fi; \
		stage="$(INCAN_SHADOW_STAGE_ROOT)"; \
		rm -rf -- "$$stage"; \
		mkdir -p "$$stage"; \
		$(SHADOW_STAGE_ENV) ./target/debug/incan new shadow_probe --yes --dir "$$stage/shadow_probe" >/dev/null; \
		$(SHADOW_STAGE_ENV) ./target/debug/incan new shadow_json_probe --yes --dir "$$stage/shadow_json_probe" >/dev/null; \
		cp "$(CURDIR)/tests/fixtures/replacement/json_stringify_scalars.incn" "$$stage/shadow_json_probe/src/main.incn"; \
		printf '\n\ndef main() -> None:\n    println(observe())\n' >> "$$stage/shadow_json_probe/src/main.incn"; \
		$(SHADOW_STAGE_ENV) ./target/debug/incan oven bake --project "$$stage/shadow_probe" >/dev/null; \
		$(SHADOW_STAGE_ENV) ./target/debug/incan oven bake --project "$$stage/shadow_json_probe" >/dev/null; \
		core_receipt="$$stage/shadow_probe/.incan/oven/executable-debug-receipt.json"; \
		json_receipt="$$stage/shadow_json_probe/.incan/oven/executable-debug-receipt.json"; \
		for receipt in "$$core_receipt" "$$json_receipt"; do \
			test -f "$$receipt" || { echo "Oven bake did not publish executable debug receipt $$receipt" >&2; exit 1; }; \
		done; \
		$(SHADOW_TEST_ENV) INCAN_SHADOW_OVEN_RECEIPT="$$core_receipt$(SHADOW_RECEIPT_PATH_SEPARATOR)$$json_receipt" \
			cargo test --test shadow_comparison_tests --test parity_corpus_tests \
				--test replacement_enumerate_zip_shadow_tests --test replacement_enumerate_zip_parity_cases \
				--test replacement_scalar_conversion_shadow_tests \
				--test replacement_isinstance_shadow_tests
	@echo "\033[32m✓ the #1146 comparison ran under Oven authority and its corpus row is green\033[0m"

# Oven home the staged comparison publishes its direct-rustc plan into, kept out of the developer's own store.
INCAN_SHADOW_OVEN_HOME ?= $(CURDIR)/target/incan_shadow_oven_home
INCAN_SHADOW_STAGE_ROOT ?= $(CURDIR)/target/incan_shadow_stage
INCAN_SHADOW_RUSTC ?= $(shell rustup which rustc)
SHADOW_RECEIPT_PATH_SEPARATOR ?= :
SHADOW_STAGE_ENV = $(TEST_RUNTIME_ENV) INCAN_HOME="$(INCAN_SHADOW_OVEN_HOME)" CARGO_NET_OFFLINE=true INCAN_NO_BANNER=1
SHADOW_TEST_ENV = $(TEST_RUNTIME_ENV) CARGO_NET_OFFLINE=true \
	INCAN_SHADOW_OVEN_HOME="$(INCAN_SHADOW_OVEN_HOME)" \
	INCAN_SHADOW_RUSTC="$(INCAN_SHADOW_RUSTC)" \
	INCAN_SHADOW_REQUIRE_LEGACY_ROUTE=1

.PHONY: test-prewarm-oven-loafs
test-prewarm-oven-loafs: test-prewarm-sdk
	@echo "\033[1mBaking or reusing the compiler-suite standard-library Loaf family...\033[0m" >&2
	@$(TEST_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_LOAF_TOOLCHAIN)" CARGO_NET_OFFLINE=true INCAN_NO_BANNER=1 \
		INCAN_STDLIB="$(CURDIR)/crates/incan_stdlib/stdlib" \
		INCAN_STDLIB_DIR="$(CURDIR)/crates/incan_stdlib/stdlib" \
		./target/debug/incan oven legacy-cargo bake-loafs \
			--compiler-root "$(CURDIR)" \
			--output "$(INCAN_TEST_OVEN_LOAF_ROOT)" \
			--suite-store "$(INCAN_TEST_OVEN_COMPILER_SUITE_STORE)" \
			--envelope compiler-suite \
			--sdk-inventory "$$(cat "$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)")/sdk-inventory.json" \
			--cargo "$$(rustup which --toolchain "$(INCAN_TEST_PUBLISHER_TOOLCHAIN)" cargo)" \
			--rustc "$$(rustup which --toolchain "$(INCAN_TEST_LOAF_TOOLCHAIN)" rustc)" \
			--format "$(INCAN_TEST_OVEN_BAKE_FORMAT)" $(if $(INCAN_TEST_OVEN_BAKE_REPORT),> "$(INCAN_TEST_OVEN_BAKE_REPORT)",)

.PHONY: test-prewarm-oven-release-loafs
test-prewarm-oven-release-loafs: test-prewarm-sdk
	@echo "\033[1mBaking or reusing the release standard-library Loaf family...\033[0m"
	@test -x "$(INCAN_TEST_OVEN_RELEASE_COMPILER_BIN)"
	@mkdir -p "$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/bin"
	@cp "$(INCAN_TEST_OVEN_RELEASE_COMPILER_BIN)" "$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/bin/incan"
	@if [ "$$(uname -s)" = "Darwin" ] && command -v codesign >/dev/null 2>&1; then \
		codesign --force --sign - "$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/bin/incan"; \
	fi
	@$(TEST_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_LOAF_TOOLCHAIN)" CARGO_NET_OFFLINE=true INCAN_NO_BANNER=1 \
		INCAN_STDLIB="$(CURDIR)/crates/incan_stdlib/stdlib" \
		INCAN_STDLIB_DIR="$(CURDIR)/crates/incan_stdlib/stdlib" \
		"$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/bin/incan" oven legacy-cargo bake-loafs \
			--compiler-root "$(CURDIR)" \
			--output "$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/share/incan/oven/loafs" \
			--envelope release \
			--sdk-inventory "$$(cat "$(INCAN_TEST_SDK_PROVIDER_PATH_FILE)")/sdk-inventory.json" \
			--cargo "$$(rustup which --toolchain "$(INCAN_TEST_PUBLISHER_TOOLCHAIN)" cargo)" \
			--rustc "$$(rustup which --toolchain "$(INCAN_TEST_LOAF_TOOLCHAIN)" rustc)"

.PHONY: test-oven-focused
test-oven-focused:
	@echo "\033[1mRunning focused Oven and Loaf regression tests...\033[0m"
	@CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --lib oven::
	@CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test cli_integration \
		lock_records_oven_interop_requirements_and_detects_input_drift -- --exact
	@CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test toolchain_installer_tests \
		oven_alpha_benchmark_records_a_verified_cargo_guard_verdict -- --exact
	@CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test toolchain_installer_tests \
		compiler_suite_action_composes_baker_guarded_runner_and_storage_evidence -- --exact

.PHONY: test-oven-pr-regressions
test-oven-pr-regressions:
	@echo "\033[1mRunning bounded Oven process-containment regressions...\033[0m"
	@CARGO_PROFILE_TEST_DEBUG=0 CARGO_BUILD_JOBS=2 cargo test --locked --features lsp --test oven_pr_regressions

.PHONY: test-oven-release-smoke
test-oven-release-smoke: test-prewarm-oven-release-loafs
	@echo "\033[1mRunning Cargo-guarded Oven release-envelope smoke...\033[0m"
	@set -e; \
		smoke_root="$$(mktemp -d "$(INCAN_TEST_OVEN_COMPILER_SUITE_OUTPUT_ROOT)/oven-release-smoke.XXXXXX")"; \
		cleanup_smoke_root() { rm -rf -- "$$smoke_root"; }; \
		trap cleanup_smoke_root EXIT; \
		mkdir -p "$$smoke_root/cargo-guard" "$$smoke_root/incan-home"; \
		printf '%s\n' '#!/bin/sh' 'printf "%s\\n" "unexpected Cargo invocation: $$*" >> "$$INCAN_OVEN_CARGO_GUARD_LOG"' 'exit 97' \
			> "$$smoke_root/cargo-guard/cargo"; \
		chmod +x "$$smoke_root/cargo-guard/cargo"; \
		: > "$$smoke_root/cargo-guard/invocations.log"; \
		run_incan() { \
			PATH="$$smoke_root/cargo-guard:$$PATH" \
			INCAN_OVEN_CARGO_GUARD_LOG="$$smoke_root/cargo-guard/invocations.log" \
			INCAN_HOME="$$smoke_root/incan-home" INCAN_NO_BANNER=1 \
			RUSTUP_TOOLCHAIN="$(INCAN_TEST_LOAF_TOOLCHAIN)" \
			"$(INCAN_TEST_OVEN_RELEASE_TOOLCHAIN_ROOT)/bin/incan" "$$@"; \
		}; \
		run_project_incan() { \
			INCAN_SOURCE_ROOT="$(CURDIR)" \
			INCAN_STDLIB="$(CURDIR)/crates/incan_stdlib/stdlib" \
			INCAN_STDLIB_DIR="$(CURDIR)/crates/incan_stdlib/stdlib" \
			INCAN_TOOLCHAIN_CRATES_DIR="$(CURDIR)/crates" \
			run_incan "$$@"; \
		}; \
		for fixture in oven_project_bake oven_release_bytes_io oven_release_file_lock oven_release_app_bake; do \
			project_root="$$smoke_root/$$fixture"; \
			cp -R "$(CURDIR)/tests/fixtures/$$fixture" "$$project_root"; \
			run_project_incan oven bake --project "$$project_root" --format json > "$$smoke_root/$$fixture-bake-first.json"; \
			test "$$(grep -Fc '"action": "toolchain_loaf"' "$$smoke_root/$$fixture-bake-first.json")" -eq 2; \
			run_project_incan oven bake --project "$$project_root" --format json > "$$smoke_root/$$fixture-bake-second.json"; \
			test "$$(grep -Fc '"action": "reused"' "$$smoke_root/$$fixture-bake-second.json")" -eq 2; \
			if [ "$$fixture" = oven_release_app_bake ]; then \
				(cd "$$project_root" && run_project_incan build src/main.incn); \
				(cd "$$project_root" && run_project_incan run src/main.incn); \
			else \
				(cd "$$project_root" && run_project_incan build --lib); \
			fi; \
			case "$$fixture" in \
				oven_release_bytes_io) test_source="src/test_bytes_io.incn" ;; \
				oven_release_file_lock) test_source="src/test_file_lock.incn" ;; \
				*) test_source="" ;; \
			esac; \
			if [ -n "$$test_source" ]; then (cd "$$project_root" && run_project_incan test "$$test_source"); fi; \
		done; \
		for command in build run test; do \
			source="$(CURDIR)/src/oven/fixtures/release_core.incn"; \
			if [ "$$command" = test ]; then source="$(CURDIR)/src/oven/fixtures/test_release_core.incn"; fi; \
			run_incan "$$command" "$$source"; \
		done; \
		test ! -s "$$smoke_root/cargo-guard/invocations.log"

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
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		./target/release/incan test tests/fixtures/test_assert_canary.incn
	@echo "\033[32m✓ Incan assertion canary passed\033[0m"

.PHONY: smoke-test-web-example
smoke-test-web-example:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mBuilding web example (build-only)...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		./target/release/incan build examples/web/hello_web.incn
	@echo "\033[32m✓ Web example built\033[0m"

.PHONY: smoke-test-nested-project-example
smoke-test-nested-project-example:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mBuilding nested_project example (build-only)...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		./target/release/incan build examples/advanced/nested_project/src/main.incn
	@echo "\033[32m✓ Nested project example built\033[0m"

.PHONY: smoke-test-examples
smoke-test-examples:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mRunning examples...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		INCAN_EXAMPLES_TIMEOUT=$${INCAN_EXAMPLES_TIMEOUT:-30} bash scripts/run_examples.sh
	@echo "\033[1mChecking documentation examples...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		INCAN_BIN=./target/release/incan bash scripts/check_docs_examples.sh

.PHONY: check-docs-examples  ## test - Typecheck committed verified documentation examples
check-docs-examples: test-prewarm-sdk
	@echo "\033[1mChecking verified documentation examples...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		INCAN_BIN=./target/debug/incan bash scripts/check_docs_examples.sh

.PHONY: smoke-test-rust-interop-examples
smoke-test-rust-interop-examples: test-prewarm-oven-loafs
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mRunning receipt-compatible Rust-interop smoke examples...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		INCAN_EXAMPLES_ONLY="examples/advanced/using_rust_crates.incn:examples/pro/rust_interop_pro.incn" \
		INCAN_EXAMPLES_TIMEOUT=30 INCAN_EXAMPLES_TIMEOUT_MODE=fail INCAN_EXAMPLES_REQUIRE_CARGO_FREE=1 \
		bash scripts/run_examples.sh

.PHONY: smoke-test-benchmarks-incan
smoke-test-benchmarks-incan:
	@$(MAKE) -s smoke-test-require-release-bin
	@echo "\033[1mChecking benchmarks (Incan build only)...\033[0m"
	@$(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" INCAN_NO_BANNER=1 \
		bash workspaces/benchmarks/check_incan.sh

# `smoke-test-examples` bakes `examples/pro/vocab_*` and `examples/advanced/library_package`
# producer/consumer pairs with the release binary. Baking their compiler-owned vocabulary helper
# needs a release-cohort Loaf, which only `test-prewarm-oven-release-loafs` bakes; without it, the
# producer bake fails with "no compatible release-cohort Loaf" before the consumer ever runs.
.PHONY: smoke-test-core
smoke-test-core: test-prewarm-oven-loafs test-prewarm-oven-release-loafs
	@$(MAKE) smoke-test-release
	@$(MAKE) smoke-test-canary
	@$(MAKE) smoke-test-web-example
	@$(MAKE) smoke-test-nested-project-example
	@$(MAKE) smoke-test-rust-interop-examples
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

.PHONY: test-verbose  ## test - Run the complete compiler suite through Oven
test-verbose: test-oven
	@echo "\033[32m✓ Oven reports each root and retains the runner transcript on failure\033[0m"

.PHONY: test-diagnose  ## test - Run the complete compiler suite through Oven and retain failure evidence
test-diagnose: test-oven
	@echo "\033[32m✓ Oven diagnostics are retained in the caller output only when a root fails\033[0m"

.PHONY: test-timings  ## test - Generate cargo compile-timing report (target/cargo-timings)
test-timings:
	@echo "\033[1mGenerating cargo timing report for test build...\033[0m"
	@cargo test --all --no-run --timings
	@echo "\033[32m✓ Timing report generated in target/cargo-timings\033[0m"

# Keep single-root diagnostics on the same short, invocation-owned scratch policy as full suite replay.
.PHONY: test-one  ## test - Run one receipt-bound compiler-suite source root (optional TEST_EXACT=module::case)
test-one: test-prewarm-oven-loafs
	@test -n "$(TEST_ROOT)" || { echo "usage: make test-one TEST_ROOT=tests/cli_integration.rs" >&2; exit 2; }
	@echo "\033[1mRunning $(TEST_ROOT)$(if $(TEST_EXACT), ($(TEST_EXACT)),) through Oven...\033[0m"
	@set -e; \
		mkdir -p "$(INCAN_TEST_OVEN_COMPILER_SUITE_OUTPUT_ROOT)"; \
		root_output="$$(mktemp -d "$(INCAN_TEST_OVEN_COMPILER_SUITE_OUTPUT_ROOT)/oven-test-one.XXXXXX")"; \
		root_tmp="$$(mktemp -d "/tmp/incan-oven-root.XXXXXX")"; \
		root_succeeded=false; \
		cleanup_root_output() { \
			rm -rf -- "$$root_tmp"; \
			if [ "$$root_succeeded" = true ]; then \
				if [ -n "$(INCAN_TEST_OVEN_TEST_ONE_REPORT)" ]; then \
					cp "$$root_output/compiler-suite-report.json" "$(INCAN_TEST_OVEN_TEST_ONE_REPORT)"; \
				fi; \
				rm -rf -- "$$root_output"; \
			else echo "Oven root failed; retaining caller output at $$root_output" >&2; fi; \
		}; \
		trap cleanup_root_output EXIT; \
		rustc_path="$$(rustup which --toolchain "$(INCAN_TEST_SUITE_TOOLCHAIN)" rustc)"; \
		fixture_cargo_path="$$(rustup which --toolchain "$(INCAN_TEST_FIXTURE_CARGO_TOOLCHAIN)" cargo)"; \
		mkdir -p "$$root_output/cargo-guard"; \
		printf '%s\n' '#!/bin/sh' 'printf "%s\\n" "unexpected Cargo invocation: $$*" >> "$$INCAN_OVEN_CARGO_GUARD_LOG"' 'exit 97' \
			> "$$root_output/cargo-guard/cargo"; \
		chmod +x "$$root_output/cargo-guard/cargo"; \
		: > "$$root_output/cargo-guard/invocations.log"; \
		PATH="$$root_output/cargo-guard:$$PATH" INCAN_OVEN_CARGO_GUARD_LOG="$$root_output/cargo-guard/invocations.log" \
			TMPDIR="$$root_tmp" $(TEST_RUNTIME_ENV) RUSTUP_TOOLCHAIN="$(INCAN_TEST_SUITE_TOOLCHAIN)" \
			CARGO_NET_OFFLINE=true INCAN_NO_BANNER=1 INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT="$(CURDIR)/target" \
			./target/debug/incan oven compiler-libtests \
				--compiler-root "$(CURDIR)" --rustc "$$rustc_path" --fixture-cargo "$$fixture_cargo_path" \
				--feature lsp --target "$(TEST_ROOT)" $(if $(TEST_EXACT),--exact "$(TEST_EXACT)") \
				--output "$$root_output" --store "$(INCAN_TEST_OVEN_COMPILER_SUITE_STORE)" \
				--format text; \
		test ! -s "$$root_output/cargo-guard/invocations.log"; \
		root_succeeded=true

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

# =============================================================================
# Release gates (local only)
#
# These prove a release the compiler suite cannot: that the flagship external consumer still builds, and that a
# first-time user on a real machine can install and build. Both are deliberately kept out of CI -- IncQL pulls
# DataFusion, and the clean rooms provision Rust twice -- so they run in front of a release, not every PR.
# =============================================================================

.PHONY: gate-incql  ## gate - Build the real IncQL consumer end to end (INCQL_CHECKOUT=..., INCAN=...)
gate-incql:
	@bash scripts/gate_incql.sh --incan "$${INCAN:-$(CURDIR)/target/release/incan}"

.PHONY: gate-cleanroom  ## gate - Install into containers with and without a mismatched Rust (DIST=...)
gate-cleanroom:
	@bash scripts/gate_cleanroom.sh $(if $(DIST),--dist "$(DIST)",) $(if $(MANIFEST),--manifest "$(MANIFEST)",)

.PHONY: bench-build-times  ## gate - Record build times across toolchains (TOOLCHAINS="0.4.0=/path/incan 0.5.0=/path/incan")
bench-build-times:
	@test -n "$(TOOLCHAINS)" \
		|| { echo 'usage: make bench-build-times TOOLCHAINS="0.4.0=/path/to/incan 0.5.0=/path/to/incan"' >&2; exit 2; }
	@bash scripts/bench_build_times.sh $(foreach toolchain,$(TOOLCHAINS),--toolchain "$(toolchain)")

.PHONY: gate-release  ## gate - Every local release gate: IncQL consumer + clean-room installs
gate-release:
	@$(MAKE) gate-incql
	@$(MAKE) gate-cleanroom
	@echo "\033[32m✓ Release gates passed\033[0m"

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

.PHONY: version  ## misc - Show version info
version:
	@echo "\033[1mIncan version:\033[0m"
	@cargo pkgid | cut -d# -f2
	@echo ""
	@echo "\033[1mRust version:\033[0m"
	@rustc --version
