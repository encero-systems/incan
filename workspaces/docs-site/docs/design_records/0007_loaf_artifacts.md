---
id: DD-0007
title: Loaf artifacts and the one consumer index
status: Draft
type: design-decision
date: 2026-10-19
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-19
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0004
  - DD-0005
  - DD-0006
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0007: Loaf artifacts and the one consumer index

## Context

dev.8 removes generated Rust crates and Cargo artifact coordinates. The library-manifest index, provider plan, and SDK inventory previously split dependency authority, while standard packages also reached the checker as source modules; their replacement needs one artifact contract for every Loaf.

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

One index per compilation records each reachable Loaf's key, artifact directory, manifest, and namespace: `pub::<key>` for ordinary dependencies and compiler-pinned `std.*` roots for standard packages. Every module path selects exactly one entry; types come from its manifest, governed bodies from `.incnbody`, and linkage from its rlib and native closure.

### Calling into a Loaf

A consumer names declarations without redeclaring them; every type and compiled function/method is exported at the crate root under its encoded canonical identity, with the crate name lowercased and non-ASCII-alphanumeric characters replaced by `_`, eliminating per-call identity records.

Every public callable's `.incnbody` declaration supplies parameters, default computations, and type parameters for consumer call checking and omitted arguments; the driver verifies calls against rlib metadata. Compiled callables publish declarations only: their implementation dependencies stay in the rlib, while functions with type parameters and open trait defaults retain governed bodies.

Published unions retain checked semantic carriers, public native owners, canonical nominal payload identities, and physical variant order; consumers use that representation for construction, narrowing, and tests, refusing conflicting owners instead of inventing wrappers or import-dependent order.

Each public model/class with type parameters has one native ADT preserving binders and authored field order; provider and consumer layouts alias its instantiations, with otherwise unused parameters retained by a private, validated zero-sized marker excluded from Debug output, which shows authored fields under the source name.

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
