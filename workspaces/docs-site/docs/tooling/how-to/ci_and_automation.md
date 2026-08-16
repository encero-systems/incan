# CI & automation (projects / CLI-first)

This page collects the canonical, CI-friendly commands for **Incan projects** (using the `incan` CLI).

If you’re running CI for the **Incan compiler/tooling repository**, see: [CI & automation (repository)](../../contributing/how-to/ci_and_automation.md).

## Recommended commands

### Type check (fast gate)

Type-check a program without building/running it:

```bash
incan check path/to/main.incn
```

Use `incan check path/to/main.incn --format json` when CI, editor tooling, or agents need deterministic diagnostics with stable codes and source spans. Passing a file without a subcommand still type-checks it for compatibility, but `incan check` is the canonical command.

### Format (CI mode)

Check formatting without modifying files:

```bash
incan fmt --check .
```

See also: [Formatting](formatting.md) and [CLI reference](../reference/cli_reference.md).

### Tests

Run all tests:

```bash
incan test .
```

See also: [Testing](testing.md) and [CLI reference](../reference/cli_reference.md).

### Run an incn file

Run a program and use its exit code as the CI result:

```bash
incan run path/to/main.incn
```

## Reproducible builds with locked dependencies

If your project uses `incan.toml` and has an `incan.lock` committed to version control, use `--locked` or `--frozen` in CI to ensure builds use exactly the locked dependency versions:

```bash
# Require incan.lock to exist and be up to date
incan build src/main.incn --locked
incan test --locked

# Same as --locked, plus Cargo runs in offline/frozen mode (no network)
incan build src/main.incn --frozen
```

If the lock file is missing or stale, the command fails immediately — no silent re-resolution.

**Recommended workflow**:

1. Developers run `incan lock` after changing dependencies (locally).
2. Commit both `incan.toml` and `incan.lock` to version control.
3. CI uses `--locked` to catch stale lock files.

See: [Managing dependencies](dependencies.md) for more details.

## GitHub Actions source-check example

Use the repository composite action to install an `incan` compiler binary in downstream project CI. Pin the action to an accepted commit SHA. The current action builds the compiler from source; it does not install the prepared 0.5 release Loaf envelope, so this hosted example deliberately stops at formatting and source checking.

```yaml
- name: Install Incan
  uses: encero-systems/incan/.github/actions/install-incan@<accepted-commit-sha>

- name: Show toolchain
  run: incan --version
```

The action builds the compiler from the same Incan repository ref used by the action, caches Cargo build artifacts by default, and adds the built binary directory to `PATH`. The default build profile is `release`; use `profile: debug` when faster compiler builds matter more than runtime performance during CI setup.

The action also installs the `wasm32-wasip1` Rust target by default. Downstream projects that depend on packages with vocab companions need that target during checks such as `incan fmt --check`, because the formatter can need to build or load dependency-provided vocab desugarers in a clean CI checkout. Projects that need a different target set can set the action's `targets` input to the complete list that should be installed with the toolchain.

```yaml
- name: Install Incan
  uses: encero-systems/incan/.github/actions/install-incan@<accepted-commit-sha>
  with:
    profile: debug
```

```yaml
- name: Install Incan
  uses: encero-systems/incan/.github/actions/install-incan@<accepted-commit-sha>
  with:
    targets: wasm32-wasip1,x86_64-unknown-linux-musl
```

For a hosted source-contract workflow, keep project-specific checks in the downstream repository and use the action only for compiler installation:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

env:
  INCAN_NO_BANNER: 1

jobs:
  incan:
    name: Incan project
    runs-on: ubuntu-latest

    steps:
      - name: Check out project
        uses: actions/checkout@v5

      - name: Install Incan
        uses: encero-systems/incan/.github/actions/install-incan@<accepted-commit-sha>

      - name: Show toolchain
        run: |
          incan --version
          rustc --version

      - name: Format
        run: incan fmt --check .

      - name: Check
        run: incan check src/main.incn --format json
```

Run `incan test --locked` and `incan build --locked` on a runner only after its toolchain installation also provides the finite, receipt-compatible release envelope. The 0.5 development checkout prepares that envelope with `make test-prewarm-oven-release-loafs`; the current downstream composite action does not yet perform that packaging step. See [Take a local project to a CI artifact](../tutorials/project_to_ci_artifact.md) for the verified local gate and the hosted handoff.

Use a matrix when the downstream project needs coverage on more than Linux:

```yaml
strategy:
  fail-fast: false
  matrix:
    os: [ubuntu-latest, macos-latest]

runs-on: ${{ matrix.os }}
```

The action is intentionally smaller than a reusable workflow: it installs the compiler. Formatting and source checks fit that contract today; tests, builds, and smoke tests also require the project-compatible release envelope.

```yaml
- name: Type check
  run: incan path/to/main.incn

- name: Format (CI)
  run: incan fmt --check .

- name: Type check (machine-readable)
  run: incan check src/main.incn --format json
```
