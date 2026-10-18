# RFC 159: Native C library Loaves and the managed Linux C toolchain

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

This RFC makes native C libraries first-class Loaves and gives Linux one managed C toolchain. A **C-library Loaf** carries a C library's sources, its public headers, and a declared build: the exact compile units, the preprocessor definitions of each named configuration, and the static archive they produce. Oven compiles it like any other unit, under RFC 124 identity, and `incan.pub` publishes its source and its compiled units like any other Loaf. A Rust crate whose build script compiles that library (`zstd-sys`, `aws-lc-sys`, `bzip2-sys` and their kind) no longer records its own C compiles: its build fact links the library Loaf's unit, and an Incan C binding under RFC 116 links the same unit. For Linux targets, the compiler, linker, archiver and sysroot are one Incan-published, content-addressed toolchain artifact that every host uses, so a native unit's toolchain identity is the same on every machine and building from source works everywhere. Apple and Windows SDKs cannot be redistributed; for those targets, native units are built by registry builders on licensed hosts and record the SDK identity they used, and a consumer either admits those units or builds with the same recorded SDK.

## Core model

1. **A C library is a Loaf.** It has its own name, version, source publication and compiled units, separate from any Rust crate that wraps it.
2. **A C-library Loaf declares its build.** Its compile units, include roots, configurations and public headers are manifest data; no build system, probe or host discovery runs when a consumer builds it.
3. **A C-library unit is an ordinary unit.** It is identified, cached, published and admitted under RFC 124 and RFC 125 like a Rust unit.
4. **Wrappers link, they do not recompile.** A Rust crate's build fact and an Incan C binding link a C-library unit; the registry admits that binding only with evidence that it builds what the crate's own build script built.
5. **Linux has one C toolchain.** Compiler, linker, archiver and sysroot are one content-addressed artifact the Incan toolchain names, so native Linux units carry a host-independent toolchain identity.
6. **Apple and Windows record their SDK.** Their SDKs are not redistributable; units for those targets are built where the SDK is licensed and name the SDK identity they used, and nothing substitutes a different one.

## Motivation

Many Rust crates contain C. `ring` compiles C and assembly derived from BoringSSL; `aws-lc-sys` compiles AWS-LC; `zstd-sys`, `bzip2-sys`, `lzma-sys`, `secp256k1-sys` and `tikv-jemalloc-sys` compile the C library they vendor; `blake3` compiles SIMD kernels written in C; `psm` assembles stack-switching code. RFC 119 already removes build scripts from Oven's authority: what such a script would have done is a declared `link` record, compiled at publisher bake, with the exact compiler named as an owner of the work. A consumer receives the finished archive inside an admitted unit.

That model is correct and still leaves two gaps, both visible in `incan.pub`'s bake of the crates.io top-1000.

**A native record only replays where its toolchain exists.** A `link` record names its compiler and SDK by an owner identity: a digest over every file the compiler installation and SDK contain. A record harvested on one developer machine names that machine's Command Line Tools and its `MacOSX27.0.sdk`; a hosted macOS builder carries a different Xcode, so the owner is absent and Oven correctly refuses to replay the record. On Linux the situation is worse: system headers live under `/usr/include`, so the only owner that covers a compile is the host's root filesystem, which no record can inventory. The result is that every crate whose closure reaches native C (over forty of the nine hundred top-1000 shards on each platform, including the whole rustls-based TLS stack, every AWS SDK client, `zip`, `rusqlite` and `stacker`) cannot be baked from source anywhere except on the machine that harvested it.

**The same C library is compiled many times, under many names.** `zstd-sys 2.0.16+zstd.1.5.7` and `zstd-sys 2.1.0+zstd.1.5.7` vendor the same zstd release and each records its own copy of the same compiles. Every `-sys` crate version that vendors a library repeats its build, and a project that reaches the library through two crate versions links two copies. A C library has an identity of its own, separate from the Rust crates that wrap it, and nothing in the package graph says so today.

Incan's own C interop has the same need from the other side. RFC 116 resolves static archives, headers and shims "through the package graph, locked by identity" and requires Linux targets to "resolve an explicit compiler and sysroot contract", but no Loaf kind carries a C library and no artifact is that Linux contract. This RFC supplies both.

Choosing features or crates that need less C is not a remedy: it depends on upstream authors changing their crates, and the libraries that matter most (cryptography, compression, databases) are C by design.

## Goals

- Define a C facet: a Loaf that carries a C library's sources, public headers, named build configurations and the static archive each produces.
- Make a C-library unit an ordinary RFC 124 unit, published and admitted under RFC 125.
- Let a Rust crate's build fact link a C-library unit instead of recording its own compiles, with an admission rule that proves the two build the same thing.
- Let an RFC 116 C binding link the same unit.
- Publish one managed C toolchain for Linux targets, content-addressed, so a native unit's toolchain identity is host-independent.
- Use the same Linux sysroot for C compiles and for the final Rust link.
- Keep Apple and Windows correct without redistributing their SDKs.

## Non-Goals

- This RFC does not run a library's own build system (CMake, autotools, Meson, Make) at consumer build time, and it does not make any such system Oven authority. A C-library Loaf declares its compile units; deriving them from an upstream build system is an adoption-time activity.
- This RFC does not define C++ ABI interop. A C-library Loaf may compile C++ sources into an archive that exposes a C interface, under the same rules as C sources.
- This RFC does not redistribute the Apple SDK, the Windows SDK or MSVC, and it does not define a managed toolchain for those targets.
- This RFC does not replace RFC 116's `system` deployment class. A library a target platform supplies (a system framework, `libc` itself) stays a declared system capability.
- This RFC does not define dynamic linking of C-library Loaves. Every C-library unit is a static archive; a bundled shared library remains RFC 116's `bundled` class.

## Guide-level explanation

### A C-library Loaf

A C-library Loaf is a Loaf whose facet is `c`. Its manifest names the library, the headers it exposes and how to compile it:

```toml
[project]
name = "native/zstd"
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

`oven bake` compiles one configuration for one target with the selected C toolchain and produces a unit: the archive `libzstd.a`, the public headers, and a receipt naming the source digests, the configuration, the definitions, the toolchain identity, the target and the profile. Nothing is discovered on the host: no `pkg-config`, no search for an installed `zstd`, no environment variable that changes the build.

### A Rust crate that wraps it

Today `zstd-sys` records a build fact whose `link` record lists every compile its build script ran. Under this RFC, its adoption instead binds each feature selection to a configuration of `native/zstd`:

```toml
[[rust.facts]]
target = "x86_64-unknown-linux-gnu"
profile = "release"
features = ["legacy", "std", "zdict_builder"]

[[rust.facts.link]]
library = { loaf = "native/zstd", version = "1.5.7", configuration = "legacy-dictbuilder" }
```

When Oven plans `zstd-sys`, it plans the `native/zstd` unit for that configuration and links its archive. If the unit is available as an asset, Oven downloads it; otherwise it compiles it from the library Loaf's source with the managed toolchain. Two crate versions that bind the same library configuration share one unit.

A crate whose build script also compiles crate-private C (a shim, a feature probe that ships code) keeps that part as an ordinary RFC 119 `link` record, compiled with the same managed toolchain.

### An Incan binding that links it

An RFC 116 binding names the library Loaf instead of a system library:

```incan
from std.interop import c

binding Zstd:
    header = "zstd.h"
    link = c.loaf_library("native/zstd", configuration="default")
```

The binding is verified against the library Loaf's public headers with the same toolchain that compiled the archive, and links the same unit a Rust crate in the same plan would.

### The managed Linux C toolchain

For Linux targets, the C toolchain is not whatever compiler a machine has installed. It is a published toolchain component, the Incan C toolchain, that bundles a Clang compiler, its resource directory, an LLVM linker and archiver, and a sysroot: glibc headers and startup objects at a fixed version, and the Linux kernel headers. Oven fetches it like any other toolchain component and verifies it by digest. Because every Linux build of a native unit uses the same artifact, the unit's toolchain identity is the artifact's identity, the same on a developer machine, a CI runner and a registry builder, and a fact recorded anywhere replays everywhere.

The same sysroot is what Oven links final Linux executables against, so a C archive and the Rust code that links it agree on the C library they were built for.

### Apple and Windows targets

The Apple SDK and the Windows SDK cannot be redistributed. For those targets, C-library units and native records are built by registry builders (RFC 125's `registry` builder kind) running on licensed hosts. Each unit records the SDK identity it was built with, exactly as RFC 116 already requires for Apple targets. A consumer admits those units by RFC 124 identity like any other asset; a consumer that builds from source must select the same SDK identity, and a host with a different SDK produces a different unit rather than substituting one.

## Reference-level explanation

### The C facet

1. A Loaf with a `[c]` table carries a C facet. A C facet must name exactly one `library` (the archive's link name), its `public-headers`, and at least one `[[c.compile]]` entry. A Loaf may carry a C facet alongside other facets under RFC 117's rules for non-overlapping source roots.
2. Each `[[c.compile]]` entry names source files or globs relative to the Loaf root, an optional `target` predicate in RFC 119's predicate grammar, and optional per-entry `include` and `defines`. Globs are expanded once, at publication, into the exact file list the source Loaf records; a consumer never expands a glob.
3. Source files are C (`.c`), assembly (`.S`, `.s`) or C++ (`.cc`, `.cpp`, `.cxx`) compiled into an archive with a C interface. Any other file kind in a compile entry is refused.
4. A **configuration** is a named set of preprocessor definitions and additional sources. `default` must exist. A configuration may `extend` exactly one other configuration; the result is that configuration's definitions and sources plus its own, and redefining an inherited definition is refused rather than overridden.
5. A C facet must not declare compiler flags beyond definitions, include roots, the language standard (`std = "c11"` and its kind) and target predicates. Optimization level and debug information follow the plan's profile, and code-generation options follow the selected toolchain and target; a facet cannot change them.
6. A C facet must not name an absolute path, an environment variable, a host tool, or a library outside the Loaf. A compile that reads a file outside the Loaf root, the facet's declared include roots, or the selected toolchain's owner is refused.

### C-library units

1. A C-library unit is the RFC 124 unit produced by compiling one configuration of one C facet for one target and profile. Its identity must include the source Loaf digest, the configuration name and its resolved definitions and source list, the target, the profile, and the C toolchain identity.
2. The unit's payload is the static archive and the public headers. The archive's object order, member names and timestamps must be deterministic, so that two builds with the same identity produce the same archive bytes.
3. A C-library unit is published and admitted under RFC 125 exactly as a Rust unit is: its asset carries its archive digest and unit identity, and a consumer admits it only on identity equality.
4. A plan that reaches the same C-library unit through several dependents must plan it once. Two configurations of the same library in one link closure are refused unless an RFC 119 isolation boundary separates them, because their symbols collide.

### Binding a Rust crate to a C-library unit

1. A Rust build fact's `link` record may, instead of listing compiles, name a C-library unit: `library = { loaf, version, configuration }`. The fact vocabulary gains this form; it is a change to the compiler's build-fact record and must land there before a registry publishes it.
2. A registry may admit such a binding for an adopted crate only with evidence that the crate's own build and the library configuration build the same thing. The evidence is mechanical: the crate's harvested native record for that binding and the library configuration must name the same source files by content digest, the same preprocessor definitions and the same include structure, differing only in the toolchain that compiled them. A binding without that evidence is refused, and the crate keeps its own `link` record.
3. Compiles a crate's build script performs beyond the library (a shim, generated glue) remain ordinary RFC 119 `link` records. A fact may hold both forms.
4. A fact that binds a C-library unit inherits the unit's toolchain requirement: on a Linux target it needs the managed toolchain identity the library unit records; on Apple and Windows targets it needs the recorded SDK identity or an admitted asset.

### Linking a C-library unit from Incan

RFC 116's binding `link` value gains `c.loaf_library(loaf, configuration=...)`, a static artifact resolved through the package graph. Verification under RFC 116 must use the library Loaf's public headers, the configuration's definitions and the same C toolchain identity as the archive the plan links.

### The managed Linux C toolchain

1. For every Linux target Oven supports, Incan publishes a C toolchain artifact: a Clang-compatible compiler, its resource directory, a linker, an archiver, and a sysroot containing the C library headers, startup objects and kernel headers for that target. Its identity is the digest of its content.
2. A native unit for a Linux target (a C-library unit, or an RFC 119 `link` record) must be compiled with a managed toolchain artifact, and its identity names that artifact. A host-installed compiler or sysroot is not selectable for a publishable Linux unit; it may be used for local development only through an explicit, non-portable override that RFC 116 already describes.
3. The toolchain artifact is selected by the Incan toolchain, not by a project: a toolchain release names exactly one C toolchain artifact per Linux target, and `oven.lock` records it. Moving to a new C toolchain is a toolchain change, and it changes every native Linux unit identity.
4. Oven's final link for a Linux target must use the same sysroot as the C toolchain artifact the plan selected.
5. The C toolchain artifact is published per build host. Whether units compiled by the artifact for different build hosts are identical, so that one unit identity covers them, is an open question below; until it is settled, the build host's artifact identity is part of the unit identity.

### Apple and Windows targets

1. A native unit for an Apple or Windows target records the compiler and SDK identity it was built with: for Apple targets, the Xcode-provided Clang and SDK identity as RFC 116 defines it; for Windows targets, the corresponding compiler and SDK identity.
2. `incan.pub` builds such units on registry builders that hold the licensed SDK and publishes them with the `registry` builder kind. A consumer admits them by unit identity.
3. A build host whose SDK identity differs from a unit's recorded identity must produce a different unit, never substitute one. A registry may hold units for several SDK identities of the same configuration; RFC 119 already selects among a binding's records by the owners a host has.

### Registry adoption of C libraries

1. `incan.pub` may adopt a C library from an upstream release (a published source archive and its checksum) as an external-source record under RFC 125, with the C facet written by the registry. The record binds the upstream location, version and checksum to the source Loaf digest.
2. `incan.pub` may also derive a C-library Loaf from the vendored sources of an adopted crates.io crate, when those sources are the library's and not crate-specific glue. The record names the crate version it was derived from.
3. Adopted C-library Loaves live under a registry-owned scope; their names and versions are the upstream library's, not the wrapping crate's.

## Design details

### Why the facet and the toolchain are one proposal

A C-library unit's identity names the toolchain that compiled it, and the point of a C-library Loaf is that any host can build or admit the same unit. Without a host-independent Linux toolchain, a C-library Loaf would still compile to a different unit on every Linux machine, and the facet would solve only the duplication half of the problem. Without the facet, a managed toolchain would make each `-sys` crate's own native record replayable but leave every crate version compiling its own copy of the library. Each part needs the other to deliver either result.

### Interaction with RFC 116

RFC 116 already treats native artifacts as package inputs and requires managed toolchain identities. This RFC supplies the package kind those artifacts can come from and the Linux toolchain RFC 116 requires but does not define. A binding that links a C-library Loaf verifies against the same headers and toolchain identity that produced the archive.

### Interaction with RFC 117

`[c]` is a third built-in source facet beside `[incan.source]` and `[rust.source]`, with a facet provider that turns a configuration, target and profile into one unit. It follows RFC 117's rules for non-overlapping roots and for what a facet may not decide (lock, registry, target, policy).

### Interaction with RFC 119

RFC 119's `link` records remain the way a crate states native work it owns. This RFC adds the record form that links a C-library unit, and requires every native Linux compile, in either form, to use the managed toolchain. RFC 119's rule that "a host without that selection refuses rather than substituting" is unchanged; this RFC makes the selection available on every Linux host.

### Interaction with RFC 124

A C-library unit is an RFC 124 unit, and its identity includes everything that changes its archive. A Rust unit that links it includes the library unit's identity among its dependency identities, so a change in the library, its configuration or the C toolchain changes every unit that links it.

### Interaction with RFC 125

C-library Loaves publish, index and serve as Loaves; their compiled units are assets. An external-source record for an upstream C library parallels RFC 125's record for a crates.io crate. The registry's catalog must show which crates bind which C-library configuration.

### Compatibility / migration

This is additive. Existing RFC 119 `link` records keep working. A registry may replace an adopted crate's own native record with a library binding only by the evidence rule above, which leaves its observable build unchanged. Native Linux records harvested with host compilers before the managed toolchain exists are not replayable on other hosts and must be harvested again with it.

## Alternatives considered

### Keep recording native work per machine

Record native facts in each build environment, pin those environments, and bake there. This works and is the interim state, but a fact replays only where its exact compiler and SDK exist, so building from source succeeds only on the machines that harvested the record, and every `-sys` crate version keeps compiling its own copy of the library.

### Use the host's system libraries

Link `libzstd`, `libssl` or `libsqlite3` from the host through `pkg-config`. This is ambient discovery: the result depends on what the host has installed, which RFC 020, RFC 116 and RFC 119 all refuse.

### Prefer crates and features that avoid C

Select pure-Rust alternatives or providers that need less native code. This depends on upstream authors and does not remove C from the libraries that matter most.

### Accept prebuilt binaries from upstream

Some projects publish prebuilt static libraries. Admitting them would give no source-to-artifact provenance and no control over the C library they were built against.

### Distribute an existing cross toolchain unchanged

Projects such as Zig bundle a C compiler and libc headers for many targets. Such a toolchain is a strong candidate for the content of the managed Linux artifact, but Incan must still publish it as its own content-addressed artifact with its own identity, so that a change upstream is a deliberate toolchain change rather than a silent one.

## Drawbacks

- Incan takes on building, publishing and updating a C toolchain artifact for every supported Linux target.
- Every wrapped library needs a written C facet and, for each crate that wraps it, configurations that match the crate's features; the evidence rule makes this exact work, not guesswork.
- Apple and Windows native units depend on registry builders with licensed hosts; a consumer without the recorded SDK and without an admitted asset cannot build those units.
- Moving the managed toolchain changes every native Linux unit identity at once.

## Layers affected

- **Oven planning and execution** — must plan C-library units through a `c` facet provider, resolve the managed Linux C toolchain from the Incan toolchain, compile only with it for publishable Linux units, plan a shared C-library unit once per closure, refuse two configurations of one library in one link closure, and link Linux executables against the same sysroot.
- **Build-fact record** — must accept a `link` record that names a C-library unit by Loaf, version and configuration, and must carry the C toolchain identity in native records.
- **Interop (RFC 116)** — must accept `c.loaf_library` as a static artifact source and verify against the library's headers with the archive's toolchain.
- **`incan.pub`** — must publish and index C-library Loaves and their units, adopt upstream C libraries as external-source records, admit crate-to-library bindings only with the evidence rule, and show bindings in its catalog.
- **Toolchain distribution** — must publish one C toolchain artifact per Linux target and build host, content-addressed, and name it in each Incan toolchain release.
- **CLI / tooling** — inspection must show a crate's native work as library bindings and crate-owned records, with the toolchain identity of each.

## Inspectability and tooling surface

- **Artifact or metadata:** the C-library unit's receipt (source digest, configuration, resolved definitions and sources, toolchain identity, target, profile, archive digest); the Rust unit's receipt lists the library units it linked; `oven.lock` records the managed C toolchain artifact.
- **Inspection command:** `oven inspect` shows, for a Rust or Incan unit, each linked C-library unit with its configuration and toolchain identity, and each crate-owned native record.
- **Diagnostics:** an unknown configuration, a redefined inherited definition, a compile reading outside its declared roots, a missing managed toolchain, a host SDK that differs from a unit's recorded SDK, and two configurations of one library in one closure each have a dedicated refusal.
- **Provenance:** a C-library Loaf names its upstream release and checksum, or the crate version whose vendored sources it was derived from; a crate's library binding names the evidence that admitted it.
- **Not implicit:** no host compiler, sysroot, `pkg-config`, environment variable or installed library ever enters a publishable unit.

## Unresolved questions

- What scope do registry-adopted C-library Loaves use (for example `native/`), and how does a library's upstream project claim it later?
- Which glibc version does the Linux sysroot target as its floor, and is `musl` a second Linux sysroot or a separate target?
- Are units compiled by the same toolchain release on different build hosts byte-identical, so that the unit identity names the release rather than the host-specific artifact?
- How is a crate's feature selection mapped to a configuration when the crate's build script chooses sources by probing the target rather than by features?
- Which artifact does the managed Linux toolchain start from: an LLVM release with a separately built sysroot, an existing bundled cross toolchain, or one built by Incan from source?
- Which Windows compiler and SDK identity does a Windows native unit record, and which Windows targets does the first release cover?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
