# Oven

Oven is the build system and the `incan` command line, written in Incan (DD-0008). It reads `loaf.toml`, resolves Loaves, plans each unit's Rust facet, orders and compiles units through `incan-rustc-driver`, and keeps the store. No Rust crate makes an Oven decision.

## Homes

| Module | Owns |
| --- | --- |
| `main.incn` | The `incan` command line: `run`, `check`, `describe-unit`, `build`. |
| `unit.incn` | The unit description (`incan.unit/1`) of a program, a library or a standard package: what the compiler reads instead of a manifest (`incan describe-unit PATH`). |
| `toolchain.incn` | The installed toolchain (`<home>/toolchains/<toolchain>/toolchain.json`, `incan.toolchain/1`): the compiler, the pinned sysroot it was built against, the ABI cache of its standard Rust crates (DD-0009), and the native closure every compiled unit links. |
| `incan_units.incn` | Checking and compiling a described unit with the toolchain's compiler, and running the program it compiled (`incan check`, `incan run`). |
| `rust_units.incn` | Rust units: a local Loaf's Rust facet and the local facets it reaches, one `rustc` per unit against compiled registry units (`incan build`). A binary that links the toolchain's own `rustc_driver` gets a runpath to it and the identity it was built against. |
| `intake.incn`, `manifest_errors.incn` | Reading `loaf.toml`: the package, its scripts and features, its path, Rust-facet and registry dependencies (standard packages are ambient, never declared), its source root, contract model bundles and C headers. Every refusal names the manifest, the field path and its TOML location. |
| `features.incn` | Package feature resolution over the local graph (RFC 114): additive, unified across active edges, optional dependencies selected by features, cycles refused. |
| `home.incn` | The home (`INCAN_HOME`) and the artifact layout of standard packages and path dependencies; the toolchain's standard packages. |
| `loaf_index.incn`, `json_fields.incn`, `loaf_resolve.incn`, `loaf_facts.incn`, `semver_req.incn` | The incan.pub index: decoding pinned Loaf declarations, selecting versions, checking recorded feature closures. |
| `rust_source.incn`, `rust_activation.incn`, `rust_graph.incn`, `cfg_predicate.incn`, `loaf_local_graph.incn` | The Rust facet: dependency declarations, `cfg` and target predicates, per-unit activation and feature propagation across the graph, keeping host and target separate. |
| `unit_codegen.incn` | Declared codegen options for exact resolved units. |
| `invocation.incn` | The restrictions a command runs under (locked, frozen, offline). |

`test_*.incn` hold each module's tests (`incan test src/`).

## Building

Until a dev.8 toolchain builds itself, the frozen dev.7 compiler builds Oven: `incan build src/main.incn` in this directory produces the `incan` program. Its standard packages come from `INCAN_STDLIB`.

`incan run` and `incan check` need an installed toolchain: its `toolchain.json` names `incan-rustc-driver` and the closure of the runtime and the standard packages' Rust crates, and the standard packages' Loaves sit in the toolchain's `loaves/` directory.
