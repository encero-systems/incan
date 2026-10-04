---
id: DD-0004
title: Isolate the native compiler driver in a pinned Loaf
status: Accepted
type: design-decision
date: 2026-10-03
review_target: v0.6
sources:
  - https://github.com/encero-systems/incan/issues/1337
  - https://github.com/encero-systems/incan/issues/1675
  - https://github.com/encero-systems/incan/issues/654
  - DD-0002
  - RFC 124
---

# DD-0004: Isolate the native compiler driver in a pinned Loaf

## Context

[Issue #1337](https://github.com/encero-systems/incan/issues/1337) establishes Incan's direct compilation route: Incan and Rust units share one compilation graph, with no generated Rust source or Cargo on the build path. This record chooses the driver boundary for that route.

rustc's internal interface changes between releases. Incan already pins its Rust toolchain ([DD-0002](0002_single_pinned_rust_version.md)); the remaining choice is where that interface lives and how Oven invokes it.

## Decision

**The native compiler driver is a separate Loaf built against the toolchain's exact pinned rustc.**

- Oven invokes the driver for each Incan or Rust compilation unit. The driver's build identity enters the identity of every unit it compiles, under [RFC 124](../RFCs/124_oven_store_unit_identity_and_cross_plan_artifact_sharing.md).
- The driver runs the Incan front end in-process. Incan declarations enter rustc as items; checked Body IR supplies their MIR bodies. No Rust source is generated, and no serialized checker-to-lowering handoff is needed.
- New lowering and orchestration code is authored in Incan. The Rust exception is the narrow adapter that holds rustc's internal types and turns the lowering's plain-data plan into MIR. Only the driver Loaf's own units may use rustc's internal interface.
- Oven owns a resident driver process and reuses it across compilation requests. Analysis-only commands continue to use the front end without loading rustc.

The lowering belongs in the compiler ring; the driver executable belongs in the toolchain ring. Oven locates it through the compiler's provider facet.

## Consequences

The compiler's dependence on rustc's unstable interface is concentrated in one adapter. Each Rust upgrade requires verifying and, where necessary, updating that adapter. The front end remains usable independently for checking and editor analysis.

The driver shares rustc's type and code-generation machinery with Rust units. Incan still owns semantic checking and Body IR lowering: invalid supplied MIR is a compiler defect. Construct coverage and end-to-end build performance must be proven through #1337's acceptance checks.

Keeping the driver resident avoids repeated process startup and library loading. It also makes driver lifecycle and per-request isolation part of the implementation work.

## Non-goals

This record does not define lowering algorithms, package-publication contracts, caller APIs, the resident-process protocol, or a rustc fork. Implementation details, spike evidence, measurements, and required adjustments to existing RFCs are tracked in [#1337](https://github.com/encero-systems/incan/issues/1337).

## Revisit condition

Revisit the boundary if a required operation cannot be expressed through rustc's available hooks, if upgrades require spreading rustc-specific code beyond the adapter, or if the integrated route cannot meet #1337's performance requirements.

## Provenance

Accepted on 2026-10-04. This record preserves the driver shape explored by #1337's spike and applies DD-0002's pinned-toolchain policy. It replaces the earlier proposal in #1337 to feed a thin driver serialized checked facts. The broader route and cutover remain owned by #1337, [#1675](https://github.com/encero-systems/incan/issues/1675), and [#654](https://github.com/encero-systems/incan/issues/654); this record does not amend an RFC or claim implementation completion. `review_target` identifies a review point.

## References

- [#1337: Direct-route implementation, evidence, and acceptance](https://github.com/encero-systems/incan/issues/1337)
- [DD-0002: Single pinned Rust version](0002_single_pinned_rust_version.md)
- [RFC 124: Oven store unit identity](../RFCs/124_oven_store_unit_identity_and_cross_plan_artifact_sharing.md)
