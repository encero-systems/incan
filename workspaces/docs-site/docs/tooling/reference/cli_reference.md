# CLI reference

This is the authoritative CLI reference for `incan` (commands, flags, paths, and environment variables).

--8<-- "_snippets/callouts/no_install_fallback.md"

## Usage

Top-level usage:

```text
incan [OPTIONS] [FILE] [COMMAND]
```

- If you pass a `FILE` without a subcommand, `incan` type-checks it (default action).

Commands:

- `check` - Type-check a file or project entrypoint, with optional stable JSON diagnostics
- `explain` - Explain a stable diagnostic code
- `build` - Compile to Rust and build an executable
- `inspect` - Inspect compiler artifacts such as generated Rust output
- `run` - Compile and run a program
- `fmt` - Format Incan source files
- `test` - Run tests (pytest-style)
- `new` - Create a new Incan project directory
- `init` - Add a starter `incan.toml` and project skeleton to an existing directory
- `version` - Update the project version in `incan.toml`
- `env` - List, inspect, or run configured project environments
- `lock` - Generate or update `incan.lock`
- `tools` - Inspect local toolchain, editor integration state, and checked metadata

## Semantic inspection surfaces

Incan 0.5 extends the machine-readable inspection surfaces introduced in 0.4. Use `incan check --format json` for the stable diagnostic plane, `incan build --report json` for successful build and artifact metadata, `incan inspect rust --format json` for current generated Rust output, `incan inspect codegraph --format jsonl` for source-structure graph facts, `incan inspect providers --format json` for SDK component and provider participation, and `incan inspect features --format json` for the additive package-feature graph.

These commands are intentionally not a single full semantic database. They are stable public surfaces that tools can join without scraping terminal prose, generated Rust, or source text independently. When a fact appears in more than one surface, consumers should prefer compiler-owned identity fields, source paths, schema versions, and explicit degraded-state or diagnostic records over human output.

## Global options

- `--no-banner`: suppress the ASCII logo banner when a command would otherwise show it (also via `INCAN_NO_BANNER=1`).
- `--color=auto|always|never`: control ANSI color output (respects `NO_COLOR`).

Banner policy:

- The banner is shown only for interactive `incan build` and `incan run` commands.
- Utility commands such as `new`, `init`, `version`, `env`, `lock`, `fmt`, and `test` stay quiet by default.

## Package-feature and SDK projection options

Compilation, locking, and semantic inspection commands share these Incan-owned options:

- `--features <FEATURES>`: Add comma-separated public features to the root Incan package projection.
- `--no-default-features`: Do not select the root package's `default` feature.
- `--all-features`: Select every public feature declared by the root package.
- `--sdk-profile <PROFILE>`: Replace the project's base SDK profile for this invocation while preserving explicit component additions and exclusions from `[sdk]`.

They are supported by `build`, `check`, `run`, `test`, and `lock`, plus the `inspect codegraph`, `inspect providers`, and `inspect features` projections. The package-feature flags do not forward names to Cargo. Backend pass-through remains explicitly prefixed as `--cargo-features`, `--cargo-no-default-features`, and `--cargo-all-features` where supported.

`incan test --feature <NAME>` is a separate test-runner option for `std.testing.feature("NAME")` collection probes. Use plural `--features` for public package features.

## Global options (debug)

These flags take a file and run a debug pipeline stage:

```bash
incan --lex path/to/file.incn
incan --parse path/to/file.incn
incan --check path/to/file.incn
incan --emit-rust path/to/file.incn
```

`incan --check path/to/file.incn --format json` remains supported for compatibility, but new tooling should prefer `incan check path/to/file.incn --format json`.

Strict mode:

```bash
incan --strict --emit-rust path/to/file.incn
```

## Commands

### `incan check`

Usage:

```text
incan check [OPTIONS] <PATH>
```

Type-checks a file or project entrypoint without building or running it. This is the canonical type-check command for humans, CI, editor integrations, and agents. Passing a file without a subcommand still type-checks it for compatibility, and the legacy debug spelling `incan --check <FILE>` remains available.

Options:

- `--format text|json`: Output human diagnostics or a stable machine-readable JSON report (default: `text`).
- `--features`, `--no-default-features`, `--all-features`: Select the root package-feature projection.
- `--sdk-profile <PROFILE>`: Select a non-persistent SDK profile for this check.

JSON output uses `schema_version: 1` and prints a deterministic report with `ok` and `diagnostics`. Each diagnostic includes a stable `code`, `severity`, compiler `phase` and `origin`, primary source span, message, notes, hints, labeled related spans, and an `incan explain <CODE>` hook. Diagnostics that compare two compiler-known values also include optional `expected` and `actual` fields, so consumers do not need to parse the message. The LSP and `incan inspect codegraph --allow-errors` project these same compiler-owned facts; codegraph uses byte offsets for source spans while the diagnostic JSON and LSP retain line and column positions. Human output remains the default and continues to use source-highlighted compiler diagnostics.

Examples:

```bash
incan check src/main.incn
incan check src/main.incn --format json
incan --check src/main.incn --format json
```

### `incan explain`

Usage:

```text
incan explain [OPTIONS] <CODE>
```

Explains a stable diagnostic code from `incan check --format json` or LSP diagnostics. The catalog is compiler-owned and versioned with the diagnostic schema, so tools can link users to an explanation without scraping terminal prose.

Options:

- `--format text|json`: Output a human explanation or the catalog entry as JSON (default: `text`).

Seeded catalog codes:

- `INCAN-P0001`: Syntax error.
- `INCAN-T0001`: Type checking error.
- `INCAN-I0001`: Import or module resolution error.
- `INCAN-I0101`: A known SDK provider module belongs to a component disabled by the project.
- `INCAN-I0102`: The project enabled an SDK component that is unavailable in the active installation.
- `INCAN-I0103`: A known package export requires a public feature projection that is not active.
- `INCAN-C0001`: CLI or tooling error.
- `INCAN-U0001`: Unknown diagnostic code.

Examples:

```bash
incan explain INCAN-T0001
incan explain INCAN-T0001 --format json
```

### `incan build`

Usage:

```text
incan build [OPTIONS] [FILE] [OUTPUT_DIR] [-- <CARGO_PASSTHROUGH>...]
incan build --lib [OPTIONS] [OUTPUT_DIR]
```

Behavior:

- Default mode compiles a source file into an executable.
- `--lib` builds the current project as a library. In this mode, `src/lib.incn` is required and `FILE` is optional.
- Prints the generated Rust project path (example): `target/incan/<name>/`
- Builds the generated Rust project and prints the binary path (example): `target/incan/.cargo-target/release/<name>`
- In `--lib` mode, also emits a library artifact under `target/lib/` (including `<name>.incnlib`).

Dependency flags:

- `--features`, `--no-default-features`, `--all-features`: Select public Incan package features for this build.
- `--sdk-profile <PROFILE>`: Select a non-persistent SDK profile for this build.
- `--offline`: Pass `--offline` to Cargo subprocesses so dependency resolution/fetching fails instead of using the network.
- `--locked`: Require `incan.lock` to exist and be up to date. Also passes `--locked` to Cargo.
- `--frozen`: Like `--locked`, plus passes `--frozen` to Cargo (offline + locked).
- `--no-offline`, `--no-locked`, `--no-frozen`: Disable matching environment defaults for this invocation.
- `--cargo-args <ARG>...`: Forward additional arguments to Cargo after policy and feature flags. Arguments after `--` are also treated as Cargo arguments.
- `--cargo-features <FEATURES>`: Enable specific Cargo features (comma-separated).
- `--cargo-no-default-features`: Disable default Cargo features.
- `--cargo-all-features`: Enable all Cargo features.
- `--release`: Explicitly request the release Cargo profile. This is the default for `incan build`, and the flag exists so first-contact flows can spell out that they are producing an optimized build.
- `--generated-cargo-target-dir <PATH>`: Use a shared Cargo target directory for generated Rust projects. This is useful for CI and downstream packages with expensive Rust dependencies because separate clean checkouts can reuse the same compiled dependency artifacts when the lock/profile/target inputs match.
- `--report json`: Emit a versioned machine-readable build report.
- `--report-output <PATH>`: Write the build report to a file instead of stdout.

Without `--locked` or `--frozen`, `incan build` creates `incan.lock` when it is missing. If an existing lockfile is stale, the command warns and reuses the embedded `Cargo.lock` payload without rewriting `incan.lock`; run `incan lock` to refresh the committed lockfile intentionally.

For `incan build --lib`, dependency preheat uses the generated lock workspace and the same release-profile Cargo target directory as the real generated library build. If the dependency graph is unchanged, the preheat stamp is reused; if it has to run, the command prints the target/profile domain before invoking Cargo and streams Cargo's progress while the preheat compiles. Vocab companion metadata and packaged desugarer artifacts are cached by companion input fingerprint; with `--generated-cargo-target-dir`, that cache lives alongside the shared generated Cargo target so repeated clean checkouts can reuse unchanged companion extraction and desugarer artifacts.

Build reports use `schema_version: 1` and describe the successful build rather than restating terminal prose. They include the compiler version, build mode, profile, project identity, entrypoint or library root, source-file breadcrumbs, generated Rust project paths, emitted artifacts, Rust and Incan dependency summaries, Cargo policy flags, interop summary, coarse timings, and notes. Library reports include phase timings for source loading, dependency resolution, lock payload resolution, Rust metadata prewarm, typechecking, API validation, export resolution, manifest metadata, vocab metadata, Rust generation, dependency preheat, Cargo build, and total wall time. When `--report json` writes to stdout, human progress moves to stderr so tooling can parse stdout directly; when `--report-output <PATH>` is supplied, the report is written to that file and ordinary progress remains human-facing.

Environment defaults:

- `INCAN_OFFLINE=1`, `INCAN_LOCKED=1`, and `INCAN_FROZEN=1` behave like their matching flags.
- `INCAN_FROZEN=1` implies offline and locked mode.
- CLI flags take precedence over these defaults. Use the `--no-*` forms to disable a policy default set by the environment.
- `INCAN_CARGO_ARGS="..."` is split on whitespace and used when no Cargo args were supplied on the CLI.
- `INCAN_LOCK_PREHEAT=0` disables dependency preheat for lock/test and generated-library build paths.
- `INCAN_GENERATED_CARGO_TARGET_DIR=<PATH>` is the environment form of `--generated-cargo-target-dir`; the explicit CLI flag wins when both are set.
- `INCAN_VOCAB_COMPANION_CACHE_DIR=<PATH>` overrides the vocab companion cache location when a package needs to keep that cache separate from the generated Cargo target.

Examples:

```bash
incan build examples/simple/hello.incn
incan build src/main.incn --offline
incan build src/main.incn --locked
incan build src/main.incn --frozen
incan build src/main.incn --features json,http --sdk-profile minimal
incan build src/main.incn --cargo-features fancy_logging
incan build src/main.incn -- --timings
incan build --lib
incan build --release
incan build src/main.incn --report json
incan build --lib --report json --report-output target/build-report.json
```

### `incan inspect rust`

Usage:

```text
incan inspect rust [OPTIONS] <PATH>
```

Generates the same Rust project that the current backend would build, then reports the emitted Rust files without invoking Cargo. Use this when you need to inspect backend output, debug generated paths, or hand artifact locations to tooling without treating `--emit-rust` as a structured interface.

Options:

- `--lib`: Inspect the library build surface rooted at `src/lib.incn`; `PATH` may be the project root or a source path inside the project.
- `--format text|json`: Output a human file list or a versioned JSON report (default: `text`).

JSON output uses `schema_version: 1` and includes the compiler version, mode, source-file breadcrumbs, generated project paths, emitted Rust file paths, crate-root markers, file sizes, and notes. Source declarations with checked docstrings preserve those docs as generated Rust doc comments for public emitted items when the compiler has checked API metadata available. Generated Rust is inspectable current backend output, not a stable Rust ABI contract; tools may use it for debugging and reporting, but public compatibility should be based on Incan source, manifests, checked API metadata, and documented CLI report schemas.

Examples:

```bash
incan inspect rust src/main.incn
incan inspect rust src/main.incn --format json
incan inspect rust . --lib --format json
```

### `incan inspect codegraph`

Usage:

```text
incan inspect codegraph [OPTIONS] <PATH>
```

Exports compiler-backed codegraph records for an Incan source file or directory. The export is a deterministic JSONL stream of Incan-language files, modules, top-level declarations, imports, public exports, body-level reference and call syntax, conservative resolved reference and call targets, containment relationships, source spans, provenance, degraded state, diagnostics, and the active provider/component/feature projection. It is intended for tools and agents that need Incan structure without scraping source text.

Options:

- `--format jsonl`: Emit newline-delimited JSON records. JSONL is the only supported 0.4 format.
- `--allow-errors`: Emit a degraded partial graph and diagnostic records when the source is broken. Without this flag, diagnostics fail the command.
- `--features`, `--no-default-features`, `--all-features`: Select which feature-conditioned source facts enter the graph.
- `--sdk-profile <PROFILE>`: Select the SDK profile used to resolve provider-backed facts.

`incan inspect codegraph` is tooling output, not runtime `std.graph`, not a generated-Rust ABI, and not a whole-program reference/call graph. The header lists the represented languages and has a typed `semantic_contexts` entry for each represented project. Each context records the SDK identity and profile, component availability and enablement, package feature closures and activation reasons, and provider identities, participation, artifacts, implementation facets, and authority provenance. Every fact record carries `language`, `provenance`, and `degraded` fields. Body facts with a compiler-proven `target_id` use checked provenance; syntax-only body facts keep syntax provenance. In v0.5, `target_id` points only at declaration records emitted in the same JSONL export; public-package manifest identity remains available in library manifests, but external package declarations are not emitted as codegraph targets yet. The v0.5 exporter emits `language: "incan"` only; first-class Rust graph records remain follow-up work.

Examples:

```bash
incan inspect codegraph src/main.incn --format jsonl
incan inspect codegraph src --format jsonl --allow-errors
incan inspect codegraph src --format jsonl --features json --sdk-profile minimal
```

### `incan inspect providers`

Usage:

```text
incan inspect providers [PATH] [OPTIONS]
```

Reports the active SDK identity and profile, component availability and enablement, selection reasons, separate provider availability, enablement, and use facts, compiled provider identities and provenance, canonical namespace claims, used modules, active features, private implementation facets, and provider manifest paths. `PATH` defaults to the current project.

Options:

- `--format text|json`: Emit concise human output or the schema-v1 provider report.
- `--features`, `--no-default-features`, `--all-features`: Select the package-feature projection whose providers should be inspected.
- `--sdk-profile <PROFILE>`: Select a non-persistent SDK profile for this projection.

Examples:

```bash
incan inspect providers
incan inspect providers . --format json
incan inspect providers src/main.incn --format json --sdk-profile minimal
```

### `incan inspect features`

Usage:

```text
incan inspect features [PATH] [OPTIONS]
```

Reports the resolved feature projection for every active Incan package: active features, optional dependencies, dependency-feature requests, required SDK components, activation reasons, active dependency edges, and feature-conditioned provider facts. `PATH` defaults to the current project.

Options:

- `--format text|json`: Emit concise human output or the schema-v1 feature report.
- `--features`, `--no-default-features`, `--all-features`: Select the root package projection to inspect.
- `--sdk-profile <PROFILE>`: Select the SDK profile used to validate component requirements.

Examples:

```bash
incan inspect features
incan inspect features . --format json
incan inspect features . --format json --no-default-features --features json
```

See [SDK components and package features](sdk_components_and_package_features.md) for the state and resolution model.

### `incan run`

Usage:

```text
incan run [OPTIONS] [FILE] [-- <CARGO_PASSTHROUGH>...]
```

Run a file:

```bash
incan run path/to/file.incn
```

Run the project's configured main script:

```bash
incan run
```

Run inline code:

```bash
incan run -c "import this"
```

If `FILE` is omitted, `incan run` uses `[project.scripts].main` from the nearest `incan.toml`. Outside a project, you must pass `FILE` or `-c`.

Dependency flags (same as `build`):

- `--features`, `--no-default-features`, `--all-features`, `--sdk-profile`, `--offline`, `--locked`, `--frozen`, `--no-offline`, `--no-locked`, `--no-frozen`, `--cargo-args`, `--cargo-features`, `--cargo-no-default-features`, `--cargo-all-features`
- `--release`: Build and run with Cargo's release profile.

### `incan fmt`

Usage:

```text
incan fmt [OPTIONS] [PATH]
```

Examples:

```bash
# Format files in place
incan fmt .

# Check formatting without modifying (CI mode)
incan fmt --check .

# Show what would change without modifying files
incan fmt --diff path/to/file.incn
```

### `incan test`

Usage:

```text
incan test [OPTIONS] [PATH]
```

Test runner flags:

| Flag                             | Description                                                                                              |
| -------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `-k <KEYWORD>`                   | Filter tests by stable test id substring                                                                 |
| `-m <EXPR>` / `--markers <EXPR>` | Filter tests by marker expression (`and`, `or`, `not`, parentheses)                                      |
| `-v`                             | Verbose output, including per-test timing and generated-harness preheat diagnostics                       |
| `-x`                             | Stop on first failure                                                                                    |
| `--slow`                         | Include slow tests (marked `@slow`)                                                                      |
| `--strict-markers`               | Reject unknown marker names during collection unless registered in `TEST_MARKERS`                        |
| `-j <N>` / `--jobs <N>`          | Run up to `N` generated worker batches concurrently (single-threaded libtest execution per batch)        |
| `--feature <NAME>`               | Enable collection-time `std.testing.feature("NAME")` probe for `skipif` / `xfailif`                      |
| `--timeout <DURATION>`           | Fail a test batch exceeding a duration (e.g., `250ms`, `5s`, `2m`)                                       |
| `--nocapture`                    | Print child test output even for passing tests                                                           |
| `--fail-on-empty`                | Return exit code 1 if no tests are collected                                                             |
| `--list`                         | List collected tests after filters without executing them                                                |
| `--format console\|json`         | Choose human console output or JSON Lines result output (`schema_version: "incan.test.v1"`)              |
| `--junit <PATH>`                 | Write a JUnit XML report                                                                                 |
| `--durations <N>`                | Print the slowest `N` test durations                                                                     |
| `--shuffle`                      | Shuffle test execution order                                                                             |
| `--seed <N>`                     | Seed for `--shuffle`                                                                                     |
| `--run-xfail`                    | Run `@xfail` tests as ordinary tests                                                                     |

Dependency flags (same as `build`):

- `--features`, `--no-default-features`, `--all-features`, `--sdk-profile`, `--offline`, `--locked`, `--frozen`, `--no-offline`, `--no-locked`, `--no-frozen`, `--cargo-args`, `--cargo-features`, `--cargo-no-default-features`, `--cargo-all-features`

Without `--locked` or `--frozen`, `incan test` creates `incan.lock` when it is missing. If an existing lockfile is stale, the command warns and reuses the embedded `Cargo.lock` payload without rewriting `incan.lock`; run `incan lock` to refresh the committed lockfile intentionally.

Timeouts apply to generated test batches. `--timeout` sets the default batch timeout and `@timeout("duration")` from `std.testing` can override the batch containing one test. Fixtures, including async fixtures, do not have separate per-fixture timeout configuration. The runner awaits async fixture teardown after ordinary assertion failures and panics while the worker remains alive. If timeout enforcement terminates a worker process, remaining teardown is best-effort.

Before executing a stale generated test harness, `incan test` preheats it with `cargo test --no-run` and stores a fingerprint next to the generated project. A normal console run prints a preheat line only when this work is needed and streams Cargo's progress for cold preheats; `-v` also shows generated-harness planning, preparation-cache hits or misses, preheat timing, Cargo target directory, and Cargo test phase timing. Lock generation also preheats non-trivial dependency graphs into the same generated test target domain before writing `incan.lock`, so the next harness run can reuse dependency artifacts instead of discovering a cold Cargo graph on the hot path. Rust metadata prewarm reports newly warmed, reused, and skipped items; with `INCAN_RUST_INSPECT_TIMING=1`, compiler timing logs also explain whether metadata came from process state, disk cache, or a cache miss such as an absent cache file, format change, or workspace fingerprint change.

Examples:

```bash
# Run all tests in a directory
incan test tests/

# Run all tests under a path (default: .)
incan test .

# Filter tests by keyword expression
incan test -k "addition"

# List matching tests without running them
incan test --list -k "test_math"

# Verbose output (include timing)
incan test -v

# Stop on first failure
incan test -x

# Include slow tests
incan test --slow

# Select marker-tagged tests
incan test -m "smoke and not slow" tests/

# Validate marker names in CI
incan test --strict-markers tests/

# Enforce a default timeout
incan test --timeout 5s tests/

# Show passing-test output
incan test --nocapture tests/

# Fail if no tests are collected
incan test --fail-on-empty

# Emit JSON Lines and a JUnit report for CI
incan test --format json --junit reports/junit.xml tests/

# Reproduce a shuffled run
incan test --shuffle --seed 12345 tests/

# Strict mode for CI
incan test --frozen

# Test one package-feature projection against a smaller SDK profile
incan test tests/ --no-default-features --features json --sdk-profile minimal
```

### `incan new`

Usage:

```text
incan new [OPTIONS] [NAME]
```

Creates a new project directory with `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. The starter source includes a small public `greeting()` function plus a test that imports and checks it, so `incan run`, `incan test`, and `incan build --release` work immediately after project creation. When run in an interactive terminal without `--yes`, it prompts for project metadata. In non-interactive contexts, pass `NAME` or `--dir`.

Options:

- `NAME`: Project name. If omitted in an interactive terminal, `incan new` prompts for it.
- `--dir <PATH>`: Directory to create or reuse. Defaults to `./<name>`.
- `--description <TEXT>`: Project description written to `[project].description` and `README.md`.
- `--author <AUTHOR>`: Author string, usually `Name <email>`, written to `[project].authors`.
- `--license <LICENSE>`: License identifier or expression written to `[project].license`.
- `--force`: Reuse a non-empty directory and overwrite existing manifest/source/test scaffold files.
- `-y`, `--yes`: Use defaults and provided flags without interactive prompts.

Examples:

```bash
# Interactive metadata prompts
incan new

# Script-friendly project creation
incan new greeter --description "A small greeting command" --license MIT --yes

# Create the project in a different directory from its name
incan new greeter --dir examples/greeter --yes
```

### `incan init`

Usage:

```text
incan init [OPTIONS] [PATH]
```

Adds Incan project files to an existing directory. Use this when you already have a directory and want to add `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. New projects usually start with `incan new` instead.

Options:

- `--name <NAME>`: Project name (default: directory name).
- `--version <VERSION>`: Project version (default: `"0.1.0"`).
- `--description <TEXT>`: Project description written to `[project].description` and `README.md`.
- `--author <AUTHOR>`: Author string, usually `Name <email>`, written to `[project].authors`.
- `--license <LICENSE>`: License identifier or expression written to `[project].license`.
- `--force`: Overwrite existing manifest/source/test scaffold files.
- `--detect`: Preserve an existing `src/main.incn` and, when the placeholder project name is still in use, derive the project name from the directory.
- `-y`, `--yes`: Use defaults and provided flags without interactive prompts.

Examples:

```bash
incan init
incan init --name my_app --description "My app" --license MIT my_project/
incan init --detect --yes
```

See: [Project configuration reference](project_configuration.md) for the full manifest format.

### `incan version`

Usage:

```text
incan version [OPTIONS] [BUMP]
```

Updates `[project].version` in `incan.toml`. `BUMP` is one of `major`, `minor`, `patch`, `alpha`, `beta`, `rc`, or `dev`. Use `--set` when you need an exact SemVer value instead of a bump.

Options:

- `BUMP`: Version bump to apply.
- `--set <VERSION>`: Explicit SemVer version to write.
- `--dry-run`: Print the planned change without writing `incan.toml`.
- `--keep-prerelease`: Keep prerelease metadata when applying `major`, `minor`, or `patch`.
- `--project <PATH>`: Project root containing `incan.toml`.

Examples:

```bash
incan version patch
incan version rc --dry-run
incan version --set 1.2.3
incan version minor --project examples/greeter
```

### `incan env`

Project environments are declared in `[tool.incan.envs]` in `incan.toml`. The ambient `default` environment is always available, and the `env` command lists available environments, shows a Hatch-style overview table, prints a compact resolved summary for one environment, or runs a named script from an environment.

Treat envs as named command contexts for repeatable workflows such as local testing, CI, docs, or release checks. They are not shell sessions or virtual environments.

Usage:

```text
incan env list [OPTIONS]
incan env show [OPTIONS] [ENV]
incan env run [OPTIONS] <ENV> <SCRIPT> [-- <ARGS>...]
```

Shared options:

- `--format text|json`: Output format for `list` and `show` (default: `text`).
- `--project <PATH>`: Project root containing `incan.toml`.

Run options:

- `--dry-run`: Print the resolved command without executing it.
- `-- <ARGS>...`: Extra arguments appended to the configured script.

Examples:

```bash
incan env list
incan env show
incan env show default
incan env show dev --format json
incan env run dev test
incan env run dev test -- --fail-on-empty
incan env run release build --dry-run
```

For a fuller explanation of the mental model and a realistic `default` / `unit` / `ci` / `docs` configuration, see: [Project lifecycle](../../language/how-to/project_lifecycle.md).

### `incan lock`

Usage:

```text
incan lock [OPTIONS] [FILE]
```

Resolves all dependencies (manifest + inline + test files) and generates or updates `incan.lock`.

If `FILE` is omitted, uses the `[project.scripts].main` entry from `incan.toml`.

Options:

- `--features <FEATURES>`: Add public Incan features to the root package's locked projection.
- `--no-default-features`: Do not select the root package's `default` feature.
- `--all-features`: Select every public feature declared by the root package.
- `--sdk-profile <PROFILE>`: Replace the base SDK profile for the locked projection while preserving manifest additions and exclusions.
- `--cargo-features <FEATURES>`: Enable specific Cargo features for resolution.
- `--cargo-no-default-features`: Disable default Cargo features.
- `--cargo-all-features`: Enable all Cargo features.

Example:

```bash
incan lock src/main.incn
incan lock                          # uses [project.scripts].main
incan lock --features metrics       # select public Incan package features
incan lock --sdk-profile minimal    # lock the minimal SDK profile projection
incan lock --cargo-features metrics # select explicit backend Cargo features
```

The generated `incan.lock` contains an embedded `Cargo.lock` payload, the expanded SDK component and provider identities, the public package-feature graph and activation reasons, private implementation-facet closure, and a fingerprint of dependency inputs. Commit it to version control for reproducible builds.

See: [Managing dependencies](../how-to/dependencies.md) for practical guidance.

### `incan tools doctor`

Usage:

```text
incan tools doctor [OPTIONS]
```

Inspects local CLI/LSP/editor pathing and offline-readiness signals. Use this when the terminal and editor appear to be using different `incan` or `incan-lsp` binaries, when diagnostics look stale after rebuilding, or before a restricted/offline build where Cargo may be unable to fetch missing dependency inputs.

Options:

- `--format text|json`: Output format (default: `text`).

The report includes:

- the running `incan` version and executable path
- `incan` and `incan-lsp` resolution from `PATH`
- `~/.cargo/bin/incan` and `~/.cargo/bin/incan-lsp` existence, executable status, and symlink targets
- editor setup guidance for `incan.lsp.path`, `incan.compiler.path`, and reload behavior

Offline-readiness:

- The doctor report is the supported preflight path for restricted or offline environments.
- The offline-readiness section is advisory. It checks local signals that affect whether Cargo can satisfy `--frozen` policy without fetching, but it does not guarantee a later `incan build --frozen` or `incan test --frozen` will succeed.
- Offline/locked policy constrains dependency resolution and fetching. Projects may still depend on Rust crates; those crates must already be available through Cargo's local inputs for an offline build to proceed.
- Use `--format json` when an editor, CI preflight, or issue template needs the same information in a machine-readable form.

Examples:

```bash
incan tools doctor
incan tools doctor --format json
```

### `incan tools metadata api`

Usage:

```text
incan tools metadata api [PATH] [OPTIONS]
```

Emits checked public API metadata for an Incan source file or project directory. The command parses and type-checks the target before producing output, so the result describes the checked API surface rather than source text alone.

If `PATH` is omitted, the current directory is inspected. If `PATH` is a directory, `src/lib.incn` is preferred and `src/main.incn` is used as a fallback.

This command is source/project inspection, not artifact inspection. It does not build the project, emit generated Rust, or read an existing `.incnlib`; use `incan build --lib` for library artifact emission.

Options:

- `--format json`: Output checked API metadata JSON (default).
- `--format markdown`: Output a generated Markdown API reference derived from the same checked metadata.

The JSON package contains:

- `schema_version`: numeric schema version for the package payload
- `package`: project name and version from `incan.toml`, when available
- `modules`: checked metadata documents for the entry module and imported local modules
- `declarations`: public functions, models, classes, traits, enums, newtypes, type aliases, consts, statics, public import aliases, and public partial callable presets
- `anchor`: stable declaration ids plus source byte spans
- `docstring`: raw declaration or method docstring text when present
- `docstring_sections`: parsed summary, parameter, return, field, alias, and decorator sections when a docstring is present
- `decorators`: resolved decorator paths and safe literal, type, or const-reference arguments

Docstring validation is strict for mechanically checkable drift. If `Args:`, `Returns:`, `Fields:`, `Aliases:`, or `Decorators:` contradict checked source structure, the command reports diagnostics and does not print JSON.

Examples:

```bash
incan tools metadata api
incan tools metadata api src/lib.incn --format json
incan tools metadata api src/lib.incn --format markdown
incan tools metadata api path/to/project
```

See: [Checked API metadata](checked_api_metadata.md) for the JSON contract.

### `incan tools metadata model`

Usage:

```text
incan tools metadata model PATH MODEL [OPTIONS]
```

Emits one contract-backed model from project-declared bundle metadata, a bundle JSON file, or a built `.incnlib` artifact. `MODEL` may be the bundle `logical_type_name` or `stable_model_id`.

Options:

- `--format incan`: Output formatted Incan `model` source (default).
- `--format json`: Output the selected canonical model bundle JSON.

Examples:

```bash
incan tools metadata model . OrderSummary --format incan
incan tools metadata model contracts/order_summary.json orders.summary --format json
incan tools metadata model target/lib/orders.incnlib OrderSummary
```

See: [Checked contract metadata](contract_metadata.md) for bundle schema, materialization, artifact inspection, and the matching LSP command.

## Outputs and paths

Build outputs:

- **Generated Rust project**: `target/incan/<name>/`
- **Built binary**: `target/incan/.cargo-target/release/<name>`
- **Built library artifact (`--lib`)**: `target/lib/<name>.incnlib` plus the generated library crate output

Cleaning:

```bash
rm -rf target/incan/
```

## Environment variables

- **`INCAN_STDLIB`**: override the stdlib directory (usually auto-detected; set only if detection fails).
- **`INCAN_FANCY_ERRORS`**: enable “fancy” diagnostics rendering (presence-based; output may change).
- **`INCAN_EMIT_SERVICE=1`**: toggle codegen emit mode (internal/debug; not stable).
- **`INCAN_NO_BANNER=1`**: disable the ASCII logo banner.
- **`NO_COLOR`**: disable ANSI color output (standard convention).

## Exit codes

General rule: success is exit code 0; errors are non-zero.

Specific behavior:

- **`incan run`**: returns the program’s exit code.
- **`incan test`**:
    - returns 0 if all tests pass
    - returns 0 if test files exist but no tests are collected
    - returns 1 if `--fail-on-empty` is set and no tests are collected
    - returns 1 if no test files are discovered under the provided path
    - returns 1 if any tests fail or an xfail unexpectedly passes (XPASS)
- **`incan fmt --check`**: returns 1 if any files would be reformatted.
- **`incan build` / `incan check` / `incan --check` / debug flags**: return 1 on compile/build errors.

## Drift prevention (maintainers)

Before a release, verify the docs stay aligned with the real CLI surface:

- Compare `incan --help` and `incan {check,explain,build,run,fmt,test,init,inspect,lock,tools} --help` against this page.
