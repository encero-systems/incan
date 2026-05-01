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

- `build` - Compile to Rust and build an executable
- `run` - Compile and run a program
- `fmt` - Format Incan source files
- `test` - Run tests (pytest-style)
- `new` - Create a new Incan project directory
- `init` - Add a starter `incan.toml` and project skeleton to an existing directory
- `version` - Update the project version in `incan.toml`
- `env` - List, inspect, or run configured project environments
- `lock` - Generate or update `incan.lock`
- `tools` - Inspect local toolchain, editor integration state, and checked metadata

## Global options

- `--no-banner`: suppress the ASCII logo banner when a command would otherwise show it (also via `INCAN_NO_BANNER=1`).
- `--color=auto|always|never`: control ANSI color output (respects `NO_COLOR`).

Banner policy:

- The banner is shown only for interactive `incan build` and `incan run` commands.
- Utility commands such as `new`, `init`, `version`, `env`, `lock`, `fmt`, and `test` stay quiet by default.

## Global options (debug)

These flags take a file and run a debug pipeline stage:

```bash
incan --lex path/to/file.incn
incan --parse path/to/file.incn
incan --check path/to/file.incn
incan --emit-rust path/to/file.incn
```

Strict mode:

```bash
incan --strict --emit-rust path/to/file.incn
```

## Commands

### `incan build`

Usage:

```text
incan build [OPTIONS] <FILE> [OUTPUT_DIR]
incan build --lib [OPTIONS] [OUTPUT_DIR]
```

Behavior:

- Default mode compiles a source file into an executable.
- `--lib` builds the current project as a library. In this mode, `src/lib.incn` is required and `FILE` is optional.
- Prints the generated Rust project path (example): `target/incan/<name>/`
- Builds the generated Rust project and prints the binary path (example):
  `target/incan/.cargo-target/release/<name>`
- In `--lib` mode, also emits a library artifact under `target/lib/` (including `<name>.incnlib`).

Dependency flags:

- `--locked`: Require `incan.lock` to exist and be up to date. Also passes `--locked` to Cargo.
- `--frozen`: Like `--locked`, plus passes `--frozen` to Cargo (offline + locked).
- `--cargo-features <FEATURES>`: Enable specific Cargo features (comma-separated).
- `--cargo-no-default-features`: Disable default Cargo features.
- `--cargo-all-features`: Enable all Cargo features.

Examples:

```bash
incan build examples/simple/hello.incn
incan build src/main.incn --locked
incan build src/main.incn --cargo-features fancy_logging
incan build --lib
```

### `incan run`

Usage:

```text
incan run [OPTIONS] [FILE]
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

- `--locked`, `--frozen`, `--cargo-features`, `--cargo-no-default-features`, `--cargo-all-features`

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
| `-v`                             | Verbose output (include timing)                                                                          |
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

- `--locked`, `--frozen`, `--cargo-features`, `--cargo-no-default-features`, `--cargo-all-features`

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
incan test --locked
```

### `incan new`

Usage:

```text
incan new [OPTIONS] [NAME]
```

Creates a new project directory with `incan.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md`, and `.gitignore`. When run in an interactive terminal without `--yes`, it prompts for project metadata. In non-interactive contexts, pass `NAME` or `--dir`.

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

- `--cargo-features <FEATURES>`: Enable specific Cargo features for resolution.
- `--cargo-no-default-features`: Disable default Cargo features.
- `--cargo-all-features`: Enable all Cargo features.

Example:

```bash
incan lock src/main.incn
incan lock                          # uses [project.scripts].main
incan lock --cargo-features metrics # include optional deps in lock
```

The generated `incan.lock` contains an embedded `Cargo.lock` payload and a fingerprint of your dependency
inputs. Commit it to version control for reproducible builds.

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

Emits checked public API metadata for an Incan source file or project directory. The command parses and type-checks the target before producing JSON, so the output describes the checked API surface rather than source text alone.

If `PATH` is omitted, the current directory is inspected. If `PATH` is a directory, `src/lib.incn` is preferred and `src/main.incn` is used as a fallback.

This command is source/project inspection, not artifact inspection. It does not build the project, emit generated Rust, or read an existing `.incnlib`; use `incan build --lib` for library artifact emission.

Options:

- `--format json`: Output checked API metadata JSON (default).

The JSON package contains:

- `schema_version`: numeric schema version for the package payload
- `package`: project name and version from `incan.toml`, when available
- `modules`: checked metadata documents for the entry module and imported local modules
- `declarations`: public functions, models, classes, traits, enums, newtypes, type aliases, consts, statics, and public import aliases
- `anchor`: stable declaration ids plus source byte spans
- `docstring`: raw declaration or method docstring text when present
- `docstring_sections`: parsed summary, parameter, return, field, alias, and decorator sections when a docstring is present
- `decorators`: resolved decorator paths and safe literal, type, or const-reference arguments

Docstring validation is strict for mechanically checkable drift. If `Args:`, `Returns:`, `Fields:`, `Aliases:`, or `Decorators:` contradict checked source structure, the command reports diagnostics and does not print JSON.

Examples:

```bash
incan tools metadata api
incan tools metadata api src/lib.incn --format json
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
- **`incan build` / `incan --check` / debug flags**: return 1 on compile/build errors.

## Drift prevention (maintainers)

Before a release, verify the docs stay aligned with the real CLI surface:

- Compare `incan --help` and `incan {build,run,fmt,test,init,lock,tools} --help` against this page.
