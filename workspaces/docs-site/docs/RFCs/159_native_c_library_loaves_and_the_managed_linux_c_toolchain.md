# RFC 159: Cloves (C-library Loaves) and the managed C toolchain

- **Status:** Draft
- **Created:** 2026-10-18
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 114 (compiled providers, SDK components, and package features)
    - RFC 116 (typed C ABI interop)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
    - RFC 124 (Oven store unit identity and cross-plan artifact sharing)
    - RFC 125 (`incan.pub` Loaf registry and baked asset distribution)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6 (in development)
- **Shipped in:** —

## Summary

This RFC makes C libraries first-class Loaves, called **Cloves**, and gives every target one managed C compiler. A Clove carries a C library's sources, its public headers, and a declared build: the exact compile units, the preprocessor definitions of each named configuration, and the static archive they produce. Oven compiles it like any other unit, under RFC 124 identity, and `incan.pub` publishes its source and its compiled units like any other Loaf. A Rust crate whose build script compiles that library (`zstd-sys`, `aws-lc-sys`, `bzip2-sys` and their kind) no longer records its own C compiles: its build fact links the library Loaf's unit, and an Incan C binding under RFC 116 links the same unit. Every native unit, on every target, is compiled by one Clang toolchain that Incan builds and publishes; only the platform files differ. For Linux, Incan publishes those files too: a sysroot with glibc 2.28, the baseline selected for this RFC from Python packaging's `manylinux_2_28` environment. Apple and Windows SDKs cannot be redistributed; units for those targets are compiled with the same managed Clang on registry builders that hold the SDK, and record the SDK identity they used.

## Core model

1. **A C library is a Loaf: a Clove.** It has its own name, version, source publication and compiled units, separate from any Rust crate that wraps it.
2. **A Clove declares its build.** Its compile units, include roots, configurations and public headers are manifest data; no build system, probe or host discovery runs when a consumer builds it.
3. **A Clove unit is an ordinary unit.** It is identified, cached, published and admitted under RFC 124 and RFC 125 like a Rust unit.
4. **Wrappers link, they do not recompile.** A Rust crate's build fact and an Incan C binding link a Clove unit; the registry admits that binding only with evidence that it builds what the crate's own build script built.
5. **One compiler for every target.** Incan builds Clang and its tools from one pinned LLVM release, with one configuration on every build host, and publishes it as one toolchain release. Every native unit names that release.
6. **Platform files are the only difference.** A Linux target's sysroot is published by Incan; an Apple or Windows target's SDK is licensed, used where it is licensed, and recorded by identity. Nothing substitutes a different one.

## Motivation

Many Rust crates contain C. `ring` compiles C and assembly derived from BoringSSL; `aws-lc-sys` compiles AWS-LC; `zstd-sys`, `bzip2-sys`, `lzma-sys`, `secp256k1-sys` and `tikv-jemalloc-sys` compile the C library they vendor; `blake3` compiles SIMD kernels written in C; `psm` assembles stack-switching code. RFC 119 already removes build scripts from Oven's authority: what such a script would have done is a declared `link` record, compiled at publisher bake, with the exact compiler named as an owner of the work. A consumer receives the finished archive inside an admitted unit.

That model is correct and still leaves two gaps, both visible in `incan.pub`'s bake of the crates.io top-1000.

**A native record only replays where its toolchain exists.** A `link` record names its compiler and SDK by an owner identity: a digest over every file the compiler installation and SDK contain. A record harvested on one developer machine names that machine's Command Line Tools and its `MacOSX27.0.sdk`; a hosted macOS builder carries a different Xcode, so the owner is absent and Oven correctly refuses to replay the record. On Linux the situation is worse: system headers live under `/usr/include`, so the only owner that covers a compile is the host's root filesystem, which no record can inventory. The result is that every crate whose closure reaches native C (over forty of the nine hundred top-1000 shards on each platform, including the whole rustls-based TLS stack, every AWS SDK client, `zip`, `rusqlite` and `stacker`) cannot be baked from source anywhere except on the machine that harvested it.

**The same C library is compiled many times, under many names.** `zstd-sys 2.0.16+zstd.1.5.7` and `zstd-sys 2.1.0+zstd.1.5.7` vendor the same zstd release and each records its own copy of the same compiles. Every `-sys` crate version that vendors a library repeats its build, and a project that reaches the library through two crate versions links two copies. A C library has an identity of its own, separate from the Rust crates that wrap it, and nothing in the package graph says so today.

Incan's own C interop has the same need from the other side. RFC 116 resolves static archives, headers and shims "through the package graph, locked by identity" and requires Linux targets to "resolve an explicit compiler and sysroot contract", but no Loaf kind carries a C library and no artifact is that Linux contract. This RFC supplies both.

Choosing features or crates that need less C is not a remedy: it depends on upstream authors changing their crates, and the libraries that matter most (cryptography, compression, databases) are C by design.

## Goals

- Define a C facet: a Loaf that carries a C library's sources, public headers, named build configurations and the static archive each produces.
- Make a Clove unit an ordinary RFC 124 unit, published and admitted under RFC 125.
- Let a Rust crate's build fact link a Clove unit instead of recording its own compiles, with an admission rule that proves the two build the same thing.
- Let an RFC 116 C binding link the same unit.
- Compile every native unit, on every target, with one Incan-built Clang toolchain, so that the compiler is never the reason two units differ.
- Publish the Linux sysroot, so that Linux native units are host-independent and buildable from source anywhere.
- Keep Apple and Windows correct without redistributing their SDKs.

## Non-Goals

- This RFC does not run a library's own build system (CMake, autotools, Meson, Make) at consumer build time, and it does not make any such system Oven authority. A Clove declares its compile units; deriving them from an upstream build system is an adoption-time activity.
- This RFC does not define C++ ABI interop. A Clove may compile C++ sources into an archive that exposes a C interface, under the same rules as C sources.
- This RFC does not redistribute the Apple SDK, the Windows SDK or the MSVC runtime.
- This RFC does not define musl-based Linux targets. A musl target would be a separate target with its own sysroot.
- This RFC does not replace RFC 116's `system` deployment class. A library a target platform supplies (a system framework, `libc` itself) stays a declared system capability.
- This RFC does not define dynamic linking of Cloves. Every Clove unit is a static archive; a bundled shared library remains RFC 116's `bundled` class.

## Guide-level explanation

### A Clove

A Clove is a Loaf whose facet is `c`; the registry adopts upstream C libraries as Cloves under the scope `c/`. Its manifest names the library, the headers it exposes and how to compile it:

```toml
[project]
name = "c/zstd"
version = "1.5.7"
license = "BSD-3-Clause OR GPL-2.0-only"

[c]
library = "zstd"
public-headers = ["lib/zstd.h", "lib/zdict.h", "lib/zstd_errors.h"]
include = ["lib", "lib/common"]

[[c.compile]]
sources = ["lib/common/*.c", "lib/compress/*.c", "lib/decompress/*.c"]

[[c.compile]]
sources = ["lib/decompress/huf_decompress_amd64.S"]
target = 'cfg(target_arch = "x86_64")'

[c.configurations.default]
defines = { ZSTD_MULTITHREAD = "1" }

[c.configurations.legacy-dictbuilder]
extends = "default"
sources = ["lib/legacy/*.c", "lib/dictBuilder/*.c"]
defines = { ZSTD_LEGACY_SUPPORT = "1" }
```

`oven bake` compiles one configuration for one target with the managed toolchain and produces a unit: the archive `libzstd.a`, the public headers, and a receipt naming the source digests, the configuration, the definitions, the toolchain release, the platform files, the target and the profile. Nothing is discovered on the host: no `pkg-config`, no search for an installed `zstd`, no environment variable that changes the build.

### A Rust crate that wraps it

Today `zstd-sys` records a build fact whose `link` record lists every compile its build script ran. Under this RFC, its adoption instead binds each binding to a configuration of `c/zstd`:

```toml
[[rust.facts]]
target = "x86_64-unknown-linux-gnu"
profile = "release"
features = ["legacy", "std", "zdict_builder"]

[[rust.facts.link]]
library = { loaf = "c/zstd", version = "1.5.7", configuration = "legacy-dictbuilder" }
```

When Oven plans `zstd-sys`, it plans the `c/zstd` unit for that configuration and links its archive. If the unit is available as an asset, Oven downloads it; otherwise it compiles it from the library Loaf's source with the managed toolchain. Two crate versions that bind the same library configuration share one unit.

A crate whose build script also compiles crate-private C (a shim, generated glue) keeps that part as an ordinary RFC 119 `link` record, compiled with the same managed toolchain.

### An Incan binding that links it

An RFC 116 binding names the library Loaf instead of a system library:

```incan
from std.interop import c

binding Zstd:
    header = "zstd.h"
    link = c.loaf_library("c/zstd", configuration="default")
```

The binding is verified against the library Loaf's public headers with the same toolchain that compiled the archive, and links the same unit a Rust crate in the same plan would.

### The managed C toolchain

The C compiler is not whatever a machine has installed. Incan builds Clang, its resource directory, an LLVM linker and an LLVM archiver from one pinned LLVM release, with one build configuration, for every build host it supports, and publishes them as one **toolchain release**. Oven fetches it like any other toolchain component and verifies it by digest. Before a release is published, the same corpus of C libraries is compiled with it on every build host, and every host must produce the same archive bytes; that is what lets a unit's identity name the release rather than the host it was built on.

What differs by target is the platform files:

- **Linux.** Incan publishes the sysroot: glibc headers and startup objects at glibc 2.28, and the matching kernel headers, taken from the `manylinux_2_28` build environment that Python wheels are built in. This selects glibc 2.28 as the minimum runtime version for the supported Linux targets; the target architecture and other declared runtime requirements must also match. It does not claim compatibility with every supported Linux distribution or every AWS Lambda runtime. Oven links final Linux executables against the same sysroot.
- **Apple.** The Xcode SDK, used on machines licensed for it. The managed Clang compiles against it; Xcode's own Clang is not used.
- **Windows.** The Windows SDK and the MSVC runtime headers and libraries, used on machines licensed for them. The managed toolchain's `clang-cl` driver compiles against them.

Because the compiler is the same everywhere, a native unit's identity is the toolchain release plus its platform files. On Linux both are Incan-published, so a fact recorded on any machine replays on every other machine.

### Apple and Windows targets

Units for Apple and Windows targets are built by registry builders (RFC 125's `registry` builder kind) running on hosts licensed for the SDK. Each unit records the SDK identity it was built with. A consumer admits those units by RFC 124 identity like any other asset; a consumer that builds from source must hold the same SDK identity, and a host with a different SDK produces a different unit rather than substituting one.

## Reference-level explanation

### The C facet

1. A Loaf with a `[c]` table carries a C facet. A C facet must name exactly one `library` (the archive's link name), its `public-headers`, and at least one `[[c.compile]]` entry. A Loaf may carry a C facet alongside other facets under RFC 117's rules for non-overlapping source roots.
2. Each `[[c.compile]]` entry names source files or globs relative to the Loaf root, an optional `target` predicate in RFC 119's predicate grammar, and optional per-entry `include` and `defines`. Globs are expanded once, at publication, into the exact file list the source Loaf records; a consumer never expands a glob.
3. Source files are C (`.c`), assembly (`.S`, `.s`) or C++ (`.cc`, `.cpp`, `.cxx`) compiled into an archive with a C interface. Any other file kind in a compile entry is refused.
4. A **configuration** is a named set of preprocessor definitions and additional sources. `default` must exist. A configuration may `extend` exactly one other configuration; the result is that configuration's definitions and sources plus its own, and redefining an inherited definition is refused rather than overridden.
5. A C facet must not declare compiler flags beyond definitions, include roots, the language standard (`std = "c11"` and its kind) and target predicates. Optimization level and debug information follow the plan's profile, and code-generation options follow the toolchain release and target; a facet cannot change them.
6. A C facet must not name an absolute path, an environment variable, a host tool, or a library outside the Loaf. A compile that reads a file outside the Loaf root, the facet's declared include roots, the toolchain release or the target's platform files is refused.

### Clove units

1. A Clove unit is the RFC 124 unit produced by compiling one configuration of one C facet for one target and profile. Its identity must include the source Loaf digest, the configuration name and its resolved definitions and source list, the target, the profile, the toolchain release and the identity of the target's platform files.
2. The unit's payload is the static archive and the public headers. The archive's object order, member names and timestamps must be deterministic, so that two builds with the same identity produce the same archive bytes.
3. A Clove unit is published and admitted under RFC 125 exactly as a Rust unit is: its asset carries its archive digest and unit identity, and a consumer admits it only on identity equality.
4. A plan that reaches the same Clove unit through several dependents must plan it once. Two configurations of the same library in one link closure are refused unless an RFC 119 isolation boundary separates them, because their symbols collide.

### Binding a Rust crate to a Clove unit

1. A Rust build fact's `link` record may, instead of listing compiles, name a Clove unit: `library = { loaf, version, configuration }`. The fact vocabulary gains this form; it is a change to the compiler's build-fact record and must land there before a registry publishes it.
2. A binding is matched to a configuration by what it compiled, not by its feature names. For each binding (features, target, profile, toolchain release), the crate's harvested native record names the exact sources and definitions its build script compiled. The registry may admit a library binding only to a configuration whose sources (by content digest), definitions and include structure are identical to that record. When no configuration matches, the registry may add one to the library Loaf, derived from the record; it must not bind to a near match.
3. A build script that chooses its sources by probing the compiler (whether it accepts a flag, whether it has an assembler for the target) is handled by the same rule: with one toolchain release, the probe has one answer per target, and the harvested record captures it. A build script whose result depends on the build host itself (its CPU, its installed packages, `-march=native`) produces no admissible record, as RFC 119 already requires.
4. Compiles a crate's build script performs beyond the library (a shim, generated glue) remain ordinary RFC 119 `link` records. A fact may hold both forms.
5. A fact that binds a Clove unit inherits the unit's toolchain and platform-file requirements.

### Linking a Clove unit from Incan

RFC 116's binding `link` value gains `c.loaf_library(loaf, configuration=...)`, a static artifact resolved through the package graph. Verification under RFC 116 must use the library Loaf's public headers, the configuration's definitions, the same toolchain release and the same platform files as the archive the plan links.

### The managed C toolchain

1. A **toolchain release** is a Clang-compatible compiler, its resource directory, a linker and an archiver, built by Incan from one pinned LLVM release with one build configuration, for each build host Incan supports. Its identity is the release, not the digest of any one host's build.
2. A toolchain release must pass a cross-host reproduction gate before publication: a published corpus of Cloves is compiled for every supported target on every build host, and every host must produce byte-identical archives. A build host that does not is published with its own identity, and units it compiles carry that identity instead of the release's.
3. Every native unit (a Clove unit, or an RFC 119 `link` record) must be compiled with a toolchain release, and its identity names that release and its platform files. A host-installed compiler is not selectable for a publishable unit; it may be used for local development only through an explicit, non-portable override that RFC 116 already describes.
4. The Incan toolchain names exactly one C toolchain release, and `oven.lock` records it. Moving to a new release is a toolchain change, and it changes every native unit identity.
5. This RFC replaces RFC 116's selection of the Xcode-provided Clang for Apple targets: Apple native units, and RFC 116 binding verification for Apple targets, use the managed toolchain release with the Xcode SDK. RFC 116's requirement to record the SDK identity, and not to redistribute the SDK, is unchanged.

### Platform files

1. **Linux.** For each supported Linux target, Incan publishes a sysroot: glibc headers, startup objects and linker scripts for glibc 2.28, and the kernel headers of the same environment, taken from the `manylinux_2_28` build image and pinned by that image's digest. Its identity is the digest of its content. The first targets are `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`. Oven's final link for a Linux target must use the same sysroot as the plan's native units.
2. **Apple.** The Xcode SDK installed on a licensed host. Its identity is the SDK's version and build identifier together with the digest of its contents. It is never published.
3. **Windows.** For `x86_64-pc-windows-msvc`, the first Windows target, the Windows SDK and the MSVC runtime headers and libraries installed on a licensed host, compiled against through the managed toolchain's `clang-cl` driver. Their identity is the SDK version, the runtime version and the digest of their contents. They are never published.
4. A host whose platform files differ from a unit's recorded identity must produce a different unit, never substitute one. A registry may hold units for several platform-file identities of the same configuration; RFC 119 already selects among a binding's records by the owners a host has.

### Registry adoption of C libraries

1. `incan.pub` may adopt a C library from an upstream release (a published source archive and its checksum) as an external-source record under RFC 125, with the C facet written by the registry. The record binds the upstream location, version and checksum to the source Loaf digest.
2. `incan.pub` may also derive a Clove from the vendored sources of an adopted crates.io crate, when those sources are the library's and not crate-specific glue. The record names the crate version it was derived from.
3. Adopted Cloves live under the registry-owned scope `c/`, with the upstream library's name and version, not the wrapping crate's.
4. An upstream project that later publishes its library under its own owner scope does not take over `c/<name>`: published versions are immutable. The registry records a supersession event on `c/<name>` that names the owner's package, so that new versions come from the owner.

## Design details

### Why the facet and the toolchain are one proposal

A Clove unit's identity names the toolchain that compiled it, and the point of a Clove is that any host can build or admit the same unit. Without one managed toolchain, a Clove would still compile to a different unit on every machine, and the facet would solve only the duplication half of the problem. Without the facet, a managed toolchain would make each `-sys` crate's own native record replayable but leave every crate version compiling its own copy of the library. Each part needs the other to deliver either result.

### Why one compiler on every target

With one compiler, the only inputs that differ between platforms are the platform files, which are recorded by identity anyway. A second compiler per platform (Xcode's Clang on Apple, `cl.exe` on Windows) would add a second identity coordinate with its own release cadence, and would make the cross-host reproduction gate impossible to state for those targets. Building Clang from one pinned release with one configuration is what makes every host's output comparable.

### Interaction with RFC 116

RFC 116 already treats native artifacts as package inputs and requires managed toolchain identities. This RFC supplies the package kind those artifacts can come from, the Linux sysroot RFC 116 requires but does not define, and one compiler for every target. It replaces RFC 116's Apple compiler selection, as stated above; everything else RFC 116 requires of Apple targets stands.

### Interaction with RFC 117

`[c]` is a third built-in source facet beside `[incan.source]` and `[rust.source]`, with a facet provider that turns a configuration, target and profile into one unit. It follows RFC 117's rules for non-overlapping roots and for what a facet may not decide (lock, registry, target, policy).

### Interaction with RFC 119

RFC 119's `link` records remain the way a crate states native work it owns. This RFC adds the record form that links a Clove unit, and requires every native compile, in either form, to use a toolchain release. RFC 119's rule that "a host without that selection refuses rather than substituting" is unchanged; this RFC makes the selection available on every Linux host and on every licensed Apple or Windows host.

### Interaction with RFC 124

A Clove unit is an RFC 124 unit, and its identity includes everything that changes its archive. A Rust unit that links it includes the library unit's identity among its dependency identities, so a change in the library, its configuration, the toolchain release or the platform files changes every unit that links it.

### Interaction with RFC 125

Cloves publish, index and serve as Loaves; their compiled units are assets. An external-source record for an upstream C library parallels RFC 125's record for a crates.io crate. The registry's catalog must show which crates bind which C-library configuration.

### Compatibility / migration

This is additive. Existing RFC 119 `link` records keep working until they are harvested again. A registry may replace an adopted crate's own native record with a library binding only by the evidence rule above, which leaves its observable build unchanged. Native records harvested with host compilers before the managed toolchain exists are not replayable on other hosts and must be harvested again with it.

## Prior art

- **Chromium** builds every platform with its own Clang: against Debian-derived sysroots it publishes for Linux, against the Xcode SDK for macOS and iOS, and through `clang-cl` against the Windows SDK. This RFC adopts the same split between one owned compiler and per-platform files.
- **Firefox** builds its own Clang toolchain for every platform it ships and pins the sysroots it builds against.
- **Python's [`manylinux` compatibility tags](https://packaging.python.org/en/latest/specifications/platform-compatibility-tags/#manylinux)** express a minimum glibc version for a particular architecture. Several baselines coexist; this RFC selects the [`manylinux_2_28` environment](https://github.com/pypa/manylinux#manylinux_2_28-almalinux-8-based) for its Linux sysroot rather than claiming it is the universal baseline of Python wheels.
- **Rust** already ships its own LLVM for code generation on every platform, so Rust code is never compiled by a host compiler; this RFC extends that property to the C a Rust crate links.
- **Zig** bundles Clang and libc headers for many targets in one download, showing that a portable cross toolchain is practical.

## Alternatives considered

### Keep recording native work per machine

Record native facts in each build environment, pin those environments, and bake there. This works and is the interim state, but a fact replays only where its exact compiler and SDK exist, so building from source succeeds only on the machines that harvested the record, and every `-sys` crate version keeps compiling its own copy of the library.

### Use the host's system libraries

Link `libzstd`, `libssl` or `libsqlite3` from the host through `pkg-config`. This is ambient discovery: the result depends on what the host has installed, which RFC 020, RFC 116 and RFC 119 all refuse.

### Prefer crates and features that avoid C

Select pure-Rust alternatives or providers that need less native code. This depends on upstream authors and does not remove C from the libraries that matter most.

### Accept prebuilt binaries from upstream

Some projects publish prebuilt static libraries. Admitting them would give no source-to-artifact provenance and no control over the C library they were built against.

### Use LLVM's official release binaries

LLVM publishes prebuilt Clang for several hosts, but different builders produce them with different configurations on each platform, so their output cannot be held to one identity across hosts. Building Clang from the same pinned release with one configuration is what the cross-host gate needs.

### Build inside the `manylinux` container

Compile Linux native units inside the `manylinux_2_28` image itself, pinned by digest. This gives the same glibc floor, but the image runs natively only on Linux hosts, ships GCC rather than Clang, and would give Linux a different compiler from every other platform. This RFC takes the image's sysroot and keeps the managed Clang.

### Use a bundled cross toolchain such as Zig

Zig's bundle would supply a compiler and libc headers in one download, but it ties the toolchain to another project's release cadence and patches and to its own glibc stub mechanism, rather than to the glibc a `manylinux` wheel is built against.

### Keep Xcode's Clang for Apple targets

This is what RFC 116 selected. It leaves Apple with a second compiler, tied to each Xcode release, whose output cannot be compared with any other host's; the managed Clang compiles against the same SDK and keeps one compiler everywhere.

## Drawbacks

- Incan takes on building, testing and publishing Clang for every supported build host, and a Linux sysroot for every supported Linux target.
- Every wrapped library needs a written C facet, and configurations that match the records of the crates that wrap it; the evidence rule makes this exact work, not guesswork.
- Apple and Windows native units depend on registry builders with licensed hosts; a consumer without the recorded SDK and without an admitted asset cannot build those units.
- Moving to a new toolchain release changes every native unit identity at once.

## Layers affected

- **Oven planning and execution** — must plan Clove units through a `c` facet provider, resolve the toolchain release and the target's platform files from the Incan toolchain, compile publishable native units only with them, plan a shared Clove unit once per closure, refuse two configurations of one library in one link closure, and link Linux executables against the plan's sysroot.
- **Build-fact record** — must accept a `link` record that names a Clove unit by Loaf, version and configuration, and must carry the toolchain release and platform-file identities in native records.
- **Interop (RFC 116)** — must accept `c.loaf_library` as a static artifact source, verify against the library's headers with the archive's toolchain and platform files, and use the managed toolchain for Apple targets.
- **`incan.pub`** — must publish and index Cloves and their units under `c/`, adopt upstream C libraries as external-source records, admit crate-to-library bindings only by the evidence rule, record supersession by an owner's package, and show bindings in its catalog.
- **Toolchain distribution** — must build and publish each toolchain release for every supported build host, run the cross-host reproduction gate before publication, publish the Linux sysroots, and name the release in each Incan toolchain.
- **Registry builders** — must hold licensed Apple and Windows SDKs, build those targets' native units with the managed toolchain, and record the platform-file identity of every unit.
- **CLI / tooling** — inspection must show a unit's native work as library bindings and crate-owned records, with the toolchain release and platform-file identity of each.

## Inspectability and tooling surface

- **Artifact or metadata:** the Clove unit's receipt (source digest, configuration, resolved definitions and sources, toolchain release, platform-file identity, target, profile, archive digest); the Rust unit's receipt lists the library units it linked; `oven.lock` records the toolchain release; the reproduction gate's results are published with each release.
- **Inspection command:** `oven inspect` shows, for a Rust or Incan unit, each linked Clove unit with its configuration, toolchain release and platform files, and each crate-owned native record.
- **Diagnostics:** an unknown configuration, a redefined inherited definition, a compile reading outside its declared roots, a missing toolchain release, platform files that differ from a unit's recorded identity, and two configurations of one library in one closure each have a dedicated refusal.
- **Provenance:** a Clove names its upstream release and checksum, or the crate version whose vendored sources it was derived from; a crate's library binding names the harvested record that admitted it.
- **Not implicit:** no host compiler, sysroot, `pkg-config`, environment variable or installed library ever enters a publishable unit.

## Design Decisions

- **Name and scope.** A C-library Loaf is a **Clove**. Registry-adopted Cloves live under the scope `c/`, with the upstream library's name and version. An upstream project that publishes its own package later is pointed to by a supersession event; published `c/` versions are never renamed or taken over.
- **glibc floor.** Linux targets use glibc 2.28, matching the selected `manylinux_2_28` baseline. Other Python wheel baselines continue to exist; this is an explicit Incan support choice, not a claim to cover every supported distribution or cloud runtime. No older glibc floor is supported by this RFC.
- **musl.** musl-based Linux targets are out of scope; a musl target would be a separate target with its own sysroot.
- **First Linux targets.** `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`.
- **Cross-host identity.** A unit's identity names the toolchain release, not a host's build of it, because every release must pass the cross-host reproduction gate; a host that fails the gate gets its own identity.
- **Feature-to-configuration mapping.** A binding is matched to a configuration by the exact sources and definitions its harvested record compiled, never by feature names; the registry derives a new configuration from the record when none matches, and host-dependent build scripts remain inadmissible.
- **Toolchain source.** Incan builds Clang and its tools from one pinned LLVM release with one configuration on every build host. The Linux sysroot is taken from the `manylinux_2_28` build image, pinned by its digest. LLVM's prebuilt releases and bundled cross toolchains are not used.
- **Apple.** Apple targets use the managed Clang against the Xcode SDK, replacing RFC 116's selection of Xcode's own Clang.
- **Windows.** The first Windows target is `x86_64-pc-windows-msvc`, compiled with the managed toolchain's `clang-cl` driver against the Windows SDK and the MSVC runtime on licensed registry builders; a unit records the SDK and runtime versions.
