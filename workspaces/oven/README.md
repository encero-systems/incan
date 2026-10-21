# Oven

Oven is the build system and the `incan` command line, written in Incan (DD-0008). It reads `loaf.toml`, resolves Loaves, plans each unit's Rust facet, orders and compiles units through `incan-rustc-driver`, and keeps the store. No Rust crate makes an Oven decision.

## Homes

| Module | Owns |
| --- | --- |
| `main.incn` | The `incan` command line: `run`, `check`, `test`, `describe-unit`, `build`. |
| `unit.incn` | The unit description (`incan.unit/1`) of a program, library or test unit: what the compiler reads instead of a manifest (`incan describe-unit PATH`). |
| `toolchain.incn` | The installed toolchain (`<home>/toolchains/<toolchain>/toolchain.json`, `incan.toolchain/1`): the compiler, the pinned sysroot it was built against, the ABI cache of its standard Rust crates (DD-0009), and the native closure every compiled unit links. |
| `incan_units.incn` | Preparing path libraries in dependency order, checking and compiling a described unit with the toolchain's compiler, and running the program it compiled (`incan check`, `incan run`, `incan build`). |
| `unit_freshness.incn` | Content fingerprints for prepared path libraries: selected unit inputs, source files, compiler inputs and reached Loaf artifacts. |
| `testing.incn` | Running a compiled test unit and reporting the collected tests' observed outcomes (`incan test PATH [UNIT OPTIONS]`). |
| `rust_units.incn` | Rust units: a local Loaf's Rust facet and the local facets it reaches, one `rustc` per unit against compiled registry units (`incan build`). A binary that links the toolchain's own `rustc_driver` gets a runpath to it and the identity it was built against. |
| `intake.incn`, `manifest_errors.incn` | Reading `loaf.toml`: the package, its scripts and features, its path, Rust-facet and registry dependencies (standard packages are ambient, never declared), its source root, contract model bundles and C headers. Every refusal names the manifest, the field path and its TOML location. |
| `features.incn` | Package feature resolution over the local graph (RFC 114): additive, unified across active edges, optional dependencies selected by features, cycles refused. |
| `home.incn` | The home (`INCAN_HOME`) and the artifact layout of standard packages and path dependencies; the toolchain's standard packages. |
| `loaf_index.incn`, `json_fields.incn`, `loaf_resolve.incn`, `loaf_facts.incn`, `semver_req.incn` | The incan.pub index: decoding pinned Loaf declarations, selecting versions, checking recorded feature closures. |
| `rust_source.incn`, `rust_activation.incn`, `rust_graph.incn`, `cfg_predicate.incn`, `loaf_local_graph.incn` | The Rust facet: dependency declarations, `cfg` and target predicates, per-unit activation and feature propagation across the graph, keeping host and target separate. |
| `unit_codegen.incn` | Declared codegen options for exact resolved units. |
| `invocation.incn` | The restrictions a command runs under (locked, frozen, offline). |

`test_*.incn` hold each module's tests. The dev.8 command accepts one test file at a time; directory discovery is not implemented.

## Test units

`incan test tests/test_example.incn` takes the same profile and feature options as `incan run`. Oven describes the file as a `test` unit and compiles it once through the same compiler entry as programs and libraries. The compiler collects tests with the session's `std.testing` marker semantics and writes their native entry names and outcome markers beside the binary in `<binary>.tests.json`. Each synchronous, zero-argument test runs in a separate native harness process. Oven prints `<file>::<function> PASSED`, `FAILED`, `SKIPPED (<reason>)`, `XFAIL (<reason>)` or `XPASS (<reason>)` and exits 1 for failures or unexpected passes. Skipped tests are never executed. Test stdout and stderr are captured; output from unexpected failures is reported on stderr.

Fixtures, test parameters, async tests, timeouts and resource markers currently produce named refusals. Their execution contracts are not implemented by this initial native harness.

## Building

Until a dev.8 toolchain builds itself, the frozen dev.7 compiler builds Oven: `incan build src/main.incn` in this directory produces the `incan` program. Its standard packages come from `INCAN_STDLIB`.

`incan run` and `incan check` need an installed toolchain: its `toolchain.json` names `incan-rustc-driver` and the closure of the runtime and the standard packages' Rust crates, and the standard packages' Loaves sit in the toolchain's `loaves/` directory.

`incan run`, `incan check` and `incan build` compile active path dependencies before their consumer, stopping on the first dependency refusal. Each library receives the features unified across the root project's selected graph, with declared aliases and inactive optional edges preserved, and writes its four DD-0007 artifacts under the root's `target/incan/loaves/<package>/<profile>` directory. A successful input fingerprint beside the complete artifact set allows unchanged libraries to be reused. Source additions, removals, content changes, feature changes and changed dependency artifacts invalidate that fingerprint. `describe-unit` describes units without compiling Incan libraries.

`incan build PATH` compiles an Incan source file without running it. A directory selects `src/lib.incn` when present, otherwise `src/main.incn`. Supplying `--registry-units FILE` selects the Rust-facet build command.
