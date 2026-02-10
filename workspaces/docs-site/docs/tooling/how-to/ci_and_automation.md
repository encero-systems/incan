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
