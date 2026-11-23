---
id: DD-0010
title: Released toolchains for CI and development
status: Accepted
type: design-decision
date: 2026-10-22
review_target: v0.6
sources:
  - Maintainer decision, 2026-10-22
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0002
  - DD-0004
  - DD-0008
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0010: Released toolchains for CI and development

## Context

Before this decision, every development checkout built the compiler and Oven with a frozen bootstrap builder: a compiler from before the direct native route, kept in its own private build environment. Repeating that bootstrap in every checkout or CI job made the builder's private environment a prerequisite for ordinary development. DD-0002 pins Rust, DD-0004 verifies the driver's compiler identity by content, and DD-0008 assigns toolchain installation and orchestration to Oven. A portable installed toolchain lets later work begin from a published compiler.

## Decision

**A completed toolchain becomes a real release: its packed toolchain is the release artifact of its version tag. CI and every later development checkout install a released artifact.** This decides how toolchains are distributed; whether a given toolchain is complete and published is the maintainer's release decision.

Released artifacts contain the complete compiled toolchain closure and relocate independently of the source installation. Installation verifies the selected compiler and driver identity; moving an archive does not relax those checks.

CI and later development use the repository's pinned, host-compatible release and Rust toolchain. CI builds and tests the proposed compiler; merely installing its seed is not evidence that the proposed compiler works.

Release production is Incan-authored and uses an installed release seed, with no frozen bootstrap builder or generated Rust production path. Qualification requires complete build provenance, case-for-case behavioral parity and byte-identical named artifacts in generations two and three. A partial native build does not establish self-hosted release production.

One release program owns production for both local use and CI. Release publication remains a separate maintainer action. Packaging, relocation checks, deterministic executable preparation and the exact fixed-point evidence belong in the supporting design.

## Consequences

Ordinary CI and development depend on a versioned release artifact and the pinned Rust installation. They no longer depend on another checkout's build paths. Release production must retain the complete named native closure; a copied driver alone is insufficient. The archive carries exactly the standard Loaves the installed toolchain compiled: a package that does not compile is absent, not stubbed. The maintainer controls release publication, and development checkouts publish nothing themselves.

## Non-goals

This decision does not declare self-hosting complete, implement a CI workflow, publish a release, define artifact signing or promise cross-platform compatibility for one host's archive. It does not change the compiler, Oven or Loaf ownership boundaries.

## Supporting design

[Toolchain packaging and release qualification](../contributing/explanation/toolchain_release_design.md) records the mechanisms and supporting evidence. Changes to those mechanisms do not change this decision unless they alter its stated boundaries or guarantees.

## Revisit condition

Revisit when the pinned Rust version changes, the toolchain becomes self-hosting, or release consumers need another host platform or distribution format.

## Provenance

Records the maintainer's October 22, 2026 decision for #2256, with the relocation gate measured the same day. It supersedes no RFC or design record.

## References

- [#2256](https://github.com/encero-systems/incan/issues/2256)
- [DD-0002: Single pinned Rust version](0002_single_pinned_rust_version.md)
- [DD-0004: Native route through the pinned rustc](0004_native_route_through_the_pinned_rustc.md)
- [DD-0008: One Oven, written in Incan](0008_one_oven_in_incan.md)
