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

If your project uses `loaf.toml` and has an `oven.lock` committed to version control, use `--locked` or `--frozen` in CI to ensure builds use exactly the locked dependency versions:

```bash
# Require oven.lock to exist and be up to date
incan build src/main.incn --locked
incan test --locked

# Same as --locked, plus Cargo runs in offline/frozen mode (no network)
incan build src/main.incn --frozen
```

If the lock file is missing or stale, the command fails immediately — no silent re-resolution.

**Recommended workflow**:

1. Developers run `incan lock` after changing dependencies (locally).
2. Commit both `loaf.toml` and `oven.lock` to version control.
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

## Read compiler reports from another tool

Documentation generators, package browsers, editor integrations, CI jobs, architecture review tools and agents can read what the compiler knows about a project without scraping `.incn` text, terminal output or generated Rust. The [inspection reports](../reference/cli_reference.md#inspection-reports) are separate, stable outputs rather than one semantic database; each answers one question:

- `incan check --format json` for diagnostics;
- `incan build --report json` for build and artifact metadata;
- `incan inspect codegraph --format jsonl` for the structure of the source, with compiler-owned provenance;
- `incan inspect registry` for one complete checked registry;
- `incan inspect rust --format json` for the generated Rust files, which carry no graph records;
- `incan tools metadata api --format json` for the checked public API.

Join them on the compiler-owned fields they share: identity fields, source paths, compiler version, project identity, and the explicit degraded state and diagnostic records. Check each report's `schema_version` before reading it, and read the structured fields rather than the human-readable output.

### Read diagnostics

Use `incan check` as the type-check command in CI, editor integrations and agents. `incan FILE` and the debug spelling `incan --check FILE --format json` still work, but new tooling should call `incan check FILE --format json`.

`diagnostics` holds warnings as well as errors, and a check with only warnings reports `ok: true`. Filter on `severity` to separate them. When a diagnostic compares two values, read `expected` and `actual` instead of parsing the message, and send people to the command in `explain` (`incan explain <CODE>`) for the longer explanation.

### Consume the codegraph

Run `incan inspect codegraph` without `--allow-errors` in release gates that need a fully checked graph. Add `--allow-errors` for work-in-progress packages and agent context, where a partial graph marked `degraded` is more useful than none.

When you consume the stream:

- treat a record kind you do not know as an opaque record rather than failing;
- store the header's `schema_version` and `compiler_version` with any index you persist, since record ids can change with file moves, renames and schema versions;
- anchor into a source buffer with a span's byte offsets, and show its line and column values rather than recomputing them from bytes under an assumed encoding;
- read a schema-7 export's `stable_identity` by its `nested: bool` field, and a schema-8 export's by its `location`: branch on the header's `schema_version`, since a schema-7 stable identity is not valid under schema 8;
- read the `origin` of a schema-1 diagnostic record, which has none, as `unknown`.

Programs that need graph values at run time use `std.graph`; runtime code does not depend on `incan inspect codegraph`.

`examples/pro/codegraph_importer` is a runnable Incan consumer of the stream. From that directory, export a graph and import it:

```bash
incan inspect codegraph ../../simple/hello.incn --format jsonl > codegraph.jsonl
incan run src/main.incn
```

It accepts schema versions 1 through 8, counts the record kinds it knows, keeps unknown kinds as opaque records, and prints a deterministic JSON summary. It does not parse `.incn`, resolve names, infer missing edges or store graph data. Build your own importer the same way: validate, persist, compare or visualize the compiler's records, but do not become their semantic authority, and add support for a later schema version deliberately rather than accepting a changed contract silently.

### Consume checked API metadata

`incan tools metadata api --format json` prints the checked public API; editors show the same facts through `textDocument/hover`. When you build on it:

- read a decorator's `decorated_callable` for the decorated function's identity and signature, rather than asking authors to repeat them in decorator arguments;
- let a re-export-only facade module publish its functions through the `projected_function` of its public aliases; it needs no loader function or runtime initialization hook;
- show a partial's presets as the parameters with `has_default: true`, and use `presets` to explain where a default came from;
- expect schema-1 metadata to carry no `is_mut` parameter flag and no `MutParam` type;
- expect metadata that predates `NativeUnion` to encode unions as `Applied` types named `Union`; a reader that does not know `NativeUnion` refuses it rather than reading it as an ordinary union;
- treat docstrings and decorator payloads as data, never as trusted executable input.

Use `incan build --lib` to produce the library artifact, and `incan tools metadata model` to inspect a model bundle.

### Map generated Rust names back to source

In generated Rust, each linker-visible Incan declaration has a reversible `incan-v1` identifier. To show a source name, decode the identifier's payload rather than reading the identifier literally.
