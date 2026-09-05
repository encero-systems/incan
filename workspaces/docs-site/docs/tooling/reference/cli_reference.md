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
- `cache` - Inspect and prune Incan-managed generated-build storage
- `oven` - Run the explicit Oven Alpha receipt, bounded-store, and native direct-rustc workflow
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

Incan 0.5 extends the machine-readable inspection surfaces introduced in 0.4. Use `incan check --format json` for the stable diagnostic plane, `incan build --report json` for successful build and artifact metadata (including the `backend` field described below), `incan inspect backend-selection --format json` for a persisted backend-selection execution receipt, `incan inspect rust --format json` for current generated Rust output, `incan inspect codegraph --format jsonl` for source-structure graph facts, `incan inspect providers --format json` for SDK component and provider participation, `incan inspect features --format json` for the additive package-feature graph, `incan inspect bindings --format json` for checked C declaration facts, `incan inspect bindings --format receipt` for redacted checked binding use, and `incan inspect interop-plan --format json` for one locked Oven interop platform handoff.

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

They are supported by `build`, `check`, `run`, `test`, and `lock`, plus the `inspect codegraph`, `inspect providers`, `inspect features`, and `inspect bindings` projections. The package-feature flags do not forward names to Cargo. Cargo-prefixed feature flags remain compatibility inputs only where explicitly documented; normal Oven build/run/test commands do not use them to publish a missing closure.

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

### Workspace scope

When the current project belongs to a workspace, supported project commands resolve a member scope before doing work. `--workspace` selects every member; repeatable `--member <name-or-path>` selects named or root-relative members. Without a selector, a command run inside a member selects that member; a workspace root uses its `default-members`, its implicit root member, or all members for a virtual root without defaults.

`check`, `build`, `test`, and `fmt` fan out in deterministic member order. `run` and `version` require exactly one member. Machine-readable check/build/test output carries the selected workspace and member identity. `incan lock` is always workspace-wide and ignores command scope when resolving the canonical root lock.

### `incan check`

Usage:

```text
incan check [OPTIONS] [PATH]
```

Type-checks a file or project entrypoint without building or running it. This is the canonical type-check command for humans, CI, editor integrations, and agents. Passing a file without a subcommand still type-checks it for compatibility, and the legacy debug spelling `incan --check <FILE>` remains available.

Options:

- `--format text|json`: Output human diagnostics or a stable machine-readable JSON report (default: `text`).
- `--features`, `--no-default-features`, `--all-features`: Select the root package-feature projection.
- `--sdk-profile <PROFILE>`: Select a non-persistent SDK profile for this check.
- `--interop-target <TRIPLE>`: Verify checked C declarations against one exact target declared by `[oven.interop]`. This does not cross-compile generated Rust or package an application.
- `--workspace`: Check every selected workspace member.
- `--member <NAME_OR_PATH>`: Check one or more selected workspace members.

JSON output uses `schema_version: 2` and prints a deterministic report with `ok` and `diagnostics`. Each diagnostic includes a stable `code`, `severity`, compiler `phase` and `origin`, primary source span, message, notes, hints, labeled related spans, and an `incan explain <CODE>` hook. `diagnostics` carries non-fatal warnings as well as errors, so `ok` reports whether any error-severity diagnostic was found rather than whether the array is empty: a successful check that produced warnings reports `ok: true`, a non-empty `diagnostics` array, and a success exit code. Filter on `severity` to separate the two. Warnings are ordered by source module, and by parser before typechecker within each module, so repeated runs over unchanged sources produce identical output. Diagnostics that compare two compiler-known values also include optional `expected` and `actual` fields, so consumers do not need to parse the message. The LSP and `incan inspect codegraph --allow-errors` project these same compiler-owned facts; codegraph uses byte offsets for source spans while the diagnostic JSON and LSP retain line and column positions. Human output remains the default and continues to use source-highlighted compiler diagnostics.

Examples:

```bash
incan check src/main.incn
incan check src/main.incn --format json
incan check --interop-target aarch64-linux-android src/main.incn
incan --check src/main.incn --format json
incan check --workspace --format json
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
- `INCAN-T0101`: Unreachable code — statements that follow a `return` in the same block. Reported as a warning, so the program still compiles.
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
incan build [OPTIONS] [FILE] [OUTPUT_DIR]
```

Behavior:

- Default mode compiles a source file into an executable.
- Prints the generated Rust project path (default example): `target/incan/<name>/`
- Builds the generated Rust project with a receipt-selected, store-owned direct-`rustc` closure and prints the binary path. Generated source and the published final binary stay under the default project `target/incan/` tree, or under the positional caller-selected `OUTPUT_DIR`.
- `--lib` builds a project library from `src/lib.incn`, emits its checked `.incnlib` manifest, and publishes
  caller-owned debug and release `rlib` outputs through the receipt-selected direct-`rustc` plan. It never restores a
  Cargo backend.

Dependency flags:

- `--features`, `--no-default-features`, `--all-features`: Select public Incan package features for this build.
- `--sdk-profile <PROFILE>`: Select a non-persistent SDK profile for this build.
- `--release`: Explicitly request the release profile. This is the default for `incan build`. Compiler-selected integer `abs` and builtin `sum` remain checked in every profile; the flag does not select different language semantics.
- `--backend <legacy|replacement>`: Declare the compiler backend for this build (#986). Defaults to `legacy`, declared explicitly even when the flag is omitted. `replacement` builds one session-owned analysis for the selected entry and package-feature projection, then directly executes the admitted zero-argument `main` Body-IR profile and its permitted same-module calls. Its bounded profile also admits compiler-selected global `enumerate(list)` and `zip(list, list)` over recursively structural checked list items: enumeration is zero-based, and Zip preserves written left/right order until its shorter list ends. Compiler-selected `len` over admitted source-local hashed sets and dictionaries returns their distinct entry/key count, including typed-empty constructors; this does not admit iteration or other aggregate operations. Compiler-selected `bool` observes only represented booleans, integers, strings, lists, sets, and dictionaries as zero/empty or nonzero/nonempty; float, tuple, bytes/frozen, Option/Result, generator, nominal, and custom truthiness remain refusals. Compiler-selected `sorted` returns a fresh ascending list only for nonempty represented integer lists; empty or non-integer lists and custom ordering remain refusals. Exact signed/unsigned widths, `f32`/`f64`, and checked decimals retain their carrier through literals, locals, lossless widening, same-module calls, entry arguments/results, Display output, receipts, and reports. Public direct/shadow exact `f32`/`f64` carriers are finite-only; ordinary `float` parsing retains its existing separately compared NaN/infinity behavior. Arithmetic on these exact carriers, their unary and resize operations, Debug formatting, aggregates, matching, and decimal scalar casts still refuse before effects under #988. Compiler-owned `isinstance(value, Target)`, including explicit `std.builtins.isinstance(...)`, consumes retained checked value and target types plus the target span. The bounded target set is `int`, `bool`, `str`, and ordinary binary `float`, and the checked value must be one of those scalars or a union made only from them. Unsupported value or target shapes refuse before effects under the broader #988 replacement-execution owner, a same-named source function keeps its own identity, and this is not general runtime reflection or type-value support. A same-name declaration or import does not borrow builtin provenance, and this is not general iterator support. Package and public imports, unsupported aggregate shapes, and other source outside that bounded #988 profile still refuse visibly. Compiler-resolved `let`/`mut` shadowing and reassignment execute through the exact retained local identity; the executor never infers scope from a repeated spelling. It does not create generated Rust or an Oven plan. See [Backend selection & execution receipts](../explanation/backend_selection_receipts.md).
- `--backend-fallback <refuse>`: Declare the visible refusal policy for an unavailable backend. The #988 direct profile exposes no legacy fallback target until it has a receipt-bound legacy execution path.
- `--shadow`: Request a source-observable shadow comparison against the replacement backend alongside normal execution. A build observes the module's `main`, whose return value the produced legacy process does not expose, so on this path the comparison is always recorded explicitly as unavailable with that reason — generated Rust is not semantic proof. The bounded profile that does run a real two-route comparison is described in [Backend selection & execution receipts](../explanation/backend_selection_receipts.md#source-observable-shadow-comparison).
- `--report json`: Emit a versioned machine-readable build report. The session-owned direct replacement path emits its own `incan.replacement_execution.v1` report because it has no generated Rust, native artifact, or Oven plan; its top-level `semantic_module` identifies the selected checked module and its source and semantic-snapshot identities, and `replacement_execution.result_type` retains the exact checked scalar result kind.
- `--report-output <PATH>`: Write the build report to a file instead of stdout. Required with `--backend replacement --report json`, because replacement execution reserves stdout and stderr for the program and diagnostics. Omitting the path is rejected before execution.
- `--workspace`: Build every selected workspace member.
- `--member <NAME_OR_PATH>`: Build one or more selected workspace members.

Normal build, run, and test first select a compatible immutable full-stdlib Loaf from the active toolchain. A project outside that envelope selects its receipt-bound extension from `$INCAN_HOME/oven/store/v2`, or `~/.incan/oven/store/v2` when `INCAN_HOME` is unset, together with the exact base Loaf recorded by the extension. The plan selection key includes target, toolchain, profile, Incan runtime, dependency, feature, and provider inputs. The receipt remains source-strict, so a source change regenerates caller-owned source and receipt while a compatible project can reuse the same immutable base plus extension. `incan inspect oven --receipt PATH --format json` reports the receipt/build-unit identities, selection hit or miss with reason, and physical/logical/reclaimable/lease-protected storage.

Legacy and Oven builds use `schema_version: 1` and describe the successful Oven build rather than restating terminal prose. They include source and generated paths, emitted artifacts, dependency and provider summaries, `oven.receipt_identity`, `oven.build_unit_identity`, `oven.plan_identity`, prepare/build/total elapsed time, a note that no normal Cargo consumer ran, and a `backend` field carrying the build's backend-selection execution receipt (#986). The #988 direct replacement profile instead uses `schema_version: "incan.replacement_execution.v1"` with `status`, `mode`, `entrypoint`, `backend`, top-level `semantic_module`, direct `replacement_execution` evidence (`result`, exact checked `result_type`, exact `stdout_bytes` and `stderr_bytes` byte arrays, the completed-print `emitted_output` projection, `output_identity`, Body-IR snapshot, canonical ownership reads, and runtime requirements), and total elapsed time. The report's `semantic_module` and `backend.semantic_module` name the same selected session-owned module with its source and semantic-snapshot identities. Replacement program output is written to its ordinary streams during execution; a later runtime or receipt-writing failure does not suppress bytes already written. Print calls flush before execution continues. JSON metadata requires `--report-output <PATH>` and never redirects program stdout to stderr. Byte arrays preserve per-stream bytes without assuming UTF-8; they do not claim a total order between the two streams. It intentionally omits `generated`, artifacts, and `oven`: those facts do not exist for direct Body-IR execution. A completed-output reuse is allowed only for the implicit legacy default and retains the verified receipt sealed by its explicit bake; explicit backend, fallback, and shadow requests take the normal preparation path instead.

A successful build also publishes that same receipt to `.incan/backend/receipt.json` in the project root, independent of `--report`. Inspect it with `incan inspect backend-selection --receipt .incan/backend/receipt.json`. See [Backend selection & execution receipts](../explanation/backend_selection_receipts.md).

Environment defaults:

- `INCAN_OVEN_MAX_PHYSICAL_BYTES=<BYTES>` changes the Oven aggregate physical allocation limit.
- `INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES=<BYTES>` and `INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES=<BYTES>` change the one-domain admission limits.

Examples:

```bash
incan build examples/simple/hello.incn
incan build src/main.incn --report json
incan build src/main.incn --features json,http --sdk-profile minimal
incan build --release
incan build src/main.incn --report json --report-output target/build-report.json
incan build src/main.incn --backend replacement --backend-fallback refuse
```

### `incan cache`

Usage:

```text
incan cache inspect [--category generated-cargo] [--format text|json]
incan cache prune [--category generated-cargo] [--dry-run] [--max-bytes BYTES] [--format text|json]
incan cache prune --identity SHA256 [--identity SHA256 ...] [--dry-run] [--format text|json]
```

`inspect` reports the cache root, configured soft limit, recursive logical file bytes, full compatibility identities, profiles, last-use timestamps, and active-use state. `prune` removes least-recently-used idle domains toward the configured or command-local limit. Repeating `--identity` instead selects exact domains and cannot be combined with `--max-bytes`. Cleanup never removes a domain with an active build lease; use `--dry-run` to preview the removable domains and logical bytes. JSON reports include numeric `schema_version: 1`; `removed_logical_bytes` is the captured logical size of successfully removed domains, and all byte fields can differ from filesystem allocation or APFS clone accounting.

### `incan oven` (Alpha)

Usage:

```text
incan oven bake [--project PATH] [--features FEATURE,...] [--no-default-features]
                [--all-features] [--format text|json]
incan oven import --target TRIPLE --toolchain IDENTITY [--project PATH] [--profile PROFILE]
                  [--feature NAME ...] [--source NAME=PATH ...] [--output PATH] [--format text|json]
incan oven plan publish --receipt PATH --manifest PATH --artifact-root PATH --domain NAME
                         [--store PATH] [--max-physical-bytes BYTES]
                         [--max-domain-physical-bytes BYTES] [--max-domain-logical-bytes BYTES]
                         [--format text|json]
incan oven store inspect|prune [--store PATH] [--max-physical-bytes BYTES]
                               [--max-domain-physical-bytes BYTES] [--max-domain-logical-bytes BYTES]
                               [--format text|json]
incan inspect oven --receipt PATH [--store PATH] [--max-physical-bytes BYTES]
                   [--max-domain-physical-bytes BYTES] [--max-domain-logical-bytes BYTES]
                   [--format text|json]
incan oven test --receipt PATH --plan SHA256 --rustc PATH --source PATH
                --output PATH --crate-name NAME --source-evidence NAME --exact TEST [--exact TEST ...]
                [--edition 2021|2024] [--store PATH] [--max-physical-bytes BYTES]
                [--max-domain-physical-bytes BYTES] [--max-domain-logical-bytes BYTES]
                [--format text|json]
incan oven run --receipt PATH --plan SHA256 --rustc PATH --source PATH
               --output PATH --crate-name NAME --source-evidence NAME
               [--edition 2021|2024] [--store PATH] [--max-physical-bytes BYTES]
               [--max-domain-physical-bytes BYTES] [--max-domain-logical-bytes BYTES]
               [--format text|json] [-- ARG ...]
```

Oven is the Alpha normal consumer backend for `incan build`, `incan test`, and `incan run`; the `incan oven` commands expose its receipt and maintenance boundary. The release ships the full checked standard-library/provider closure as one Loaf family with target/profile-specific debug and release variants.

`bake` is the project-facing preparation command. Its `--features`, `--no-default-features`, and `--all-features` flags select the same public Incan package-feature projection used by normal commands. It records debug and release receipts, reports `toolchain_loaf` when a profile selects the immutable full-stdlib Loaf directly, and reports `reused` on an exact warm project extension. A project that needs artifacts outside the installed standard-library envelope may explicitly invoke the bounded compatibility publisher once. The resulting `<identity>.loaf/loaf.json` names the exact base Loaf, canonicalizes compiler-owned runtime artifacts, overlapping locked registry units, and vocabulary auxiliaries against that verified release cohort, retains only the project-owned third-party and provider fragment, and binds each direct registry alias to the exact locked package, version, registry, and checksum. For every discovered target and profile, including each distinct declared executable entrypoint, `bake` also compiles the caller-owned final output through direct `rustc`, seals checked `.incnlib` metadata and declared provider sidecars, and publishes a completed project-output Loaf under a stable target identity for exact replay. The final output remains project-owned rather than becoming part of the shared dependency Loaf; normal build/test/run never invoke Cargo or use a fallback.

Project-extension schema 9, packaged-library schema 6, completed-output schema 12, and project-inspection-authority schema 1 form the current Alpha handoff. Completed-output schema 12 retains schema 11's singular inspection authority, isolates every declared executable by stable target identity and profile, keys additional entrypoints by project-relative path, and carries report schema 2 for frontend-free replay. The report represents project paths with tagged values and machine-specific external authorities with opaque slots, so replay can restore caller-owned paths without treating another machine's external path spelling as authority. Canonical semantic lock authority excludes only the derived dependency fingerprint: a non-strict normal command warns and reuses when only that fingerprint is stale, while `--locked` and `--frozen` reject the stale lock before output selection. Rust path-dependency authority also includes the exact Cargo workspace package and dependency facts selected through `workspace = true`. A completed output selects its exact inspection authority before frontend work, so a source-current, feature-matched bake can serve subsequent application, library, and test consumers without mixing authority from another receipt lineage; any meaningful change to source, semantic lock content, feature projection, target, profile, compiler, SDK, dependency, or sealed artifact misses rather than weakening that identity.

A release envelope may select registry-source authority independently from its feature-unified linkable closure, so supported standard-library Rust facades can be inspected without mixing direct-`rustc` artifacts. `import` reads frozen Cargo declarations only as compatibility evidence and does not run Cargo.

The internal release publisher is the sole Cargo-backed producer for supported Oven Alpha envelopes. It creates or exactly reuses the release or compiler-suite standard-library Loaf family, and the compiler-suite path also prepares or reuses the bounded receipt-compatible suite store. Each `loaf.json` binds the direct-`rustc` plan, declared artifacts, compatibility, provenance, digests, and byte accounting. The explicit publisher Cargo may differ from the consumer `rustc`; the latter defines the Loaf toolchain identity. Compiler-suite JSON reports distinguish `complete-suite`, `selected-complete-roots`, and `exact-diagnostic` selection modes and expose separate `complete_root_success` and `complete_suite_success` verdicts.

`plan publish` validates a publisher-provided direct-rustc manifest and copies its declared regular-file closure into one immutable policy-bounded entry. `test` selects that stored plan and closure with an active lease, inventories the produced native libtest binary, and rejects every `--exact` name absent from that inventory before test execution. `run` compiles and executes one caller-owned binary from the same stored closure, retains the entry lease through process completion, and forwards arguments only after `--` to that binary.

`interop bake` and `interop stage` are the separate v0.5 C-ABI publisher and handoff commands. They consume a current package lock, explicit compiler/SDK evidence, a sealed base plan, and declared package files to bake native archives or stage digest-verified bundled runtime files. They never invoke Cargo, Gradle, Xcode, signing, or physical devices.

The default Oven store is `$INCAN_HOME/oven/store/v2`, or `~/.incan/oven/store/v2` when `INCAN_HOME` is unset. Its everyday-developer policy retains at most 9 GiB of aggregate physical allocation, with a 6 GiB physical and 6 GiB logical allowance per compatibility domain. The compiler-suite policy explicitly raises those limits to 16 GiB aggregate physical, 6 GiB domain physical, and 4 GiB domain logical. The aggregate allowance includes the previous committed Loaf generation while a replacement is staged, so an interrupted update does not require deleting the last valid generation. Physical allocation and logical artifact bytes—plan bytes plus copied manifest-declared files—are distinct report fields. `incan oven store inspect` also reports reclaimable and lease-protected physical allocation; `incan inspect oven --receipt PATH` adds the receipt/build-unit identity plus a `hit`, `miss`, or `ambiguous` selection reason. Environment overrides use whole-byte values: `INCAN_OVEN_MAX_PHYSICAL_BYTES`, `INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES`, and `INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES`. Publication is fail-closed when a single domain exceeds its allowance or active leases prevent safe reclamation. See [Oven Alpha](../explanation/oven_alpha.md) for the compatibility envelope, artifact-plan schema boundary, and exclusions.

### `incan inspect backend-selection`

Usage:

```text
incan inspect backend-selection --receipt PATH [--format text|json]
```

Reads, verifies, and renders one persisted backend-selection execution receipt (#986), normally `.incan/backend/receipt.json` written by a successful `incan build`. Verification calls `BackendExecutionReceipt::verify_identity`, which recomputes the receipt's content identity and the selection identity it embeds; a receipt whose recorded identity does not match its own content — tampered, hand-edited, or stale — is refused rather than rendered. `--format text` prints the selected/executed backend, selection reason, fallback policy and outcome, shadow-comparison state, compiler version, and both content identities; `--format json` prints the full receipt. See [Backend selection & execution receipts](../explanation/backend_selection_receipts.md).

Examples:

```bash
incan inspect backend-selection --receipt .incan/backend/receipt.json
incan inspect backend-selection --receipt .incan/backend/receipt.json --format json
```

### `incan inspect rust`

Usage:

```text
incan inspect rust [OPTIONS] <PATH>
```

Generates the same Rust project that the current backend would build, then reports the emitted Rust files without invoking Cargo. Use this when you need to locate backend output, debug generated paths, or hand artifact locations to tooling without treating `--emit-rust` as a structured interface. The command reports file paths and sizes rather than Rust source, so reading the generated code means opening those files, where declarations appear under their `incan-v1` identifiers rather than their source names.

Options:

- `--lib`: Inspect the library build surface rooted at `src/lib.incn`; `PATH` may be the project root or a source path inside the project.
- `--format text|json`: Output a human file list or a versioned JSON report (default: `text`).

JSON output uses `schema_version: 1` and includes the compiler version, mode, source-file breadcrumbs, generated project paths, emitted Rust file paths, crate-root markers, file sizes, and notes. Source declarations with checked docstrings preserve those docs as generated Rust doc comments for public emitted items when the compiler has checked API metadata available. Generated Rust is inspectable current backend output, not a stable Rust ABI contract; tools may use it for debugging and reporting, but public compatibility should be based on Incan source, manifests, checked API metadata, and documented CLI report schemas. Under RFC 120 each linker-visible Incan-origin declaration is emitted under a reversible `incan-v1` identifier carrying its canonical identity, so a tool that needs source names decodes that payload rather than reading the identifier literally.

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

Exports compiler-backed codegraph records for an Incan source file or directory. The export is a deterministic JSONL stream of Incan-language files, modules, top-level declarations, imports, public exports, checked C binding declarations, direct C calls admitted through `unsafe:`, body-level reference and call syntax, conservative resolved reference and call targets, containment relationships, source spans, provenance, degraded state, diagnostics, and the active provider/component/feature projection. It is intended for tools and agents that need Incan structure without scraping source text.

Options:

- `--format jsonl`: Emit newline-delimited JSON records. JSONL is the only supported 0.4 format.
- `--allow-errors`: Emit a degraded partial graph and diagnostic records when the source is broken. Without this flag, diagnostics fail the command.
- `--features`, `--no-default-features`, `--all-features`: Select which feature-conditioned source facts enter the graph.
- `--sdk-profile <PROFILE>`: Select the SDK profile used to resolve provider-backed facts.

`incan inspect codegraph` is tooling output, not runtime `std.graph`, not a generated-Rust ABI, and not a whole-program reference/call graph. The header lists the represented languages and has a typed `semantic_contexts` entry for each represented project. Each context records the SDK identity and profile, component availability and enablement, package feature closures and activation reasons, and provider identities, participation, artifacts, implementation facets, and authority provenance. Every fact record carries `language`, `provenance`, and `degraded` fields. A checked `c_binding` record contains the structural C declaration contract, while `c_binding_call` marks an explicit-unsafe direct symbol call and links it to the binding and, where present, its ordinary `call` record. Those records are not artifact, shim, runtime-library, bridge/facade, or editor receipts. Schema v7 declaration, reference, and call records carry a structured compiler-owned `canonical_identity` when resolution proves one. Aliases and re-exports preserve the original declaration identity. `target_id` remains an optional link to a declaration record in the same JSONL export and is never used to infer identity; a checked external-package or builtin identity can therefore coexist with `target_id: null`. Syntax-only and ambiguous facts remain identityless. The exporter emits `language: "incan"` only; first-class Rust graph records remain follow-up work.

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

### `incan inspect interop-plan`

Usage:

```text
incan inspect interop-plan [PATH] --target <TRIPLE> [--format text|json]
```

Projects a standalone package or selected workspace member's exact locked Oven interop target into a deterministic, versioned deployment handoff. The command requires the selected target in `[[oven.interop.targets]]` and a current canonical `incan.lock`; workspace members use the single workspace-root lock. It refuses to emit a plan after a declared interop file or deployment fact changes.

The JSON report contains package-relative input receipts, target/toolchain/SDK/platform requirements, include roots, definitions, dependency-ordered static, bundled, and system actions, runtime names, placements, minimum platform constraints, and governed shim inputs and logical outputs. It does not build, stage, link, sign, publish, or invoke Gradle or Xcode.

Examples:

```bash
incan inspect interop-plan --target aarch64-linux-android
incan inspect interop-plan . --target aarch64-apple-ios --format json
incan inspect interop-plan packages/mobile --target aarch64-apple-ios --format json
```

See [SDK components and package features](sdk_components_and_package_features.md) for the state and resolution model.

### `incan inspect bindings`

Usage:

```text
incan inspect bindings [PATH] [--format text|json|receipt] [--target <TRIPLE>] [OPTIONS]
```

Reports compiler-checked C binding declaration facts for a source file or project directory. The JSON report is deterministic and source-anchored: it includes bindings, headers, logical system-library capabilities, symbols, exact C type contracts, enum constants, and plain structures. `--format receipt` instead emits a redaction-safe binding-use receipt, optionally joined to one locked Oven target. `PATH` defaults to the current project.

Options:

- `--format text|json`: Output a concise terminal summary or the schema-versioned JSON report (default: `text`).
- `--features`, `--no-default-features`, `--all-features`: Select which feature-conditioned declarations are checked and projected.
- `--sdk-profile <PROFILE>`: Select the SDK profile used to resolve the checked source graph.
- `--target <TRIPLE>`: With `--format receipt`, validate and join one current locked Oven interop target; it is rejected with the declaration-report formats.

The command is strict: it emits ordinary compiler diagnostics rather than partial data when the source graph is not valid. Its checked compilation path runs the normal host-target C probe. The declaration formats remain declaration projections; receipt mode can join compiler-checked use to one locked target and an already-selected execution receipt, and can retain an explicit target-declared binding-to-artifact-name relation only after compilation produces that exact binding. It does not resolve native artifacts, compile shims, emit a linker invocation, classify a complete bridge/façade design, or expose LSP data. Use the [inspection how-to](../how-to/inspect_checked_c_bindings.md) for the review workflow and the [binding inspection JSON schema](binding_inspection_schema.md) for the machine contract.

Examples:

```bash
incan inspect bindings
incan inspect bindings src/main.incn --format json
incan inspect bindings . --format json --features sqlite --sdk-profile minimal
incan inspect bindings . --format receipt --target aarch64-apple-darwin
```

### `incan run`

Usage:

```text
incan run [OPTIONS] [FILE] [-- <PROGRAM_ARG>...]
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

- `--features`, `--no-default-features`, `--all-features`, and `--sdk-profile` select the Incan/package compatibility inputs.
- `--release`: Build and run with the Oven release profile.
- `--workspace`, `--member <NAME_OR_PATH>`: Select a scope that must resolve to exactly one member. Inline `-c` code cannot select a member.

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
incan fmt --workspace --check
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
| `-v`                             | Verbose output, including per-test timing                                                                    |
| `-x`                             | Stop on first failure                                                                                    |
| `--slow`                         | Include slow tests (marked `@slow`)                                                                      |
| `--strict-markers`               | Reject unknown marker names during collection unless registered in `TEST_MARKERS`                        |
| `-j <N>` / `--jobs <N>`          | Run up to `N` Oven native-test batches concurrently (single-threaded libtest execution per batch)          |
| `--feature <NAME>`               | Enable collection-time `std.testing.feature("NAME")` probe for `skipif` / `xfailif`                      |
| `--timeout <DURATION>`           | Rejected by Oven Alpha until native timeout enforcement is implemented                                    |
| `--nocapture`                    | Print child test output even for passing tests                                                           |
| `--fail-on-empty`                | Return exit code 1 if no tests are collected                                                             |
| `--list`                         | List collected tests after filters without executing them                                                |
| `--format console\|json`         | Choose human console output or JSON Lines result output (`schema_version: "incan.test.v1"`)              |
| `--junit <PATH>`                 | Write a JUnit XML report                                                                                 |
| `--durations <N>`                | Print the slowest `N` test durations                                                                     |
| `--shuffle`                      | Shuffle test execution order                                                                             |
| `--seed <N>`                     | Seed for `--shuffle`                                                                                     |
| `--run-xfail`                    | Run `@xfail` tests as ordinary tests                                                                     |
| `--workspace`                    | Run every selected workspace member                                                                      |
| `--member <NAME_OR_PATH>`        | Run one or more selected workspace members                                                               |

Dependency flags (same as `build`):

- `--features`, `--no-default-features`, `--all-features`, and `--sdk-profile` select the Incan/package compatibility inputs.

Oven Alpha rejects `--timeout` and `@timeout("duration")` rather than silently running without enforcement. Native timeout enforcement, including its fixture-teardown contract, remains outside the supported Alpha envelope.

Each batch generates a caller-owned test harness and receipt, selects exactly one prepared direct-`rustc` plan from the bounded Oven store, compiles the harness, inventories native libtest names, and then runs only verified exact names. A missing compatible plan fails as an unsupported Oven-native provider/dependency envelope; `incan test` never runs Cargo, reads a generated Cargo target directory, or tells the user to prepare Cargo state.

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
incan test --workspace --format json
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
- `--workspace`, `--member <NAME_OR_PATH>`: Select a scope that must resolve to exactly one member. These conflict with `--project`.

Examples:

```bash
incan version patch
incan version rc --dry-run
incan version --set 1.2.3
incan version minor --project examples/greeter
incan version patch --member packages/api
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

Inside a workspace, `incan lock` always resolves every member's effective dependencies and publishes the one canonical root `incan.lock`, even when invoked from one member. It does not create or consume member-local locks. Cooperative publishers serialize generation and publication with a stable advisory lock under compiler-owned `target/incan_lock` state and replace the completed root lock atomically after synchronizing its staged contents. If a legacy project-root `.incan.lock.incan.lock` already exists, new compilers acquire it before the hidden guard so an older compiler still using that inode remains serialized. A project without the legacy sidecar uses the new protocol directly. Whenever that sidecar is absent—whether it never existed or was removed—do not run old and new compilers concurrently because an older compiler cannot discover the hidden guard.

Because the lock refresh covers every member, command-local `--features`, `--no-default-features`, `--all-features`, and `--sdk-profile` selections apply to every member projection in that refresh. A requested public feature must therefore be declared by each workspace member; use member manifests for different persistent per-member selections.

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

### `incan workspace inspect`

Usage:

```text
incan workspace inspect [OPTIONS]
```

Prints the compiler-validated workspace graph and the scope selected for this invocation. It does not infer membership from paths or run a separate dependency resolver.

Options:

- `--format text|json`: Human summary or versioned JSON projection (default: `text`).
- `--workspace`: Select every workspace member for the projected command scope.
- `--member <NAME_OR_PATH>`: Select one or more named or root-relative members.

The JSON projection includes canonical root and manifest paths, ordered members and root-member status, defaults and exclusions, the selection origin, shared declarations, effective inherited dependency provenance, explicit workspace environment extensions, root-lock state, the locked feature/component/provider graph for every member, stale member-local locks, and configuration that is reserved for later policy or source evaluation. Workspace capability application is intentionally reported as member-local only.

Examples:

```bash
incan workspace inspect
incan workspace inspect --workspace --format json
incan workspace inspect --member packages/api --format json
```

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
- **Built binary**: `target/incan/<name>/oven/release/<name>`
- **Built library artifact (`--lib`)**: `target/lib/<name>.incnlib` plus the generated library crate output

Cleaning generated source and the project-local published binary (only while no Incan command is using it):

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
