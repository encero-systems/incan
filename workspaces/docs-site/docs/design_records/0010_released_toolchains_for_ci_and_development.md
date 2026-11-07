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

Oven's `incan toolchain pack OUT` produces the archive from an installed toolchain. It contains the named compiler driver, runtime dependencies, compiled standard Loaves, Rust standard-library ABI caches and prepared registry closure. Artifact paths are relative to their containing documents. The machine's Rust sysroot and local driver identity proof are not release contents.

`incan toolchain install ARCHIVE` installs under the selected Incan home. The archive must name the toolchain this Oven supports. Installation binds the local Rust sysroot and verifies the staged driver's identity before replacing an existing installation. Moving the archive between machines does not relax the pinned compiler or loaded-library identity checks.

**An archive is releasable only if the toolchain it installs relocates.** The installed-verification gate (`scripts/verify_installation.incn`) audits that every artifact path in every document Oven reads stays inside the fresh installation and that no retained record names the builder's home or source checkout. It then runs the complete contract census through that installation, and release qualification requires every installed outcome to equal the direct candidate outcome.

### CI and later development

CI runners first install Rust 1.98.0 with `rustc-dev`, then install the released artifact the repository pins. Later development uses the same published artifact rather than reconstructing a private bootstrap environment. The artifact and Rust installation must match the runner's host platform.

Release production must build the runtime, lowering, driver, the standard packages, Oven, contract runner and LSP from source with an installed release pack, the seed. The frozen bootstrap builder and generated Rust are retired as production paths. Acceptance requires an audit of every build unit excluding the private bootstrap inputs, a case-for-case census match on the same source and keys, and byte-identical driver, runtime, lowering, standard primitives, Oven and runner artifacts in generations two and three. Generation one is built by the seed and can legitimately retain the seed's code generation; generations two and three are both built from this source by a compiler built from this source, so their equality is the fixed point. Until that complete gate passes, a partial native build does not establish a bootstrap-free release producer.

Release executable profiles strip debug provenance before the platform linker signs an executable. Library profiles stay unchanged, store layout paths remain custody evidence rather than artifact bytes, and the reusable producer identity continues to retain the exact compiler and linked inputs.

`scripts/release.incn` is the release-production authority. It retains phase logs, timings, a refusal-capable receipt and the packed bundle, and its fixed-point mode retains three digest sets and compares generations two and three for the six named artifacts above byte for byte. CI runs the same program; its release workflow enables fixed-point mode once the reproducibility stage lands. An installed seed is a prerequisite, never the compiler under test. The repository pins each host's seed tag and digest in `build-toolchain.json`, while publication remains a separate maintainer action.

CI builds each PR's compiler with the installed toolchain and verifies the resulting compiler. Installing a release artifact is a prerequisite for that build, not evidence that the PR's compiler was tested.

## Consequences

Ordinary CI and development depend on a versioned release artifact and the pinned Rust installation. They no longer depend on another checkout's build paths. Release production must retain the complete named native closure; a copied driver alone is insufficient. The archive carries exactly the standard Loaves the installed toolchain compiled: a package that does not compile is absent, not stubbed. The maintainer controls release publication, and development checkouts publish nothing themselves.

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
