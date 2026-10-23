---
id: DD-0010
title: The dev.8 release toolchain and CI
status: Accepted
type: design-decision
date: 2026-10-22
review_target: v0.6
sources:
  - Maintainer decision, 2026-10-22 (dev.8 release lane)
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0002
  - DD-0004
  - DD-0008
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0010: The dev.8 release toolchain and CI

## Context

Development lanes currently bootstrap the dev.8 compiler and Oven with the frozen dev.7 builder. Repeating that bootstrap in every lane or CI job makes the builder's private environment a prerequisite for ordinary development. DD-0002 pins Rust, DD-0004 verifies the driver's compiler identity by content, and DD-0008 assigns toolchain installation and orchestration to Oven. A portable installed toolchain lets later work begin from a published compiler.

## Decision

**When dev.8 is complete, it becomes a real release. Its packed toolchain is the release artifact of the `0.6.0-dev.8` tag. CI and every later development lane install that artifact.** This is the distribution decision, not a claim that dev.8 is already complete or published.

Oven's `incan toolchain pack OUT` produces the archive from an installed toolchain. It contains the named compiler driver, runtime dependencies, compiled standard Loaves, Rust standard-library ABI caches and prepared registry closure. Artifact paths are relative to their containing documents. The machine's Rust sysroot and local driver identity proof are not release contents.

`incan toolchain install ARCHIVE` installs under the selected Incan home. The archive must name the toolchain this Oven supports. Installation binds the local Rust sysroot and verifies the staged driver's identity before replacing an existing installation. Moving the archive between machines does not relax the pinned compiler or loaded-library identity checks.

**An archive is releasable only if the toolchain it installs relocates.** The gate (`workspaces/oven/tests/verify_toolchain_relocation.sh`) packs the installed toolchain, installs it into a fresh directory against a copied sysroot, and audits that every artifact path in every document Oven reads is relative and resolves inside that installation. It then runs the check, testing and interop contract families against the relocated installation with the source installation and its builder directories absent. Each case must have the source installation's outcome.

### CI and later lanes

CI runners first install Rust 1.98.0 with `rustc-dev`, then install the dev.8 release artifact. Later development lanes use the same published artifact rather than reconstructing the private bootstrap environment. The artifact and Rust installation must match the runner's host platform.

Until self-hosting is proven, the maintainer's bootstrap machine produces the release toolchain with the frozen dev.7 builder. After the installed dev.8 toolchain can build its compiler's Incan Loaves, CI builds each PR's compiler with that installed toolchain and verifies the resulting compiler. Installing a release artifact is a prerequisite for that build, not evidence that the PR's compiler was tested.

## Consequences

Ordinary CI and development lanes depend on a versioned release artifact and the pinned Rust installation. They no longer depend on another lane's build paths. Release production must retain the complete named native closure; a copied driver alone is insufficient. The archive carries exactly the standard Loaves the installed toolchain compiled: a package that does not compile is absent, not stubbed. The maintainer controls release publication, and development lanes publish nothing themselves.

Measured on 2026-10-22 on the maintainer's macOS arm64 machine, with core, testing, interop, async, system, codecs, compression and web compiled: the zstd archive is 464,503,294 bytes (443 MiB) and installs in 17.1 s. The relocated installation, with the source installation absent, reproduced the source outcomes case for case in all three families.

## Non-goals

This decision does not declare self-hosting complete, implement a CI workflow, publish a release, define artifact signing or promise cross-platform compatibility for one host's archive. It does not change the compiler, Oven or Loaf ownership boundaries.

## Revisit condition

Revisit when the pinned Rust version changes, the toolchain becomes self-hosting, or release consumers need another host platform or distribution format.

## Provenance

Records the maintainer's October 22, 2026 decision for #2256, with the relocation gate measured the same day. It supersedes no RFC or design record.

## References

- [#2256](https://github.com/encero-systems/incan/issues/2256)
- [DD-0002: Single pinned Rust version](0002_single_pinned_rust_version.md)
- [DD-0004: Native route through the pinned rustc](0004_native_route_through_the_pinned_rustc.md)
- [DD-0008: One Oven, written in Incan](0008_one_oven_in_incan.md)
