# Oven

Oven is the build system and the `incan` command line, written in Incan (DD-0008). It reads `loaf.toml`, resolves Loaves, plans each unit's Rust facet, orders and compiles units through `incan-rustc-driver`, and keeps the store. No Rust crate makes an Oven decision.

## Homes

| Module | Owns |
| --- | --- |
| `main.incn` | The `incan` command line: `run`, `check`, `test`, `describe-unit`, `build`, `fmt`, `inspect codegraph`. |
| `graph_inspection.incn`, `graph_dependencies.incn`, `graph_incan.incn`, `graph_rust.incn`, `graph_loaves.incn`, `graph_cache.incn`, `graph_queries.incn` | Selecting inspection units, reading published Loaf graphs, caching legacy exports by input digest, and querying compiler-owned records. Incan facts come from the shared compiler exporter; Rust facts come from analysis in the installed rustc driver. |
| `unit.incn` | The unit description (`incan.unit/1`) of a program, library or test unit: what the compiler reads instead of a manifest (`incan describe-unit PATH`). |
| `toolchain.incn` | The installed toolchain (`<home>/toolchains/<toolchain>/toolchain.json`, `incan.toolchain/1`): the compiler, the pinned sysroot it was built against, the ABI cache of its standard Rust crates (DD-0009), and the native closure every compiled unit links. |
| `toolchain_archive.incn` | Packing the installed toolchain's named artifacts and rewriting toolchain, registry and standard Loaf paths for relocation. |
| `incan_units.incn` | Preparing path libraries in dependency order, checking and compiling a described unit with the toolchain's compiler, and running the program it compiled (`incan check`, `incan run`, `incan build`). |
| `unit_freshness.incn` | Content fingerprints for prepared path libraries: selected unit inputs, source files, compiler inputs and reached Loaf artifacts. |
| `testing.incn` | Running a compiled test unit and reporting the collected tests' observed outcomes (`incan test PATH [UNIT OPTIONS]`). |
| `rust_units.incn` | Rust units: a local Loaf's Rust facet and the local facets it reaches, one `rustc` per unit against compiled registry units (`incan build`). A binary that links the toolchain's own `rustc_driver` gets a runpath to it and the identity it was built against. |
| `intake.incn`, `manifest_errors.incn` | Reading `loaf.toml`: the package, its scripts and features, its path, Rust-facet and registry dependencies (standard packages are ambient, never declared), its source root, contract model bundles and C headers. Every refusal names the manifest, the field path and its TOML location. |
| `features.incn` | Project feature requests and the Incan view of the shared local facet graph (RFC 114). |
| `home.incn` | The home (`INCAN_HOME`) and the artifact layout of standard packages and path dependencies; the toolchain's standard packages. |
| `json_fields.incn`, `semver_req.incn` | Typed JSON fields and version requirement checks for prepared registry units. |
| `rust_source.incn`, `rust_activation.incn`, `cfg_predicate.incn`, `loaf_local_graph.incn` | The shared local graph: dependency declarations, `cfg` and target predicates, per-unit activation and feature propagation, keeping host and target separate. |

`test_*.incn` hold each module's tests. The dev.8 command accepts one test file at a time; directory discovery is not implemented.

## Test units

`incan test tests/test_example.incn` takes the same profile and feature options as `incan run`. Oven describes the file as a `test` unit and compiles it once through the same compiler entry as programs and libraries. The compiler collects tests with the session's `std.testing` marker semantics and writes their native entry names and outcome markers beside the binary in `<binary>.tests.json`. Each synchronous, zero-argument test runs in a separate native harness process. Oven prints `<file>::<function> PASSED`, `FAILED`, `SKIPPED (<reason>)`, `XFAIL (<reason>)` or `XPASS (<reason>)` and exits 1 for failures or unexpected passes. Skipped tests are never executed. Test stdout and stderr are captured; output from unexpected failures is reported on stderr.

Fixtures, test parameters, async tests, timeouts and resource markers currently produce named refusals. Their execution contracts are not implemented by this initial native harness.

## Building

`incan toolchain pack OUT` writes the installed toolchain to a `.tar`, `.tar.gz` or `.tar.zst` archive. It includes the named driver and Rust ABI caches, compiled standard Loaves, runtime native dependencies and prepared registry units. Paths in the toolchain manifest, registry closure and Loaf native closures are relative to their containing directories. Rust's sysroot and the driver's local identity proof are not archived. Packing requires `tar` with support for the selected compression format.

`incan toolchain install ARCHIVE` extracts the archive into staging beside `$INCAN_HOME/toolchains/0.6.0-dev.8`, refuses a mismatched toolchain name, creates its `sysroot` link and invokes the staged driver with `--identity SYSROOT`. The local sysroot comes from `INCAN_SYSROOT`, otherwise `rustup run 1.98.0 rustc --print sysroot`, otherwise `rustc --print sysroot`. Install the matching Rust 1.98.0 toolchain with `rustc-dev` first. Only successful identity verification replaces an existing installation; a refusal preserves it.

Until a dev.8 toolchain builds itself, the frozen dev.7 compiler builds Oven: `incan build src/main.incn` in this directory produces the `incan` program. Its standard packages come from `INCAN_STDLIB`.

`incan run` and `incan check` need an installed toolchain: its `toolchain.json` names `incan-rustc-driver` and the closure of the runtime and the standard packages' Rust crates, and the standard packages' Loaves sit in the toolchain's `loaves/` directory.

`incan run`, `incan check` and `incan build` compile active path dependencies before their consumer, stopping on the first dependency refusal. Each library receives the features unified across the root project's selected graph, with declared aliases and inactive optional edges preserved, and writes its five DD-0007 artifacts under the root's `target/incan/loaves/<package>/<profile>` directory. A successful input fingerprint beside the complete artifact set allows unchanged libraries to be reused. Source additions, removals, content changes, feature changes and changed dependency artifacts invalidate that fingerprint. `describe-unit` describes units without compiling Incan libraries.

`incan build PATH` compiles an Incan source file without running it. A directory selects `src/lib.incn` when present, otherwise `src/main.incn`. A Rust project uses the prepared registry units named by the installed toolchain.

## Inspecting source relationships

Run `incan inspect codegraph PATH --format jsonl` for a source file or project. `--format json` returns an array of the same records. A Rust source path selects its owning Loaf; a Rust project exports its own units and reads dependency graphs from their Loaf artifacts. A legacy source-backed dependency without a graph receives one cached checking-only export. Registry artifacts that do not yet ship a graph remain external references; source-free artifacts cannot be reconstructed by inspection.

Use `--query defs|refs|callers|callees|imports TERM` instead of `--format` to print one `kind identity file:line` line per hit. A term is an exact short name, qualified declaration name or full exported graph ID; ambiguous short names include their matching declarations. Imports select module names. For example, `incan inspect codegraph loaves/toolchain/incan-rustc-driver --query callers resolve_rust_facts` finds callers across its local closure. Query locations are relative to the repository, or the owning project outside a repository. Queries default to 20 hits; `--limit N` accepts a nonnegative integer and appends `… N more` when hits are omitted. Definitions carry span-free `stable_identity` values. Spans remain source anchors, independent of those identities.

Compiled libraries carry `<name>.incngraph`, with span-free declaration IDs and Loaf-relative source paths, beside their native artifacts. Compilation projects the graph from the same checked unit and frozen Body IR. Rust compilation collects its graph during the ordinary rustc analysis pass. The kernel owns the versioned indexed-postcard format; JSON is generated only for query/export output. Inspection rejects unsupported versions and malformed records rather than replacing a published graph with source facts.

Own-unit and legacy graphs are cached under the requesting project's `target/incan/codegraph/`, one entry per unit. Source and dependency digests invalidate changed exports; published dependency graphs remain authoritative until rebuilt. Completed query answers are keyed by graph contents and arguments.

Export is tolerant by default (RFC 106, "Checked and tolerant export"). A unit that fails checking contributes only its parseable facts, with no checked signatures and no resolved call or reference edges; its hits end in `(unchecked)`, it publishes no interface for its dependents, and every inspection prints one stderr warning line naming the unit and its first diagnostic. A Rust unit that rustc refuses has no parse-only facts and contributes none. Either outcome is cached with the unit's digest, so one bad unit never forces another unit to be exported again. `--strict` refuses instead, listing every unchecked unit's diagnostics. The header's `mode` is `allow_errors` or `strict`, and its `degraded` is true while any selected unit is unchecked. An abnormal compiler exit fails the command and caches nothing.

Rust receiver types and signatures use canonical crate-qualified paths so a consumer's method target links to the defining crate's declaration. Rust call records with `kind: indirect` have no proved target; `virtual_method` names a trait requirement rather than a set of possible implementors. These records describe a lower bound on reachability, never proof that an unlisted implementation cannot be called.

Incan libraries without published graphs are checked in dependency order. Each checking pass supplies both its source graph and a checking-only interface under the inspection cache. Vocabulary providers require execution, so their dependency closure is prepared through the ordinary library build before checking the consumer. Other Incan inspection passes produce no native publication. Rust checking may need a native library for an Incan dependency: inspection uses an exact library recorded by the project's successful `incan build` in `target/incan/rust/<profile>/units.json`, or an explicitly installed toolchain extern, and refuses if neither is available. A bootstrap-emitted Rust graph cannot replace the authored Incan graph: publication must include the Incan manifest and complete artifact set. Source projections do not assert source/artifact equivalence.

Projects can include source context that their binary does not statically link:

```toml
[tool.incan.codegraph]
sources = ["relative/path/to/runtime"]
```

These projects contribute their Incan graphs without changing compilation dependencies. Paths resolve relative to the declaring project; a cycle is refused. The compiler uses this to include its native runtime's authored source. `--substitute NAME=DIR` remains an explicit build-graph selection, as for `incan build`; inspection uses the installed toolchain's rustc without requiring `RUSTC`. Checking-only export still observes checker admission limits: the current compiler source closure refuses an unadmitted `NativeUnionDeclaration::clone`, and the runtime source refuses slice `to_vec` and `str::split_whitespace`. A published authored Loaf graph avoids rechecking its dependency source.

## Records engine control channel

`INCAN_ENGINE=legacy|records` selects the migration path. The default is `legacy`. `records` connects file-based `run` and `build` to the engine, plus `inspect run ID --proofs`. Other commands refuse without entering legacy. Help and version remain available; an invalid selector is an error. The first producer handles ordinary program units with standard packages: feature-selection flags, additional program entries, editable path dependencies, vocabulary and foreign headers are refused explicitly.

Oven carries the internal `--internal-engine` service entry. Its inputs are the physical store directory, full store identity, profile, service generation, compiler generation and idle timeout in seconds. The service holds a store-level lifetime lock, authenticates Unix peer credentials and the complete `Hello` identity, and retires after 600 seconds without a client, request, worker, effect or reader pin. Explicit internal timeout arguments support lifecycle tests. Runtime socket names remain short when the store directory is deep. Windows transport has not been qualified.

The transport is the `ipc` module of the platform Loaf, `incan_platform` in `loaves/platform`: Unix connections and listeners, bounded reads and length-framed writes, data-credit windows and OS peer identity. It carries bytes only. `owner_protocol.incn` encodes one JSON document per frame for the owner and compiler-worker clients. `engine_wire.incn`, `engine_messages.incn` and `engine_codec.incn` define the 64-byte `incan.engine/1` header and every versioned message family. `engine_channel.incn` owns session and sequence checks and keeps eight data credits per direction in the platform's credit windows. Its `Blocked` outcome applies back-pressure before encoding or retaining the caller's message; synchronous control frames bypass exhausted data credits. There is no outgoing message queue.

The service handles negotiation, submission, events, completion, interest cancellation, execution pins, publication resolution, drain and stop. A file submission names its absolute source in `requested_roots`, its project view in `view`, and `debug` or `release` in `input_selection`; additional build options are currently refused. `Accepted` precedes metadata-first capture. `Captured` names the actual epoch identity and committed snapshot. `engine_epochs.incn` stores capture revisions and their selected heads in the same record store, so a service restart can reuse capture evidence without retaining a decoded epoch while idle. The unit producer captures the existing conservative authored inventory, including additions; per-import reach remains a later producer boundary. Build and run capture the same program inputs.

`engine_programs.incn` obtains the existing bakery's whole compiled unit under its RFC 124 identity. `incan_units.incn` remains the single claim, worker compilation and publication path; a successful bake revalidates its inputs before publishing. These are unit records, not declaration records. The engine stores a finalized native descriptor and selects its exact payload in the caller's virtual view. `engine_actions.incn` holds a substrate lease before returning `Complete`; `Pin` confirms the caller's exact result, and `Release` or disconnection drops the lease. The CLI waits for its child while it holds that interest, and never runs or replays a program in the service. `Resolve` reports whether a native descriptor publication committed; it does not execute or select a result for another caller. Collection is still disabled in the underlying bakery.

`engine_client.incn` records the compiler's admitted content identity once per installed pin and checks only the pin on an unchanged request. A native descriptor hit avoids native preparation and compiler activation. `--engine-report` explicitly retains and prints a report from `engine_proofs.incn`; an ordinary unchanged request does not retain a report. Its capture counters describe capture operations, not compiler reads of frozen facts. The CLI reference lists the report schema.

Run `incan test src/test_engine_protocol.incn` with the newly built Oven installed in `INCAN_HOME/bin/incan`, or set `OVEN_ENGINE_BINARY` to that Oven executable. The tests exercise real concurrent service starts and idle exit as well as binary framing, all message codecs, credit pressure, identity checks and lifecycle state.
