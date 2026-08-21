---
id: DD-0001
title: Defer GPU target capability and graphics contracts
status: Accepted
type: design-decision
date: 2026-08-11
review_target: v0.8 planning
sources:
  - RFC 092
  - https://github.com/encero-systems/incan/issues/1041
  - https://github.com/encero-systems/incan/pull/1044
---

# DD-0001: Defer GPU target capability and graphics contracts

## Context

[RFC 092](../RFCs/closed/rejected/092_interactive_runtime_stdlib_contracts.md) defined the Incan-owned standard-library and artifact contracts needed by interactive runtime consumers (subsequently rejected; covered by RFC 104/036/117-119). GPU target capability and graphics contracts are not required for that work, Incan v0.6's self-hosting direction, Oven authority, the direct-HIR backend cutover, or the first interactive runtime target.

[Issue #1041](https://github.com/encero-systems/incan/issues/1041) records the need for a separate, consumer-backed design before Incan admits a GPU capability family. Keeping that work inside RFC 092 would expand its scope without a concrete consumer and could prematurely constrain later graphics work.

## Decision

GPU target capability and graphics contracts are deferred from the v0.6 and RFC 092 implementation scope.

The accepted decision is to defer the work, not to implement it now. v0.8 planning is the next review point for deciding whether the work should be scheduled; it is not a commitment to ship GPU support in v0.8.

## Consequences

- v0.6 does not include GPU-specific target capability, resource, artifact, diagnostic, or rendering contracts.
- Existing target and interactive-runtime abstractions remain focused on their current consumers.
- A future GPU proposal must begin with a concrete runtime consumer and revisit the constraints and acceptance checklist in #1041.
- Any future contract must integrate with Incan's existing target, capability, artifact, receipt, and diagnostics authorities without turning the compiler into a graphics framework.

## Non-goals

- Implementing GPU code generation, shaders, device APIs, or graphics contracts.
- Defining a renderer, scene graph, UI framework, or graphics-framework integration in the compiler.
- Replacing, reopening, or superseding RFC 092.
- Guaranteeing GPU support in v0.8.

## Revisit condition

Review this decision during or before v0.8 planning if:

- GPU target capability becomes blocking for a supported runtime or integration;
- a concrete consumer-backed proposal with acceptance criteria is ready; or
- the constraints recorded in #1041 change materially.

The review must decide whether the first public surface is metadata-only or includes typed resource handles. It must also show how capability discovery, resource lifecycle and sharing, artifact references, diagnostics, and receipts work end to end for the selected consumer.

## Provenance

This record derives from RFC 092, issue #1041, and the design-record proposal developed in [PR #1044](https://github.com/encero-systems/incan/pull/1044). It records the accepted deferral and does not supersede those authorities.

## References

- [RFC 092: Interactive Runtime Stdlib Contracts](../RFCs/closed/rejected/092_interactive_runtime_stdlib_contracts.md) (rejected)
- [Issue #1041: GPU target capability and graphics contracts](https://github.com/encero-systems/incan/issues/1041)
- [PR #1044: Add design doc deferring GPU target capability to v0.8](https://github.com/encero-systems/incan/pull/1044)
