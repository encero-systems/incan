---
id: DD-0007
title: Loaf artifacts and the one consumer index
status: Accepted
type: design-decision
date: 2026-10-19
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-19
  - Maintainer acceptance, 2026-11-08: standard packages are ordinary Loaves
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0004
  - DD-0005
  - DD-0006
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0007: Loaf artifacts and the one consumer index

## Context

Incan 0.6 removes generated Rust crates and Cargo artifact coordinates. The library-manifest index, provider plan, and SDK inventory previously split dependency authority, while standard packages also reached the checker as source modules; their replacement needs one artifact contract for every Loaf.

## Decision

**A compiled Loaf is one artifact directory, and a consumer reads every Loaf (its dependencies and the standard packages alike) through one index.**

Standard packages are ordinary Loaves exposed through the dedicated virtual `std` namespace. That namespace is the distinction in how consumers address them; it does not grant a different artifact contract or compiler consumption path.

A Loaf publishes checked declarations, caller-instantiable bodies, compiled code and its native dependency closure together. It may publish inspection facts on explicit request. All consumers use canonical declaration identities from one index; standard packages take the same route as ordinary dependencies.

A checked source interface may supply declarations and governed bodies before native compilation, but grants no compiled callable or native layout. A release toolchain requires complete compiled Loaves.

Consumers preserve the producer's checked type, layout, callable and ownership contracts. They do not reconstruct them from source spellings or manufacture replacement carriers. Compiled implementations remain in the provider; only governed instantiable bodies are compiled by consumers.

Runtime dependency selection is demand-driven. An incomplete plan identifies its missing prerequisites without publishing a final result, and progress must be demonstrable. Standard packages are compiled Loaves supplied by the toolchain, never source modules collected into the consumer.

## Consequences

- The provider plan, the library-manifest index, the SDK inventory with its components, profiles and bootstrap grants, the standard source publication, and the generated-crate artifact record are replaced by the artifact and the index.
- Ordinary dependency preparation remains eager (DD-0005); an explicitly selected checked source interface supplies only its checked declarations and governed bodies, and a consumer requiring its compiled items refuses until native compilation completes.
- The checker sees the standard library only through manifests, the same contract every dependency already meets.
- Incan native compilation omits graph collection. Inspection and explicit graph publication share a checked export cache keyed by exact source, compiler and dependency inputs; native and graph proofs remain separate.

## Non-goals

- Fetching Loaves from a remote registry, and version solving; the lock and loaves.pub own those.
- The format details of each file beyond what this record names.

## Supporting design

[Loaf artifacts and consumer indexing](../contributing/explanation/loaf_artifact_design.md) records the mechanisms and supporting evidence. Changes to those mechanisms do not change this decision unless they alter its stated boundaries or guarantees.

## Revisit condition

Revisit when a Loaf needs more than one library unit per profile, or when cross-compilation needs per-target artifacts.

## Provenance

Accepted by the maintainer on 2026-11-08: standard packages are ordinary Loaves and use the same compiler consumption mechanism as other libraries. Acceptance records that architectural boundary, not implementation completion. Builds on DD-0004 (the pinned driver), DD-0005 (eager preparation) and DD-0006 (Rust interop on the direct route). Supersedes nothing.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- DD-0004, DD-0005, DD-0006
