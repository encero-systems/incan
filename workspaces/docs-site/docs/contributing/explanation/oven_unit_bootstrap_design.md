# Oven unit boundary and bootstrap

This page holds the detailed design supporting [DD-0008](../../design_records/0008_one_oven_in_incan.md), whose status is **Accepted**. It preserves the technical contracts relocated from that record; the decision was accepted on 2026-11-08, but acceptance does not establish implementation completion. The decision record retains the architectural choices and their rationale.

The per-project owner and frozen-builder sections below preserve the original design, not the current engine topology or release producer. [DD-0013](../../design_records/0013_the_incan_build_engine.md) replaces the per-project owner; [DD-0010](../../design_records/0010_released_toolchains_for_ci_and_development.md) replaces the frozen builder. Those later decisions govern where they differ.

## Design contracts

### What Oven owns

- Reading `loaf.toml`. Oven is the only reader of a project manifest.
- Resolving Loaves and writing the lock (`oven.lock`).
- The Rust facet of every unit: the `crates-io/*` Loaves it links, their `cfg` and feature activation, and the `rustc` invocation of each Rust unit.
- Native Incan compilation policy: debug units use optimization level 0, preserving the existing assertion and overflow policy; other profiles retain the driver's optimization. The selected flags are part of the unit's reuse identity.
- Unit orchestration: every Loaf a unit reaches is compiled before the unit (DD-0005), standard packages included (DD-0007).
- The store: prepared units, their identities and their reuse, so that unchanged work is never repeated (DD-0005).
- The toolchain and home layout.
- The `incan` command line. `incan` is an Incan program.

### What the compiler is

The compiler is the Rust code Oven calls. `incan-rustc-driver` compiles one unit, an Incan unit from source or a Rust unit, from the inputs Oven passes. Checking and analysis take the same unit description. The unit description names the unit's source, kind, package identity and profile, its active features, the artifact directory of every Loaf it reaches, the Rust externs and search paths it links, and the manifest facts the checker reads: declared Rust crates, contract models and C interop declarations. The compiler reads no manifest, resolves nothing and keeps no store.

### One program, no exchange

Oven's decisions and the actions they lead to run in one program, so they share typed values rather than exchanging documents. The `incan.oven.*` wire schemas are removed. The unit description and the lock are the only formats that cross a boundary. A tool that runs outside Oven, such as the language server, asks for the description of the unit owning a file with `incan describe-unit PATH`.

### A resident owner

`incan run`, `build` and `test` may ask a resident Oven process for a unit's executable. That owner is the same Oven program in a service mode, serial per project and toolchain generation, reached over a private Unix socket with the `incan.owner/1` frames. It retains Oven's own objects (the toolchain, the prepared unit and its ledger), reconciles every input on each request exactly as a command does, and never runs a user's program: the command receives the executable's path and becomes it. A unit the owner does not serve is prepared in the command.

### Bootstrap

A toolchain cannot build Oven and the compiler before one of its own exists. Until then, a frozen bootstrap builder compiles them: a compiler from before the direct native route, kept in its own private build environment. [DD-0010](../../design_records/0010_released_toolchains_for_ci_and_development.md) replaces it with an installed released toolchain.

During bootstrap, `incan.toolchain/1` names the prepared registry catalog in `registry_units` and may name the standard producer's exact closure in `registry_lock`. These are explicit stand-ins for loaves.pub's baked Loaves until Oven fetches them. Oven selects one coherent compiled closure for the active manifest requirements, honoring recorded producer entries and refusing an undecided ambiguity, builds local Rust facets through its existing unit compiler, and writes exact `externs`, `search`, and `rust_abi` inputs into `incan.unit/1`. Checking and compilation consume these same fields; a search directory never substitutes for a named artifact. Published Loaf native bindings remain ownership evidence when a source graph replaces an ambient alias. The directories of a toolchain generation, its own and its registry's, are named once by a content-addressed search index rather than listed in every description and native closure. Temporary bootstrap helpers stay outside the repository and retire with self-hosting.
