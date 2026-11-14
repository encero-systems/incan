# Toolchain packaging and release qualification

This page holds the detailed design supporting [DD-0010](../../design_records/0010_released_toolchains_for_ci_and_development.md), whose status remains **Accepted**. It preserves the technical contracts relocated from that record; relocation does not approve a draft or establish implementation completion. The decision record retains the architectural choices and their rationale.

## Design contracts

Oven's `incan toolchain pack OUT` produces the archive from an installed toolchain. It contains the named compiler driver, runtime dependencies, compiled standard Loaves, Rust standard-library ABI caches and prepared registry closure. Artifact paths are relative to their containing documents. The machine's Rust sysroot and local driver identity proof are not release contents.

`incan toolchain install ARCHIVE` installs under the selected Incan home. The archive must name the toolchain this Oven supports. Installation binds the local Rust sysroot and verifies the staged driver's identity before replacing an existing installation. Moving the archive between machines does not relax the pinned compiler or loaded-library identity checks.

**An archive is releasable only if the toolchain it installs relocates.** The gate (`scripts/verify_installation.incn`) packs the installed toolchain, installs it into a fresh directory against a copied sysroot, and audits that every artifact path in every document Oven reads is relative and resolves inside that installation. It then runs the check, testing and interop contract families against the relocated installation with the source installation and its builder directories absent. Each case must have the source installation's outcome.

### CI and later development

CI runners first install Rust 1.98.0 with `rustc-dev`, then install the released artifact the repository pins. Later development uses the same published artifact rather than reconstructing a private bootstrap environment. The artifact and Rust installation must match the runner's host platform.

Release production must build the runtime, lowering, driver, standard packages, Oven, contract runner and LSP from source with an installed release pack, the seed. The frozen bootstrap builder and generated Rust are retired as production paths. Acceptance requires an audit of every build unit excluding the private bootstrap inputs, a case-for-case census match on the same source and keys, and byte-identical driver, runtime, lowering, standard-library floor, Oven and runner artifacts in generations two and three. Generation one is built by the seed and can legitimately retain the seed's code generation; generations two and three are both built from this source by a compiler built from this source, so their equality is the fixed point. Until that complete gate passes, a partial native build does not establish a bootstrap-free release producer.

Release executable profiles strip debug provenance and remap checkout and selected-toolchain source locations to logical prefixes before the platform linker signs an executable. On macOS, a release executable retains its required UUID, derives that UUID from the unsigned executable content, and receives its ad-hoc signature only after the UUID is final. Library profiles stay unchanged, store layout paths remain custody evidence rather than artifact bytes, and the reusable producer identity continues to retain the exact compiler and linked inputs.

`scripts/release.incn` is the release-production authority. To run it on your own machine, see [Build and verify a release locally](../how-to/build-a-release-locally.md). It retains phase logs, timings, a refusal-capable receipt and the packed bundle, and its fixed-point mode retains three digest sets and compares generations two and three for the six named artifacts above byte for byte. CI runs the same program; the release workflow enables fixed-point mode, while ordinary stage qualification omits it because the proof builds two additional generations. An installed seed is a prerequisite, never the compiler under test. The repository pins each host's seed tag and digest in `build-toolchain.json`, while publication remains a separate maintainer action.

CI builds each PR's compiler with the installed toolchain and verifies the resulting compiler. Installing a release artifact is a prerequisite for that build, not evidence that the PR's compiler was tested.

## Historical qualification evidence

The following is the dated observation recorded with DD-0010, not a claim that later revisions have passed the same gate.

Measured on 2026-10-22 on the maintainer's macOS arm64 machine, with core, testing, interop, async, system, codecs, compression and web compiled: the zstd archive is 464,503,294 bytes (443 MiB) and installs in 17.1 s. The relocated installation, with the source installation absent, reproduced the source outcomes case for case in all three families.
