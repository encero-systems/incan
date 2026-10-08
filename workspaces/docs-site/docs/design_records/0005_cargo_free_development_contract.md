---
id: DD-0005
title: Cargo-free development with eager dependencies and Just Enough Compilation
status: Accepted
type: design-decision
date: 2026-10-08
review_target: v0.6
sources:
  - Maintainer product-contract discussion, 2026-10-08
  - https://github.com/encero-systems/incan/issues/1337
  - https://github.com/encero-systems/incan/issues/1675
  - https://github.com/encero-systems/incan/issues/1698
  - DD-0004
  - RFC 119
  - RFC 124
  - RFC 125
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0005: Cargo-free development with eager dependencies and Just Enough Compilation

## Context

The Cargo-free route must provide a straightforward application and compiler development experience. Users should run their programs and select their tests without managing dependency preparation. Contributors need evidence that each edit causes only necessary work, rather than repeated preparation of the compiler, SDK, and dependency graph.

[Issue #1675](https://github.com/encero-systems/incan/issues/1675) establishes application latency and cutover requirements. [Issue #1698](https://github.com/encero-systems/incan/issues/1698) owns the compiler's Cargo-free build and test path. This record captures the product behavior agreed in the maintainer discussion on 2026-10-08. Acceptance of the contract does not establish that the current implementation satisfies it.

## Decision

**The toolchain prepares dependencies eagerly and performs Just Enough Compilation (JEC) for each requested operation. Ordinary development commands require no manual baking or prewarming.**

### Test drives

| Action                                                            | Required experience                                                                                                                                               |
| ----------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Run a prepared application with `incan run`                       | Use the ready dependency graph automatically and produce the application's output without a preparation step.                                                     |
| Change one application line and run again                         | Compile only the affected units, reuse unchanged dependencies, and produce the changed output.                                                                    |
| Add a compatible, already prepared dependency to `loaf.toml`      | Select it from the local cache and make the updated graph ready in under 100 ms.                                                                                  |
| Add a dependency that is not prepared yet                         | Start preparation when the valid requirement change is observed. If Run catches preparation underway, show concise progress and execute automatically when ready. |
| Change an Incan lowering function and run one selected fixture    | Rebuild only the affected compiler units and required dependents, then exercise the updated compiler.                                                             |
| Repeat that fixture without changes                               | Reuse the compiler, SDK, and dependencies; execute only the requested fixture.                                                                                    |
| Use the same compatible dependency in another project or worktree | Share its immutable prepared units while keeping mutable project outputs isolated.                                                                                |

### Ordinary command surface

`incan run` and `incan test` own the preparation necessary for their request. Users do not need to invoke a bake command, set internal preparation variables, select a backend, or understand SDK and artifact-store layout to use them. Necessary dependency work remains part of the toolchain's operation even when no explicit preparation command is exposed.

The complete developer path is Cargo-free, including compiler bootstrap, dependency preparation, compilation, and test execution. An installed toolchain supplies stage zero as required by #1698. A Cargo guard installed only after bootstrap cannot establish this property for the whole command. The direct route retains #1337's no-generated-Rust requirement.

### Eager dependency preparation

For an active project, observing a valid requirement change in `loaf.toml` starts resolution and preparation without waiting for Run or Test. Use compatible units already in the local cache, obtain missing compatible assets through `incan.pub`, and compile from admitted source when no compatible prepared asset is available, according to [RFC 125](../RFCs/125_incan_pub_loaf_registry_and_baked_asset_distribution.md).

Preparation is bound to the selected graph. A later manifest change cannot cause a command to execute a stale graph. Run and Test join preparation of the graph they require; they do not start a duplicate copy of the same work. If that preparation is incomplete, they show concise progress and continue automatically once it succeeds. A preparation failure produces an actionable diagnostic.

The local readiness requirement is **under 100 ms from observing the valid manifest change to the dependency graph becoming ready**, including necessary resolution, compatibility checks, and project dependency-state updates. This case assumes the required metadata and compatible prepared units are already admitted locally. It requires no network access or compilation and no copying of the dependency closure into the project. Observation delay must be reported separately rather than hidden by this measurement boundary.

A downloaded source Loaf alone is not a prepared unit. Toolchain, target, profile, features, and effective inputs must match under [RFC 124](../RFCs/124_oven_store_unit_identity_and_cross_plan_artifact_sharing.md). Downloading missing content and performing necessary compilation are separate measurement cases; this record assigns them no invented latency guarantee.

### Just Enough Compilation and testing

JEC applies to compiler development and test preparation as well as application builds:

- Recompile a unit only when its effective compilation inputs require it. Rebuild dependents and relink only where changed inputs make that work necessary.
- Reuse unchanged compiler, SDK, and dependency units. A compiler edit does not justify preparing the entire SDK or dependency closure again.
- An unchanged repeat performs zero compiler, SDK, or dependency compilation and no unnecessary relinking.
- Selecting one fixture prepares only what that fixture needs and executes only that fixture. It does not run the census or unrelated tests.
- Full-suite execution shares common preparation across cases. Broader verification remains necessary at integration boundaries; precise selection during development does not establish full-suite acceptance.
- Report compilation deliberately required by a fixture separately from preparation of the compiler and its dependencies.

These are work requirements, not a new compiler rebuild deadline. Elapsed time follows from efficiency and remains measured. JEC does not excuse unnecessary resolution, validation, copying, inspection, or linking merely because compilation counts are zero.

Reuse preserves the existing receipt, integrity, compatibility, and lease checks. A candidate lookup is not proof that an artifact is safe to execute. Equivalent retained proofs may avoid repeated work; weakening the input identity or accepting stale outputs is not an optimization.

### Acceptance evidence

Retain the selected source, compiler, driver, SDK, registry, target, profile, and feature identities. Evidence must reconcile requested work with actual execution and show:

- which units were reused, obtained, compiled, or linked, and why;
- which selected cases actually executed;
- that the changed lowering reached the compiler exercised by the fixture;
- that unchanged repeats and compatible worktrees reuse the same prepared units;
- elapsed time and storage costs alongside the work performed;
- zero Cargo invocations across the complete command boundary.

A passing command exit or timestamp alone does not prove these properties. Wrong results, diagnostic mismatches, unexpected compilation, and unintended case execution must be visible to an acceptance check. Explicitly pending cases retain their observed outcomes and dispositions.

### Existing application performance requirements

JEC for compiler authoring does not replace #1675's application and cutover performance requirements. Its small-program, warm-store, developer-machine conditions continue to apply, including unchanged-run overhead under 50 ms and a one-line application edit reaching first output under 250 ms. Measure complete user journeys, including preparation waits; component timings alone cannot establish those requirements.

## Consequences

Dependency preparation requires observation of active projects and coordination between background work and ordinary commands. Compatible consumers need shared immutable units, isolated mutable outputs, and correct lifecycle handling. Registry asset availability affects how often a dependency addition requires compilation.

The developer loop needs observable invalidation and reuse decisions. Recovery work should first prove one Cargo-free lowering-edit and selected-test loop, then remove every unjustified rebuild and preparation step before expanding construct lanes. Implementation sequencing and branch integration remain in the owning issues and implementation plans.

## Non-goals

- Choosing an editor integration, project-watching service, resident-process protocol, or background scheduling mechanism.
- Introducing a new dependency resolver, artifact identity, or package trust model.
- Setting an arbitrary latency deadline for rebuilding the compiler after a lowering edit.
- Promising the local 100 ms readiness result for missing downloads, source-only dependencies, or incompatible prepared units.
- Claiming current fixture parity, performance conformance, or implementation completion.

## Revisit condition

Revisit this contract if measured user journeys require different readiness behavior, if eager preparation performs unnecessary work or interferes with foreground requests, or if the admitted-unit model cannot provide correct reuse at the required granularity. Any revision must retain explicit measurement conditions and evidence of the work performed.

## Provenance

Accepted in the maintainer product-contract discussion on 2026-10-08. That discussion established ordinary commands without manual preparation, eager preparation on requirement changes, automatic waiting when Run catches incomplete preparation, local prepared-dependency readiness under 100 ms, and JEC as the acceptance basis for compiler edits and selected tests.

This record adds those product requirements while preserving the build, identity, distribution, and cutover authorities in #1337, #1675, #1698, [DD-0004](0004_native_route_through_the_pinned_rustc.md), and RFCs 119, 124, and 125. It does not supersede them or amend an RFC. Any required architectural change follows the owning RFC process. `review_target` identifies a review point, not a delivery claim.

## References

- [#1337: Native compilation through the pinned rustc](https://github.com/encero-systems/incan/issues/1337)
- [#1675: Replacement-route parity and cutover](https://github.com/encero-systems/incan/issues/1675)
- [#1698: Cargo-free toolchain build and test](https://github.com/encero-systems/incan/issues/1698)
- [DD-0004: Pinned native compiler driver](0004_native_route_through_the_pinned_rustc.md)
- [RFC 119: Oven-native Rust build facets](../RFCs/119_oven_native_rust_build_facets_and_cargo_interoperation.md)
- [RFC 124: Unit identity and cross-plan sharing](../RFCs/124_oven_store_unit_identity_and_cross_plan_artifact_sharing.md)
- [RFC 125: Loaf registry and baked asset distribution](../RFCs/125_incan_pub_loaf_registry_and_baked_asset_distribution.md)
