# CLI reference

`incan` is Oven, the Incan command line.

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
incan bakery gc [--keep-days N] | verify | repair-payload SHA256 FILE
incan inspect codegraph PATH [GRAPH OPTIONS]
incan inspect run ID --proofs
incan mcp PATH
incan toolchain pack OUT
incan toolchain install ARCHIVE
```

There is no implicit command or implicit source path.

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

Unknown features, invalid feature edges and unknown options fail the command. Feature-selection options require an owning project. Unsupported command-option combinations exit `2` before dependency preparation. Unit commands accept one path. Package features are specified in [Project configuration](project_configuration.md#projectfeatures).

## `incan run`

`PATH` names an Incan entry source. Arguments after `--` are passed to the program. `--argv0 TEXT` sets the program's argument zero. Repeatable `--program-entry PATH` adds program entries. The compiled program inherits stdin, stdout and stderr; its exit status is the command's exit status.

With `INCAN_ENGINE=records`, `PATH` must name one ordinary program source file, the profile must be `debug` or `release`, and editable path dependencies, vocabulary entries, foreign headers, feature-selection flags, substitutions and additional program entries are refused. Build and run select the same compiled unit. The program runs only after successful capture and native selection. `--engine-report` retains a proof report and prints one JSON line before any program output. Without that flag, proof reporting adds no console output. The flag is refused in legacy mode.

## `incan check`

Checks the source at `PATH` without producing an executable. `--format` accepts `text` (default) or `json`. Successful checks exit zero; errors exit nonzero.

The JSON report has `schema_version: 2`, Boolean `ok`, and a `diagnostics` array. Diagnostic entries carry `code`, `severity`, `phase`, `origin`, `message`, `primary_span`, `notes`, `hints` and `explain`, with additional related or comparison fields when applicable. `explain` is always the string `incan explain CODE`, with the diagnostic's code. Message wording is not stable. A span carries its source file and positions, including byte offsets.

## `incan test`

`PATH` names a source file containing test functions. The command compiles the collected tests and executes each in its own process on every invocation. Each test prints one line: `FILE::TEST` followed by `PASSED`, `FAILED`, `SKIPPED (REASON)`, `XFAIL (REASON)` or `XPASS (REASON)`, where `FILE` is the source file's name. A failed test's output follows on stderr. A compilation error, a failing test or an `XPASS` returns nonzero. Directory collection, `-k`, `--jobs`, `--list`, JSON and JUnit output flags are not accepted by this command.

Collected test functions take no arguments and return `None` or `Result[None, E]`. A result-returning test passes with `Ok(None)` and fails with `Err(error)`; `E` must satisfy the `Debug` trait bound. `@skip`, `@skipif`, `@xfail` and `@xfailif` set a test's outcome policy; `@test`, `@slow`, `@mark` and `@serial` are accepted and do not change execution. Fixtures, parametrized tests, `@timeout` and `@resource` are refused.

## `incan build`

An explicit source file selects that unit. For a directory, `lib.incn` in the project's source root selects a library; otherwise `main.incn` there selects a program. The source root is `[build] source-root`, otherwise `src` when present, otherwise the project directory. Other project directories select the Rust facet build.

A library source named `lib.incn` produces an rlib and checked library sidecars. Path dependency artifacts reside under `target/incan/loaves/<package>/<profile>/`. Program executables reside under `target/incan/<profile>/bin/<crate>`. `--substitute NAME=DIR` supplies local Rust dependency substitutions for a Rust-project build. `--program-entry` is accepted for Incan programs. `--argv0` is refused by builds. `--lib` and `--report` are not accepted.

With `INCAN_ENGINE=records`, only explicit program source files are accepted, with the same restrictions and `--engine-report` behavior as records-mode `run`. Build does not execute the selected program.

## `incan describe-unit`

Writes the selected source's unit description as a JSON object. Execution-only options are refused.

| Fields | JSON type | Contract |
| --- | --- | --- |
| `schema` | String | `incan.unit/1`. |
| `source`, `project_root`, `source_root` | Strings | Entry file, owning project directory and authored source directory. |
| `kind` | String | `program`, `library` or `test`; the default is `program`. |
| `program_entries` | Array of strings | Additional selected program-entry paths. |
| `package`, `version`, `profile` | Strings | Package identity, package version and selected artifact profile. |
| `declared_features`, `active_features` | Arrays of strings | Declared root features and the selected root features. |
| `loaves` | Array of objects | Each entry has string `key`, `name` and `dir`, Boolean `standard`, and optional string `source_root`. The latter is omitted when authored source is unavailable. |
| `loaves[].toolchain_provided` | Boolean | Whether the toolchain supplies the facade rather than the project declaring a dependency. Default: `false`. |
| `rust_crates` | Array of strings | Declared Rust crate aliases. |
| `contract_models`, `c_headers` | Arrays of strings | Selected model-bundle paths and C header paths. |
| `vocab`, `vocab_entry` | String or null | Prepared vocabulary registration path and declared vocabulary entry path. |
| `vocabulary`, `vocab_desugarer` | Booleans | Whether vocabulary is active and whether this unit supplies its desugarer. |
| `vocab_admissions` | Array of strings | Admitted vocabulary dependency keys. |
| `vocab_policy` | String | Vocabulary policy identity. |
| `externs` | Array of objects | Exact native bindings, each with string `name` and `path`. |
| `search`, `rust_abi` | Arrays of strings | Additional native search directories and Rust ABI cache paths. |
| `search_index` | String or null | The toolchain generation's `incan.search-index/1` file, named by the SHA-256 of its content. `search` lists directories beyond this index. |

## `incan fmt`

Formats one or more files or directory trees. `--check` leaves files unchanged and exits `1` when formatting would change a file. `--` ends option parsing. Only `.incn` files are selected; hidden directories, `target`, `node_modules` and nested symbolic links are skipped. Explicitly named files retain their single-file selection. Directory-read errors and no discovered source files are errors. Files that cannot be formatted are reported; other selected files are still processed. The [code style](../../language/reference/code_style.md) specifies formatting rules.

## `incan owner stop`

Stops resident project owners belonging to the selected Incan home. `OVEN_OWNER=off` disables owner use for run, build and test. Executables may still be reused with owners disabled. Tests execute afresh even when their executable is reused.

## `incan bakery gc`

Removes superseded manifests, index pages and record segments that no current view or live snapshot lease reaches. A leased old snapshot remains readable, including unopened values. Once collection expires an unleased historical view, a later attempt to lease that descriptor fails. Unreferenced unit and sealed-set payloads are removed unless a live preparation guard protects their digest. Unit staging is removed only when its producer claim is free; other staging and root history remain retained. `--keep-days N` accepts one through six decimal digits, from `0` through `999999`; omitting it retains all root history. The option does not expire roots. `INCAN_BAKERY` selects the physical bakery directory.

Success exits `0` and reports retained and removed unit counts, dropped roots, cleared staging directories, removed layout and payload files, and reclaimed immutable bytes. A corrupt reachable layout or live lease descriptor, an I/O failure, or continuous arrival of new snapshot leases returns nonzero. A deferred collection removes no layout files. Publication waits during collection; reads through registered snapshots remain available.

The byte count sums the logical lengths of removed immutable files; staging bytes are excluded. It excludes replacement manifests and filesystem allocation or shared-extent accounting.


## `incan bakery verify`

Reads every unit record and every byte of every payload file in the selected bakery. A damaged record is named. A payload file that does not match its committed digest is reported by its canonical object path. Verification does not change records or payloads. Exits `0` when nothing is damaged and `1` otherwise.


## `incan bakery repair-payload SHA256 FILE`

Re-admits an explicit replacement file only when its full SHA-256 digest matches the committed payload name. A mismatched replacement is refused before changing the store. Existing damaged bytes are moved to quarantine; frozen payloads are never edited in place. This command does not select another record revision or namespace.

## `incan inspect codegraph`

Graph options, schemas, queries, context packs and snapshot checks are specified in [Agent context CLI and MCP](agent_context_cli.md). `--strict` refuses a graph containing unchecked units. The default returns available facts and warns about unchecked units.

## `incan mcp`

Serves the selected project's graph through newline-delimited JSON-RPC on stdin/stdout. Closing stdin ends the server. Protocol methods, tools, resources and errors are specified in the [MCP reference](agent_context_cli.md#mcp-transport).

## `incan inspect run ID --proofs`

Requires `INCAN_ENGINE=records`. `ID` is the 32-character lowercase hexadecimal `run` field printed by a command with `--engine-report`. The command prints that retained report as one JSON object; an unknown or malformed id is an error. Reports are retained only when explicitly requested.

| Field | JSON type | Contract |
| --- | --- | --- |
| `schema` | String | `incan.engine.proofs/1`. |
| `run` | String | The run id. |
| `granularity` | String | `unit-record`. |
| `unit_evaluations` | Integer | Whole-unit producer evaluations performed for this request; a compiled-unit hit is zero. |
| `native_bakes` | Integer | Successful native units published by this request. |
| `source_stats`, `source_reads`, `source_hashes`, `source_listings` | Integers | Editable-input operations counted by request capture, including configuration and the toolchain pin. |
| `frozen_stats`, `frozen_reads`, `frozen_hashes` | Integers | Frozen-input operations counted by request capture. These counters do not count compiler reads of admitted facts. |

## `incan toolchain pack` and `install`

`pack OUT` archives the installed toolchain closure. Supported extensions are `.tar`, `.tar.gz` and `.tar.zst`. Paths in the packed toolchain manifest are relative; the local Rust sysroot is excluded.

`install ARCHIVE` stages and verifies the archive before publishing it under `INCAN_HOME/toolchains/0.6.0-dev.8/`. Absolute archive members, parent traversal and archive-supplied `sysroot` members are refused. Manifest paths must be portable, the toolchain name must match this Oven, and named artifacts must exist. Installation binds a local Rust sysroot and verifies the driver's compiler identity. A refused archive preserves the existing installation.

## Environment variables

| Name | Contract |
| --- | --- |
| `INCAN_HOME` | Toolchain, cache and owner home. Default: `.incan` under `HOME`, or `USERPROFILE` when `HOME` is absent. |
| `INCAN_STDLIB` | Optional directory containing standard-package source directories with `loaf.toml`; otherwise the installed toolchain's `stdlib` directory is selected. Compiled artifacts belong to the installed toolchain and selected profile. |
| `INCAN_ENGINE` | `legacy` (default) or `records`. Records mode accepts the file-based `run` and `build` commands and `inspect run ID --proofs`; unsupported commands fail without falling back to legacy. |
| `INCAN_BAKERY` | Optional physical bakery directory; otherwise `<INCAN_HOME>/bakery`. |
| `INCAN_SYSROOT` | Explicit local Rust sysroot for installation. Otherwise installation probes Rustup `1.98.0`, then `rustc`. Identity verification is required. |
| `OVEN_OWNER` | `off` disables resident owner use. |
| `OVEN_TRACE` | `-` writes invocation traces to stderr; another nonempty value names a trace output file. |

## Exit codes

`0` indicates success, including help. Unknown commands and unknown options exit `2`; the error is followed by ``Run `incan --help` for usage.`` Invalid unit-command syntax also exits `2`. Other argument-value refusals, preparation and compilation failures return nonzero. `run` returns the program's status. `test` returns nonzero when any collected test fails.
