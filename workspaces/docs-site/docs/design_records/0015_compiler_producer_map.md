---
id: DD-0015
title: The compiler producer map
status: Draft
type: design-decision
date: 2026-11-07
review_target: v0.6
sources:
  - Maintainer condition of 2026-10-30, set on approving the build engine design, that a producer-map sub-design precede the ranked semantic graph stage
  - Maintainer clarification of 2026-11-08 separating decisions from implementation design
  - DD-0012, DD-0013 and the build engine architecture (BE-07 to BE-12, BE-32, BE-34)
  - RFC 106, RFC 120, RFC 124, RFC 165, RFC 166, RFC 168
  - https://github.com/encero-systems/incan/issues/2481
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0015: The compiler producer map

## Context

DD-0012 makes compiler results reusable records. DD-0013 requires a producer map before the ranked semantic graph is implemented. The missing decision is how the compiler owns each answer and its dependencies without giving the engine a second implementation of language semantics.

## Decision

This record remains Draft while the linked design's open questions are reviewed.

1. **The compiler owns producers; the engine asks for results.** Each record family has one authoritative producer per compiler generation. A producer enters the existing semantic operation. Unsupported work is explicit, never a silent whole-program fallback.
2. **Contributions are independently reusable.** Visitors and passes contribute facts to a declaration's logical representation. Each contribution has an owning producer, its own value digest and tracked dependencies. Identity follows the compiler's checked declarations; documentation and source positions remain separate from semantic facts.
3. **Prepared, tracked reads are the dependency authority.** The compiler establishes prerequisites before evaluating a contribution. Results and their read evidence publish atomically. The engine reuses validated results and processes changes in dependency rank order, stopping propagation when an output stays equal. Compiler-owned joint evaluations handle genuine semantic cycles.
4. **Migration preserves one semantic implementation.** Replace whole-program projections one family at a time, using record-scoped entries into the existing checker, Body IR builder and lowering. A migrated family has no selectable projection fallback or parallel grammar authority.
5. **Correctness and reuse both require evidence.** Compare warm results with a fresh computation and compare the new route with the existing compiler. The design defines the output-equivalence checks and exact evaluation counts that prove an edit recomputes only the required contributions.

## Rationale and consequences

Separating contributions lets a body edit reuse callers that observed only an unchanged signature. Keeping tracked reads at compiler boundaries makes this reuse explainable without teaching the engine language semantics. The main cost is restructuring compiler operations and their reads, not adding a second checker.

Native output remains a whole compiled unit in this stage. Per-function native recompilation is qualified separately. Producer profiles bound reuse across compiler changes.

## Non-goals

New language semantics, a public compiler API, a new intermediate representation, parallel scheduling policy, or per-function native compilation. This record does not choose unresolved schema homes, protocol versions or delivery-stage boundaries.

## Design and review

The [producer-map design](../contributing/explanation/build_engine_architecture.md#producer-map-design-draft) owns the producer table, keys, schemas, ranks, preparation protocol, dependency indexes, migration plan, worked example and verification matrix. Its [open questions](../contributing/explanation/build_engine_architecture.md#open-design-questions) remain pending; the recommendations are not approvals.

## Revisit condition

Revisit if legal programs require dependencies the ranked design cannot represent, warm and cold results disagree, or tracking costs defeat the intended reuse.

## Provenance

Refines DD-0012 and DD-0013 without superseding them. The maintainer required the producer map on 2026-10-30 and clarified the independently reusable contribution model and decision/design separation on 2026-11-08.

## References

- [DD-0012: Incremental compilation over a persistent semantic store](0012_incremental_compilation_over_a_semantic_store.md)
- [DD-0013: The Incan build engine](0013_the_incan_build_engine.md)
- [Build engine architecture](../contributing/explanation/build_engine_architecture.md)
- [Build engine tracking issue #2481](https://github.com/encero-systems/incan/issues/2481)
