# RFC 119: Oven-native Rust build facets and Cargo interoperation

- **Status:** Planned
- **Created:** 2026-08-04
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies)
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 034 (`incan.pub` package registry)
    - RFC 041 (first-class Rust interop authoring)
    - RFC 043 (Rust trait implementation from Incan)
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 114 (compiled providers, SDK components, and package features)
    - RFC 117 (`loaf.toml` and Oven's language-neutral project model)
    - RFC 118 (Incan and Oven command-line surfaces)
    - #975 (Oven: Cargo-free Incan/Rust toolchain)
    - #990 (governed `std.process` for Oven execution)
    - #991 (Oven host-kernel and Incan-module bootstrap contract)
- **Issue:** [#1012](https://github.com/encero-systems/incan/issues/1012)
- **RFC PR:** —
- **Written against:** v0.5
- **Target scope:** v0.6 design and implementation planning; this is not a shipment claim.
- **Shipped in:** —

## Summary

RFC 117 establishes a language-neutral Loaf project model, but it deliberately does not specify enough Rust build behavior to replace Cargo as the normal authority for an explicitly supported Rust project. This RFC defines that next layer: Oven-native Rust build facets, an Oven-owned crate graph and direct-`rustc` plan, controlled build-time providers, Rust test and IDE projections, and explicit Cargo interoperation.

Oven consumes crates.io and registered Cargo-compatible sources through the typed `crate` provider contract. It does not publish Loaves or `*.loaf` assets to crates.io. `loaf.toml` remains the sole authored project manifest, `oven.lock` remains the resolved graph, and a bake emits immutable target-bound `*.loaf` assets plus receipts. Cargo remains runnable only through explicit Cargo-compatibility mode; an existing Cargo project must explicitly opt into Loaf adoption and is never silently merged into an Oven project.

## Core model

1. **A Rust facet is a built-in Loaf facet:** a conventional Rust-only Loaf uses the ordinary `src/` layout. A mixed or nonstandard Rust root declares `[rust.source]`; compact Rust-specific exceptions live beneath `rust.*`. In every case Oven, not a neighbouring `Cargo.toml`, supplies the crate facts needed to plan and invoke `rustc`.
2. **Oven owns the graph:** dependency selection, feature resolution, target/host partitioning, toolchain choice, lock identity, execution plan, cache reuse, artifacts, and receipts belong to Oven.
3. **Crates.io is consumption-only:** typed `crate` dependencies default to crates.io and may use registered compatible sources. Publishing remains in Incan's registry ecosystem.
4. **Cargo manifests are provider metadata only when explicitly selected:** a registry crate's `Cargo.toml` can be parsed by the selected crate provider as constrained source metadata. It cannot define a Loaf workspace, lock, registry trust, lifecycle action, project target policy, or publication identity.
5. **Build-time code has explicit host semantics:** a discovered `build.rs` is a generic host-provider candidate and never self-authorizes execution. A selected procedural-macro crate is compiled for the build host and executed by the selected `rustc` through its normal proc-macro ABI, as in Cargo; Oven does not replace that expansion mechanism. Both paths have explicit toolchain, host/target, policy, and receipt facts.
6. **Host and target are different build domains:** build scripts, procedural macros, and compiler plugins execute for the build host; libraries, binaries, test subjects, and carriers are compiled for the selected target. Oven plans and locks both domains separately.
7. **Rust test work is first-class:** unit tests, integration tests, doctests, examples, and benchmarks are explicit build/run roles, not side effects of a generic `rustc` invocation. They use RFC 117's named environments: `test` by default for test roles and `docs` by default for documentation roles.
8. **IDE projection is derived and warm-reusing:** Oven materializes a Rust-analyzer-compatible project projection from one explicit inspection selection. It first reuses every receipt-compatible pre-warmed `*.loaf` asset and provider/artifact closure, including across equivalent clean worktrees or implementation slices; the lightweight local projection then describes that selected graph. Switching an editor selection never implies rebuilding every target, dependency, or provider, and the projection does not create or require a synthetic authoritative `Cargo.toml`.
9. **Cargo is an explicit interoperation boundary:** `oven cargo ...` invokes Cargo as Cargo-authoritative compatibility mode. Explicit adoption may import selected facts into a new `loaf.toml`, but no directory with both manifests receives mixed semantics.
10. **The Rust core remains narrow but real:** as decided by RFC 118, Oven's operational core/API owns planning, resolution, policy, stores, leases, provider execution, and crash-safe publication in Rust. Its public API is language-neutral and does not leak Rust borrows.
11. **Incan-first authoring remains policy:** a Rust facet makes Rust projects and explicitly justified mixed work supportable. It does not authorize new Rust in Incan products without a demonstrated limitation and tracked removal path.
12. **Native compatibility is proved, not implied:** the supported Rust envelope is defined by an executable conformance corpus. Unsupported Cargo behavior produces a precise diagnostic or requires explicit Cargo mode.

## Motivation

RFC 117 makes an Oven-native Rust project architecturally possible: it has an authored manifest, a typed dependency graph, provider trust, target policy, lock identity, receipts, and an explicit boundary around Cargo. That is necessary but not sufficient. A Rust source file alone does not identify a complete compilation unit, determine which dependencies compile for the build host versus the final target, express crate features, control native linking, or explain test and documentation artifacts.

Cargo is valuable because it solves those practical concerns for a large Rust ecosystem. But it is not a sufficient authority for a project that includes Incan sources, checked interop, target carriers, governed actions, receipts, persistent stores, and Incan package publication. Oven should not hide Cargo under a different command name; it must own its build graph and make any Cargo interoperation explicit.

The right objective is therefore neither “emulate every Cargo behavior” nor “rewrite Rust as Incan.” It is a bounded native-Rust contract that builds serious selected Rust Loaves, consumes the ordinary crate ecosystem, produces target-bound `*.loaf` assets, and makes its limits inspectable. Cargo remains available for compatibility while that envelope expands through evidence.

## Goals

- Specify the full Rust facet needed to bake an Oven-native Rust Loaf without using project Cargo metadata as authority.
- Resolve `crate` dependencies from crates.io or registered compatible sources into the same `oven.lock` and receipt model as Loaf and capability dependencies.
- Specify deterministic Rust feature resolution, dependency roles, source checksums, target conditions, toolchain selection, and host-versus-target build units.
- Support an explicitly bounded and inspectable path for Rust build scripts, procedural macros, native linking, generated Rust inputs, and linker/sysroot selection.
- Specify first-class roles for Rust libraries, binaries, integration tests, unit tests, examples, doctests, benchmarks, and caller projections.
- Produce `*.loaf` assets and receipts whose identities cover the complete Rust compilation and provider closure.
- Define a Rust-analyzer-compatible derived project projection without restoring Cargo manifest authority.
- Preserve explicit `oven cargo ...` behavior for Cargo projects and define a deliberate adoption boundary for projects choosing to become Loaves.
- Establish a conformance corpus that identifies the supported native Rust envelope and guards Cargo-free regressions.

## Non-Goals

- Publishing Oven-native Loaves, `*.loaf` assets, or normal Oven packages to crates.io.
- Making `Cargo.toml`, `Cargo.lock`, Cargo workspace discovery, Cargo build profiles, or arbitrary Cargo command behavior part of ordinary Loaf semantics.
- Supporting every Cargo crate on day one, or claiming compatibility from source discovery alone.
- Implicitly importing, merging, or executing a neighbouring Cargo project after `loaf.toml` exists.
- Defining the `*.loaf` archive/wire format or Incan registry transport protocol.
- Redefining RFC 097's Rust-host caller ABI, RFC 116's C ABI safety contract, RFC 117's project authority, or RFC 118's command ownership.
- Moving Oven's operational core/API into Incan. RFC 118's justified Rust exception remains in force.

## Guide-level explanation

### An Oven-native Rust Loaf

An explicitly Rust-authored project adopts Oven by authoring `loaf.toml`, not by placing Cargo configuration beside Rust sources and hoping it is inferred:

```toml
[project]
name = "telemetry-engine"
version = "0.8.0"

[dependencies]
serde = { crate = "serde", version = "1", features = ["derive"] }
tokio = { crate = "tokio", version = "1", features = ["rt-multi-thread", "macros"] }
```

With `src/lib.rs`, `src/main.rs`, or `src/bin/telemetry.rs` in the conventional locations, this project needs no Rust table. The essential contract is that Oven can identify every compiled crate, its root, edition, crate types, entry point, enabled features, and dependency/linkage closure without reading a project `Cargo.toml`. The Rust facet remains convention-first: `src/lib.rs`, `src/bin/`, `tests/`, `examples/`, `benches/`, and documentation tests require no restatement in the manifest. Authors declare only meaningful deviations, such as an additional crate, a nonstandard root or crate type, a feature-gated target, a locally authored proc macro, or build-provider intent. The expanded host/target unit graph is derived inspection state in `oven plan` and `oven.lock`, not normal authoring burden.

A project that mixes Incan and Rust names both roots explicitly:

```toml
[incan.source]
root = "sources/incan"

[rust.source]
root = "sources/rust"
```

The roots must not overlap. The exact compact shape for a genuine Rust exception belongs beneath `rust.*`; for example, a future settled grammar may name a non-conventional proc-macro crate as `[rust.crates.incan_derive]`. This RFC deliberately does not use an anonymous `[[sources]]` list or require an author to reproduce Cargo's whole manifest surface.

The `crate` dependencies are ordinary Rust ecosystem inputs. They default to crates.io, but Oven records their canonical source, checksum, signatures or trust facts where available, selected feature closure, and host/target use in `oven.lock`. The project's publication identity remains an Incan registry identity; `oven publish` does not mean `cargo publish`.

### Build-time work is visible before it runs

Suppose `native-sys` requires code generation and native linking. Its `build.rs` does not execute simply because the file exists. The selected crate metadata reports a provider candidate; Oven's generic build-script provider exposes a plan like:

```text
host provider: native-sys build script via rust.build-script
  inputs: native-sys permitted source tree, selected features, build host/target, selected toolchain, allowed environment keys
  outputs: generated bindings, link search paths, link libraries, cfg facts
  effects: execute host helper, read declared inputs, write provider staging area

target Rust crate: native-sys
  target: aarch64-linux-android
  consumes: locked generated bindings and normalized link facts
```

The provider's host executable, argument vector, allowed environment, observed directives, generated-output digests, and policy result enter the receipt. Unsupported directives fail with a provider diagnostic; observe mode records an admitted provider's actual effects, while governed mode denies an ungranted effect class. Oven does not silently retry through Cargo.

Procedural macros follow the same host/target split, but not a new Oven expansion protocol. Oven compiles the macro for the build host, then invokes the selected `rustc` with the normal proc-macro artifact so that `rustc` loads and expands it through its native ABI, as it would in a Cargo build. The target crate never treats the host macro binary as a target artifact. Oven records the macro artifact and host toolchain as facts of the consuming compile unit; it does not cache individual macro expansions or route a bake through a separate macro server.

### Rust testing, documentation, and IDEs

Rust work has explicit roles:

```text
oven test --rust unit
oven test --rust integration
oven test --rust doc
oven bake --example telemetry-smoke
oven bench --rust parser
```

The exact command spelling remains RFC 118 territory. The roles do not: an integration test is a separate target-linked test crate; a doctest is a compiled test input with a source origin; an example and a benchmark are named build units. Each can be selected, planned, cached, and receipted independently.

For editor support, Oven may materialize a derived `rust-project.json`-style projection from the selected Rust graph. That projection identifies the same roots, crate dependencies, cfg facts, features, build-data outputs, and target configuration used by the plan. It is generated state, not a second authored manifest.

### Cargo remains available, but never ambiguous

There are three intentionally distinct routes:

```text
oven cargo test
  Cargo compatibility mode: Cargo.toml is authoritative.

oven adopt cargo --manifest ./Cargo.toml
  Explicit adoption: inspect/import selected source facts, write or propose loaf.toml,
  then make future Oven operations Loaf-authoritative.

oven bake
  Oven-native mode: loaf.toml and oven.lock are authoritative; Cargo files are ignored.
```

`oven adopt cargo` is illustrative command spelling. The important rule is explicit re-authoring: a user chooses to cross the boundary, sees the proposed Loaf contract and unsupported Cargo behavior, and accepts the resulting project state. Oven does not treat every checked-out `Cargo.toml` as a latent Loaf.

### Publishing and consuming

Oven-native packages publish to Incan's registry ecosystem. `incan.pub` is the public endpoint for the foreseeable future; registry identity and trust remain explicit Oven configuration rather than a hard-coded domain in `loaf.toml`.

Oven can consume a crate from crates.io because Rust libraries are part of the world it interoperates with. That is not reciprocal authority: an Oven-native package is not automatically a crates.io package. If a future product needs a Cargo-consumable publication projection, it must be a separately specified bridge with explicit provenance to a selected `*.loaf` asset.

## Reference-level explanation

### Rust facet identity and discovery

1. A Loaf that selects a Rust facet must resolve every compiled Rust unit before execution. A unit has a crate name, package identity, root, edition, role, crate type or target kind, enabled feature set, target condition, dependency/linkage closure, and source digest.
2. A conventional single library root may default to `src/lib.rs`; a conventional single binary may default to `src/main.rs`. The derived crate name follows the project name only when that mapping is unambiguous. All other roots and identities must be explicitly declared.
3. Libraries, binaries, integration tests, examples, doctests, benchmarks, build scripts, procedural macros, generated sources, and caller projections have distinct roles. A role determines its compilation mode, host/target domain, artifact kind, test scheduling, and receipt contribution.
4. The Rust facet may use `cfg`-style target conditions only as explicit, normalized predicates over the selected target and build host. Ambient host probing must not silently add a source or feature.
5. Project public features remain RFC 114 Loaf capabilities. A Rust-provider feature is requested through the typed `crate` dependency and affects a Rust unit only through an explicit mapping; it never becomes a public Loaf feature by name coincidence.

### Crate providers and feature resolution

1. A `crate` dependency defaults to crates.io. A registered compatible source may override that origin only through RFC 117 registry configuration and workspace trust policy.
2. A selected crate provider may read the crate package's `Cargo.toml` as constrained provider metadata after the typed `crate` dependency and source have already been selected. It may use only the metadata required to understand that dependency package's Rust units, dependency requirements, feature declarations, target conditions, build-time code, and native-link facts.
3. A provider Cargo manifest must not establish workspace membership outside the package, alter a Loaf registry or trust policy, introduce a second lock, change a Loaf target/delivery policy, run an undeclared lifecycle operation, or publish a Loaf.
4. Oven resolves a deterministic feature closure over selected crate provider requirements. The lock records requested and selected features for every host and target unit. Development-only and test-only requirements do not enter a release artifact closure unless a declared role requires them.
5. `oven.lock` records the provider's canonical source, immutable package identity, checksum or content digest, selected Rust metadata subset, feature closure, target predicates, transitive provider graph, and observed trust facts. It does not copy a `Cargo.lock` as Oven authority.
6. An unsupported provider manifest construct must identify the dependency, field, source span when available, and the available choices: a supported provider, an explicit adaptation declaration, or Cargo compatibility mode.

### Direct Rust compilation and artifact identity

1. Oven invokes the selected Rust compiler through its controlled Rust operational core/API; Cargo is not a subprocess in an Oven-native bake.
2. Each compile unit is keyed by the compiler/toolchain identity, normalized command inputs, source digests, dependency artifact identities, selected features, cfg facts, build host, target, profile, generated inputs, native-link facts, and provider receipts.
3. Host units and target units are distinct graph nodes. A build script, procedural macro, or compiler plugin is always a host node even when it was selected by a target crate.
4. A profile is an Oven target policy that maps to normalized compiler/codegen/link options. Cargo profile inheritance is not read; any Cargo-compatible translation is only part of explicit adoption or Cargo mode.
5. A completed bake emits one or more target-bound `*.loaf` assets according to the selected Rust roles and carriers. Every asset identifies the selected Loaf/facet, target, carrier, profile, graph digest, payload digest, relevant provider receipts, and final execution receipt.

### Build scripts, procedural macros, and generated inputs

1. A `build.rs` discovered from selected crate metadata is a provider candidate, not permission to execute. Oven supplies the generic `rust.build-script` host-provider role; Cargo metadata may report the role and its source facts, but cannot select or authorize it.
2. Oven runs an admitted `rust.build-script` provider in isolated staging. Its conservative base input identity contains the permitted package input tree, selected features, build host, target, provider and toolchain identities, and approved environment values or redacted identities. Its receipt captures arguments, outputs, normalized directives, generated-input digests, filesystem/network/process effects, and exit status.
3. The initial directive vocabulary is explicit: rerun source and environment inputs, cfg and check-cfg facts, controlled environment outputs, generated-input locations and digests, warnings and diagnostics, and native link search paths, libraries, and arguments. Unknown directives are errors, not opaque passthroughs.
4. A procedural-macro crate is a host Rust unit. Oven locks its host crate closure, compiler identity, macro artifact digest, and diagnostics identity. The selected `rustc` loads and invokes that artifact through its normal proc-macro ABI; Oven must not insert a custom expansion bridge or standalone macro server into a bake. The target crate never treats the host macro binary as a target artifact.
5. Observe mode preserves Cargo-equivalent host execution for a selected procedural macro while recording macro artifact, compiler invocation, observable inputs/effects, and diagnostics at the consuming compile-unit boundary. Oven does not claim an independently controlled receipt for every macro call. A backend that cannot observe an input needed for cache correctness must mark the result uncacheable rather than reuse it falsely.
6. In governed mode, any containment applies around the compiler process that hosts macro execution. It must be explicit and receipt-visible; if the host cannot enforce the selected policy without changing behavior, Oven denies the governed operation rather than silently weakening policy. Permissive mode remains an explicit escape hatch. There is no implicit Cargo fallback.
7. Native compilation/linking, system probing, and named external tools remain separate effect classes for a build-script provider, such as `native.cc`, `native.pkg-config`, `tool.cmake`, `tool.protoc`, and `tool.git`. A package can report a needed effect class but cannot grant it.
8. Rerun directives refine inspection and later invalidation but are not assumed to capture every dynamic read on a first execution; the conservative base input identity remains part of cache correctness.
9. A generated source participates only through an identified provider output. A change to its producing receipt or digest invalidates the dependent unit; a cache hit must identify the provider receipt it reused.

### Native linkage, carriers, and cross compilation

1. A Rust unit's native library, framework, system SDK, C ABI shim, linker, and sysroot requirements are typed capability/provider facts under RFCs 116 and 117. They are never inferred from an unrecorded local linker search path.
2. Cross compilation selects a build host and target separately. The plan must report both, the toolchain/sysroot/linker identities, selected capability providers, and any provider node that executes for the host.
3. Target artifacts are assigned to an explicit carrier. A Rust library, executable, `cdylib`, Rust-host caller projection, JNI library, Python carrier, or C ABI carrier can be selected only when the target and provider contracts permit it.
4. The receipt distinguishes compile, link, package, test, and deployment facts so a successful host test cannot be mistaken for a target bake or device proof.

### Rust test, documentation, and IDE projections

1. Unit tests compile with their owning crate. Integration tests, examples, doctests, and benchmarks are distinct named units with declared source origins and dependency roles.
2. Test execution uses Oven's scheduler, selected environment, capability policy, target selection, output retention, and receipt model. Test roles select the standard `test` environment unless an explicit compatible environment is selected; documentation roles select `docs` by default. Environment-only dependencies do not enter an ordinary release bake unless the selected role requires them. Test execution must not silently dispatch to `cargo test` in Loaf mode.
3. A doctest extraction or documentation generator must record its source spans, generated test inputs, compiler/provider facts, and diagnostics mapping. Generated Rust remains inspectable but is not the public source of truth.
4. A Rust-analyzer-compatible projection is derived from one explicit Oven inspection selection and may be invalidated with the same receipt-bound facts. Its standard project descriptor contains only the selected crate roots, editions, exact dependency aliases, first-party membership, resolved cfg/target facts, selected sysroot, and non-sensitive compile environment or identified generated-output paths needed for analysis. An Oven sidecar identifies the selection's plan and receipt, freshness state, policy state, and diagnostics mapping; that sidecar, not the descriptor, retains Oven provenance.
5. The active inspection selection defaults to the `dev` environment and the Loaf-selected target, or the host-native target if no project target is selected. Target switching is explicit and visible; Oven must not prepare every target merely because an editor is open.
6. Before materializing a selection, Oven must look up the receipt-compatible pre-warmed `*.loaf` asset and provider/artifact closure. It must reuse that closure across equivalent clean worktrees and implementation slices, then materialize only the small workspace-local projection needed for the current source paths. A miss invalidates and rebuilds only the affected selection and units.
7. An Oven-owned inspection session may enable editor-side procedural-macro expansion under explicit observe, governed, or permissive policy. That session records the selected macro artifact, toolchain, policy result, and expansion status without treating editor expansion as a bake or affecting a bake cache. A general compatibility export cannot claim to govern or receipt macro execution performed by an independent editor process.
8. Editor diagnostics and runnable actions must delegate through the selected Oven plan and emit the required Rust diagnostics; they must not dispatch to Cargo. Projection data must not disclose sensitive environment values or duplicate dependency trees merely for inspection.

### Cargo compatibility and explicit adoption

1. `oven cargo ...` is an explicit compatibility operation for a directory with `Cargo.toml` and no `loaf.toml`. Cargo remains authoritative for its manifest, lock, build scripts, procedural macros, profiles, workspace discovery, and side effects. Oven records that it ran Cargo mode and may retain a wrapper receipt; it does not reinterpret Cargo results as an Oven-native lock.
2. A directory that contains `loaf.toml` is always a Loaf project for normal Oven operations. A neighbouring Cargo manifest is diagnosed and ignored.
3. Adoption is an explicit user-requested transformation. It may parse Cargo metadata as input, propose a `loaf.toml`, enumerate unsupported or policy-sensitive behavior, and write project state only after the mutation/policy rules of RFC 076 approve it.
4. Adoption must never leave an implicit mixed state. Once a user accepts the Loaf contract, future Oven-native operations use `loaf.toml` and `oven.lock`; continuing to run Cargo requires an explicit Cargo operation.
5. Cargo-compatible caller packages remain RFC 097 interoperability projections. They may consume selected outputs, but must not cause Cargo to resolve, compile, or select the Incan implementation graph.

### Supported-envelope conformance

The native Rust claim is valid only for the published conformance envelope. The initial matrix must include at least:

| Class                         | Required evidence                                                                                  |
| ----------------------------- | -------------------------------------------------------------------------------------------------- |
| Pure Rust library and binary  | direct `rustc` graph, feature closure, lock/offline/relocation, receipt and cache reuse            |
| Mixed Incan/Rust Loaf         | cross-language dependency/linkage facts and source-mapped diagnostics                              |
| Rust integration and doctests | separately selected test roles, scheduler/receipt behavior, failure diagnostics                    |
| Proc macro and build provider | Cargo-equivalent direct-`rustc` macro expansion, host artifact/compile-unit receipt, observe/governed policy behavior, directive diagnostics, generated-input invalidation, cross-target target consumer |
| Rust IDE selection            | selected-plan projection and provenance sidecar, cfg/target/generated-input analysis, Oven-only diagnostics actions, explicit editor macro policy, and pre-warmed closure reuse across equivalent slices |
| Native-link crate             | capability/toolchain/linker facts and target-bound carrier asset                                   |
| Cross target                  | separated host/target plan, sysroot/linker identity, no host result mislabelled as target proof    |
| Cargo project                 | explicit `oven cargo` execution and a no-implicit-adoption diagnostic                              |
| Cargo adoption                | proposed Loaf contract, policy-gated write, unsupported-feature diagnostic, then Loaf-only bake    |

The matrix is a compatibility statement, not a one-time benchmark. It must run on every change that can alter resolver, provider, compiler, cache, target, or receipt behavior.

## Design details

### Relationship to RFC 117

RFC 117 owns one manifest authority, one typed dependency graph, target policy, registry trust, `oven.lock`, `*.loaf` identity, and the rule that a discovered Cargo file is ignored in Loaf mode. This RFC supplies the detailed Rust planner and provider contract that RFC 117 intentionally leaves open. It does not create a second Rust manifest or make a Rust facet an exception to workspace authority.

The illustrative `rust.*` tables in this RFC are source-facet data within `loaf.toml`, not a return to one manifest per implementation language. `[rust.source]` names a mixed or nonstandard Rust root, and other `rust.*` tables carry compact Rust-specific exceptions. No `[oven.rust]` prefix is needed: `loaf.toml` is already the Oven-managed project document. Incan uses the parallel built-in `[incan.source]` root; C remains behind `[interop.c]` rather than becoming a peer top-level source namespace.

### Relationship to RFC 118

RFC 118 owns command ownership. This RFC defines what the Rust-oriented project operations mean once selected:

- `oven bake`, `oven test`, inspection, lock, registry, and publication are Oven operations;
- `incan run` and `incan test` inside a Loaf remain shallow delegates to the same Oven plan and receipt;
- `oven cargo ...` is the only ordinary path that invokes Cargo; and
- any `oven adopt cargo` spelling remains a user-visible, policy-gated mutation rather than an implicit discovery behavior.

### Relationship to RFC 097

RFC 097 defines a Rust-host caller facet and its stable ABI/package metadata. RFC 119 supplies the native-Rust build graph that can compile a caller projection, but does not make Cargo a dependency of the Incan implementation graph or expose generated implementation Rust as the caller contract.

### Publication authority

An Oven-native package's publication identity is a Loaf registry identity. `incan.pub` is the default and only registered endpoint for the foreseeable future. Crates.io is a Rust crate source, not an Oven-native publish target.

## Alternatives considered

### Keep Cargo as the hidden Oven backend

Rejected. It would leave build planning, invalidation, feature selection, build-time effects, target identity, and diagnostics under Cargo's opaque authority while presenting Oven as if it owned them.

### Implement all Cargo behavior before supporting Rust Loaves

Rejected. Cargo has a large historical compatibility surface. Oven should publish a growing explicit conformance envelope instead of promising undocumented parity or delaying useful native support indefinitely.

### Refuse crates.io inputs

Rejected. Rust crates, SemVer requirements, target triples, compiler artifacts, and registry packages are part of the ecosystem Oven must interoperate with. Refusing them would create an isolated package universe rather than a better build authority.

### Publish Oven-native packages to crates.io

Rejected. It would make Cargo's package format and release lifecycle a public authority over Loaves. Rust-host interoperability can use explicit projections without turning crates.io into Oven's publication system.

### Treat every Cargo project as an adoptable Loaf automatically

Rejected. It would revive implicit Cargo semantics, surprise build-script/proc-macro effects, and make support obligations unbounded. Adoption must be explicit, reviewed, and policy-gated.

## Drawbacks

- The Rust facet and direct compiler planner are substantial engineering work; Cargo currently hides much of this complexity.
- Provider-mediated build scripts and procedural macros need sandboxing, receipt storage, and a practical support policy.
- A bounded compatibility matrix will require explicit effect grants or Cargo compatibility for ordinary Cargo packages whose host behavior lacks a supported, inspectable provider path.
- Maintaining a Rust-analyzer projection and source-mapped diagnostics is additional tooling work.
- A different publication authority means Cargo-native consumption of an Oven package remains an explicit interoperability problem, not a default workflow.

## Layers affected

- **Loaf manifest and validation** — must validate Rust facet identities, roles, crate roots, editions, crate types, feature mappings, target predicates, and adoption proposals.
- **Resolver and registries** — must resolve crate providers, constrained provider metadata, deterministic feature closures, trust/integrity facts, and host/target partitions into `oven.lock`.
- **Rust operational core/API** — must plan direct `rustc` units, toolchains, sysroots, linkers, provider execution, artifacts, receipts, stores, leases, and recovery without Cargo in native mode.
- **Provider and policy engine** — must declare, approve, execute, cache, invalidate, and inspect build scripts, generated inputs, native-link facts, and effect receipts; it must apply governed containment around compiler processes that host proc macros without substituting a new expansion mechanism.
- **Compiler service API and diagnostics** — must expose Rust/compiled-source diagnostics and source maps without making Oven invoke the Incan CLI or generated Rust the public contract.
- **Test, docs, and IDE tooling** — must model Rust test/documentation roles, scheduler facts, outputs, and derived Rust-analyzer-compatible project metadata.
- **CLI and project mutation** — must expose canonical Oven operations and explicit Cargo/adoption modes according to RFCs 076 and 118.
- **Registry and publication tooling** — must publish Loaf artifacts only to registered Incan registry identities and prevent accidental crates.io publication.

## Inspectability and tooling surface

- **Manifest and lock:** inspection reports the selected Rust facet, crate roles, conventional defaults or explicit declarations, provider origins, feature closure, host/target partition, and normalized toolchain/linker facts.
- **Plan:** before execution, Oven shows every Rust compile unit, host macro artifacts, build-script providers, policy mode, expected effect classes, selected target/carrier/profile, and expected `*.loaf` asset/receipt.
- **Artifacts and receipts:** each `*.loaf` asset exposes its facet, graph digest, payload digest, target, carrier, profile, provider receipts, and final bake receipt.
- **Diagnostics:** invalid roots, unsupported provider metadata, build-script directives, proc-macro behavior, feature conflicts, source/target incompatibility, Cargo coexistence, and failed adoption name the affected source/dependency and the relevant explicit alternative.
- **IDE:** the derived Rust project projection and Oven provenance sidecar identify the selected plan and receipt from which they were created, whether a receipt-compatible pre-warmed closure was reused, and whether the projection is fresh; neither is edited as authority.
- **Not implicit:** no Cargo project discovery, Cargo lock reuse, build-script execution, proc-macro execution, native linker search path, registry registration, or crates.io publication occurs merely because a file or dependency exists.

## Implementation plan

### Phase 1: Rust facet and crate-provider graph

- Settle the Rust-facet TOML grammar and conventional-default rules.
- Resolve crates.io and registered crate providers into the typed Oven graph with source/trust/feature/target identities.
- Produce a direct-`rustc` proof for a library and binary with locked/offline/relocation receipts.

### Phase 2: Roles, linkage, and target artifacts

- Add explicit test, example, benchmark, doctest, caller-projection, carrier, native-link, linker, and sysroot roles.
- Prove host and cross-target planning with target-bound `*.loaf` assets and carrier-aware receipts.

### Phase 3: Controlled build-time providers

- Implement the generic build-script directive vocabulary, provider isolation, observe/governed effect policy, output capture, and conservative invalidation rules.
- Add Cargo-equivalent direct-`rustc` procedural-macro support, source-mapped diagnostics, and observe/governed host-process conformance proofs.
- Extend the conformance corpus with generated-input, native-link, and cross-target cases.

### Phase 4: IDE and Cargo interoperation

- Materialize the derived Rust-analyzer project projection and Oven provenance sidecar from explicit inspection selections, reusing receipt-compatible pre-warmed Loaf closures across equivalent worktrees and slices.
- Prove target switching, generated-input analysis, Oven-only diagnostics actions, and explicit editor macro policy without rebuilding an unchanged dependency/provider closure.
- Implement explicit Cargo compatibility receipts and policy-gated adoption proposals.
- Prove that normal Loaf execution never falls back to Cargo.

### Phase 5: Release gate

- Publish the supported native-Rust conformance envelope and run it as a release gate.
- Keep unsupported behavior diagnostic and route only explicit compatibility invocations through Cargo.

## Design decisions

- **Convention-first Rust authoring:** Rust is a direct built-in semantic facet. A conventional Rust source layout must not be restated merely to adopt Loaf. `[rust.source]` is used only for a mixed or nonstandard root, and compact Rust-specific deviations remain beneath `rust.*`; the complete compilation graph remains plan and lock state.
- **Named environment use:** RFC 117 supplies the standard `dev`, `test`, `lint`, and `docs` environments. Rust test roles select `test` by default and documentation roles select `docs` by default. A bake is not an environment: its target, carrier, profile, and feature closure remain explicit plan selections.
- **Proc-macro compatibility:** a proc macro is a foreign Rust compiler feature. Oven compiles it for the build host and lets the selected `rustc` load and invoke it through the normal ABI, matching Cargo behavior. Oven improves graph/artifact reuse without caching individual expansions or changing macro execution semantics; stricter containment is governed-policy behavior only.
- **IDE selection, provenance, and warm reuse:** an editor uses one visible Oven inspection selection, defaulting to `dev` and the Loaf-selected or host-native target. Oven first reuses receipt-compatible pre-warmed Loaf/provider/artifact closures across equivalent clean worktrees and implementation slices, then derives a small local standard Rust project descriptor plus an Oven provenance sidecar. The descriptor is a compatibility export, not authority; it neither triggers every target build nor duplicates dependency output.
- **Editor-side macro execution:** an Oven-owned inspection session may expand procedural macros only under explicit observe, governed, or permissive policy and records session-level macro/toolchain/policy status separately from bake receipts. A third-party editor that consumes the compatibility descriptor is never presented as governed Oven execution.
- **Splitting a project's declarations across different build outputs is done by physically separating Loaves, not by tagging individual declarations:** this RFC's bake model already lets one compilation unit produce multiple `*.loaf` assets when different carriers are selected (see "Native linkage, carriers, and cross compilation"), but that is the same code packaged multiple ways, not different code per output. A project that genuinely needs different subsets of its own source in different outputs (for example, browser-facing code and server-facing code in one logical project) expresses that as separate Loaves under RFC 117's workspace model — the same idiom Rust uses (separate crates in a workspace) and Python uses (separate sub-packages), not a novel Incan mechanism. This RFC does not add a declaration-level tagging or attribute system for routing individual functions or modules into different carrier outputs from a single Loaf; RFC 117's existing workspace/member model already provides the separation, with a shared common Loaf as a dependency when code needs to be reused across the split. This closes the gap left by RFC 092's rejection using a mechanism this RFC already has, not a new one.
- **`rust.*` exceptions describe one Loaf's own single compilation unit, never an additional crate:** `[rust.source]`, and any other `rust.*` field, may only describe exceptions for the Rust unit that Loaf's own convention-first root would otherwise identify — a nonstandard root, crate type, edition, or non-default name for that one unit — plus additional *build roles* sharing that same unit, such as extra binaries alongside one library (Cargo's `[[bin]]` pattern). There is no `[rust.crates.<name>]`-style table for declaring a second, independently identified crate inside one Loaf's manifest. A genuinely separate Rust crate — most commonly a procedural-macro crate, which Rust's own compiler requires to be a distinct crate from anything that uses it — is a sibling Loaf under RFC 117's workspace model, referenced as an ordinary `loaf`-kind dependency with a `path` origin override:

    ```toml
    # myproject/loaf.toml
    [workspace]
    members = ["loaves/incan_derive"]

    [dependencies]
    incan_derive = { loaf = "incan_derive", path = "loaves/incan_derive" }

    [rust.source]
    root = "sources/rust"
    ```

    ```toml
    # myproject/loaves/incan_derive/loaf.toml
    [project]
    name = "incan_derive"

    [rust]
    crate-type = ["proc-macro"]
    ```

    The sibling crate is `loaf`-kind, not `crate`-kind, even though its content is entirely Rust: per RFC 117's own language-neutral model, `crate`-kind is for consuming the *external* Rust ecosystem (crates.io or a registered Cargo-compatible source), while `loaf`-kind is for this project's own package graph regardless of implementation language. Target predicates *within* one compilation unit (ordinary `#[cfg(...)]` source-level conditionals) remain native Rust and need no manifest declaration at all; this RFC already commits to passing the selected target through so `rustc`'s own `cfg` evaluation works unmodified.
- **Governed-mode containment delegates to existing platform-native primitives; Oven does not implement its own sandboxing:** on Linux, containment uses Landlock or delegates to an established userspace sandboxing tool such as bubblewrap rather than hand-rolled namespace/seccomp wiring; on macOS, `sandbox-exec`/Seatbelt profiles; on Windows, AppContainer or Job Objects. These are security-audited, already-existing primitives; Oven integrates with them rather than authoring new OS-level containment code, the same "reuse existing infrastructure" reasoning behind this RFC's proc-macro and generator decisions elsewhere. Where a build host has no viable primitive for a selected policy, governed mode is refused outright rather than silently weakened or emulated, exactly as this RFC's reference text already requires ("if the host cannot enforce the selected policy without changing behavior, Oven denies the governed operation"). Observe mode's Cargo-equivalent behavior is unaffected by this choice: observe mode records effects without enforcing containment, so it has no platform-primitive dependency at all.
- **The native-mode conformance bar is the price of a real benefit, not friction for its own sake, and that benefit is native-mode-only:** Oven-native's value over plain Cargo is not "compiles any single cold build faster" — a native bake still invokes the same `rustc` Cargo would. The concrete payoff is avoiding *redundant* rebuild work across environments: reusing a receipt-compatible pre-warmed Loaf/provider/artifact closure across equivalent clean worktrees, CI runners, and implementation slices (see "IDE selection, provenance, and warm reuse") gives a significant speed advantage over raw Cargo under warm circumstances, on top of the receipted, auditable, unified Incan+Rust project model Cargo cannot offer at all. Neither benefit is available in Cargo-compatibility mode: `oven cargo ...` is Cargo being fully authoritative over its own manifest, lock, and side effects, with no Oven-owned graph, receipt, or content-addressed store to reuse against. A project keeping `Cargo.toml` and wrapping it with `oven cargo` gets Cargo's own native performance characteristics, not Oven's — the reuse and receipt benefits require adopting `loaf.toml`.
- **Crate/provider promotion from Cargo-only to Oven-native reuses the existing conformance-corpus mechanism rather than a new process:** a crate or provider class graduates when (1) every build-time directive it emits is covered by the already-known directive vocabulary (see "Build scripts, procedural macros, and generated inputs") with no unknown directive falling back to a passthrough, (2) it is added to the conformance corpus with a passing artifact/receipt-equivalence proof against the same crate built through Cargo-compatibility mode, and (3) governed-mode containment succeeds on every supported build host if its build-time work needs host access at all. This is deliberately the same "compatibility statement, not a one-time benchmark" mechanism this RFC already commits to running on every relevant change, applied at crate/provider grain rather than only at the project-shape grain the existing conformance matrix covers. Most real-world `build.rs` usage (`rerun-if-changed`, `cfg` emission, ordinary link-lib/link-search) is expected to already fall within the known vocabulary; the bar mainly excludes exotic, undemonstrated build-time patterns, not the mainstream ecosystem.
- **No speculative naming for a future registry endpoint:** `incan.pub` is the registry for the foreseeable future. This RFC does not name, brand, or reserve a hypothetical second public endpoint; an unregistered domain speculatively mentioned in earlier drafts is removed rather than carried forward as a design question.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
