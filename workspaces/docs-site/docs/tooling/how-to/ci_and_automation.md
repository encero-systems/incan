# CI & automation (projects / CLI-first)

This page collects the canonical, CI-friendly commands for **Incan projects** (using the `incan` CLI).

If you’re running CI for the **Incan compiler/tooling repository**, see: [CI & automation (repository)](../../contributing/how-to/ci_and_automation.md).

## Recommended commands

### Type check (fast gate)

Type-check a program without building/running it (default action when no subcommand is provided):

```bash
incan path/to/main.incn
```

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

If your project uses `incan.toml` and has an `incan.lock` committed to version control, use `--locked` or `--frozen` in
CI to ensure builds use exactly the locked dependency versions:

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

## GitHub Actions example

```yaml
- name: Type check
  run: incan path/to/main.incn

- name: Format (CI)
  run: incan fmt --check .

- name: Tests (locked)
  run: incan test --locked

- name: Build (locked)
  run: incan build src/main.incn --locked
```

## Using the reusable Incan composite action

For projects that need to build with the Incan compiler (e.g., library packages), use the **reusable composite action** from the Incan repository. This action:

- Downloads a pre-built `incan` binary from cache if available
- Falls back to building from source if not cached
- Caches the built binary for faster subsequent runs

### Basic usage

```yaml
jobs:
  my-job:
    runs-on: ubuntu-latest
    steps:
      - name: Check out your project
        uses: actions/checkout@v4

      - name: Install Incan (cached)
        uses: dannys-code-corner/incan/.github/actions/install-incan@main
        with:
          incan-ref: main  # or specific SHA, tag, or branch
          incan-repo: dannys-code-corner/incan
          runner-os: ${{ runner.os }}
```

### With matrix strategy

```yaml
jobs:
  my-job:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest]
    steps:
      - name: Check out your project
        uses: actions/checkout@v4

      - name: Install Incan (cached)
        uses: dannys-code-corner/incan/.github/actions/install-incan@main
        with:
          incan-ref: main
          incan-repo: dannys-code-corner/incan
          runner-os: ${{ matrix.os }}

      - name: Build your project
        run: make build
```

### With specific version

```yaml
jobs:
  my-job:
    runs-on: ubuntu-latest
    steps:
      - name: Check out your project
        uses: actions/checkout@v4

      - name: Install Incan (cached)
        uses: dannys-code-corner/incan/.github/actions/install-incan@main
        with:
          incan-ref: v0.2-dev  # or specific SHA, tag, or branch
          incan-repo: dannys-code-corner/incan
          runner-os: ${{ runner.os }}
```

### Reusable workflow (job-level)

The Incan repository also publishes a **reusable workflow** at `.github/workflows/install-incan.yml`. Reusable workflows are referenced with **`jobs.<job_id>.uses`**, not as a step inside `steps:`.

That workflow runs a single **Ubuntu** job, builds (or restores) the compiler, and exposes `cache-key` and `incan-path` outputs. Other jobs in the caller workflow do **not** automatically get `incan` on `PATH`; for each runner that needs the compiler, prefer the **composite action** in this section (or wire artifacts/cache yourself). See the workflow file header comments in the Incan repo for a minimal `workflow_call` example.

### How caching works

- **Cached paths**: The release `incan` binary at `incan/target/release/incan` (default `cargo build --release` with package default features, which include the CLI).
- **Cache key**: `incan-bin-<ref>-<os>` (e.g., `incan-bin-main-ubuntu-latest`)
- **First run**: Builds from source (~5 minutes) and caches the binary
- **Subsequent runs**: Downloads cached binary (~5 seconds)
- **Different refs**: Each ref gets its own cache entry
- **Different OSes**: Each OS gets its own cache entry

### Cache management

To force a rebuild (e.g., after pushing changes to a ref):

1. Go to your repository's **Actions** tab
2. Click **Caches** in the left sidebar
3. Find and delete the cache entry for `incan-bin-<ref>-<os>`

Or use a different ref (tag/branch) for the new version to get a fresh cache.
