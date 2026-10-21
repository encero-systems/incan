---
id: DD-0007
title: Loaf artifacts and the one consumer index
status: Draft
type: design-decision
date: 2026-10-19
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-19 (dev.8 cull)
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0004
  - DD-0005
  - DD-0006
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0007: Loaf artifacts and the one consumer index

## Context

Until 0.6.0-dev.8 a compiled Incan library was a generated Rust crate: its artifact record named a `Cargo.toml` and a `src/lib.rs`, its private implementation choices were Cargo features, and the standard library reached consumers through a separate SDK inventory of published packages. A consumer found its dependencies through three structures that each described part of the same thing: the library-manifest index, the provider plan, and the SDK inventory with its components and profiles. The standard library additionally reached the checker as source modules. dev.8 removes the generator and Cargo, so none of those artifact coordinates exist any more.

## Decision

**A compiled Loaf is one artifact directory, and a consumer reads every Loaf (its dependencies and the standard packages alike) through one index.**

### The artifact

`incan-rustc-driver --library` writes, for one Loaf and one profile:

| File | Contents |
|---|---|
| `<name>.incnlib` | the library manifest: public exports, vocabulary, soft keywords, contract metadata, the Rust ABI of its Rust facet |
| `<name>.incnbody` | the governed bodies: the executable representation of every body a caller instantiates (functions with type parameters) |
| `lib<name>.rlib` | the library unit: every other body, compiled once, with a crate-root export for each public name |
| `<name>.native.json` | the unit's native closure: the rlibs and search paths it links |

### The index

One `LoafIndex` per compilation holds an entry per Loaf the project reaches: its key, its artifact directory, its manifest, and the namespace it owns. An ordinary dependency owns `pub::<key>`. A standard package owns the `std.*` roots that the compiler-pinned `STANDARD_PACKAGE_NAMESPACE_POLICIES` assign to it. A module path resolves to exactly one entry. The checker reads imported types from the entry's manifest, Body IR takes governed bodies from its `.incnbody`, and the driver links its rlib and native closure.

### Calling into a Loaf

A consumer names a Loaf's declarations and never declares them again. A Loaf exports every type and every compiled function or method at its crate root under the encoding of its canonical identity, so the item a consumer names is `<crate>::<encoded identity>`, where the crate is the package name in lowercase with every character other than an ASCII letter or digit spelled `_`. The item follows from the declaration's identity alone, so no call needs a per-call record.

The `.incnbody` carries a declaration record for every public callable beside the governed bodies: its parameters with the computation each default reduces to, and its type parameters. The consumer's Body IR and lowering complete an omitted argument and check a call from that record. The driver checks the call against the rlib's metadata.

The manifest's native union table records each locally compiled union's checked semantic carrier, public native item, and payloads in physical variant order. Nominal payloads retain canonical declaration identities. A consumer reads this representation from the selected Loaf and uses those variant slots for construction, narrowing, and tests; it never derives a replacement wrapper or payload order from its local import spellings. Conflicting native owners for the same semantic carrier are refused.

A public model or class with type parameters is published as one native ADT with its declared binders and authored field order. Concrete layouts in its provider and consumers alias instantiations of that ADT. A private zero-sized marker retains type parameters that have no authored field; construction supplies that marker, metadata validation checks it, and Debug output includes only authored fields under the source declaration name.

Every package identity takes this one route. No Body IR record, lowering branch or driver path is specific to the standard packages.

### The standard library

The standard packages are Loaves like any other. A project depends on the standard packages whose namespaces it imports, at the versions the compiler pins. Their artifacts come from the toolchain: built from `loaves/stdlib/*` by the dev.8 compiler in a checkout, and shipped in the local incan.pub cache by an installation. No source module of the standard library is collected into a consumer.

## Consequences

- The provider plan, the library-manifest index, the SDK inventory with its components, profiles and bootstrap grants, the standard source publication, and the generated-crate artifact record are replaced by the artifact and the index.
- A dependency is always compiled before its consumer is checked (DD-0005's eager preparation), so there is no checked-source or parser-source stand-in for a missing artifact.
- The checker sees the standard library only through manifests, the same contract every dependency already meets.

## Non-goals

- Fetching Loaves from a remote registry, and version solving; the lock and incan.pub own those.
- The format details of each file beyond what this record names.

## Revisit condition

Revisit when a Loaf needs more than one library unit per profile, or when cross-compilation needs per-target artifacts.

## Provenance

Builds on DD-0004 (the pinned driver), DD-0005 (eager preparation) and DD-0006 (Rust interop on the direct route). Supersedes nothing.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- DD-0004, DD-0005, DD-0006
