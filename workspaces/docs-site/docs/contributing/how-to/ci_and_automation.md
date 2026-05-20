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
