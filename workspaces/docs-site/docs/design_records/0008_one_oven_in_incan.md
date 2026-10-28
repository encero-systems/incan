---
id: DD-0008
title: One Oven, written in Incan
status: Draft
type: design-decision
date: 2026-10-20
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-20 (dev.8 cull)
  - https://github.com/encero-systems/incan/issues/1698
  - DD-0005
  - DD-0007
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0008: One Oven, written in Incan

## Context

Until 0.6.0-dev.8 Oven had two halves. The Rust crates under `loaves/oven/` and the `oven` command owned the store, leases, receipts, `rustc` planning and execution, and the project owner process. The Incan project `workspaces/oven` owned manifest intake, Loaf resolution, native plan selection and Rust feature activation, and the Rust half called it by exchanging versioned JSON documents. The Rust half also resolved dependency closures, selected plans and activated features on its own, so both halves decided the same questions. The compiler read `loaf.toml` through a third model of its own.

## Decision

**There is one Oven, and it is the Incan project. No Rust crate makes an Oven decision.**

### What Oven owns

- Reading `loaf.toml`. Oven is the only reader of a project manifest.
- Resolving Loaves and writing the lock (`oven.lock`).
- The Rust facet of every unit: the `crates-io/*` Loaves it links, their `cfg` and feature activation, and the `rustc` invocation of each Rust unit.
- Native Incan compilation policy: debug units use optimization level 0, preserving the existing assertion and overflow policy; other profiles retain the driver's optimization. The selected flags are part of the unit's reuse identity.
- Unit orchestration: every Loaf a unit reaches is compiled before the unit (DD-0005), standard packages included (DD-0007).
- The store: prepared units, their identities and their reuse, so that unchanged work is never repeated (DD-0005).
- Private checked-source artifacts may be shared across profiles only under an explicit complete semantic equivalence witness selected by Oven; profile-specific native lowering and linking remain fresh, and public Loaf executable representations are unchanged.
- The toolchain and home layout.
- The `incan` command line. `incan` is an Incan program.

### What the compiler is

The compiler is the Rust code Oven calls. `incan-rustc-driver` compiles one unit, an Incan unit from source or a Rust unit, from the inputs Oven passes. Checking and analysis take the same unit description. The unit description names the unit's source, kind, package identity and profile, its active features, the artifact directory of every Loaf it reaches, the Rust externs and search paths it links, and the manifest facts the checker reads: declared Rust crates, contract models and C interop declarations. The compiler reads no manifest, resolves nothing and keeps no store.

### One program, no exchange

Oven's decisions and the actions they lead to run in one program, so they share typed values rather than exchanging documents. The `incan.oven.*` wire schemas are removed. The unit description and the lock are the only formats that cross a boundary. A tool that runs outside Oven, such as the language server, asks for the description of the unit owning a file with `incan describe-unit PATH`.

### A resident owner

`incan run`, `build` and `test` may ask a resident Oven process for a unit's executable. That owner is the same Oven program in a service mode, serial per project and toolchain generation, reached over a private Unix socket with the `incan.owner/1` frames. It retains Oven's own objects (the toolchain, the prepared unit and its ledger), reconciles every input on each request exactly as a command does, and never runs a user's program: the command receives the executable's path and becomes it. A unit the owner does not serve is prepared in the command.

### Bootstrap

Until a dev.8 toolchain builds itself, the frozen 0.6.0-dev.7 builder compiles Oven and the dev.8 compiler.

During bootstrap, `incan.toolchain/1` names the prepared registry catalog in `registry_units` and may name the standard producer's exact closure in `registry_lock`. These are explicit stand-ins for incan.pub's baked Loaves until Oven fetches them. Oven selects one coherent compiled closure for the active manifest requirements, honoring recorded producer entries and refusing an undecided ambiguity, builds local Rust facets through its existing unit compiler, and writes exact `externs`, `search`, and `rust_abi` inputs into `incan.unit/1`. Checking and compilation consume these same fields; a search directory never substitutes for a named artifact. Published Loaf native bindings remain ownership evidence when a source graph replaces an ambient alias. The directories of a toolchain generation, its own and its registry's, are named once by a content-addressed search index rather than listed in every description and native closure. Temporary bootstrap helpers stay outside the repository and retire with self-hosting.

## Consequences

- `oven_rustc`, `oven_store`, `oven_interop`, the `oven` command (`oven-cli`) and `incan_oven_facet` are removed. So are the driver's completed-output selection, native plan selection, bake and publication, and the Rust project owner process (the owner above is written in Incan and holds no store).
- The compiler's manifest model (`oven_model::manifest`) and its project discovery are replaced by the unit description.
- Oven's host functions (file observation, process execution, store writes) are written in Incan over the standard library and Rust interop (DD-0006).

## Non-goals

- The format of the store and of the unit description beyond what this record names.
- Remote Loaf fetching and version solving, which incan.pub and the lock own (DD-0007).

## Revisit condition

Revisit if a Rust crate must make an Oven decision because Incan cannot yet express it. That crate is then named here, together with the gap that forces it.

## Provenance

Builds on DD-0005 (eager preparation and Just Enough Compilation) and DD-0007 (Loaf artifacts). Supersedes nothing.

## References

- [Issue #1698](https://github.com/encero-systems/incan/issues/1698)
- DD-0005, DD-0006, DD-0007
