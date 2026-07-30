# Architecture review — Oven positioning paper

Date: 2026-07-28

## Scope

- `workspaces/docs-site/docs/whitepapers/incan_oven_positioning.md` (untracked draft)
- RFC 079 artifact-graph direction
- RFC 116 typed C ABI interop direction
- Current Cargo-backed project and lockfile boundary

## Findings

1. **Warning — Loaf needs an explicit native deployment-handoff boundary.**
   The paper defines Loaves as target-compatible build products
   (`incan_oven_positioning.md:53-58`, `114-131`) and treats native linking as a
   build-planner concern (`93-99`), but it does not say how a resolved native
   package becomes usable by an Android or iOS application build. Add a
   deliberately non-normative statement: a Loaf may carry resolved native
   artifact identities, binding-verification and shim-build receipts, and a
   versioned target deployment plan. That plan must be useful input to Gradle
   and Xcode, while those host build systems retain final application assembly
   and signing. Do not choose a Gradle plugin, Xcode integration format, or
   concrete command protocol in this paper.

2. **Warning — "drop-in replacement" sets a stronger compatibility expectation
   than the paper actually intends.** The qualification at
   `incan_oven_positioning.md:47-49` is good, but the headline phrase still
   suggests Cargo CLI and behavioral parity. Replace it with language such as
   "adoption-compatible project authority" or "Cargo-independent toolchain for
   existing Rust projects." That preserves the important promise—no source
   rewrite or parallel ecosystem—without locking Oven into Cargo command or
   cache emulation.

## Result

The strategic ownership model is coherent: Oven owns resolution, verification,
shim building, locks, caches, staging, and the generated deployment plan; host
platform tools consume that result for final app packaging and signing. The
paper needs the two boundaries above to make that model explicit without
prematurely specifying its implementation.
