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

Before a release, compare `incan --help` and each command's `--help` with the [CLI reference](../../tooling/reference/cli_reference.md).

### IncQL closure gate

After producing the literal Cargo/Oven attestation, run the pinned downstream closure gate with an installed release compiler:

```bash
make gate-incql \
  INCQL_CHECKOUT=/path/to/incql \
  INCAN=/path/to/release/incan \
  INCQL_EQUIVALENCE_REPORT=/path/to/incql-equivalence.json
```

The checkout must be clean and match the revision and committed root `oven.lock` digest in `scripts/incql_gate_pin.json`. The gate removes derived `.incan` and `target` outputs but preserves every committed lock. The equivalence report is the output named by `OVEN_EQUIV_ATTESTATION` when running `make test-oven-artifact-equivalence`. A passing gate requires the complete attested registry closure and runs the Oven quickstart with Cargo and publisher-only native/tool commands unavailable. See the [`make gate-incql` contract](../../tooling/reference/cli_reference.md#make-gate-incql) for the accepted evidence and refusal conditions.

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
workspaces/release/toolchain/package_archive.sh "${TARGET}" --out-dir dist
```

The release workflow prewarms the support workspace's registry sources before packaging. The packager resolves and verifies the reduced support workspace lock with Cargo's offline mode, ships it as `<package>/crates/Cargo.lock`, and retains the SDK provider seed's shared lock. Other compiler and SDK preparation subprocesses are not covered by that offline flag. An installed compiler selects its own `<toolchain>/crates/Cargo.lock` ahead of any enclosing checkout lock, so SDK component identity and rebuilding continue to use the dependency closure shipped with that toolchain.

`CARGO_BIN` is the highest-precedence Cargo selection when a caller supplies an exact executable. Otherwise the packager skips Cargo executables under a `target/` directory and asks the matching Rustup for the compatibility publisher's pinned `nightly-2026-03-24` Cargo. `INCAN_RELEASE_PUBLISHER_TOOLCHAIN` can select another exact publisher toolchain for a controlled packaging run; it does not change the Rust 1.98.0 compiler identity sealed into the Loafs.

Release Loaf publication uses the same native-toolchain defaults as the compiler suite: Command Line Tools `clang`, `clang++`, and the canonical macOS SDK on macOS, or `/usr/bin/clang`, `/usr/bin/clang++`, and `/usr` elsewhere. Set all three of `INCAN_RELEASE_CC`, `INCAN_RELEASE_CXX`, and `INCAN_RELEASE_C_SYSROOT` to exact paths to override them.

Expect two publication phases. The command first publishes the ordinary release Loaf family under the staged package, then bakes the release policy project against that package-local family. That explicit release-policy bake materializes its admitted runtime foundation, rebuilds the runtime dependency closure with the retained compiler, selects the exact `core_engine` output for `TARGET` from the structured bake report, and republishes the envelope with those retained members. A later normal command that selects the release `ToolchainLoaf` only acquires and proves that same-generation closure; an absent or invalid closure refuses rather than triggering a consumer bake. A failure in either phase stops archive creation.

## Measure SDK preparation in hosted CI

To run the compiler, SDK, verified documentation and generated-reference checks without the heavy Oven suite, dispatch CI against the branch you want to measure:

```bash
gh workflow run ci.yml --ref <branch> -f heavy=false -f reference=true
```

The Linux compiler and SDK handoff job restores the compatible SDK cache or prepares the SDK once, then publishes the selected provider artifact. Documentation and generated-reference jobs consume that artifact independently. Heavy runs also supply it to Linux C ABI, Oven preparation and release checks. macOS prepares its own platform-specific SDK.

After the run finishes, download its preparation evidence:

```bash
gh run download <run-id> -n test-linux-sdk-preparation-reports
```

Inspect the session's `summary.json` for `cache_hit`, `published` or `failed`. Its elapsed time starts after the SDK store lock is acquired. A cold publication also records each component's existing build report and a timing record; unavailable reports are marked explicitly. Build timings contain inclusive nested phases, so they must not be added together. Compare SDK preparation separately from the documentation-check step and native Loaf preparation.

Repeat the dispatch at the same commit after the first run completes to measure reuse. A warm run should select the same SDK identity without rebuilding components. Keep the compiler, Rust toolchain and source unchanged between the pair. The cross-run cache can lose a save reservation to another run; the same-run artifact remains the required input for consumers. An absent or incompatible handoff fails validation instead of silently starting another SDK publication.
