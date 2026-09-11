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
