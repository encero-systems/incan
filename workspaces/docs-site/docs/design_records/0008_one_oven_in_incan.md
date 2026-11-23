---
id: DD-0008
title: One Oven, written in Incan
status: Accepted
type: design-decision
date: 2026-10-20
review_target: v0.6
sources:
  - Maintainer discussion, 2026-10-20 (removal of the Rust Oven crates)
  - Maintainer acceptance, 2026-11-08: one Incan-authored Oven owns project and build decisions
  - https://github.com/encero-systems/incan/issues/1698
  - DD-0005
  - DD-0007
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0008: One Oven, written in Incan

## Context

Before this decision, Oven had two halves. The Rust crates under `loaves/oven/` and the `oven` command owned the store, leases, receipts, `rustc` planning and execution, and the project owner process. The Incan project `workspaces/oven` owned manifest intake, Loaf resolution, native plan selection and Rust feature activation, and the Rust half called it by exchanging versioned JSON documents. The Rust half also resolved dependency closures, selected plans and activated features on its own, so both halves decided the same questions. The compiler read `loaf.toml` through a third model of its own.

## Decision

**There is one Oven, and it is the Incan project. No Rust crate makes an Oven decision.**

Oven alone owns project manifests, dependency resolution and locks, feature selection, compilation policy, preparation and reuse, toolchain layout and the command line. The compiler receives an explicit unit description; it neither resolves the project again nor keeps a second store.

Oven's decisions and actions share typed values in one Incan program rather than exchanging documents between competing implementations. External tools request the owning unit's description through that boundary.

A resident owner may reuse prepared state, but must reconcile inputs and leave execution of the user's program to the requesting command. [DD-0013](0013_the_incan_build_engine.md) replaces this record's original per-project owner with one engine per physical bakery.

Bootstrap is temporary and explicit, not a second production authority. [DD-0010](0010_released_toolchains_for_ci_and_development.md) replaces the frozen bootstrap builder with an installed released toolchain. The detailed unit and bootstrap contracts remain in the supporting design.

## Consequences

- `oven_rustc`, `oven_store`, `oven_interop`, the `oven` command (`oven-cli`) and `incan_oven_facet` are removed. So are the driver's completed-output selection, native plan selection, bake and publication, and the Rust project owner process. DD-0013 governs the replacement engine and physical-store ownership.
- The compiler's manifest model (`oven_model::manifest`) and its project discovery are replaced by the unit description.
- Oven's host functions (file observation, process execution, store writes) are written in Incan over the standard library and Rust interop (DD-0006).

## Non-goals

- The format of the store and of the unit description beyond what this record names.
- Remote Loaf fetching and version solving, which loaves.pub and the lock own (DD-0007).

## Supporting design

[Oven unit boundary and bootstrap](../contributing/explanation/oven_unit_bootstrap_design.md) records the mechanisms and supporting evidence. Changes to those mechanisms do not change this decision unless they alter its stated boundaries or guarantees.

## Revisit condition

Revisit if a Rust crate must make an Oven decision because Incan cannot yet express it. That crate is then named here, together with the gap that forces it.

## Provenance

Accepted by the maintainer on 2026-11-08: one Incan-authored Oven owns project and build orchestration, while the compiler consumes the selected inputs. Acceptance records this ownership decision, not implementation completion. Builds on DD-0005 (eager preparation and Just Enough Compilation) and DD-0007 (Loaf artifacts). DD-0010 and DD-0013 govern the later release-bootstrap and resident-engine arrangements described above. Supersedes nothing.

## References

- [Issue #1698](https://github.com/encero-systems/incan/issues/1698)
- DD-0005, DD-0006, DD-0007
