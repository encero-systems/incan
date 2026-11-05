# CLI reference

An Incan installation provides two commands, `oven` and `incan`, as one program. `oven` is the project command: it runs, checks, tests, builds and describes units, inspects their operational facts, and manages the bakery and resident owners. `incan` is the language command: compiler diagnostics, formatting, semantic inspection, the context server and the toolchain. `incan run`, `incan check`, `incan test` and `incan build` perform the same operation as the `oven` command of the same name: the same unit, profile, features, artifacts and exit status.

## Usage

```text
oven --help | -h | help
oven --version
oven run PATH [UNIT OPTIONS] [--argv0 TEXT] [--program-entry PATH]... [-- ARGS...]
oven check PATH [--format text|json] [UNIT OPTIONS]
oven test PATH [UNIT OPTIONS]
oven describe-unit PATH [UNIT OPTIONS]
oven build PATH [--substitute NAME=DIR]... [--program-entry PATH]... [UNIT OPTIONS]
oven owner stop
oven bakery gc [--keep-days N] | status [--format text|json] | verify | repair-payload SHA256 FILE
oven inspect dependencies PATH [UNIT OPTIONS]
oven inspect run ID --proofs

incan --help | -h | help
incan --version
incan run PATH [UNIT OPTIONS] [--argv0 TEXT] [--program-entry PATH]... [-- ARGS...]
incan check PATH [--format text|json] [UNIT OPTIONS]
incan explain CODE
incan test PATH [UNIT OPTIONS]
incan build PATH [--substitute NAME=DIR]... [--program-entry PATH]... [UNIT OPTIONS]
incan fmt [--check] PATH...
incan inspect codegraph PATH [GRAPH OPTIONS]
incan mcp PATH
incan toolchain pack OUT
incan toolchain install ARCHIVE
```

There is no implicit command or implicit source path. Each name runs only its own commands. A command only `oven` runs is an unknown command to `incan`: `incan bakery status` exits `2` and reports ``incan: unknown command `bakery` ``. A language command given to `oven` exits `2` without running and names the command that runs it: `oven fmt` reports ``oven: `oven fmt` is a language command: run `incan fmt` ``.

## Help and version

`oven --help`, `oven -h` and `oven help` print the `oven` synopses above and the CLI reference location, then exit `0`; `incan --help`, `incan -h` and `incan help` do the same for `incan`. `COMMAND --help` or `COMMAND -h` prints that command's usage, options and one example, then exits `0`. Nested commands include the subcommand: `oven owner stop --help`, `oven bakery status --help`, `oven inspect dependencies --help`, `incan inspect codegraph --help`, `incan toolchain pack --help` and `incan toolchain install --help`. Help does not require a project or installed toolchain. Arguments after `--` and option values are not interpreted as help flags.

`oven --version` prints `oven 0.6.0-dev.8` and `incan --version` prints `incan 0.6.0-dev.8`, then exit `0`. `--version --help` prints its usage and example.

## Unit options

| Option | Value and default | Contract |
| --- | --- | --- |
| `--profile NAME` | String, `debug` | Selects the artifact profile. Standard packages must have artifacts for that profile. |
| `--features A,B` | Comma-separated names, empty | Adds public root-package features. |
| `--no-default-features` | Boolean, false | Disables selection of the root package's `default` feature. |
| `--all-features` | Boolean, false | Selects every declared root-package feature. |

Unknown features, invalid feature edges and unknown options fail the command. Feature-selection options require an owning project. Unsupported command-option combinations exit `2` before dependency preparation. Unit commands accept one path. Package features are specified in [Project configuration](project_configuration.md#projectfeatures).

## `oven run`

`PATH` names an Incan entry source. Arguments after `--` are passed to the program. `--argv0 TEXT` sets the program's argument zero. Repeatable `--program-entry PATH` adds program entries. The compiled program inherits stdin, stdout and stderr; its exit status is the command's exit status.

With `INCAN_ENGINE=records`, `PATH` must name one ordinary program source file, the profile must be `debug` or `release`, and editable path dependencies, vocabulary entries, foreign headers, feature-selection flags, substitutions and additional program entries are refused. Build and run select the same compiled unit. The program runs only after successful capture and native selection. `--engine-report` retains a proof report and prints one JSON line before any program output. Without that flag, proof reporting adds no console output. The flag is refused in legacy mode.

## `oven check`

Checks the source at `PATH` without producing an executable. `--format` accepts `text` (default) or `json`. Successful checks exit zero; errors exit nonzero.

The JSON report has `schema_version: 2`, Boolean `ok`, and a `diagnostics` array. Diagnostic entries carry `code`, `severity`, `phase`, `origin`, `message`, `primary_span`, `notes`, `hints` and `explain`, with additional related or comparison fields when applicable. `explain` is always the string `incan explain CODE`, with the diagnostic's code. Message wording is not stable. A span carries its source file and positions, including byte offsets.

## `incan explain`

`CODE` names a stable diagnostic in the installed compiler's catalog, matched without regard to ASCII case. The command prints its title, severity, phase, summary, explanation, examples, common causes, fixes and documentation link when present, then exits `0`. An unknown code exits `1` and names the three nearest catalog codes. Missing or extra operands and unknown options exit `2`. No project is required. This command is available with either engine selection.

## `oven test`

`PATH` names a source file containing test functions, or a Loaf directory. For a directory, the command runs the Loaf's Rust facet tests when it declares a Rust facet, then every `test_*.incn` module directly under its source root and its `tests` directory; a Loaf with neither is refused. The command compiles the collected tests and executes each in its own process on every invocation. Each test prints one line: `FILE::TEST` followed by `PASSED`, `FAILED`, `SKIPPED (REASON)`, `XFAIL (REASON)` or `XPASS (REASON)`, where `FILE` is the source file's name. A failed test's output follows on stderr. A compilation error, a failing test or an `XPASS` returns nonzero. `-k`, `--jobs`, `--list`, JSON and JUnit output flags are not accepted by this command.

Collected test functions take no arguments and return `None` or `Result[None, E]`. A result-returning test passes with `Ok(None)` and fails with `Err(error)`; `E` must satisfy the `Debug` trait bound. `@skip`, `@skipif`, `@xfail` and `@xfailif` set a test's outcome policy; `@test`, `@slow`, `@mark` and `@serial` are accepted and do not change execution. Fixtures, parametrized tests, `@timeout` and `@resource` are refused.

## `oven build`

An explicit source file selects that unit. For a directory, `lib.incn` in the project's source root selects a library; otherwise `main.incn` there selects a program. The source root is `[build] source-root`, otherwise `src` when present, otherwise the project directory. Other project directories select the Rust facet build.

A library source named `lib.incn` produces an rlib and checked library sidecars. Path dependency artifacts reside under `target/incan/loaves/<package>/<profile>/`. Program executables reside under `target/incan/<profile>/bin/<crate>`. `--substitute NAME=DIR` supplies local Rust dependency substitutions for a Rust-project build. `--program-entry` is accepted for Incan programs. `--argv0` is refused by builds. `--lib` and `--report` are not accepted.

With `INCAN_ENGINE=records`, only explicit program source files are accepted, with the same restrictions and `--engine-report` behavior as records-mode `run`. Build does not execute the selected program.

## `oven describe-unit`

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

## `oven owner stop`

Stops resident project owners belonging to the selected Incan home. `OVEN_OWNER=off` disables owner use for run, build and test. Executables may still be reused with owners disabled. Tests execute afresh even when their executable is reused.

## `oven bakery gc`

Removes superseded manifests, index pages and record segments that no current view or live snapshot lease reaches. A leased old snapshot remains readable, including unopened values. Once collection expires an unleased historical view, a later attempt to lease that descriptor fails. Unreferenced unit and sealed-set payloads are removed unless a live preparation guard protects their digest. Staging is removed, with its reservation, only when its producer's claim is free: the claim its [reservation](#storage-admission) names or, for staging without one, the claim of the unit identity it is named by. Staging whose producer is not known remains retained. A project's registration holds the units its latest successful result of each unit uses, until a newer result of that unit replaces it or the project's directory is removed. A unit record that only a replaced registration or a removed project used leaves the store with the name entries that select it, so its payloads are removed in the same collection. A record that a current registration reaches, directly or through the records it reaches, stays however long ago it was registered; so does a record that no registration has named, such as one the installed toolchain supplied. Nothing expires by age: `--keep-days N` accepts one through six decimal digits, from `0` through `999999`, and does not change what collection keeps. Collection applies to the bakery the [storage policy](#storage-policy) selects.

Success exits `0` and reports retained and expired unit counts, retired projects, cleared staging directories, removed layout and payload files, and reclaimed immutable bytes. A corrupt reachable layout or live lease descriptor, an I/O failure, or continuous arrival of new snapshot leases returns nonzero. A deferred collection removes no layout files. Publication waits during collection; reads through registered snapshots remain available.

The byte count sums the logical lengths of removed immutable files; staging bytes are excluded. It excludes replacement manifests and filesystem allocation or shared-extent accounting.


## `oven bakery verify`

Reads every unit record and every byte of every payload file in the selected bakery. A damaged record is named. A payload file that does not match its committed digest is reported by its canonical object path. Verification does not change records or payloads. Exits `0` when nothing is damaged and `1` otherwise.


## `oven bakery repair-payload SHA256 FILE`

Re-admits an explicit replacement file only when its full SHA-256 digest matches the committed payload name. A mismatched replacement is refused before changing the store. Existing damaged bytes are moved to quarantine; frozen payloads are never edited in place. This command does not select another record revision or namespace.

## `oven bakery status`

Reports where the selected bakery's bytes are and why they remain. Reading the status writes no registration, record, reservation or staging directory. `--format` accepts `text` (default) or `json`; both state the same facts. Success exits `0`; an unreadable store or an invalid [storage policy](#storage-policy) exits `1`.

Byte counts are logical file lengths. A payload that several projects reach counts once in the live total and once in each of those projects' totals. Filesystem allocation and copy-on-write sharing are not measured.

| Field | JSON type | Contract |
| --- | --- | --- |
| `schema` | String | `incan.oven.store-status/1`. |
| `store` | String | The bakery directory. |
| `store_source` | String | What selected the bakery directory: `INCAN_BAKERY`, the configuration file whose `storage.root` names it, or `default`. |
| `observed_at` | Integer | Seconds since the Unix epoch at which the status was observed. |
| `accounting` | String | The basis of the byte counts. |
| `policy` | Object | The effective storage policy: `file`, the configuration file that was read or an empty string, and `root`, `min_free_bytes`, `failed_attempt_ttl_seconds` and `history_ttl_seconds`, each an object with its `value` and its `source`, `default` or the file. `root` is the resolved path the configuration selects, also when `INCAN_BAKERY` overrides it. |
| `admission` | Object | `available_bytes` observed on the bakery's filesystem, `reserve_bytes`, `reserved_bytes` held by live reservations, and `admissible_bytes`, the largest write admitted now, negative inside the reserve. |
| `total_bytes` | Integer | Logical bytes of the regular files under the bakery directory. |
| `units` | Object | `units`, the unit records, and `payload_bytes`, the unique payload bytes they name. |
| `live` | Object | `projects` whose directory exists, and the `units` and unique `payload_bytes` their registrations reach. |
| `roots` | Array | One object per registered project, largest first: `project`, `orphaned` (its directory is gone), `registrations`, and the `units` and `payload_bytes` its registrations reach. |
| `unregistered` | Object | `units` and `payload_bytes` of the records no live registration reaches and collection keeps, such as those the installed toolchain supplied. |
| `collectable` | Object | What `oven bakery gc` would reclaim now: `units`, orphaned `projects`, `payload_bytes`, ended `staging_directories` and their `staging_bytes`, the sum of both byte counts in `bytes`, and `estimate`, the basis. Superseded layout files and payloads no record names are not counted. |
| `staging` | Object | `active`, the reservations of live bakes; `ended`, each ended attempt's `staging`, `work`, `bytes`, `end_observed_at` and `reclaimable_at`, both null until an admission observes the end; and `unrecorded`, the `directories` and `bytes` of staging without a reservation. |

A reservation object has `staging`, `work`, `reserved_bytes`, `basis` (`estimate`, `exact` or `unknown`), `phase` (`staging`, `sealing` or `ended`), `live`, `allocated_bytes`, `outstanding_bytes` and `created`.

## Storage admission

A bake is admitted against free space before it writes. A write is admitted when the bakery filesystem's observed free space, minus the bytes live concurrent reservations still hold, minus the free-space reserve `storage.min-free-bytes`, covers it. Admission in one bakery is serialized, so concurrent commands never count the same free bytes. An Incan unit or Rust library is admitted before it stages for the payload size of the unit last baked under its name. Other bakes, and bakes with no earlier unit of their name, are admitted before they stage while the bakery's filesystem is outside the reserve. Every bake is admitted for its exact staged size before its staged files are stored. A command that bakes nothing is not admitted and is never refused. Admission never removes content to make room.

An admitted bake holds a reservation under `<bakery>/reservations/` until it seals or its staging directory is removed. The reservation holds space while its producer holds its claim. Bytes already in its staging directory are part of the observed free space and are not counted again.

A refused admission stops the command with exit status `3` and a decision-required outcome. Nothing prompts. With `--format json` the outcome is one JSON document on stdout; otherwise its readable explanation is printed on stderr. A bake refused before storing its staged files removes its staging directory and its reservation. A later command observes free space, reservations and the store again.

| Field | JSON type | Contract |
| --- | --- | --- |
| `schema` | String | `incan.oven.storage-decision/1`. |
| `outcome` | String | `decision-required`. |
| `store` | String | The bakery directory. |
| `observed_at` | Integer | Seconds since the Unix epoch at which the pressure was observed. |
| `operation` | Object | `work`, the bake or seal and its unit identity; `required_bytes`; and `basis`: `estimate` (the unit last baked under the same name), `exact` (the staged bytes) or `unknown`. |
| `policy` | Object | The effective storage policy, as in `oven bakery status`. |
| `available_bytes` | Integer | Free bytes observed on the bakery's filesystem. |
| `reserved_bytes` | Integer | Bytes live concurrent reservations hold. |
| `reservations` | Array | The concurrent reservations holding space, as in `oven bakery status`. |
| `admissible_bytes` | Integer | Available minus reserved minus the reserve; negative inside the reserve. |
| `shortfall_bytes` | Integer | Required minus admissible. |
| `collectable` | Object | What `oven bakery gc` would reclaim, as in `oven bakery status`. |
| `actions` | Array | The actions that can change the outcome, each with `action` and `consequence`. `collect` has `command` (`oven bakery gc`) and `reclaims_bytes`; `wait` has `reserved_bytes`; `lower-reserve` has `key`, `file` and `current_bytes`; `free-space` has no further fields. `resolves` states whether the action alone covers the shortfall by the observed numbers. `collect` is listed only when collection would reclaim bytes, `wait` only when concurrent reservations hold space, and `lower-reserve` only when the reserve is above zero. |

A bake whose producer released its claim without sealing has ended. Its staging directory is kept for `storage.failed-attempt-ttl` after an admission first observes that end, then an admission removes it with its reservation. A clock reading earlier than the recorded end restarts the window at that reading. Staging without a reservation is removed only by `oven bakery gc`. Admission and this window never remove a unit record, a payload or a registration.

## Storage policy

`config.toml` in the Incan home may hold a `[storage]` table. Omitting the file, the table or a key selects the default.

| Key | Type | Meaning | Default |
| --- | --- | --- | --- |
| `root` | Nonempty path string | Bakery directory; a relative path is resolved against the directory that holds `config.toml` | `bakery` in the Incan home |
| `min-free-bytes` | Nonnegative integer | Free filesystem bytes that admission leaves untouched | `6442450944` (6 GiB) |
| `failed-attempt-ttl` | Duration string | How long an ended attempt's staging is kept after its end is observed | `"24h"` |
| `history-ttl` | Duration string | Maximum automatic diagnostic-history window | `"24h"` |

A command uses the bakery that `INCAN_BAKERY` names when it is set and nonempty, then `storage.root`, then `bakery` in the Incan home. `INCAN_BAKERY` applies to that invocation only and never changes `config.toml`. `oven bakery status` reports the bakery directory and which of the three selected it.

A duration is a nonnegative integer of at most twelve digits followed by `s`, `m`, `h` or `d`, for seconds, minutes, hours or 24-hour days. An unknown key, a value of another type, an empty `root`, a negative reserve or a malformed duration fails the command that reads the policy, naming the file, the key and the accepted form. Without `INCAN_BAKERY`, selecting the bakery reads the policy, so an invalid configuration fails every command that uses the bakery. A zero reserve adds no reserve; a write must still fit in observed free space. A zero `failed-attempt-ttl` makes an ended attempt reclaimable when its end is first observed. Other tables of the file are not part of this policy.

## `incan inspect codegraph`

Graph options, schemas, queries, context packs and snapshot checks are specified in [Agent context CLI and MCP](agent_context_cli.md). `--profile NAME` selects the artifact profile as specified under [Unit options](#unit-options). `--strict` refuses a graph containing unchecked units. The default returns available facts and warns about unchecked units.

## `oven inspect dependencies`

Prints the packages the unit at `PATH` reaches, as a build of the same unit links them. The unit is checked and planned as for `oven build`; nothing is generated or published. Execution-only options are refused. The text report is:

```text
KIND `PACKAGE` reaches N package(s)
LIBRARY
  DECLARATION
    via REQUESTER -> ... -> REQUESTER
    requested by ORIGIN
```

`KIND` is `program`, `library` or `test`. Packages are listed by name in ascending order. Under each package, every declaration that entered it from the unit or from another package is listed as its kind and canonical path, for example `function pub::incan_stdlib_codecs::encoding::hex::hexlify`. The `via` line lists the declarations that requested it, starting at the requested root, and is omitted when the declaration is itself a root. `ORIGIN` is `checked source`, `a selected comparison default`, `a selected primitive adapter`, or `runtime operation NAME at FILE:LINE:COLUMN`, with `FILE` relative to the project. A package that a requested but uncovered root selects is listed with `selected by an uncovered root; no declaration is loaded`. Declared dependencies and standard packages the closure does not reach are not listed; [`oven describe-unit`](#oven-describe-unit) lists everything available. A failure exits `1` with the same diagnostic a build reports.

## `incan mcp`

Serves the selected project's graph through newline-delimited JSON-RPC on stdin/stdout. Closing stdin ends the server. Protocol methods, tools, resources and errors are specified in the [MCP reference](agent_context_cli.md#mcp-transport).

## `oven inspect run ID --proofs`

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

`install ARCHIVE` stages and verifies the archive before publishing it under `INCAN_HOME/toolchains/0.6.0-dev.8/`. Absolute archive members, parent traversal and archive-supplied `sysroot` members are refused. Manifest paths must be portable, the toolchain name must match this Oven, and named artifacts must exist. Installation binds a local Rust sysroot and verifies the driver's compiler identity. A refused archive preserves the existing installation. After publishing, installation exposes the running program as both `oven` and `incan` in its directory: a missing name becomes a symbolic link to it and prints `entry points: oven and incan in DIR`. A different file with either name is refused and left in place.

## Environment variables

| Name | Contract |
| --- | --- |
| `INCAN_HOME` | Toolchain, cache and owner home. Default: `.incan` under `HOME`, or `USERPROFILE` when `HOME` is absent. |
| `INCAN_STDLIB` | Optional directory containing standard-package source directories with `loaf.toml`; otherwise the installed toolchain's `stdlib` directory is selected. Compiled artifacts belong to the installed toolchain and selected profile. |
| `INCAN_ENGINE` | `legacy` (default) or `records`. Records mode accepts the file-based `run` and `build` commands and `inspect run ID --proofs`. Help, version and `explain` do not select an engine. Other unsupported commands fail without falling back to legacy. |
| `INCAN_BAKERY` | Optional bakery directory for one invocation. It overrides `storage.root` in the home's `config.toml` without changing the file; otherwise the [storage policy](#storage-policy) selects the bakery, `<INCAN_HOME>/bakery` by default. |
| `INCAN_SYSROOT` | Explicit local Rust sysroot for installation. Otherwise installation probes Rustup `1.98.0`, then `rustc`. Identity verification is required. |
| `OVEN_OWNER` | `off` disables resident owner use. |
| `OVEN_TRACE` | `-` writes invocation traces to stderr; another nonempty value names a trace output file. |
| `INCAN_ENGINE_TRACE` | Existing directory for per-process engine JSONL traces. |
| `INCAN_JEC_TRACE` | Existing directory for per-process JEC compatibility traces. |

A requested trace that cannot be written produces one `incan: trace incomplete: REASON` warning on stderr after command or program output. Write, text-bound, record-bound and invalid-span failures are reported without changing the command's exit status. Successful or disabled file tracing leaves program stdout and stderr bytes unchanged. A program terminated by a signal while Oven waits to report a failed trace returns the shell status `128 + signal`.

## Exit codes

`0` indicates success, including help. Unknown commands, unknown options and commands given to the other name exit `2`; the error is followed by ``Run `oven --help` for usage.`` or ``Run `incan --help` for usage.``, naming the invoked command. Invalid unit-command syntax also exits `2`. Other argument-value refusals, preparation and compilation failures return nonzero. `run` returns the program's status. `test` returns nonzero when any collected test fails.

`3` is a decision-required outcome: [storage admission](#storage-admission) refused a write, and the command stopped before writing it without removing stored content to make room. It is distinct from success and from failure; under `--format json` its document is printed on stdout.
