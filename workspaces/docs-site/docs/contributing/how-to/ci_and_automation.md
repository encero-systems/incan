# CI & automation (repository / contributors)

This page collects the canonical commands for CI-friendly, deterministic automation **for this repository** (compiler/tooling contributors).

[CI and Automation]:../../tooling/how-to/ci_and_automation.md

If you’re trying to set up CI for an **Incan project** (using the `incan` CLI), see: [CI and Automation] as part of the Tooling How-To.

## Recommended commands

### Build

Build the project:

```bash
make build
```

### Format and lint (Rust codebase)

Run all quality checks (formatting, linting, unused dependencies):

```bash
make check
```

### Tests

Run all tests:

```bash
make test
```

### Examples (smoke test)

Run all examples:

```bash
make examples
```

!!! tip "Timeouts"
    You can tune timeouts via `INCAN_EXAMPLES_TIMEOUT` (default is 5 seconds).

### Full smoke test

Run all tests, examples, and benchmarks:

```bash
make smoke-test
```

### Docs site build

Build the docs site:

```bash
make docs-build
```

## Build a toolchain release archive

Use the release packager from the repository root after building the target `incan` and `incan-lsp` binaries and the host-runnable SDK provider builder:

Ensure `jq` is available. Before a local package run, populate the Cargo registry cache while network access is available:

```bash
workspaces/release/toolchain/fetch_release_support_workspace_sources.sh
```

Then package the target:

```bash
TARGET="aarch64-apple-darwin"
INCAN_BIN="target/${TARGET}/release/incan" \
INCAN_LSP_BIN="target/${TARGET}/release/incan-lsp" \
INCAN_SDK_PROVIDER_BUILDER_BIN="target/release/incan" \
RUSTUP_TOOLCHAIN="1.98.0" \
workspaces/release/toolchain/package_archive.sh "${TARGET}" --out-dir dist
```

The release workflow prewarms the support workspace's registry sources before packaging. The packager resolves and verifies the reduced support workspace lock with Cargo's offline mode, ships it as `crates/Cargo.lock`, and retains the SDK provider seed's shared lock. Other compiler and SDK preparation subprocesses are not covered by that offline flag. An installed compiler selects the sibling `crates/Cargo.lock` ahead of any enclosing checkout lock, so SDK component identity and rebuilding continue to use the dependency closure shipped with that toolchain.

`CARGO_BIN` is the highest-precedence Cargo selection when a caller supplies an exact executable. Otherwise the packager skips Cargo executables under a `target/` directory and selects the first remaining Cargo on `PATH`. When that executable has a sibling Rustup, `RUSTUP_TOOLCHAIN` selects an exact toolchain; without it, Rustup's active override or default applies. A Cargo installation without a sibling Rustup is used directly. The release workflow sets `RUSTUP_TOOLCHAIN=1.98.0`; local packaging follows the same rule only when the caller sets that variable.

Expect two publication phases. The command first publishes the ordinary release Loaf family under the staged package, then bakes the release policy project against that package-local family. That explicit release-policy bake materializes its admitted runtime foundation, rebuilds the runtime dependency closure with the retained compiler, selects the exact `core_engine` output for `TARGET` from the structured bake report, and republishes the envelope with those retained members. A later normal command that selects the release `ToolchainLoaf` only acquires and proves that same-generation closure; an absent or invalid closure refuses rather than triggering a consumer bake. A failure in either phase stops archive creation.

## Measure SDK preparation in hosted CI

To run the compiler, SDK, verified documentation and generated-reference checks without the heavy Oven suite, dispatch CI against the branch you want to measure:

```bash
gh workflow run ci.yml --ref <branch> -f heavy=false -f reference=true
```

The Linux compiler and SDK handoff job restores the compatible SDK cache or prepares the SDK once, then publishes the selected provider artifact. Documentation and generated-reference jobs consume that artifact independently. Heavy runs also supply it to Linux C ABI, Oven preparation, shadow comparison and release checks. macOS prepares its own platform-specific SDK.

After the run finishes, download its preparation evidence:

```bash
gh run download <run-id> -n test-linux-sdk-preparation-reports
```

Inspect the session's `summary.json` for `cache_hit`, `published` or `failed`. Its elapsed time starts after the SDK store lock is acquired. A cold publication also records each component's existing build report and a timing record; unavailable reports are marked explicitly. Build timings contain inclusive nested phases, so they must not be added together. Compare SDK preparation separately from the documentation-check step and native Loaf preparation.

Repeat the dispatch at the same commit after the first run completes to measure reuse. A warm run should select the same SDK identity without rebuilding components. Keep the compiler, Rust toolchain and source unchanged between the pair. The cross-run cache can lose a save reservation to another run; the same-run artifact remains the required input for consumers. An absent or incompatible handoff fails validation instead of silently starting another SDK publication.
