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
- `init` - Create a starter `incan.toml` and project skeleton (entry point, test file)
- `lock` - Generate or update `incan.lock`

## Global options

- `--no-banner`: suppress the ASCII logo banner (also via `INCAN_NO_BANNER=1`).
- `--color=auto|always|never`: control ANSI color output (respects `NO_COLOR`).

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

Run inline code:

```bash
incan run -c "import this"
```

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

- `-k <KEYWORD>`: Filter tests by keyword expression.
- `-v`: Verbose output (include timing).
- `-x`: Stop on first failure.
- `--slow`: Include slow tests (marked `@slow`).
- `--fail-on-empty`: Return exit code 1 if no tests are collected.

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

# Verbose output (include timing)
incan test -v

# Stop on first failure
incan test -x

# Include slow tests
incan test --slow

# Fail if no tests are collected
incan test --fail-on-empty

# Strict mode for CI
incan test --locked
```

### `incan init`

Usage:

```text
incan init [OPTIONS] [PATH]
```

Creates a starter `incan.toml` in the specified directory (default: current directory).

Options:

- `--name <NAME>`: Project name (default: directory name).
- `--version <VERSION>`: Project version (default: `"0.1.0"`).

Example:

```bash
incan init
incan init --name my_app my_project/
```

See: [Project configuration reference](project_configuration.md) for the full manifest format.

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

- Compare `incan --help` and `incan {build,run,fmt,test,init,lock} --help` against this page.
