---
id: DD-0012
title: Incremental compilation over a persistent semantic store
status: Accepted
type: design-decision
date: 2026-10-29
review_target: 0.6.0-dev.9
sources:
  - Maintainer design review, 2026-10-29
  - https://github.com/encero-systems/incan/issues/1346
  - RFC 102, RFC 106, RFC 120, RFC 124, RFC 125
  - DD-0007
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0012: Incremental compilation over a persistent semantic store

## Context

On 0.6.0-dev.8, a one-body edit to the compiler's lowering library (37,000 Incan lines) redoes every phase over the whole library, about 211 s against 254 s cold. Reusing work one declaration category at a time did not converge.

#1346 names the same gap from the inspection side. Facts are discarded after every invocation, so the LSP, MCP and the code graph recompute what they need. Both problems come from one missing thing: a durable, identity-keyed record of what the compiler computed and what each result read.

## Decision

--8<-- "design_records/0012_semantic_store_architecture.svg"

1. **One store per Incan home: the bakery.** It holds typed records keyed by canonical identity (RFC 120). The code-graph record kinds are its public model, and the JSONL export is a projection of it. The compiler's own results are records in the same store: signatures, layouts, default expressions, checked bodies, Body IR, generic instances, plan functions and codegen-unit membership. Compiled units live in it under RFC 124's unit identity. Oven only asks the bakery, and a missing fact is baked once and found by every later ask, so cold and warm builds take one path. Toolchain bundles and registry assets (RFC 125) arrive as inserts.
2. **Records form a per-declaration graph of typed reads.** Each record is one facet of one declaration. Each read it makes is recorded, and its type is the type of record it reads: a signature, a body (a generic instance reads its body), a layout, or a name.
3. **Namespaces are graph nodes.**
   - A namespace holds name slots, and each slot is present or absent. A lookup that finds nothing reads an absent slot, so adding the name later invalidates exactly that lookup.
   - An import creates a one-way binding in the importing namespace. When the imported declaration changes, only the items that read the binding are retargeted. A change inside the importing module never reaches the namespace it imports from.
4. **The graph is a directed acyclic graph.**
   - Circular imports are refused.
   - Return types are always written (or `None` by rule), and bodies read signatures, never bodies.
   - A default expression is its own record.
   - A type that holds itself goes through an indirection.
   - Every record therefore has a fixed rank: its namespace's position in import order, then its phase.
5. **Change is one sweep with early cutoff.**
   - Changed inputs seed the graph.
   - Records are recomputed in rank order, each at most once, after its inputs are final.
   - A record recomputed to its old value stops propagation.
   - A build with no changed input touches no graph data.
6. **Loaves are immutable.** A Loaf ships its records, its namespace hashes and its fingerprints. Reads cross into a Loaf, but nothing inside it is recomputed or invalidated from outside. A Loaf changes only when another version is swapped in whole, which is diffed by namespace hash and recomputes only the readers whose reads changed.
7. **Versions are strict, and compatibility is evidence.**
   - A lock pins exact versions.
   - From the records a consumer actually read, Oven can derive the versions it is compatible with ("1.15–1.25 by evidence") and swap a version in without a full rebuild.
   - That evidence covers the interface the consumer used, not the behavior inside a dependency's ordinary function bodies.
8. **Native code follows the same identities.** rustc incremental reuse is enabled only where a definition's fingerprint is a tracked input, and codegen units are keyed by identity so one change does not reshuffle the others.
9. **Storage is our own format on the IncQL-DB substrate, written in Incan.**
   - Immutable, memory-mapped segments: one canonical base plus small per-build deltas.
   - A manifest switched atomically, so readers take snapshots and never wait.
   - Edges stored in both directions, and fingerprints of 128 bits.
   - The compacted base is byte-identical across rebuilds, and is what a Loaf ships.
10. **Correctness is checkable.** A verify mode recomputes cold and compares byte for byte. A missing or damaged record is a miss, never wrong output, and source locations never enter semantic fingerprints.

## Consequences

- One edit costs time in proportion to the records it actually affects. An unchanged build reads the manifest and stops.
- The LSP, MCP, the code graph and Oven read the store instead of recompiling, and "what changed" is answered from its log.
- The checker, Body IR and lowering become per-record computations that record their reads. That restructuring is the main cost.
- The existing reuse work (edit-stable identities, semantic digests, the checked-artifact codec, read witnesses) becomes part of the store. The per-category capture admissions are retired.
- Any gap between the Incan-written store and equivalent Rust is a compiler defect to fix, not a reason to write it in Rust.

## Non-goals

- Query languages, analytics, vector search or remote stores. These are IncQL-DB's, built on the same substrate.
- Parallel scheduling. Records of equal rank allow it, but it is a later decision.
- New user-facing syntax or semantics.

## Revisit condition

Revisit if a phase needs whole-program state that per-record reads cannot express, or if read tracking keeps an unchanged or one-body build above its efficiency-ledger target.

## Provenance

Agreed in the maintainer's design review of 2026-10-29. It derives from RFC 102's one-model contract, RFC 106, RFC 120 and DD-0007, and supersedes none of them. Background research comparing rustc, salsa, Buck2 DICE, Zig, Swift and Turbopack informed the graph and storage choices.

## References

- #1346, #1143 (Slice 9), #573 (RFC 106), #1037 (Just Enough Compilation), #2475 (recursive classes through `Option`)
- DD-0004, DD-0007
