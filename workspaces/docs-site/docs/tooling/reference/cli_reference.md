# CLI reference

`incan` is Oven, the Incan command line. This page specifies `0.6.0-dev.8`.

## Usage

```text
incan --help | -h | help
incan --version
incan run PATH [UNIT OPTIONS] [--argv0 TEXT] [--program-entry PATH]... [-- ARGS...]
incan check PATH [--format text|json] [UNIT OPTIONS]
incan test PATH [UNIT OPTIONS]
incan describe-unit PATH [UNIT OPTIONS]
incan build PATH [--substitute NAME=DIR]... [--program-entry PATH]... [UNIT OPTIONS]
incan fmt [--check] PATH...
incan owner stop
incan inspect codegraph PATH [GRAPH OPTIONS]
incan mcp PATH
incan toolchain pack OUT
incan toolchain install ARCHIVE
```

There is no implicit command or implicit source path. `new`, `init`, `lock`, `env`, `explain`, `tools`, `cache`, `workspace`, `oven`, `--release`, `--locked`, `--offline`, `--frozen`, `--sdk-profile` and `--emit-rust` are not CLI interfaces in this release.

## Help and version

`incan --help`, `incan -h` and `incan help` print the command synopses above and the CLI reference location, then exit `0`. `incan COMMAND --help` or `incan COMMAND -h` prints that command's usage, options and one example, then exits `0`. Nested commands include the subcommand: `incan owner stop --help`, `incan inspect codegraph --help`, `incan toolchain pack --help` and `incan toolchain install --help`. Help does not require a project or installed toolchain. Arguments after `--` and option values are not interpreted as help flags.

`incan --version` prints `incan 0.6.0-dev.8` and exits `0`. `incan --version --help` prints its usage and example.

## Unit options

| Option | Value and default | Contract |
| --- | --- | --- |
| `--profile NAME` | String, `debug` | Selects the artifact profile. Standard packages must have artifacts for that profile. |
| `--features A,B` | Comma-separated names, empty | Adds public root-package features. |
| `--no-default-features` | Boolean, false | Disables selection of the root package's `default` feature. |
| `--all-features` | Boolean, false | Selects every declared root-package feature. |

Unknown features, invalid feature edges and unknown options fail the command. Unit commands accept one path. Package features are specified in [Project configuration](project_configuration.md#projectfeatures).

## `incan run`

`PATH` names an Incan entry source. Arguments after `--` are passed to the program. `--argv0 TEXT` sets the program's argument zero. Repeatable `--program-entry PATH` adds program entries. The compiled program inherits stdin, stdout and stderr; its exit status is the command's exit status.

## `incan check`

Checks the source at `PATH` without producing an executable. `--format` accepts `text` (default) or `json`. Successful checks exit zero; errors exit nonzero.

The JSON report has `schema_version: 2`, Boolean `ok`, and a `diagnostics` array. Diagnostic entries carry `code`, `severity`, `phase`, `origin`, `message`, `primary_span`, `notes` and `hints`, with additional related or comparison fields when applicable. Message wording is not stable. A span carries its source file and positions, including byte offsets. An `explain` string in a report does not provide an `incan explain` command in this CLI.

## `incan test`

`PATH` names a source file containing test functions. The command compiles the collected tests and executes them on every invocation. Successful results print `FILE::TEST PASSED`. A compilation error or failing test returns nonzero. Directory collection, `-k`, `--jobs`, `--list`, JSON and JUnit output flags are not accepted by this command.

Collected test functions take no arguments and return `None` or `Result[None, E]`. A result-returning test passes with `Ok(None)` and fails with `Err(error)`; `E` must satisfy the `Debug` trait bound.

## `incan build`

An explicit source file selects that unit. For a directory, `src/lib.incn` selects a library; otherwise `src/main.incn` selects a program. Other project directories select the Rust facet build.

A library source named `lib.incn` produces an rlib and checked library sidecars. Path dependency artifacts reside under `target/incan/loaves/<package>/<profile>/`. Program executables reside under `target/incan/<profile>/bin/<crate>`. `--substitute NAME=DIR` supplies local Rust dependency substitutions. `--program-entry` is accepted for programs. `--argv0` is refused by builds. `--lib` and `--report` are not accepted.

## `incan describe-unit`

Writes the selected source's unit description as JSON with `schema: "incan.unit/1"`. Fields include `source`, `kind`, `program_entries`, `project_root`, `source_root`, `package`, `version`, `profile`, `declared_features`, `active_features`, `loaves`, `rust_crates`, `contract_models`, `c_headers`, `externs`, `search` and `rust_abi`. Each Loaf entry has `key`, `name`, `dir` and `standard`. Execution-only options are refused.

## `incan fmt`

Formats one or more files or directory trees. `--check` leaves files unchanged and exits `1` when formatting would change a file. `--` ends option parsing. Only `.incn` files are selected; hidden directories, `target` and `node_modules` are skipped. No discovered source files is an error. Files that cannot be formatted are reported; other selected files are still processed. The [code style](../../language/reference/code_style.md) specifies formatting rules.

## `incan owner stop`

Stops resident project owners belonging to the selected Incan home. `OVEN_OWNER=off` disables owner use for run, build and test. Executables may still be reused with owners disabled. Tests execute afresh even when their executable is reused.

## `incan inspect codegraph`

Graph options, schemas, queries, context packs and snapshot checks are specified in [Agent context CLI and MCP](agent_context_cli.md). `--strict` refuses a graph containing unchecked units. The default returns available facts and warns about unchecked units.

## `incan mcp`

Serves the selected project's graph through newline-delimited JSON-RPC on stdin/stdout. Closing stdin ends the server. Protocol methods, tools, resources and errors are specified in the [MCP reference](agent_context_cli.md#mcp-transport).

## `incan toolchain pack` and `install`

`pack OUT` archives the installed toolchain closure. Supported extensions are `.tar`, `.tar.gz` and `.tar.zst`. Paths in the packed toolchain manifest are relative; the local Rust sysroot is excluded.

`install ARCHIVE` stages and verifies the archive before publishing it under `INCAN_HOME/toolchains/0.6.0-dev.8/`. Absolute archive members, parent traversal and archive-supplied `sysroot` members are refused. Manifest paths must be portable, the toolchain name must match this Oven, and named artifacts must exist. Installation binds a local Rust sysroot and verifies the driver's compiler identity. A refused archive preserves the existing installation.

## Environment variables

| Name | Contract |
| --- | --- |
| `INCAN_HOME` | Toolchain, cache and owner home. Default: `.incan` under `HOME`, or `USERPROFILE` when `HOME` is absent. |
| `INCAN_STDLIB` | Required directory containing standard-package source directories with `loaf.toml`. Compiled artifacts belong to the installed toolchain and selected profile. |
| `INCAN_SYSROOT` | Explicit local Rust sysroot for installation. Otherwise installation probes Rustup `1.98.0`, then `rustc`. Identity verification is required. |
| `OVEN_OWNER` | `off` disables resident owner use. |
| `OVEN_TRACE` | `-` writes invocation traces to stderr; another nonempty value names a trace output file. |

## Exit codes

`0` indicates success, including help. Unknown commands and unknown options exit `2`; the error is followed by ``Run `incan --help` for usage.`` Invalid unit-command syntax also exits `2`. Other argument-value refusals, preparation and compilation failures return nonzero. `run` returns the program's status. `test` returns nonzero when any collected test fails.
