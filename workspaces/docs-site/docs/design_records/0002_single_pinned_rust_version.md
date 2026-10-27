---
id: DD-0002
title: Pin one Rust version and one generated-project edition
status: Accepted
type: design-decision
date: 2026-08-24
review_target: v0.7 planning
sources:
  - https://github.com/encero-systems/incan/issues/1168
  - https://github.com/encero-systems/incan/pull/1157
  - RFC 119
---

# DD-0002: Pin one Rust version and one generated-project edition

## Context

Incan provisions its own Rust without reconfiguring the user's default toolchain. That ownership requires a deliberate version policy: a floating release or CI selector can otherwise change the compiler used for the same source tag.

This decision predates dev.8's native route. “Generated projects” names the earlier Rust-emission producer; [DD-0004](0004_native_route_through_the_pinned_rustc.md) defines current native compilation. The implementation inventory, retired conformance fixtures and September 2026 measurements formerly recorded here remain historical evidence in this file's revision history and the linked issues, rather than current validation targets.

## Decision

Incan pins **one Rust version** across everything that builds or ships, and **one edition** for the generated-project boundary this record originally governed.

- The shipped compiler, contributor toolchain, declared minimum supported Rust version and CI build/test toolchains name **Rust 1.98.0** exactly.
- The generated-project edition is **2024**. The native route's compilation boundary is defined by DD-0004.
- Each value has one authority and changes deliberately. A newer available release does not move the pin.

Pinning the contributor minimum alongside the shipped version is intentional. Keeping a lower minimum would preserve a distinction that no longer protects a separate build-from-source installation contract.

The review target is a point to reconsider the pin, not a commitment to change it.

## Boundaries and consequences

- Rebuilding a release selects the same Rust compiler. This fixes the compiler input; it does not by itself prove byte-identical complete toolchains.
- Contributors use the pinned compiler, and CI verifies that selection rather than trusting an ambient stable toolchain.
- Edition changes require behavioral validation, including reserved identifiers, lifetime capture, mutable statics and temporary scopes. An edition bump must not silently break accepted source programs.
- Recoverable linker-symbol identities depend on the pinned compiler's encoding. A symbol probe proves only the inputs and artifacts it actually measures; it does not prove repository-wide pin selection or correct projection of every Incan declaration. Source identities and concrete Rust specializations remain distinct facts.
- Nightly rustfmt is a development-only formatter because the formatting configuration uses unstable comment options. It neither selects the production compiler nor compiles user programs. The retired Cargo publisher's nightly metadata flags are not a production exception.

## Non-goals

- Changing project-format compatibility policy, Oven's build/receipt contracts or provider planning.
- Defining toolchain provisioning mechanics, which belong to Oven and the installer.
- Making generated Rust a stable public contract or restoring the retired producer.
- Tracking Rust stable releases on a schedule.

## Revisit condition

Reconsider the pin when the compiler needs a named language/library feature it lacks, a security advisory affects it, or the supported platforms require another release. Reconsider the edition when a newer edition offers a concrete benefit at the relevant compilation boundary. Routine availability of a newer release is not a revisit condition.

## Provenance

The policy comes from the v0.6 move away from the former Cargo-based producer, [RFC 119](../RFCs/119_oven_native_rust_build_facets_and_cargo_interoperation.md), and the audit in issue #1168. PR #1157 exposed the earlier edition constant and reserved-keyword gap. Historical source inventories and symbol-size receipts do not establish current native-route coverage or close separate acceptance criteria.

## References

- [Issue #1168: generated-project edition and shipped Rust pin](https://github.com/encero-systems/incan/issues/1168)
- [Issue #1174: recoverable emitted-name projection](https://github.com/encero-systems/incan/issues/1174)
- [Issue #1182: symbols reconciled against compiler declarations](https://github.com/encero-systems/incan/issues/1182)
- [PR #1157: historical source-observable shadow comparison](https://github.com/encero-systems/incan/pull/1157)
- [RFC 119: Oven-native Rust build facets and Cargo interoperation](../RFCs/119_oven_native_rust_build_facets_and_cargo_interoperation.md)
