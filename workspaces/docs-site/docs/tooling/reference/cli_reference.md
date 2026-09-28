# CLI reference

This page specifies the `incan` command line: its commands, options, outputs, paths, environment variables and exit codes.

## Usage

```text
incan [OPTIONS] [FILE] [COMMAND]
```

`incan FILE` type-checks `FILE` alone, without resolving a workspace scope; outside a workspace it reports what `incan check FILE` reports.

| Command | Does |
| --- | --- |
| [`build`](#incan-build) | Compiles a program or a library. |
| [`cache`](#incan-cache) | Inspects and prunes generated-build storage. |
| [`check`](#incan-check) | Type-checks a file or project entrypoint. |
| [`env`](#incan-env) | Lists, shows and runs project environments. |
| [`explain`](#incan-explain) | Explains a diagnostic code. |
| [`fmt`](#incan-fmt) | Formats Incan source files. |
| `help` | Prints the help of `incan` or of the named command. |
| [`init`](#incan-init) | Adds a project to a directory, creating the directory when it is missing. |
| [`inspect`](#incan-inspect) | Reports compiler artifacts and projections. |
| [`lock`](#incan-lock) | Generates or updates `oven.lock`. |
| [`new`](#incan-new) | Creates a project directory. |
| [`oven`](#incan-oven) | Prepares, inspects and maintains Oven outputs and stores. |
| [`run`](#incan-run) | Compiles and runs a program. |
| [`test`](#incan-test) | Runs tests. |
| [`tools`](#incan-tools) | Reports toolchain state and checked metadata. |
| [`version`](#incan-version) | Updates the project version. |
| [`workspace`](#incan-workspace-inspect) | Reports the workspace graph and command scope. |

## Global options

These options come before the command.

- `--no-banner`: suppress the logo banner. `INCAN_NO_BANNER=1` does the same.
- `--color auto|always|never` (default `auto`): control ANSI color. Under `auto`, `NO_COLOR` disables color.
- `-h`, `--help`: print the help and exit with `0`. Every command also takes `-h` and `--help`, and `incan help [COMMAND]` prints the same help.
- `-V`, `--version`: print `incan <VERSION>` on one line and exit with `0`.

The banner shows only for an interactive `incan build` or `incan run`. `incan` with no command, no `FILE` and no debug option prints the help and exits with `1`.

## Debug options

Each takes a file and runs one pipeline stage:

| Option | Output |
| --- | --- |
| `--lex FILE` | Tokens. |
| `--parse FILE` | The syntax tree. |
| `--check FILE [--format text\|json]` | Diagnostics; outside a workspace, `--format json` produces the same report as `incan check FILE --format json`. |
| `--emit-rust FILE [--strict]` | Generated Rust; `--strict` makes it warning-clean. |

A debug option cannot be combined with `FILE`. `--format` is accepted only with `--check`, and `--strict` only with `--emit-rust`.

## Package-feature and SDK profile options

- `--features <FEATURE,...>`: select public features of the root Incan package, comma-separated.
- `--no-default-features`: do not select the root package's `default` feature.
- `--all-features`: select every public feature the root package declares.
- `--sdk-profile <PROFILE>`: replace the project's base SDK profile for this invocation; the component additions and exclusions in `[sdk]` still apply.

`build`, `check`, `run`, `test`, `lock`, `inspect codegraph`, `inspect providers`, `inspect features` and `inspect bindings` accept all four, and `oven bake` accepts the three feature options. They select Incan package features only; Rust dependency features are selected by `incan lock --cargo-features` and its companions.

`incan test --feature <NAME>` is a different option: it enables a `std.testing.feature("NAME")` probe at collection.

## Lock policy options

`build`, `run` and `test` accept these options, and read the matching environment variable when the option is absent:

| Option | Environment variable | Requires |
| --- | --- | --- |
| `--locked` | `INCAN_LOCKED` | An up-to-date `oven.lock`. |
| `--offline` | `INCAN_OFFLINE` | Locked inputs available without fetching. |
| `--frozen` | `INCAN_FROZEN` | Both of the above. |

`--no-locked`, `--no-offline` and `--no-frozen` disable the environment variable for one invocation. An environment variable enables its option when set to `1`, `true`, `TRUE`, `on` or `ON`. `--help` does not list these options.

Under `--locked` or `--frozen`, a Rust git dependency selected by `branch` is refused.

An `oven.lock` whose only stale part is its dependency fingerprint is reported with a warning, and the command continues without using it as the lock or rewriting it; under `--locked` or `--frozen` it is refused.

## Inspection reports

| Report | Command | `schema_version` |
| --- | --- | --- |
| Diagnostics | `incan check --format json` | `2` |
| Diagnostic explanation | `incan explain --format json` | `2` |
| Build report | `incan build --report json` | `2`; `"incan.replacement_execution.v1"` with `--backend replacement` |
| Backend-selection receipt | `incan inspect backend-selection --format json` | `2` |
| Generated Rust | `incan inspect rust --format json` | `2` |
| Codegraph | `incan inspect codegraph --format jsonl` | `8`, on the header record |
| SDK components and providers | `incan inspect providers --format json` | `1` |
| Package features | `incan inspect features --format json` | `1` |
| C bindings | `incan inspect bindings --format json`, or `--format receipt` | `2` |
| Interop deployment plan | `incan inspect interop-plan --format json` | `3` |
| Executable representation | `incan inspect representation --format json` | none |
| Typed registry | `incan inspect registry` | See [`std.registry`](../../language/reference/stdlib/registry.md) |
| Test results | `incan test --format json` | `"incan.test.v1"` |
| Generated-build cache | `incan cache inspect --format json`, `incan cache prune --format json` | `1` |
| Workspace graph | `incan workspace inspect --format json` | `1` |
| Checked API metadata | `incan tools metadata api --format json` | `2` |

Each report is separate and versioned on its own. A fact that appears in more than one report carries the same identity fields and source paths. Human-readable output is not part of the contract.

## Workspace scope

In a project that belongs to a workspace, `check`, `build`, `test`, `fmt`, `run` and `version` resolve a member scope first:

- `--workspace` selects every member, and `--member <NAME_OR_PATH>`, repeatable, selects named or root-relative members.
- Without either, a command run inside a member selects that member. At the workspace root it selects `default-members`, else the implicit root member, else every member of a virtual root.
- `check`, `build`, `test` and `fmt` run over the selected members in a fixed order. `run` and `version` require the scope to resolve to exactly one member.
- Machine-readable `check`, `build` and `test` output names the workspace and member.
- `incan lock` always covers the whole workspace (see [`incan lock`](#incan-lock)).

In a workspace, the machine-readable output of `check` and `build` is one report for the selected members:

| Command | `schema_version` | Fields |
| --- | --- | --- |
| `incan check --format json` | `"incan.workspace.check.v1"` | `workspace`, `ok`, and `results`: per member, `member`, `target` (the checked path, or `null`), and the member's diagnostics report as `report`, or `ok: false` with `error`. |
| `incan build --report json` | `"incan.workspace.build.v1"` | `workspace`, `ok`, and `results`: per member, `member` and `ok`, with `error` when `ok` is `false`. |

`workspace` holds `root` and `selected_scope`, which holds `origin` and `members`; `member` and each selected member hold `name` and `root`. `origin` is `current_member`, `default_members`, `implicit_root_member`, `workspace` or `explicit_members`. `incan test --format json` in a workspace adds a `workspace_scope` event with the same `workspace` object, and a `workspace_member_error` event for a member that cannot run.

## `incan check`

```text
incan check [OPTIONS] [PATH]
```

Type-checks a file or project entrypoint without building it. `PATH` defaults to `.`.

Options:

- `--format text|json` (default `text`).
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options).
- `--interop-target <TRIPLE>`: check the C declarations against one target declared in `[interop.c]`. It does not cross-compile or package.
- `--workspace`, `--member <NAME_OR_PATH>`: the [workspace scope](#workspace-scope).

The JSON report:

- has `schema_version: 2`, `ok`, and `diagnostics`;
- lists each diagnostic with its `code`, `severity` (`error`, `warning` or `hint`), `phase` (`lex`, `parse`, `typecheck`, `import`, `tooling` or `unknown`), `origin` (`lexer`, `parser`, `import_resolver`, `typechecker`, `tooling` or `unknown`), `message`, `primary_span`, `notes`, `hints` and `explain` command; `expected` and `actual` when it compares two values; `related_spans`, each a `span` with a `label`, when there are any; and `related_declarations`, each a canonical declaration `identity` with a `label`, when there are any;
- includes warnings: `ok` is `false` only when an error is present, and a check with only warnings succeeds;
- orders warnings by source module, parser before typechecker within a module, and is identical across runs over unchanged sources;
- gives each span as a `file` with a `start` and an exclusive `end`, each position a 1-based `line`, a 1-based `column` counted in Unicode scalar values, and a byte `offset`.

`incan inspect codegraph --allow-errors` and the LSP report the same diagnostics for source that does not check. Codegraph spans carry byte offsets and line and column positions, and codegraph reports no warnings for source that checks.

Text output, the default, is source-highlighted diagnostics.

```bash
incan check src/main.incn
incan check src/main.incn --format json
incan check --interop-target aarch64-linux-android src/main.incn
incan check --workspace --format json
```

## `incan explain`

```text
incan explain [OPTIONS] <CODE>
```

Explains a diagnostic code. `--format text|json` (default `text`); `json` prints the catalog entry. The catalog is versioned with the diagnostic schema.

The JSON report has `schema_version: 2`, `found`, and `entry`, which holds `code`, `title`, `severity`, `phase`, `summary`, `explanation`, `examples`, `common_causes`, `fixes` and `docs_url`. For a code outside the catalog, the command prints the `INCAN-U0001` entry, with `found: false` in JSON, and exits with `1`.

| Code | Meaning |
| --- | --- |
| `INCAN-P0001` | Syntax error. |
| `INCAN-T0001` | Type checking error. |
| `INCAN-T0101` | Unreachable code: statements after a `return` in the same block. A warning. |
| `INCAN-T0102` | A method assigns to a field, or calls a method that changes one, through a plain `self` receiver. |
| `INCAN-T0103` | A value displayed by `print`, `println`, `str` or an f-string `{value}`, or given as the type argument of a `Display` bound, has no printed form: a union value, a `Generator`, a function, `bytes`, or a `model`, `class`, `enum` or `newtype` whose type provides no `Display`. See [Display](../../language/reference/strings.md#display). |
| `INCAN-T0104` | A `list`, `dict`, `set`, `tuple`, `Option`, `Result`, frozen collection or `Generator` annotation names no type arguments, alone or nested in another annotation. |
| `INCAN-T0105` | A Rust associated call such as `HashMap.new()` leaves the owner's type arguments open, and nothing later fixes them. |
| `INCAN-T0106` | An `Fn`, `FnMut` or `FnOnce` marker from `std.rust` names more than two parameters, or bounds a type parameter of a nominal declaration. |
| `INCAN-T0107` | A `@route` handler's return type is not a response type: `str`, `bytes`, `None`, `Json[...]`, `Html`, `Response`, a wrapper deriving `IntoResponse`, or a `Result` of these. |
| `INCAN-T0108` | A `@route` handler parameter that no `{segment}` of the path binds and no extractor supplies: `Json[...]`, `Query[...]`, `Path[...]`, `Body`, `Request`, a `str` or `bytes` body, or a wrapper deriving `FromRequestParts`. |
| `INCAN-T0109` | `/`, `//`, `%` or `**` applied to two values of the same type parameter, when no trait bounding that parameter defines the operator's hook (`__div__`, `__floordiv__`, `__mod__` or `__pow__`). |
| `INCAN-T0110` | A method decorator's shape, or a function a decorator returns in the method's place, writes the receiver as `&Owner` or `&mut Owner` instead of `Owner` for a `self` method or `mut Owner` for a `mut self` method. |
| `INCAN-T0111` | A name the source declares or binds starts with `__incan_`, the prefix reserved for the compiler, other than a method named `__incan_new`. See [Reserved name prefix](../../language/reference/imports_and_modules.md#reserved-name-prefix). |
| `INCAN-T0112` | A `@route` handler's `Json[T]`, `Query[T]` or `Path[T]` parameter, or `Json[T]` return, carries a model or class with no JSON form (no `@derive(json)` and no adopted `std.serde.json` trait), directly or inside a collection. |
| `INCAN-T0113` | A `model` or `class` field, or an `enum` variant payload, whose type does not implement `Clone` and `Debug`, such as a `JoinHandle[T]` field. |
| `INCAN-T0114` | A `set` element type or `dict` key type that does not implement `Eq` and `Hash`, in an annotation, a literal, a comprehension, a `set(...)` call, or as the type argument of a generic call that uses its type parameter as a set element or dict key. |
| `INCAN-T0115` | An argument that is not a task where `spawn`, `timeout`, `timeout_ms`, `race_timeout` or `arm` requires one, such as `spawn(work)`, or `spawn(fut)` after `fut = work()`. |
| `INCAN-T0116` | A decorator chain of a `self` method, whose shapes name the receiver, that cannot pass the receiver: the decorator is imported or reached through a value or a method; a shape that names the receiver is written through a type alias; a `return` gives something other than the accepted callable or a function of the module named directly; the decorator calls, stores or passes on the callable it accepts; a function of the chain is `pub`; or a function of the chain is used outside it, other than a direct call of a returned function in its module. See [Decorators](../../language/reference/language.md#decorators). |
| `INCAN-T0117` | An immutable binding or a field of one, a list or dict element, a static, or a `for` loop variable passed to a `mut` parameter whose changes reach the caller (a parameter of any type except `int`, `float`, `bool` or a Rust type, and not `*args` or `**kwargs`) when the call changes the parameter, or when the argument's type cannot be copied, such as a `Generator`. |
| `INCAN-T0118` | A `dict.get(key)` whose result is a copy of the stored value, on a dict whose value type cannot be copied, such as a `Generator` or a Rust type without `Clone`. See [Reading a dict entry with `get`](../../language/explanation/static_storage.md#reading-a-dict-entry-with-get). |
| `INCAN-T0119` | A `for` loop took the task handles out of a list, and the list is then read inside or after the loop, a closure captured it before the loop, or an enclosing loop repeats the loop; or a `for` loop that hands each task handle on iterates something it cannot take the handles out of, such as a list element, a field, `values()`, `enumerate(...)` or a `mut` parameter. See [`JoinHandle[T]`](../../language/reference/stdlib/async.md#joinhandlet). |
| `INCAN-I0001` | Import or module resolution error. |
| `INCAN-I0101` | A known SDK provider module belongs to a component the project disables. |
| `INCAN-I0102` | The project enables an SDK component that is unavailable in the active installation. |
| `INCAN-I0103` | A known package export requires a public feature projection that is not active. |
| `INCAN-C0001` | CLI or tooling error. |
| `INCAN-U0001` | Unknown diagnostic code. |

```bash
incan explain INCAN-T0001
incan explain INCAN-T0001 --format json
```

## `incan build`

```text
incan build [OPTIONS] [FILE] [OUTPUT_DIR]
```

Compiles `FILE`, or the project entrypoint, to an executable and prints the generated project path and the binary path. Generated source and the binary go under `target/incan/`, or under `OUTPUT_DIR` when given.

With no `FILE`, a project that declares one or more `[[rust.bin]]` entries and no `[project.scripts].main` builds the declared toolchain binaries from stored direct-`rustc` plans. This mode accepts `OUTPUT_DIR`. It refuses `--release`, backend-selection options, package-feature options, `--sdk-profile`, lock-policy options, Cargo feature or passthrough controls, generated-Cargo target-directory controls, and build reports. `INCAN_OVEN_COMPILER_SUITE_STORE` selects the compiler-suite store for this mode.

Options:

- `--lib`: build the library rooted at `src/lib.incn`: its checked `.incnlib` manifest, debug and release `rlib` outputs, and its [package executable representation](package_executable_representation.md). `--help` does not list this option.
- `--release`: build the release profile, which is the default for `incan build`. The profile does not change language semantics; integer `abs` and `sum` stay overflow-checked in every profile.
- `--backend legacy|replacement` (default `legacy`): select the compiler backend. `replacement` runs the entrypoint's zero-argument `main` directly, without generating Rust or an Oven plan, for a program within the replacement backend's supported profile; a program outside it is refused before it runs. The profile is described in [Backend selection & execution receipts](../explanation/backend_selection_receipts.md).
- `--backend-fallback refuse`: refuse an unavailable backend instead of substituting another. `refuse` is the only policy.
- `--shadow`: record a shadow comparison against the replacement backend. For a build, the comparison is recorded as unavailable, with its reason; the comparison that does run is described in [Source-observable shadow comparison](../explanation/backend_selection_receipts.md#source-observable-shadow-comparison).
- `--report json`: emit a machine-readable build report.
- `--report-output <PATH>`: write the report to `PATH` instead of stdout. Required with `--backend replacement --report json`; without it that command is refused before it runs.
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options), the [lock policy options](#lock-policy-options), and `--workspace` and `--member <NAME_OR_PATH>` ([workspace scope](#workspace-scope)).

Build report, `legacy` backend:

- `schema_version: 2`;
- source and generated paths, emitted artifacts, dependency and provider summaries;
- `oven.receipt_identity`, `oven.build_unit_identity` and `oven.plan_identity`;
- prepare, build and total elapsed time;
- a note that no Cargo consumer ran;
- `backend`: the build's backend-selection execution receipt.

Build report, `replacement` backend:

- `schema_version: "incan.replacement_execution.v1"`;
- `status`, `mode`, `entrypoint`, `backend`, and `semantic_module`, which names the selected module with its source and semantic-snapshot identities, as `backend.semantic_module` does;
- `replacement_execution`: `result`, the exact checked `result_type`, the `stdout_bytes` and `stderr_bytes` byte arrays, the `emitted_output` projection, `output_identity`, the Body-IR snapshot, canonical ownership reads and runtime requirements;
- in `replacement_execution`, `package_declarations_decoded`, `package_payload_bytes_read` and `package_content_bytes_verified` (see [package execution report fields](package_executable_representation.md#execution-report));
- total elapsed time;
- no `generated`, artifact or `oven` fields.

The replacement backend writes program output to stdout and stderr while it runs, and each print flushes. A later failure does not withdraw output already written. The report never goes to the program's stdout or stderr. The byte arrays keep each stream's bytes, which need not be UTF-8, and record no order between the two streams.

A successful build also writes its backend-selection receipt to `.incan/backend/receipt.json` in the project root (see [`incan inspect backend-selection`](#incan-inspect-backend-selection)). A reused completed output keeps the receipt its bake sealed; that reuse applies only to the implicit `legacy` default, and an explicit `--backend` or `--shadow` prepares the build again.

`build`, `run` and `test` select a compatible standard-library Loaf from the active toolchain and, for a project outside it, the project extension its bake published to the Oven store. A missing compatible selection is refused; these commands do not prepare one. `incan inspect oven --receipt PATH --format json` reports the plan selection for the receipt's build unit and its reason.

```bash
incan build examples/simple/hello.incn
incan build src/main.incn --report json --report-output target/build-report.json
incan build src/main.incn --features json,http --sdk-profile minimal
incan build --lib
incan build src/main.incn --backend replacement --backend-fallback refuse --report json --report-output report.json
```

## `incan cache`

```text
incan cache inspect [--category generated-cargo] [--format text|json]
incan cache prune [--category generated-cargo] [--dry-run] [--max-bytes BYTES] [--format text|json]
incan cache prune --identity SHA256 [--identity SHA256 ...] [--dry-run] [--format text|json]
```

- `inspect` reports the cache root, the configured soft limit, recursive logical file bytes, full compatibility identities, profiles, last-use times, and whether each domain is in use.
- `prune` removes idle domains, least recently used first, until the cache is within the configured limit, or `--max-bytes`. `--identity`, repeatable, removes exactly the named domains instead and cannot be combined with `--max-bytes`.
- A domain with an active build lease is never removed. `--dry-run` reports the domains and logical bytes a prune would remove, and removes nothing.
- JSON reports carry `schema_version: 1`. `removed_logical_bytes` is the logical size of the removed domains; byte fields can differ from filesystem allocation.

## `incan oven`

```text
incan oven bake [--project PATH] [--target TRIPLE] [--features FEATURE,...] [--no-default-features]
                [--all-features] [--format text|json]
incan oven import --target TRIPLE --toolchain IDENTITY [--project PATH] [--profile PROFILE]
                  [--feature NAME ...] [--source NAME=PATH ...] [--output PATH] [--format text|json]
incan oven harvest --target TRIPLE --cargo PATH --rustc PATH --output PATH [--project PATH]
                   [--profile release|debug] [--cargo-lock PATH] [--format text|json]
incan oven interop bake --target TRIPLE [--project PATH] [--base-receipt PATH] [--c-compiler PATH]
                        [--cxx-compiler PATH] [--archiver PATH] [--toolchain-version VERSION]
                        [--sdk-root PATH] [--sdk-version VERSION] [--sdk-identity-file PATH]
                        [STORE OPTIONS] [--format text|json]
incan oven interop stage --target TRIPLE --base-receipt PATH --adapter android|ios --output PATH
                         [--project PATH] [STORE OPTIONS] [--format text|json]
incan oven plan publish --receipt PATH --manifest PATH --artifact-root PATH --domain NAME
                        [STORE OPTIONS] [--format text|json]
incan oven store inspect [STORE OPTIONS] [--format text|json]
incan oven store prune [STORE OPTIONS] [--dry-run] [--format text|json]
incan oven test --receipt PATH --plan SHA256 --rustc PATH --source PATH --output PATH
                --crate-name NAME --source-evidence NAME --exact TEST [--exact TEST ...]
                [--edition 2021|2024] [STORE OPTIONS] [--format text|json]
incan oven run --receipt PATH --plan SHA256 --rustc PATH --source PATH --output PATH
               --crate-name NAME --source-evidence NAME [--edition 2021|2024]
               [STORE OPTIONS] [--format text|json] [-- ARG ...]
incan oven compiler-libtests [--compiler-root PATH] [--rustc PATH] [--feature NAME ...]
                             [--target SOURCE ...] [--exact TEST ...] [--output PATH]
                             [STORE OPTIONS] [--format text|json]
```

`STORE OPTIONS` are `--store PATH`, `--max-physical-bytes BYTES`, `--max-domain-physical-bytes BYTES` and `--max-domain-logical-bytes BYTES`. `--store` defaults to `$INCAN_HOME/oven/store/v2`, or `~/.incan/oven/store/v2` when `INCAN_HOME` is unset.

| Subcommand | Contract |
| --- | --- |
| `bake` | Prepares, or reuses, the project's Loafs for debug and release. `--project` defaults to `.`. `--target` selects one Rust target for every project target and both profiles, and defaults to the active compiler's host target; the selected toolchain must already support it, or the bake fails. The target is part of every receipt and reuse identity: a warm bake reuses only outputs sealed for the same target and toolchain. The report gives each output's `target`, and an `action` per profile: `toolchain_loaf` when the profile uses the toolchain's standard-library Loaf directly, `reused` for an exact warm project extension, or `baked`. |
| `import` | Records frozen Cargo declarations as receipt evidence without running Cargo. `--profile` defaults to `release`; `--output` defaults to `.incan/oven/receipt.json` below `--project`. |
| `harvest` | See [`incan oven harvest`](#incan-oven-harvest). |
| `interop bake` | Compiles the locked C and C++ shims and static inputs for one locked target into one direct-`rustc` interop plan. Without `--base-receipt`, it prepares the debug base first. It does not invoke Cargo, Gradle, Xcode or signing. |
| `interop stage` | Stages a baked interop plan's bundled runtime files, verified by digest, into the new directory `--output` in the Android or iOS layout. Existing output is never replaced. It does not invoke a platform build tool. |
| `plan publish` | Validates a direct-`rustc` artifact manifest against `--artifact-root` and copies its declared files into one immutable store entry. |
| `store inspect` | Reports physical allocation and logical artifact bytes, reclaimable and lease-protected allocation, and, for a direct-`rustc` plan, the full receipt of the compilation that produced it. |
| `store prune` | Removes inactive entries to meet the store policy. `--dry-run` reports them and removes nothing. |
| `test` | Compiles a native test binary from a stored plan, lists its tests, refuses any `--exact` name that is not listed, then runs the named tests. `--edition` defaults to `2024`. |
| `run` | Compiles one binary from a stored plan and runs it, holding the store entry's lease until the process exits, and passes it the arguments after `--`. `--edition` defaults to `2024`. |
| `compiler-libtests` | Compiles and runs the compiler repository's native test suite through a stored direct-`rustc` plan; `--target` and `--exact` select part of it. |

Store policy:

| Limit | Default | Compiler-suite store | Environment variable |
| --- | --- | --- | --- |
| Aggregate physical allocation | 12 GiB | 16 GiB | `INCAN_OVEN_MAX_PHYSICAL_BYTES` |
| Physical allocation per compatibility domain | 12 GiB | 6 GiB | `INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES` |
| Logical bytes per compatibility domain | 6 GiB | 4 GiB | `INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES` |

- The limits are whole bytes. The aggregate limit includes the previous committed Loaf generation while a replacement is staged.
- Before it compiles, an explicit `oven bake` reserves 4 GiB of staging space. It reclaims inactive entries, oldest first, until the space fits, and prints a `note:` naming each one. An entry under a live lease is never reclaimed.
- Publication is refused when one domain exceeds its limit, or when active leases prevent reclaiming enough space.
- Reports give physical allocation and logical bytes (plan bytes plus the manifest-declared files) as separate fields.
- A `closure-proofs/` directory inside the store root records each closure that one command materialized in full. Deleting it is safe; it is recreated.

`incan inspect oven --receipt PATH` reports the receipt's build-unit identity, its selection result (`hit`, `miss` or `ambiguous`) with the reason, and the store accounting. It rehashes the closure it inspects and does not read a closure proof.

The Oven design, its compatibility envelope and its exclusions are explained in [Oven Alpha](../explanation/oven_alpha.md).

### `incan oven harvest`

```text
incan oven harvest --target TRIPLE --cargo PATH --rustc PATH --output PATH [--project PATH]
                   [--profile release|debug] [--cargo-lock PATH] [--format text|json]
```

Runs the compatibility publisher once over the registry `[rust-dependencies]` of a checked `loaf.toml`, for exactly `--target` and `--profile`, and writes incan.pub fact proposals from what it observed.

Options:

- `--project <PATH>` (default `.`): the checked manifest, or its directory.
- `--target <TRIPLE>`, `--profile release|debug` (default `release`): the selection the facts bind.
- `--cargo <PATH>`, `--rustc <PATH>`: the publisher's tools. The facts bind the `rustc` identity; the Cargo version is recorded as provenance.
- `--cargo-lock <PATH>`: resolve within an existing lock instead of resolving afresh.
- `--output <PATH>`: the proposal directory.
- `--format text|json`: text lists proposals and refusals; JSON is the harvest report.

Output:

- One proposal per registry package version whose build script emitted only `cfg` answers and `OUT_DIR` files: `<output>/<name>-<version>-<profile>/proposal.json`, with the retained `OUT_DIR` files under `out/` beside it.
- A proposal binds the toolchain, target, profile and complete feature set observed. Its `evidence` records the capture receipt, the compiler closure identity, the host, `cargo_version`, and the Cargo lock and manifest digests; `hazards` lists `RUSTC_BOOTSTRAP` and `nightly-rustc` when present.
- `<output>/refusals-<profile>.json` names each unit not proposed, with one reason: `not-registry-backed`, `build-script-unit` (the script's own execution node; its library is proposed separately), `linked-libraries`, `linked-paths`, `tool-probes`, `environment-observed`, `output-not-retained`, `target-mismatch`, `multiple-build-script-edges`, `conflicting-observations`, `malformed-checksum` or `malformed-output`.
- Harvesting into an existing output directory changes nothing when every proposal binds the same facts. A proposal that would bind different facts there is refused.

Admitting a proposal into the registry is a separate step, `incan-pub add-fact`.

## `incan inspect`

### `incan inspect backend-selection`

```text
incan inspect backend-selection --receipt PATH [--format text|json]
```

Verifies and prints a backend-selection execution receipt, such as the `.incan/backend/receipt.json` a successful build writes. A receipt whose recorded content identity or embedded selection identity does not match its content is refused. `--format text` (the default) prints the selected and executed backend, the selection reason, the fallback policy and outcome, the shadow-comparison state, the compiler version and both identities; `--format json` prints the receipt. See [Backend selection & execution receipts](../explanation/backend_selection_receipts.md).

### `incan inspect oven`

```text
incan inspect oven --receipt PATH [STORE OPTIONS] [--format text|json]
```

See [`incan oven`](#incan-oven).

### `incan inspect rust`

```text
incan inspect rust [OPTIONS] <PATH>
```

Generates the Rust project the `legacy` backend builds and reports its files, without compiling them.

- `--lib`: inspect the library rooted at `src/lib.incn`; `PATH` is the project root or a source path inside it.
- `--format text|json` (default `text`).

The JSON report has `schema_version: 2` and gives the compiler version, mode, source files, generated project paths, emitted Rust file paths, crate-root markers, file sizes and notes. It lists paths and sizes, not Rust source.

- When checked API metadata is available, a public emitted item carries its declaration's checked docstring as a Rust doc comment.
- Each linker-visible Incan declaration is emitted under a reversible `incan-v1` identifier that encodes its canonical identity and source name.
- Generated Rust is not a stable interface. The stable contracts are Incan source, manifests, checked API metadata and the report schemas on this page.

```bash
incan inspect rust src/main.incn --format json
incan inspect rust . --lib --format json
```

### `incan inspect codegraph`

```text
incan inspect codegraph [OPTIONS] <PATH>
```

Exports codegraph records for an Incan source file or directory as a deterministic JSONL stream.

- `--format jsonl` (the default and only format).
- `--allow-errors`: for source that does not check, emit a partial graph marked degraded, with diagnostic records. Without it, an error fails the command. Warnings of source that checks produce no records.
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options), which select the feature-conditioned facts and the provider-backed facts.

The stream holds files, namespaces, modules, top-level declarations, imports, public exports, checked registry entries, checked capability declarations, checked C binding declarations, direct C calls inside `unsafe:` and the public functions that reach them through a private bridge, body-level reference and call syntax, resolved reference and call targets where resolution proves them, containment, source spans, provenance, degraded state, diagnostics, and the active provider, component and feature projection. The record contract is in [Codegraph inspection](codegraph_inspection.md).

- The header has `languages: ["incan"]` and a `semantic_contexts` entry for each project: its SDK identity and profile, component availability and enablement, package feature closures with activation reasons, and provider identities, participation, artifacts, implementation facets and provenance.
- Every record carries `degraded`, and every record other than the header carries `language` and `provenance`. `language` is `"rust"` on the import record of a `rust::` import, on the reference records for the Rust items it names and their containment records, and on a reference whose checked target is a Rust crate item; it is `"incan"` on every other record.
- A `c_binding` record holds a checked C declaration; a `c_binding_call` record marks a direct symbol call inside `unsafe:` and links it to the binding and to its ordinary `call` record, when there is one.
- Declaration, reference and call records carry `canonical_identity` when resolution proves one; an alias or re-export keeps the original declaration's identity. `target_id` optionally links a record to a declaration record in the same export and is not an identity: a checked identity can come with `target_id: null`. A syntax-only or ambiguous fact has no identity.
- The export is not the runtime `std.graph` module, not a generated-Rust interface, and not a whole-program call graph.

```bash
incan inspect codegraph src --format jsonl --allow-errors
incan inspect codegraph src --format jsonl --features json --sdk-profile minimal
```

### `incan inspect providers`

```text
incan inspect providers [PATH] [OPTIONS]
```

Reports, for `PATH` (default `.`), the active SDK identity and profile, component availability and enablement with selection reasons, provider availability, enablement and use, compiled provider identities and provenance, canonical namespace claims, used modules, active features, private implementation facets and provider manifest paths.

- `--format text|json` (default `text`); JSON is the schema-1 provider report.
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options).

### `incan inspect features`

```text
incan inspect features [PATH] [OPTIONS]
```

Reports, for every active Incan package of `PATH` (default `.`), its active features, optional dependencies, dependency-feature requests, required SDK components, activation reasons, active dependency edges, and feature-conditioned provider facts.

- `--format text|json` (default `text`); JSON is the schema-1 feature report.
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options).

The state and resolution model is in [SDK components and package features](sdk_components_and_package_features.md).

### `incan inspect bindings`

```text
incan inspect bindings [PATH] [--format text|json|receipt] [--target TRIPLE] [OPTIONS]
```

Reports the checked C binding declarations of `PATH` (default `.`).

- `--format text|json|receipt` (default `text`). `json` is the deterministic, source-anchored declaration report: bindings, headers, logical system-library capabilities, symbols, exact C type contracts, enum constants and plain structures. `receipt` is a redaction-safe binding-use receipt.
- `--target <TRIPLE>`: with `--format receipt`, validate and join one current locked Oven interop target. It is refused with the other formats.
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options).

Source that does not check produces diagnostics, not a partial report. Checking runs the host-target C probe. A receipt can join checked use to one locked target and an already-selected execution receipt, and records a target-declared binding-to-artifact relation only for a binding the compilation produced. The command does not resolve native artifacts, compile shims or link. The JSON contract is the [binding inspection schema](binding_inspection_schema.md); the review workflow is in [Inspect checked C bindings](../how-to/inspect_checked_c_bindings.md).

### `incan inspect interop-plan`

```text
incan inspect interop-plan [PATH] --target TRIPLE [--format text|json]
```

Prints the deployment plan for one locked interop target of a package or selected workspace member at `PATH` (default `.`). The target is declared in `[[interop.c.targets]]`, and a current `oven.lock` is required: the workspace root's lock for a member. A plan whose declared interop files or deployment facts changed after locking is refused.

The JSON report gives package-relative input receipts, target, toolchain, SDK and platform requirements, include roots, definitions, dependency-ordered static, bundled and system actions, runtime names, placements, minimum platform constraints, and shim inputs and outputs. The command does not build, stage, link, sign or publish. The plan contract is in [Interop deployment plans](interop_deployment_plans.md).

### `incan inspect representation`

```text
incan inspect representation [PATH] [--format text|json]
```

Reports the executable representation a package publishes: its encoded version, and which public declarations it covers. `PATH` (default `.`) is a package manifest, a generated artifact root, or a project root with artifacts under `target/lib`.

- Coverage comes from the representation's declared index. A declaration covered with an empty body is reported as covered.
- A declaration the publisher refused carries one reason: `unsupported_construct`, `private_dependency`, `unresolved_reference`, `required_declaration_unavailable` or `no_executable_declaration`.
- A member that executes through its declaring type is reported as `type_context`, with that owner.
- A package without a representation reports `none published`. A representation of a version this build cannot read reports that version, and its index is not read.
- The JSON report adds each declaration's canonical identity, a covered declaration's direct public requirements, and each public identity the manifest declares that the index does not mention.
- The command reads only the published representation: it does not compile, execute or decode declarations, or resolve dependencies.

### `incan inspect registry`

```text
incan inspect registry <CANONICAL_IDENTITY> [--project PATH] [--format json]
```

Prints one complete checked typed registry, such as `feature::functions`, or `package::feature::functions` where the short form is ambiguous, without executing user modules. `--project` defaults to `.`. The registry contract is in [`std.registry`](../../language/reference/stdlib/registry.md).

## `incan run`

```text
incan run [OPTIONS] [FILE] [-- <PROGRAM_ARG>...]
```

Compiles and runs `FILE`, the inline code of `-c`, or the project's `[project.scripts].main`. Outside a project, `FILE` or `-c` is required. Arguments after `--` are passed to the executed program in order.

- `-c <CODE>`, `--command <CODE>`: run inline source. It cannot be combined with `FILE` or select a workspace member.
- `--release`: build the release profile; the default is the debug profile.
- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options), the [lock policy options](#lock-policy-options), and `--workspace` and `--member <NAME_OR_PATH>`, which must resolve to one member.

```bash
incan run path/to/file.incn
incan run path/to/file.incn -- --verbose input.txt
incan run
incan run -c "import this"
```

## `incan fmt`

```text
incan fmt [OPTIONS] [PATH]
```

Formats the Incan files under `PATH` (default `.`) in place. A directory is searched recursively, skipping hidden directories, `target/` and `node_modules/`.

- `--check`: change nothing; list each file that would change and exit with `1` if there is one.
- `--diff`: change nothing; print the changes as a diff and exit with `1` if a file would change.
- `--workspace`, `--member <NAME_OR_PATH>`: the [workspace scope](#workspace-scope).

A `PATH` that holds no `.incn` file, or a file that cannot be read, parsed or written, exits with `1`.

```bash
incan fmt .
incan fmt --check .
incan fmt --diff path/to/file.incn
```

## `incan test`

```text
incan test [OPTIONS] [PATH]
```

Collects and runs the tests under `PATH` (default `.`).

| Option | Effect |
| --- | --- |
| `-k <EXPR>` | Select the tests whose stable test id contains `EXPR`. |
| `-m <EXPR>`, `--markers <EXPR>` | Select tests by marker expression (`and`, `or`, `not`, parentheses). |
| `-v`, `--verbose` | Verbose output, with per-test timing. |
| `-x`, `--exitfirst` | Stop after the first failure. |
| `--slow` | Include tests marked `@slow`. |
| `--strict-markers` | Refuse unknown marker names at collection, unless registered in `TEST_MARKERS`. |
| `-j <N>`, `--jobs <N>` | Run up to `N` test batches at once (default `1`); each batch runs its tests on one thread. |
| `--feature <NAME>` | Enable the collection-time `std.testing.feature("NAME")` probe for `skipif` and `xfailif`. |
| `--timeout <DURATION>` | Time limit for each test batch, such as `250ms`, `5s` or `2m`. A test's `@timeout(...)` overrides it. A batch that exceeds its limit is stopped and fails. |
| `--nocapture` | Print test output for passing tests too. |
| `--fail-on-empty` | Exit with `1` when no tests are collected. |
| `--list` | List the selected tests without running them. |
| `--format console\|json` | Console output (default), or JSON Lines results with `schema_version: "incan.test.v1"`. |
| `--junit <PATH>` | Write a JUnit XML report. |
| `--durations <N>` | Print the `N` slowest test durations. |
| `--shuffle`, `--seed <N>` | Shuffle the run order, optionally with a fixed seed. |
| `--run-xfail` | Run `@xfail` tests as ordinary tests. |
| `--workspace`, `--member <NAME_OR_PATH>` | The [workspace scope](#workspace-scope). |

`incan test` also takes the [package-feature and SDK profile options](#package-feature-and-sdk-profile-options) and the [lock policy options](#lock-policy-options).

Each test batch compiles through one prepared direct-`rustc` plan from the Oven store and runs only the test names its binary lists. A batch without a compatible prepared plan is refused. `incan test` does not run Cargo or read a Cargo target directory.

```bash
incan test tests/
incan test -k "addition"
incan test --list -k "test_math"
incan test -m "smoke and not slow" tests/
incan test --format json --junit reports/junit.xml tests/
incan test --shuffle --seed 12345 tests/
incan test --frozen
incan test tests/ --no-default-features --features json --sdk-profile minimal
```

## `incan new`

```text
incan new [OPTIONS] [NAME]
```

Creates a project directory with `loaf.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md` and `.gitignore`. The starter source has a public `greeting()` function and a test that imports it; `incan run`, `incan test` and `incan build --release` succeed on the new project. In an interactive terminal without `--yes`, it prompts for the project metadata; otherwise `NAME` or `--dir` is required.

- `NAME`: the project name.
- `--dir <PATH>`: the directory to create or reuse (default `./<name>`).
- `--description <TEXT>`: written to `[project].description` and `README.md`.
- `--author <AUTHOR>`: written to `[project].authors` as given, such as `Name <email>`.
- `--license <LICENSE>`: a license identifier or expression, written to `[project].license`.
- `--force`: reuse a non-empty directory and overwrite the generated files.
- `-y`, `--yes`: take the defaults and given options without prompting.

```bash
incan new greeter --description "A small greeting command" --license MIT --yes
incan new greeter --dir examples/greeter --yes
```

## `incan init`

```text
incan init [OPTIONS] [PATH]
```

Adds `loaf.toml`, `src/main.incn`, `tests/test_main.incn`, `README.md` and `.gitignore` to the directory `PATH` (default `.`), creating it when it does not exist.

- `--name <NAME>` (default: the directory name).
- `--version <VERSION>` (default `0.1.0`).
- `--description <TEXT>`, `--author <AUTHOR>`, `--license <LICENSE>`: as for `incan new`.
- `--force`: overwrite existing generated files.
- `--detect`: keep an existing `src/main.incn`, and derive the project name from the directory while the placeholder name is in use.
- `-y`, `--yes`: take the defaults and given options without prompting.

The manifest format is in [Project configuration](project_configuration.md).

```bash
incan init --name my_app --description "My app" --license MIT my_project/
incan init --detect --yes
```

## `incan version`

```text
incan version [OPTIONS] [BUMP]
```

Updates `[project].version` in `loaf.toml`.

- `BUMP`: `major`, `minor`, `patch`, `alpha`, `beta`, `rc` or `dev`.
- `--set <VERSION>`: write this SemVer version instead of a bump.
- `--dry-run`: print the change without writing it.
- `--keep-prerelease`: keep prerelease metadata on a `major`, `minor` or `patch` bump.
- `--project <PATH>`: the project root.
- `--workspace`, `--member <NAME_OR_PATH>`: a scope that resolves to one member. They cannot be combined with `--project`.

```bash
incan version patch
incan version rc --dry-run
incan version --set 1.2.3
incan version patch --member packages/api
```

## `incan env`

```text
incan env list [OPTIONS]
incan env show [OPTIONS] [ENV]
incan env run [OPTIONS] <ENV> <SCRIPT> [-- <ARGS>...]
```

Works with the environments declared in `[tool.incan.envs]` of `loaf.toml`. The `default` environment always exists.

- `list` lists the environments. `show` prints one environment resolved, or an overview of all when `ENV` is omitted. `run` runs a script of an environment, with the arguments after `--` appended.
- `--format text|json` (default `text`), for `list` and `show`.
- `--project <PATH>`: the project root.
- `--dry-run`, for `run`: print the resolved command without running it.

Environments are explained in [Project lifecycle](../../language/how-to/project_lifecycle.md).

```bash
incan env show dev --format json
incan env run dev test -- --fail-on-empty
incan env run release build --dry-run
```

## `incan lock`

```text
incan lock [OPTIONS] [FILE]
```

Resolves the project's dependencies (from the manifest, inline declarations and test files) and writes `oven.lock`. The entrypoints are every `[project.scripts]` entry and `src/lib.incn` when it exists; `FILE` adds one more entrypoint whose inline dependencies are included.

- The [package-feature and SDK profile options](#package-feature-and-sdk-profile-options), which shape the locked projection.
- `--cargo-features <FEATURES>`, `--cargo-no-default-features`, `--cargo-all-features`: select Rust dependency features for resolution.

`oven.lock` holds an embedded `Cargo.lock`, the SDK component and provider identities, the public package-feature graph with activation reasons, the private implementation-facet closure, and a fingerprint of the dependency inputs. No command reads an `incan.lock` file.

In a workspace, `incan lock` resolves every member and writes one `oven.lock` at the workspace root, from any member. It creates and reads no member-local lock. Each member is locked with its declared package-feature activation: `--features`, `--no-default-features` and `--all-features` do not reach the lock, while `--sdk-profile` and the Cargo feature options apply to every member. Publication is atomic; see [Project lifecycle](../../language/reference/project_lifecycle.md) for the locking protocol.

Dependency management is covered in [Managing dependencies](../how-to/dependencies.md).

```bash
incan lock src/main.incn
incan lock --features metrics
incan lock --sdk-profile minimal
incan lock --cargo-features metrics
```

## `incan workspace inspect`

```text
incan workspace inspect [OPTIONS]
```

Prints the validated workspace graph and the member scope this invocation selects. Membership comes from the workspace manifest, never from paths. The command reads the manifests and the root `oven.lock`; it does not resolve dependency versions. Outside a workspace it exits with `1`.

- `--format text|json` (default `text`).
- `--workspace`, `--member <NAME_OR_PATH>`: the scope to project.

The JSON report has these fields:

| Field | Contents |
| --- | --- |
| `schema_version` | `1`. |
| `invocation` | `current_dir`. |
| `workspace` | The canonical `root` and `manifest_path`. |
| `members` | Each member in order: `name`, `root`, `manifest_path`, `is_root_member`, `effective_dependencies` (`libraries`, `rust` and `rust_dev`, each dependency with the `origin` of its declaration, `member` or `workspace`, and a Rust dependency with its `workspace_features` and `member_features`), `workspace_environment_extends`, and `capabilities: []`. |
| `default_members`, `exclusions` | The default member names, and the workspace manifest's `exclude` entries. |
| `selected_scope` | The selection `origin` and the selected `members`, as in the [workspace scope](#workspace-scope) reports. |
| `lock` | The root lock's `canonical_path`, `exists`, `state` (`status` `present` with the lock's `fingerprint`, `cargo_features` and `semantic` graph; `invalid` with `error`; or `missing`), and `stale_member_local_locks`: each member-local `oven.lock` path. |
| `shared_dependencies` | The workspace's `libraries`, `rust` and `rust_dev` declarations. |
| `shared_environments`, `shared_policy`, `shared_sources` | The workspace manifest's environment, policy and source sections. |
| `capabilities` | `status: "member_local_only"` and a `reason`. |
| `warnings` | `unmatched_glob` with its `pattern`, and `unused_shared_dependency` with its `dependency_kind` and `name`. |

```bash
incan workspace inspect --workspace --format json
incan workspace inspect --member packages/api --format json
```

## `incan tools`

### `incan tools doctor`

```text
incan tools doctor [--format text|json]
```

Reports:

- the running `incan` version and executable path;
- how `PATH` resolves `incan` and `incan-lsp`;
- whether `~/.cargo/bin/incan` and `~/.cargo/bin/incan-lsp` exist, are executable, and what they link to;
- guidance on the editor settings `incan.lsp.path` and `incan.compiler.path`, and how to reload;
- offline readiness: local signals that decide whether locked Rust inputs are available without fetching. A ready report does not guarantee that a later `--frozen` build or test succeeds.

Using the report is covered in [Troubleshooting](../how-to/troubleshooting.md).

### `incan tools metadata api`

```text
incan tools metadata api [PATH] [--format json|markdown]
```

Prints the checked public API of `PATH` (default `.`); for a directory, of `src/lib.incn`, else `src/main.incn`. The source is type-checked first. It does not build, generate Rust, or read an existing `.incnlib`.

- `--format json` (default): the checked API metadata.
- `--format markdown`: a Markdown API reference from the same metadata.

The JSON package holds `schema_version: 2`, `package` (the project name, and version when declared; omitted without a project name), `public_namespaces` (omitted when empty), and `modules` (the entry module and its imported local modules); each module holds its `declarations`: public functions, models, classes, traits, enums, newtypes, type aliases, consts, statics, public import aliases and public partial presets, each with an `anchor` (stable id and byte span). Functions, models, classes, traits, enums, newtypes and methods also carry their `docstring`, parsed `docstring_sections` and resolved `decorators`.

The recognized docstring sections are `Args:` (or `Parameters:`), `Returns:`, `Fields:`, `Aliases:` and `Decorators:`. A docstring that contradicts the checked source produces diagnostics and no JSON: an `Args:` or `Fields:` section that omits a checked parameter or field; an entry in any section that names no checked parameter, field, alias or decorator, or repeats a name; or a `Returns:` type that differs from the checked return type.

The JSON contract is in [Checked API metadata](checked_api_metadata.md).

### `incan tools metadata model`

```text
incan tools metadata model PATH MODEL [--format incan|json]
```

Prints one contract-backed model from a project's declared bundle metadata, a bundle JSON file, or a built `.incnlib`. `MODEL` is the bundle's `logical_type_name` or `stable_model_id`.

- `--format incan` (default): the model as formatted Incan source.
- `--format json`: the canonical model bundle.

The bundle contract is in [Checked contract metadata](contract_metadata.md).

## Outputs and paths

| Output | Path |
| --- | --- |
| Generated Rust project | `target/incan/<name>/` |
| Built binary | `target/incan/<name>/oven/release/<name>` |
| Library artifact (`--lib`) | `target/lib/<name>.incnlib`, beside the generated library output |
| Backend-selection receipt | `.incan/backend/receipt.json` |
| Oven store | `$INCAN_HOME/oven/store/v2`, or `~/.incan/oven/store/v2` |

`target/incan/` holds generated source and the project's published binary. Removing it while no Incan command is running is safe.

## Environment variables

| Variable | Effect |
| --- | --- |
| `INCAN_HOME` | Root of the Oven store and other user-level Incan state (default `~/.incan`). |
| `INCAN_STDLIB` | Overrides the detected standard-library directory. |
| `INCAN_NO_BANNER=1` | Suppresses the logo banner. |
| `NO_COLOR` | Disables ANSI color. |
| `INCAN_LOCKED`, `INCAN_OFFLINE`, `INCAN_FROZEN` | See [Lock policy options](#lock-policy-options). |
| `INCAN_OVEN_MAX_PHYSICAL_BYTES`, `INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES`, `INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES` | See the [store policy](#incan-oven). |
| `INCAN_EMIT_SERVICE=1` | Switches the code-emission mode. Not stable. |

## Exit codes

A command exits with `0` on success and a nonzero code on failure.

| Command | Exit code |
| --- | --- |
| `incan run` | The program's exit code. |
| `incan test` | `0` when every test passes, and when test files exist but no test is collected; `1` when a test fails, an `@xfail` test passes, no test file is found under `PATH`, or no test is collected under `--fail-on-empty`. |
| `incan fmt` | `1` when a file would be reformatted under `--check` or `--diff`, when `PATH` holds no `.incn` file, or when a file cannot be read, parsed or written. |
| `incan build`, `incan check`, `incan FILE` and the debug options | `1` on a compile or build error. |
| `incan explain` | `1` for a code outside the catalog. |
| `incan -h`, `incan -V` and every `--help` | `0`. |
| `incan` with no command, `FILE` or debug option | `1`, after printing the help. |
| Any command given an unknown option or a missing argument | `1`. |
