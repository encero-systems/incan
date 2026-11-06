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

Before running the gate, use a clean checkout at the revision and committed root `oven.lock` digest in `scripts/incql_gate_pin.json`. Supply the equivalence report named by `OVEN_EQUIV_ATTESTATION` when running `make test-oven-artifact-equivalence`.

The gate performs these steps:

1. Validate the checkout with `incan oven gate registry-pin` and stage IncQL's source-relative vocab companion path from the canonical Incan vocab crate.
2. Remove derived `.incan` and `target` outputs while preserving committed root and example locks.
3. Bake the IncQL library and quickstart consumer with Cargo and publisher-only native/tool commands unavailable.
4. Validate both locks with `incan oven gate consumer-graph --expect scripts/incql_gate_expectations.toml`, passing the equivalence report. Refuse a Cargo-layout executable and require an Oven executable.
5. Run the quickstart, check its completion marker and forbidden-command log, and run `registry-pin` again.

The pin, digest, typed-lock, expectation and equivalence checks run in Rust. A passing gate requires the complete attested consumer registry closure and the quickstart completion marker. See the [`make gate-incql` contract](../../tooling/reference/cli_reference.md#make-gate-incql) for accepted inputs, retained outputs and refusal conditions.

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

The native graph and its lock must match the runner platform. CI selects the checked-in Darwin or Linux compiler graph before fetching its pinned index and source archives; the source cache key includes both platform locks. A missing platform graph or lock fails preparation.

Linux jobs that fetch native sources, restore the SDK store or consume its handoff run on Ubuntu 24.04. The source-fetch action checks out a pinned incan.pub source revision and runs its shared native-owner preparation script in a fresh runner-temporary directory. The script reconstructs the compiler, resource tree, declared runtime files and header sysroot from digest-pinned package archives, then CI exports that private root as `INCAN_OVEN_LINK_OWNERS` before SDK preparation or validation. The source revision supplies transport code and its recipe; the selected graph and compiler's index pin remain the authority for admitted native facts.

Cargo builds the initial bootstrap executable. The subsequent native bootstrap produces the compiler together with its receipt, output record and two bound helper executables. CI transfers that complete bundle and restores executable permissions before consuming the SDK handoff. After validating the SDK, each fresh consumer prepares the ordinary standard packages with the source-current compiler before running project commands. That preparation uses the existing standard-package registry and a separate producer graph; ordinary commands keep the selected broad compiler graph. SDK validation and ordinary package preparation are separate required steps, and either failure stops the job.

After the run finishes, download its preparation evidence:

```bash
gh run download <run-id> -n test-linux-sdk-preparation-reports
```

Inspect the session's `summary.json` for `cache_hit`, `published` or `failed`. Its elapsed time starts after the SDK store lock is acquired. A cold publication also records each component's existing build report and a timing record; unavailable reports are marked explicitly. Build timings contain inclusive nested phases, so they must not be added together. Compare SDK preparation separately from the documentation-check step and native Loaf preparation.

Repeat the dispatch at the same commit after the first run completes to measure reuse. A warm run should select the same SDK identity without rebuilding components. Keep the compiler, Rust toolchain and source unchanged between the pair. The cross-run cache can lose a save reservation to another run; the same-run artifact remains the required input for consumers. An absent or incompatible handoff fails validation instead of silently starting another SDK publication.
