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

Incan builds skip codegraph projection. Inspection checks source on demand and retains its graph under a key covering the compiler, unit, source and dependency inputs; unchanged requests reuse that graph. The LSP projects its checked editor buffers on request. `incan build path/to/lib.incn --codegraph` explicitly publishes `<name>.incngraph` beside the native artifacts for consumers that need inspection without source. A current library exports through the same checked inspection cache without recompiling native code; separate proofs bind both artifacts to their exact inputs. Graphs retain span-free declaration IDs and Loaf-relative source paths. Rust compilation still collects its graph during ordinary rustc analysis. The kernel owns the versioned indexed-postcard format; JSON is generated only for query/export output. Inspection rejects unsupported versions and malformed records rather than replacing a published graph with source facts.

Own-unit and legacy graphs are cached under the requesting project's `target/incan/codegraph/`, one entry per unit. Source and dependency digests invalidate changed exports; published dependency graphs remain authoritative until rebuilt. Completed query answers are keyed by graph contents and arguments.

The [codegraph throughput measurements](codegraph-throughput.md) record the native build savings and measurement limits.

Export is tolerant by default (RFC 106, "Checked and tolerant export"). A unit that fails checking contributes only its parseable facts, with no checked signatures and no resolved call or reference edges; its hits end in `(unchecked)`, it publishes no interface for its dependents, and every inspection prints one stderr warning line naming the unit and its first diagnostic. A Rust unit that rustc refuses has no parse-only facts and contributes none. Either outcome is cached with the unit's digest, so one bad unit never forces another unit to be exported again. `--strict` refuses instead, listing every unchecked unit's diagnostics. The header's `mode` is `allow_errors` or `strict`, and its `degraded` is true while any selected unit is unchecked. An abnormal compiler exit fails the command and caches nothing.

Rust receiver types and signatures use canonical crate-qualified paths so a consumer's method target links to the defining crate's declaration. Rust call records with `kind: indirect` have no proved target; `virtual_method` names a trait requirement rather than a set of possible implementors. These records describe a lower bound on reachability, never proof that an unlisted implementation cannot be called.

Incan libraries without published graphs are checked in dependency order. Each checking pass supplies both its source graph and a checking-only interface under the inspection cache. Vocabulary providers require execution, so their dependency closure is prepared through the ordinary library build before checking the consumer. Other Incan inspection passes produce no native publication. Rust checking may need a native library for an Incan dependency: inspection uses an exact library recorded by the project's successful `incan build` in `target/incan/rust/<profile>/units.json`, or an explicitly installed toolchain extern, and refuses if neither is available. A bootstrap-emitted Rust graph cannot replace the authored Incan graph: publication must include the Incan manifest and complete artifact set. Source projections do not assert source/artifact equivalence.

Projects can include source context that their binary does not statically link:

```toml
[tool.incan.codegraph]
sources = ["relative/path/to/runtime"]
```

These projects contribute their Incan graphs without changing compilation dependencies. Paths resolve relative to the declaring project; a cycle is refused. The compiler uses this to include its native runtime's authored source. `--substitute NAME=DIR` remains an explicit build-graph selection, as for `incan build`; inspection uses the installed toolchain's rustc without requiring `RUSTC`. Checking-only export still observes checker admission limits: the current compiler source closure refuses an unadmitted `NativeUnionDeclaration::clone`, and the runtime source refuses slice `to_vec` and `str::split_whitespace`. A published authored Loaf graph avoids rechecking its dependency source.
