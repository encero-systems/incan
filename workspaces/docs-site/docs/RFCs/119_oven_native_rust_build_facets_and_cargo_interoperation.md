# RFC 119: Oven-native Rust build facets and Cargo interoperation

- **Status:** In Progress
- **Created:** 2026-08-04
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies)
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 125 (`incan.pub` Loaf registry and baked asset distribution; supersedes RFC 034)
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

RFC 117 establishes a language-neutral Loaf project model, but it deliberately does not specify enough Rust build behavior to replace Cargo as the normal authority for an explicitly supported Rust project. This RFC defines that next layer: Oven-native Rust build facets, an Oven-owned crate graph and direct-`rustc` plan, inert build-script inventory, declared generated/link/tool inputs, Rust test and IDE projections, and explicit Cargo interoperation.

Oven consumes third-party Rust packages only as Loaves: a Loaf registry adopts a crates.io package into a Loaf whose manifest carries its Rust facet, features, and Loaf dependencies (RFC 125), and Oven resolves, fetches, and compiles it like any other Loaf. Oven never reads a crates.io index, a crate's `Cargo.toml`, or a `Cargo.lock`, and never runs Cargo on any build path. It does not publish Loaves or `*.loaf` assets to crates.io. `loaf.toml` remains the sole authored project manifest, `oven.lock` remains the resolved graph, and a bake emits immutable target-bound `*.loaf` assets plus receipts. Oven never runs Cargo. An existing Cargo project becomes a Loaf through a one-time, explicit conversion that writes its `loaf.toml`; it is never silently merged into an Oven project.

## Core model

1. **A Rust facet is a built-in Loaf facet:** a conventional Rust-only Loaf uses the ordinary `src/` layout. A mixed or nonstandard Rust root declares `[rust.source]`; compact Rust-specific exceptions live beneath `rust.*`. In every case Oven, not a neighboring `Cargo.toml`, supplies the crate facts needed to plan and invoke `rustc`.
2. **Oven owns the graph:** dependency selection, feature resolution, target/host partitioning, toolchain choice, lock identity, execution plan, cache reuse, artifacts, and receipts belong to Oven.
3. **Loaves, not crates:** every dependency is a `loaf` dependency. A third-party Rust package reaches Oven as the Loaf a Loaf registry adopted it into (RFC 125); crates.io is a source the registry adopts from, never a source Oven reads. Publishing remains in Incan's registry ecosystem.
4. **The adopted Loaf's manifest is the only Rust metadata:** an adopted Loaf's `loaf.toml` states its Rust facet, features, dependencies, and build facts in the grammar this RFC defines. The upstream `Cargo.toml` and `build.rs` travel in the Loaf archive as inert files; Oven never reads them.
5. **Build scripts are inert; procedural macros keep their compiler ABI:** a `build.rs` is never compiled, executed, or interpreted by Oven. Its presence is reported once as a warning and otherwise ignored; everything a build script would have told Cargo is stated directly in the Loaf's manifest (see "Build scripts, procedural macros, and generated inputs"). A selected procedural-macro crate is compiled for the build host and executed by the selected `rustc` through its normal proc-macro ABI, as in Cargo; Oven does not replace that expansion mechanism, and it has explicit toolchain, host/target, policy, and receipt facts.
6. **Host and target are different build domains:** procedural macros, compiler plugins, and bake-time tools execute for the build host; libraries, binaries, test subjects, and carriers are compiled for the selected target. Oven plans and locks both domains separately.
7. **Rust test work is first-class:** unit tests, integration tests, doctests, examples, and benchmarks are explicit build/run roles, not side effects of a generic `rustc` invocation. They use RFC 117's named environments: `test` by default for test roles and `docs` by default for documentation roles.
8. **IDE projection is derived and warm-reusing:** Oven materializes a Rust-analyzer-compatible project projection from one explicit inspection selection. It first reuses every receipt-compatible pre-warmed `*.loaf` asset and provider/artifact closure, including across equivalent clean worktrees or implementation slices; the lightweight local projection then describes that selected graph. Switching an editor selection never implies rebuilding every target, dependency, or provider, and the projection does not create or require a synthetic authoritative `Cargo.toml`.
9. **No Cargo, one conversion:** Oven never invokes Cargo. A Cargo project crosses into Oven once, through an explicit conversion that reads its `Cargo.toml` and writes a `loaf.toml`; from then on only the Loaf manifest counts, and no directory with both manifests receives mixed semantics. `oven cargo …` may exist only as familiar Cargo-style command names for Oven's own operations.
10. **The Rust core remains narrow but real:** as decided by RFC 118, Oven's operational core/API owns planning, resolution, policy, stores, leases, physical compiler/tool execution, and crash-safe publication in Rust. Its public API is language-neutral and does not leak Rust borrows. Build-script execution is absent from that host boundary.
11. **Incan-first authoring remains policy:** a Rust facet makes Rust projects and explicitly justified mixed work supportable. It does not authorize new Rust in Incan products without a demonstrated limitation and tracked removal path.
12. **Native compatibility is proved, not implied:** the supported Rust envelope is defined by an executable conformance corpus. Behavior outside that envelope produces a precise diagnostic; there is no Cargo fallback.

## Motivation

RFC 117 makes an Oven-native Rust project architecturally possible: it has an authored manifest, a typed dependency graph, provider trust, target policy, lock identity, receipts, and an explicit boundary around Cargo. That is necessary but not sufficient. A Rust source file alone does not identify a complete compilation unit, determine which dependencies compile for the build host versus the final target, express crate features, control C/C++ compilation and linked libraries, or explain test and documentation artifacts.

Cargo is valuable because it solves those practical concerns for a large Rust ecosystem. But it is not a sufficient authority for a project that includes Incan sources, checked interop, target carriers, governed actions, receipts, persistent stores, and Incan package publication. Oven should not hide Cargo under a different command name; it must own its build graph and make any Cargo interoperation explicit.

The right objective is therefore neither “emulate every Cargo behavior” nor “rewrite Rust as Incan.” It is a bounded native-Rust contract that builds serious selected Rust Loaves, consumes the ordinary crate ecosystem, produces target-bound `*.loaf` assets, and makes its limits inspectable. Cargo remains available for compatibility while that envelope expands through evidence.

## Goals

- Specify the full Rust facet needed to bake an Oven-native Rust Loaf without using project Cargo metadata as authority.
- Resolve third-party Rust packages as adopted Loaves into the same `oven.lock` and receipt model as every other Loaf and capability dependency, with no Cargo metadata on the path.
- Define the Rust facet, feature, and dependency grammar an adopted Loaf's manifest uses, as the contract between the registry that writes it and the Oven resolver that reads it.
- Specify deterministic Rust feature resolution, dependency roles, source checksums, target conditions, toolchain selection, and host-versus-target build units.
- Support an explicitly bounded and inspectable path for procedural macros, C/C++ compilation and linked libraries, generated Rust inputs, bake-time tools, and linker/sysroot selection, with no build-script execution anywhere in it.
- Specify first-class roles for Rust libraries, binaries, integration tests, unit tests, examples, doctests, benchmarks, and caller projections.
- Produce `*.loaf` assets and receipts whose identities cover the complete Rust compilation and provider closure.
- Define a Rust-analyzer-compatible derived project projection without restoring Cargo manifest authority.
- Define a one-time conversion for Cargo projects choosing to become Loaves, and keep Cargo off every Oven path.
- Establish a conformance corpus that identifies the supported native Rust envelope and guards Cargo-free regressions.

## Non-Goals

- Publishing Oven-native Loaves, `*.loaf` assets, or normal Oven packages to crates.io.
- Making `Cargo.toml`, `Cargo.lock`, Cargo workspace discovery, Cargo build profiles, or arbitrary Cargo command behavior part of ordinary Loaf semantics.
- Supporting every Cargo crate on day one, or claiming compatibility from source discovery alone.
- Implicitly importing, merging, or executing a neighboring Cargo project after `loaf.toml` exists.
- Compiling, executing, interpreting, or sandboxing `build.rs` in Oven, including through an opt-in or fallback provider route.
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
serde = { loaf = "crates-io/serde", version = "1", features = ["derive"] }
tokio = { loaf = "crates-io/tokio", version = "1", features = ["rt-multi-thread", "macros"] }
```

With `src/lib.rs`, `src/main.rs`, or `src/bin/telemetry.rs` in the conventional locations, this project needs no Rust table. The essential contract is that Oven can identify every compiled Rust unit, its root, edition, type, entry point, enabled features, and dependency/linkage closure without reading a project `Cargo.toml`. The Rust facet remains convention-first: `src/lib.rs`, `src/bin/`, `tests/`, `examples/`, `benches/`, and documentation tests require no restatement in the manifest. Authors declare only meaningful deviations, such as an additional build role, a nonstandard root or crate type, a feature-gated target, or declared cfg/generated/link/tool inputs. A locally authored proc macro is a separate sibling-Loaf dependency. The expanded host/target unit graph is derived inspection state in `oven plan` and `oven.lock`, not normal authoring burden.

A project that mixes Incan and Rust names both roots explicitly:

```toml
[incan.source]
root = "sources/incan"

[rust.source]
root = "sources/rust"
```

The roots must not overlap. Compact exceptions beneath `rust.*` describe this Loaf's own Rust unit and its build roles. A distinct crate, such as a procedural-macro crate, is a sibling Loaf referenced through a `loaf` dependency; it is not an additional `rust.crates` table. This RFC does not use an anonymous `[[sources]]` list or require an author to reproduce Cargo's whole manifest surface.

`crates-io/serde` and `crates-io/tokio` are the Loaves `incan.pub` adopted from those crates.io packages. Their dependency keys, `serde` and `tokio`, are the names this project's Rust code uses for them. They resolve like any Loaf: Oven reads the registry's index and the adopted manifests, never a crates.io index or a `Cargo.toml`, and records each Loaf's identity, archive digest, trust outcome, selected feature closure, and host/target use in `oven.lock`. The project's publication identity remains an Incan registry identity; `oven publish` does not mean `cargo publish`.

### Build-time work is visible before it runs

Suppose `helper-sys` needs a generated binding, a compiled C helper, and two compile-time flags that its `build.rs` used to probe for. Under Oven that script is inert: Oven does not compile it, run it, or read its directives, and reports its presence once as a warning. The Loaf states what the script would have discovered:

```toml
[[rust.facts]]
toolchain = "rustc 1.98.0 (88d9e12ae 2026-08-18)"
target = "aarch64-apple-darwin"
profile = "release"
features = []
cfg = ["has_neon", "stable_intrinsics"]

[[rust.facts.link]]
name = "sys-helper"
target = 'cfg(target_arch = "aarch64")'
executable = { name = "clang", owner = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", path = "bin/clang", digest = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }
objects = [{ name = "helper.o", language = "c", arguments = [{ literal = "-c" }, { input = "helper-source" }, { literal = "-o" }, { output = "helper.o" }] }]
sources = [{ name = "helper-source", kind = "file", path = "c/helper.c", digest = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" }]
library = { name = "sys_helper", kind = "static" }

[[rust.facts.tool]]
name = "bindgen"
target = 'cfg(target_arch = "aarch64")'
executable = { name = "bindgen", owner = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd", path = "bin/bindgen", digest = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" }
arguments = [{ input = "header" }, { literal = "--output" }, { output = "bindings" }]
inputs = [{ name = "header", kind = "file", path = "include/helper.h", digest = "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff" }]
outputs = [{ name = "bindings", kind = "file", path = "generated/bindings.rs" }]
```

The nested arrays of tables are the authored grammar. Every field is plan-visible before anything runs. `cfg` and `out` are data: the plan shows normalized flags and owner-relative digests of the committed files or trees. `link` and `tool` are publisher-side bake work with complete declared input and output contracts. Their ordered arguments may use `{ owner = "..." }` to name a portable path below the executable owner; the executable plus every such path form the recursively inventoried, identity-verified compiler closure and the only owner data admitted to execution. A `link` record declares its digested source closure, sorted explicit object compilations and logical static library. Each object references at least one declared source and exactly its own name through `{ output = "..." }`, and may list `reads`: the logical names of declared sources its compile reads without naming them in its arguments, such as a header included relative to the source's own directory, taken from the compiler's dependency output when the fact is recorded. Every declared source is referenced by an object's arguments or `reads`, and the Oven materializes an object's `reads` with its other inputs, and source and object names share one namespace. Source paths may overlap, including compiled files within an include tree, while a source may not overlap an object output and object outputs may not overlap each other. The Oven invokes the one declared compiler once per object, verifies the exact object set, and writes the indexed archive itself; no archiver is declared. A `tool` record declares digested inputs and logical outputs. Product bytes, product digests and producer receipts belong to the admitted asset and receipt rather than either fact table. Their products enter the same admitted selected-unit and store path as other generated and linked-library inputs; a consumer receives the finished archive and generated inputs and never executes either. A tool is a host-domain unit whose executable identity, inputs, outputs, selected target association, product asset and receipt are part of the consuming closure.

The `cfg` list is deliberately the *answer* rather than the question. A build script asks "does this `rustc` have `core::error::Error`?" at every build; under Oven's frozen toolchain that answer is a constant, so the manifest records it once and the answer is re-derived when the freeze moves.

Procedural macros follow the same host/target split, but not a new Oven expansion protocol. Oven compiles the macro for the build host, then invokes the selected `rustc` with the normal proc-macro artifact so that `rustc` loads and expands it through its normal Rust proc-macro ABI, as it would in a Cargo build. The target crate never treats the host macro binary as a target artifact. Oven records the macro artifact and host toolchain as facts of the consuming compile unit; it does not cache individual macro expansions or route a bake through a separate macro server.

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

### Coming from Cargo

A Cargo project crosses into Oven once:

```text
oven adopt cargo --manifest ./Cargo.toml
  One-time conversion: read the project's Cargo.toml, propose a loaf.toml,
  name anything it cannot express, and write it once the user accepts.

oven bake
  loaf.toml and oven.lock are authoritative; Cargo files are ignored.
```

`oven adopt cargo` is illustrative command spelling. The important rule is explicit re-authoring: a user chooses to cross the boundary, sees the proposed Loaf contract and anything it cannot express, and accepts the resulting project state. The conversion reads files; it does not run Cargo. Oven does not treat every checked-out `Cargo.toml` as a latent Loaf, and Oven never runs Cargo. For familiarity, `oven cargo build`, `oven cargo test`, and similar spellings may map to the corresponding Oven operations on a Loaf; they are names, not a Cargo mode.

### Publishing and consuming

Oven-native packages publish to Incan's registry ecosystem. `incan.pub` is the public endpoint for the foreseeable future; registry identity and trust remain explicit Oven configuration rather than a hard-coded domain in `loaf.toml`.

Oven consumes Rust libraries from the crates.io ecosystem because they are part of the world it interoperates with, and it consumes them as the Loaves the registry adopted them into. That is not reciprocal authority: an Oven-native package is not automatically a crates.io package. If a future product needs a Cargo-consumable publication projection, it must be a separately specified bridge with explicit provenance to a selected `*.loaf` asset.

## Reference-level explanation

### Rust facet identity and discovery

1. A Loaf that selects a Rust facet must resolve every compiled Rust unit before execution. A unit has a name, package identity, root, edition, role, type or target kind, enabled feature set, target condition, dependency/linkage closure, and source digest.
2. A conventional single library root may default to `src/lib.rs`; a conventional single binary may default to `src/main.rs`. The derived unit name follows the project name only when that mapping is unambiguous. All other roots and identities must be explicitly declared.
3. Libraries, binaries, integration tests, examples, doctests, benchmarks, procedural macros, bake-time tools, generated sources, and caller projections have distinct roles. A role determines its compilation mode, host/target domain, artifact kind, test scheduling, and receipt contribution.
4. The Rust facet may use `cfg`-style target conditions only as explicit, normalized predicates over the selected target and build host. Ambient host probing must not silently add a source or feature.
5. Project public features remain RFC 114 Loaf capabilities. A Rust unit's `cfg(feature = …)` set is exactly its Loaf's enabled feature set (see "Rust facet grammar"); a feature of a dependency Loaf is requested on that dependency and never becomes a public feature of the requesting Loaf by name coincidence.

### Rust facet grammar

This grammar is the whole contract between a Loaf registry that adopts a Rust package and the Oven resolver that reads it. The registry writes it; Oven reads nothing else.

1. **Rust table.** `[rust]` declares the Loaf's own Rust unit when convention cannot: `name` (the name Rust code uses for this unit, defaulting to the project name with `-` replaced by `_`), `type` (`"lib"` by default, or `"proc-macro"`), and `edition`. The root is `[rust.source] root` or the conventional `src/lib.rs`. A `proc-macro` Loaf is always compiled as a host unit.
2. **Features.** `[project.features]` uses RFC 114's grammar, extended with one member form:

    ```text
    feature_member ::= feature_name
                     | "dep:" dependency_name
                     | dependency_name "/" feature_name
                     | dependency_name "?/" feature_name
    ```

    `dependency_name "?/" feature_name` is a **weak** dependency feature: it requests the feature only if the dependency is active for another reason, and never activates it. A plain `dependency_name "/" feature_name` keeps RFC 114's meaning and must name a non-optional dependency or one this feature activates with `dep:`. The `default` feature, when declared, is enabled unless a consumer sets `default-features = false`. Every feature of a Rust-facet Loaf is also a `cfg(feature = "<name>")` of its Rust unit; there is no separate mapping table.
3. **Dependencies.** A Rust-facet Loaf's `[dependencies]` entries are `loaf` dependencies with the keys `loaf`, `version`, `features`, `default-features`, `optional`, and `target`. The entry key is the extern name the unit's code uses, so a renamed dependency is a different key over the same `loaf`. `version` is a semantic-version requirement: comma-joined comparators of the forms `^v`, `~v`, `=v`, `>v`, `>=v`, `<v`, `<=v`, and `*` wildcards, where a bare version means `^v`. `target` is either a normalized `cfg(...)` predicate over the selected target in the same grammar the build facts use, or an exact target triple that matches only that triple; the dependency applies only where it holds. A key may instead hold an array of entry tables, one per declaration, each with its own `target`: every entry whose target holds applies, their features union, every applicable requirement must hold, and the dependency is active if any applicable entry is non-optional or the key is enabled. A `cfg(...)` predicate is written canonically: atoms are `ident` or `ident = "value"`; `all(...)`, `any(...)`, and `not(...)` separate their arguments with `, ` and carry no other whitespace; argument order is as written. A predicate that does not parse is refused with the construct named. A dependency on a `proc-macro` Loaf is a host edge.
4. **No other inputs.** A Rust-facet Loaf has no development, build, or links dependencies: tests of a dependency are never compiled by a consumer, a build script's effect is carried by its build facts, and `links` only fed build scripts. An adopted Loaf must not declare them, and Oven refuses a manifest that does.

### Rust-facet Loaves and feature resolution

1. Oven resolves Rust-facet Loaves exactly as it resolves every Loaf (RFC 117): from the registry index and the selected Loaf manifests, through registered Loaf registries only. A registry's index lines carry each version's dependencies and features so that the whole graph resolves before any archive is fetched.
2. Feature resolution is unified per domain: one enabled feature set per Loaf version for target units, and a separate one for host units (procedural macros and everything they depend on). The lock records requested and selected features for every host and target unit. A Loaf version appears once per domain; two different enabled sets for one Loaf in one domain are a resolution error, never two units.
3. Target predicates are evaluated against the selected target for target units and against the build host for host units, using the selected toolchain's normalized `cfg` facts.
4. A non-weak feature member `x/f`, where `x` names an optional dependency, also enables the feature named `x` when the feature table defines one and some optional declaration of `x` applies, its target holding for the domain being resolved. `dep:x` never enables the feature `x`. A declaration whose target does not hold contributes nothing: it neither activates the dependency nor enables a feature of the same name.
5. `oven.lock` records each Loaf's registry identity, version, archive digest, feature closure per domain, evaluated target predicates, and trust outcome. It contains no `Cargo.lock` and no crate-level resolution Oven did not perform.
6. A build fact (see "Build scripts, procedural macros, and generated inputs") applies only to the exact binding it was recorded for: toolchain, target, profile, and the complete enabled feature set. The resolver's unification is therefore part of the contract: a registry's recorded binding is found only when Oven unifies features to the same set. A fact with `link` or `tool` records also names each executable's owner identity, and applies only where that owner is present: a unit compiled elsewhere is consumed as its finished asset and needs no local owner, while a local compile under a different owner records the fact for that owner first.
7. A manifest construct outside this grammar must identify the Loaf, the field, and its source span, and is refused.

### Direct Rust compilation and artifact identity

1. Oven invokes the selected Rust compiler through its controlled Rust operational core/API; Cargo is not a subprocess in an Oven-native bake.
2. Each compile unit is keyed by the compiler/toolchain identity, normalized command inputs, source digests, dependency artifact identities, selected features, cfg facts, build host, target, profile, generated inputs, linked-library facts, tool products, and the receipts that admitted those facts.
3. Host units and target units are distinct graph nodes. A procedural macro, compiler plugin, or bake-time tool is always a host node even when it was selected by a target crate. A `build.rs` is inventory, never a graph unit.
4. A profile is an Oven target policy that maps to normalized compiler/codegen/link options. Cargo profile inheritance is not read; any translation of Cargo profiles happens only in the explicit one-time conversion.
5. A completed bake emits one or more target-bound `*.loaf` assets according to the selected Rust roles and carriers. Every asset identifies the selected Loaf/facet, target, carrier, profile, graph digest, payload digest, relevant provider receipts, and final execution receipt.

### Build scripts, procedural macros, and generated inputs

1. A `build.rs` is inert. Oven never compiles it, executes it, interprets its directives, or admits it as a provider candidate; there is no `rust.build-script` role. A discovered `build.rs` produces one warning naming the package and is otherwise ignored. There is no policy mode, observe mode, or explicit opt-in that runs it.
2. What a build script would have emitted is declared in the manifest instead, in the package's own terms rather than Cargo's directive protocol: `cfg` (compile-time flags, optionally target-conditional through the same predicate grammar used for dependency edges), `out` (committed owner-relative files or trees the source includes; replaces ambient `OUT_DIR`), `link` (declared C/C++ sources compiled at publisher bake into a named archive, or a named static, shared, or system library), and `tool` (a publisher-side generator represented as a host unit with complete declared inputs and outputs). Target-gated features and typed configuration cover the remaining cases. Nothing in Cargo's directive protocol becomes Oven authority: not `rerun-if-*`, `rustc-check-cfg`, `links`, `DEP_*`, or script-emitted warnings.
3. `cfg` values are the normalized resolved answer for the selected toolchain freeze and target, not a probe. The toolchain and target are already unit-identity coordinates, so a manifest is scoped to a freeze and its `cfg` list must be re-derived when the freeze moves. Empty, repeated, malformed, or unsupported values are refused rather than normalized by guesswork.
4. A procedural-macro crate is a host Rust unit. Oven locks its host crate closure, compiler identity, macro artifact digest, and diagnostics identity. The selected `rustc` loads and invokes that artifact through its normal proc-macro ABI; Oven must not insert a custom expansion bridge or standalone macro server into a bake. The target crate never treats the host macro binary as a target artifact.
5. Observe mode preserves Cargo-equivalent host execution for a selected procedural macro while recording macro artifact, compiler invocation, observable inputs/effects, and diagnostics at the consuming compile-unit boundary. Oven does not claim an independently controlled receipt for every macro call. A backend that cannot observe an input needed for cache correctness must mark the result uncacheable rather than reuse it falsely.
6. In governed mode, any containment applies around the compiler process that hosts macro execution. It must be explicit and receipt-visible; if the host cannot enforce the selected policy without changing behavior, Oven denies the governed operation rather than silently weakening policy. Permissive mode remains an explicit escape hatch. There is no implicit Cargo fallback.
7. `link` and `tool` work happens only in the publisher's bake. A consumer receives the finished archive, link name, and generated inputs inside the admitted unit; it never compiles C, runs a generator, or probes the system. A bake that needs a C compiler or a named tool declares its exact executable identity and complete inputs and outputs, and a host without that selection refuses rather than substituting.
8. A generated source participates only through an identified `out` file or tree or a declared `tool` output. The selected unit records its stable logical name, owner-relative source, and content digest. Missing, extra, overlapping, escaping, symlink-substituted, or tampered inputs and outputs are refused. Two producers must not claim the same logical output or linked-library identity; ambiguity is an error rather than a path- or order-based tie-break.
9. The selected-unit identity retains target, toolchain, profile, features, normalized cfg facts, source closure, generated inputs, linked-library inputs, tool products, and dependency identities. Physical materialization must bind every portable owner-relative input back to an admitted owner and revalidate its bytes before execution. Publication and cache reuse must retain the same provenance; a store hit cannot manufacture missing authority.
10. Adoption belongs to the Loaf registry, not to Oven (RFC 125). A third-party package gains its Loaf manifest when the registry adopts it, and its build facts when the registry records them for a binding: the registry compiles the package's `build.rs` once with the pinned `rustc` against its build-dependency Loaves, executes it sandboxed under a declared environment for that binding, and records its directives as the fields above. No Cargo runs at any step. The record names the toolchain, target, profile, complete feature set, upstream package identity, accepted directives, and generated/link/tool byte identities; an unknown required directive refuses, and a changed freeze requires a new binding. Oven itself still never compiles, executes, or interprets a `build.rs`.

### C/C++ and linked libraries, carriers, and cross compilation

1. A Rust unit's linked libraries, frameworks, system SDKs, C ABI shims, linker, and sysroot requirements are typed capability/provider facts under RFCs 116 and 117. They are never inferred from an unrecorded local linker search path.
2. Cross compilation selects a build host and target separately. The plan must report both, the toolchain/sysroot/linker identities, selected capability providers, and any provider node that executes for the host.
3. Target artifacts are assigned to an explicit carrier. A Rust library, executable, `cdylib`, Rust-host caller projection, JNI library, Python carrier, or C ABI carrier can be selected only when the target and provider contracts permit it.
4. The receipt distinguishes compile, link, package, test, and deployment facts so a successful host test cannot be mistaken for a target bake or device proof.
5. An **isolation boundary** is a declared carrier edge across which no Rust type, trait object, or runtime state passes: a separate process, or a C ABI or another external ABI carrier under RFC 116. It is the only permitted way for two distinct compiled instances of one package to coexist in one deliverable (RFC 124 refuses them within a single link closure). The boundary, and the fact that a second instance lives behind it, must be declared in the plan and visible in the receipt; an undeclared second instance is a plan error, not a warning.

### Rust test, documentation, and IDE projections

1. Unit tests compile with their owning crate. Integration tests, examples, doctests, and benchmarks are distinct named units with declared source origins and dependency roles.
2. Test execution uses Oven's scheduler, selected environment, capability policy, target selection, output retention, and receipt model. Test roles select the standard `test` environment unless an explicit compatible environment is selected; documentation roles select `docs` by default. Environment-only dependencies do not enter an ordinary release bake unless the selected role requires them. Test execution must not silently dispatch to `cargo test` in Loaf mode.
3. A doctest extraction or documentation generator must record its source spans, generated test inputs, compiler/provider facts, and diagnostics mapping. Generated Rust remains inspectable but is not the public source of truth.
4. A Rust-analyzer-compatible projection is derived from one explicit Oven inspection selection and may be invalidated with the same receipt-bound facts. Its standard project descriptor contains only the selected crate roots, editions, exact dependency aliases, first-party membership, resolved cfg/target facts, selected sysroot, and non-sensitive compile environment or identified generated-output paths needed for analysis. An Oven sidecar identifies the selection's plan and receipt, freshness state, policy state, and diagnostics mapping; that sidecar, not the descriptor, retains Oven provenance.
5. The active inspection selection defaults to the `dev` environment and the Loaf-selected target, or the host-native target if no project target is selected. Target switching is explicit and visible; Oven must not prepare every target merely because an editor is open.
6. Before materializing a selection, Oven must look up the receipt-compatible pre-warmed `*.loaf` asset and provider/artifact closure. It must reuse that closure across equivalent clean worktrees and implementation slices, then materialize only the small workspace-local projection needed for the current source paths. A miss invalidates and rebuilds only the affected selection and units.
7. An Oven-owned inspection session may enable editor-side procedural-macro expansion under explicit observe, governed, or permissive policy. That session records the selected macro artifact, toolchain, policy result, and expansion status without treating editor expansion as a bake or affecting a bake cache. A general compatibility export cannot claim to govern or receipt macro execution performed by an independent editor process.
8. Editor diagnostics and runnable actions must delegate through the selected Oven plan and emit the required Rust diagnostics; they must not dispatch to Cargo. Projection data must not disclose sensitive environment values or duplicate dependency trees merely for inspection.

### Converting a Cargo project

1. Oven never invokes Cargo. A directory with `Cargo.toml` and no `loaf.toml` is not an Oven project; an Oven operation there names the conversion command and does nothing else.
2. A directory that contains `loaf.toml` is always a Loaf project for normal Oven operations. A neighboring Cargo manifest is diagnosed and ignored.
3. Conversion is an explicit user-requested transformation. It may read the project's own `Cargo.toml` as input, propose a `loaf.toml` (its dependencies become `loaf` dependencies on the adopted Loaves), enumerate what it cannot express, and write project state only after the mutation/policy rules of RFC 076 approve it. It runs no Cargo.
4. Conversion must never leave an implicit mixed state. Once a user accepts the Loaf contract, Oven operations use `loaf.toml` and `oven.lock` only.
5. `oven cargo <command>`, where offered, maps a familiar Cargo command name onto the corresponding Oven operation on a Loaf. It never invokes Cargo and never reads a `Cargo.toml`.
6. Cargo-compatible caller packages remain RFC 097 interoperability projections. They may consume selected outputs, but must not cause Cargo to resolve, compile, or select the Incan implementation graph.

### Supported-envelope conformance

The native Rust claim is valid only for the published conformance envelope. The initial matrix must include at least:

| Class                         | Required evidence                                                                                  |
| ----------------------------- | -------------------------------------------------------------------------------------------------- |
| Pure Rust library and binary  | direct `rustc` graph, feature closure, lock/offline/relocation, receipt and cache reuse            |
| Mixed Incan/Rust Loaf         | cross-language dependency/linkage facts and source-mapped diagnostics                              |
| Rust integration and doctests | separately selected test roles, scheduler/receipt behavior, failure diagnostics                    |
| Proc macro, link, and tool stages    | Cargo-equivalent direct-`rustc` macro expansion, host artifact/compile-unit receipt, observe/governed policy behavior, inert-`build.rs` warning, `cfg`/`out`/`link`/`tool` field coverage, generated-input invalidation, cross-target target consumer |
| Rust IDE selection            | selected-plan projection and provenance sidecar, cfg/target/generated-input analysis, Oven-only diagnostics actions, explicit editor macro policy, and pre-warmed closure reuse across equivalent slices |
| Linked-library crate                 | capability/toolchain/linker facts and target-bound carrier asset                                   |
| Cross target                  | separated host/target plan, sysroot/linker identity, no host result mislabeled as target proof    |
| Cargo project                 | a diagnostic naming the conversion command; no Cargo invoked                                      |
| Cargo adoption                | proposed Loaf contract, policy-gated write, unsupported-feature diagnostic, then Loaf-only bake    |

The matrix is a compatibility statement, not a one-time benchmark. It must run on every change that can alter resolver, provider, compiler, cache, target, or receipt behavior.

For an adopted package, conformance requires literal byte equivalence between two independent Oven-native bakes under one recorded deterministic setup, and that every selected binding's feature set equals the one its registry adoption recorded, in addition to receipt and behavioral proof. No Cargo build serves as the reference. The comparison must record the exact artifact kinds, selected host and target, toolchains, features, profile, source identity, and path-remapping inputs. The comparison set includes the selected provider/consumer shape's complete metadata-reachable artifact closure: Rust libraries and metadata sidecars, direct and transitive registry units, and applicable generated inputs, linked-library archives, and tool products. Comparing a root library alone does not establish closure equivalence. A staging-path difference is a failed equivalence case until deterministic paths or remapping remove it; behavioral similarity must not silently replace this requirement.

## Design details

### Relationship to RFC 117

RFC 117 owns one manifest authority, one typed dependency graph, target policy, registry trust, `oven.lock`, `*.loaf` identity, and the rule that a discovered Cargo file is ignored in Loaf mode. This RFC supplies the detailed Rust planner and provider contract that RFC 117 intentionally leaves open. It does not create a second Rust manifest or make a Rust facet an exception to workspace authority.

The illustrative `rust.*` tables in this RFC are source-facet data within `loaf.toml`, not a return to one manifest per implementation language. `[rust.source]` names a mixed or nonstandard Rust root, and other `rust.*` tables carry compact Rust-specific exceptions. No `[oven.rust]` prefix is needed: `loaf.toml` is already the Oven-managed project document. Incan uses the parallel built-in `[incan.source]` root; C remains behind `[interop.c]` rather than becoming a peer top-level source namespace.

### Relationship to RFC 118

RFC 118 owns command ownership. This RFC defines what the Rust-oriented project operations mean once selected:

- `oven bake`, `oven test`, inspection, lock, registry, and publication are Oven operations;
- `incan run` and `incan test` inside a Loaf remain shallow delegates to the same Oven plan and receipt;
- no Oven operation invokes Cargo, and `oven cargo ...`, where offered, only names Oven operations; and
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

Rejected. Rust crates, SemVer requirements, target triples, compiler artifacts, and registry packages are part of the ecosystem Oven must interoperate with. Refusing them would create an isolated package universe rather than a better build authority. Oven consumes them as adopted Loaves instead: the ecosystem stays reachable, and the registry, not Oven, is the one place that ever reads crates.io.

### Read crate metadata directly, with Cargo once at adoption

Rejected. An earlier revision let Oven parse a registry crate's `Cargo.toml` as constrained provider metadata and harvested build facts from one Cargo build. Both put Cargo's model on Oven's side of the boundary: two readers of one format drift, and a consumer's resolution would depend on files it does not own. The adopted Loaf manifest moves the one translation to the registry, where it happens once per package version, and leaves Oven a single grammar to read.

### Publish Oven-native packages to crates.io

Rejected. It would make Cargo's package format and release lifecycle a public authority over Loaves. Rust-host interoperability can use explicit projections without turning crates.io into Oven's publication system.

### Treat every Cargo project as an adoptable Loaf automatically

Rejected. It would revive implicit Cargo semantics, surprise build-script/proc-macro effects, and make support obligations unbounded. Adoption must be explicit, reviewed, and policy-gated.

## Drawbacks

- The Rust facet and direct compiler planner are substantial engineering work; Cargo currently hides much of this complexity.
- Procedural macros still execute host code at bake and need receipt storage and a practical support policy; declaring a package's needs in the manifest moves work from the script author to whoever adopts the package.
- A bounded compatibility matrix leaves packages whose generated, linked-library, or tool requirements cannot be expressed by the supported manifest contract unavailable until their adoption can declare them; there is no Cargo route to fall back on.
- Maintaining a Rust-analyzer projection and source-mapped diagnostics is additional tooling work.
- A different publication authority means Cargo-native consumption of an Oven package remains an explicit interoperability problem, not a default workflow.

## Layers affected

- **Loaf manifest and validation** — must validate Rust facet identities, roles, unit roots, editions, unit types, feature mappings, target predicates, and adoption proposals.
- **Resolver and registries** — must resolve Rust-facet Loaves from registry index lines and adopted manifests, with deterministic per-domain feature closures, target predicates, trust/integrity facts, and host/target partitions, into `oven.lock`.
- **Rust operational core/API** — must materialize and execute already selected compiler, C/C++ compilation, linked-library, tool, artifact, receipt, store, lease, and recovery operations without Cargo; it must not recover build-script execution from source inventory.
- **Provider and policy engine** — must declare, cache, invalidate, and inspect `cfg`, `out`, `link`, and `tool` facts and their receipts, and warn on an inert `build.rs`; it must apply governed containment around compiler processes that host proc macros without substituting a new expansion mechanism.
- **Compiler service API and diagnostics** — must expose Rust/compiled-source diagnostics and source maps without making Oven invoke the Incan CLI or generated Rust the public contract.
- **Test, docs, and IDE tooling** — must model Rust test/documentation roles, scheduler facts, outputs, and derived Rust-analyzer-compatible project metadata.
- **CLI and project mutation** — must expose canonical Oven operations and the one-time Cargo-project conversion according to RFCs 076 and 118.
- **Registry and publication tooling** — must publish Loaf artifacts only to registered Incan registry identities and prevent accidental crates.io publication.

## Inspectability and tooling surface

- **Manifest and lock:** inspection reports the selected Rust facet, crate roles, conventional defaults or explicit declarations, provider origins, feature closure, host/target partition, and normalized toolchain/linker facts.
- **Plan:** before execution, Oven shows every Rust compile unit, host macro artifact, link and tool stage, inert build-script inventory warning, selected target/carrier/profile, exact input and output identities, and expected `*.loaf` asset/receipt. Proc-macro policy is shown separately from link/tool execution facts.
- **Artifacts and receipts:** each `*.loaf` asset exposes its facet, graph digest, payload digest, target, carrier, profile, generated/link/tool provenance, applicable proc-macro receipts, and final bake receipt.
- **Diagnostics:** invalid roots, manifest constructs outside the Rust facet grammar, a `build.rs` whose needs are not yet declared, proc-macro behavior, feature conflicts, source/target incompatibility, Cargo coexistence, and failed adoption name the affected source/dependency and the relevant explicit alternative.
- **IDE:** the derived Rust project projection and Oven provenance sidecar identify the selected plan and receipt from which they were created, whether a receipt-compatible pre-warmed closure was reused, and whether the projection is fresh; neither is edited as authority.
- **Not implicit:** no Cargo project discovery, Cargo lock reuse, build-script execution, proc-macro execution, link/tool execution, linker search path, registry registration, or crates.io publication occurs merely because a file or dependency exists.

## Implementation Plan

### Phase 1: Rust facet and adopted-Loaf graph

- Settle the Rust-facet TOML grammar and conventional-default rules ("Rust facet grammar").
- Resolve adopted Rust-facet Loaves into the typed Oven graph with source/trust/feature/target identities, and prove the resolver reproduces the feature sets of the registry's recorded bindings.
- Produce a direct-`rustc` proof for a library and binary with locked/offline/relocation receipts.

### Phase 2: Roles, linkage, and target artifacts

- Add explicit test, example, benchmark, doctest, caller-projection, carrier, linked-library, linker, and sysroot roles.
- Prove host and cross-target planning with target-bound `*.loaf` assets and carrier-aware receipts.

### Phase 3: Declared build-time units and procedural macros

- Implement the inert-`build.rs` warning, the `cfg`, `out`, `link`, and `tool` manifest fields and their flow into the direct-`rustc` invocation, and the harvest-based adoption that writes them from an existing Cargo build.
- Add Cargo-equivalent direct-`rustc` procedural-macro support, source-mapped diagnostics, and observe/governed host-process conformance proofs.
- Extend the conformance corpus with generated-input, linked-library, and cross-target cases.

### Phase 4: IDE and Cargo interoperation

- Materialize the derived Rust-analyzer project projection and Oven provenance sidecar from explicit inspection selections, reusing receipt-compatible pre-warmed Loaf closures across equivalent worktrees and slices.
- Prove target switching, generated-input analysis, Oven-only diagnostics actions, and explicit editor macro policy without rebuilding an unchanged dependency/provider closure.
- Implement the policy-gated one-time conversion of a Cargo project.
- Prove that normal Loaf execution never falls back to Cargo.

### Phase 5: Release gate

- Publish the supported native-Rust conformance envelope and run it as a release gate.
- Keep unsupported behavior diagnostic; no invocation routes through Cargo.

## Progress Checklist

This checklist tracks the full RFC. The dev.6 ecosystem slice (#1561) delivers the inert-script, declared-input, adoption, and conformance work; it does not by itself complete the remaining Rust roles, IDE, or caller surfaces.

### Spec reconciliation

- [x] Reconcile build-script policy with #1561: scripts are inert, declared inputs replace directives, and proc-macro host semantics remain separate.

### Rust facet and graph

- [x] Settle and document the authored `link` and `tool` grammar; the remaining conventional Rust-facet defaults stay open.
- [ ] Verify selected adopted-Loaf source, feature, toolchain, and host/target identities, including feature-set parity with the registry's recorded bindings.
- [ ] Prove direct library and binary compilation with locked, offline, and relocated consumption.

### Declared inputs and execution

- [ ] Warn once for build-script presence and retire executable script candidates and carriers.
- [x] Carry declared cfg and committed generated inputs through planning, admission, and direct compilation (a registry record's `cfg` and `out` govern the unit in the policy engine, the publisher's observation must agree, and the runtime foundation carries the declared inputs consumers compile against).
- [x] Harvest manifests and provenance from an explicit existing Cargo build (`oven harvest`, `bake-loafs --harvest-dir`; the release closure's 120 registry package versions are governed by incan.pub records harvested from its own capture).
- [ ] Execute declared C/C++ and tool units only during publisher baking; the macOS tool-unit executor and generated-input projection refuse consumer execution, missing or substituted tools, undeclared reads, and missing, escaped, or extra outputs, while native-link execution and end-to-end corpus admission remain open.
- [ ] Preserve normal host procedural-macro compilation and expansion semantics.
- [x] Define the Rust `incan oven equivalence` gate over stable binding/role/path keys, raw bytes, the recorded deterministic setup, RFC 124 unit identity, and negative identity and integrity cases; the gate emits RFC 125 `attest` evidence only after every comparison succeeds.
- [ ] Run that gate over the complete pinned Incan and IncQL closures and admit the resulting per-binding attestations.

### Roles, tooling, and release

- [ ] Verify Rust test, documentation, example, benchmark, and caller roles with host/target separation.
- [ ] Verify warm-reusing IDE projections, target switching, and explicit editor macro policy.
- [ ] Verify that no Oven operation invokes Cargo and that the Cargo-project conversion is explicit.
- [ ] Retire the compatibility publisher's Cargo observation once incan.pub records govern the corpus, and the compiler-suite foundation's Cargo build once that family bakes through the Loaf-native route; the harvest wire contract, the registry authority and pin, and the policy engine stay.
- [ ] Publish the supported conformance envelope and update user-facing reference, adoption, and release documentation.

## Design decisions

- **Convention-first Rust authoring:** Rust is a direct built-in semantic facet. A conventional Rust source layout must not be restated merely to adopt Loaf. `[rust.source]` is used only for a mixed or nonstandard root, and compact Rust-specific deviations remain beneath `rust.*`; the complete compilation graph remains plan and lock state.
- **Named environment use:** RFC 117 supplies the standard `dev`, `test`, `lint`, and `docs` environments. Rust test roles select `test` by default and documentation roles select `docs` by default. A bake is not an environment: its target, carrier, profile, and feature closure remain explicit plan selections.
- **Proc-macro compatibility:** a proc macro is an ordinary Rust compiler feature. Oven compiles it for the build host and lets the selected `rustc` load and invoke it through the normal ABI, matching Cargo behavior. Oven improves graph/artifact reuse without caching individual expansions or changing macro execution semantics; stricter containment is governed-policy behavior only.
- **IDE selection, provenance, and warm reuse:** an editor uses one visible Oven inspection selection, defaulting to `dev` and the Loaf-selected or host-native target. Oven first reuses receipt-compatible pre-warmed Loaf/provider/artifact closures across equivalent clean worktrees and implementation slices, then derives a small local standard Rust project descriptor plus an Oven provenance sidecar. The descriptor is a compatibility export, not authority; it neither triggers every target build nor duplicates dependency output.
- **Editor-side macro execution:** an Oven-owned inspection session may expand procedural macros only under explicit observe, governed, or permissive policy and records session-level macro/toolchain/policy status separately from bake receipts. A third-party editor that consumes the compatibility descriptor is never presented as governed Oven execution.
- **Splitting a project's declarations across different build outputs is done by physically separating Loaves, not by tagging individual declarations:** this RFC's bake model already lets one compilation unit produce multiple `*.loaf` assets when different carriers are selected (see "C/C++ and linked libraries, carriers, and cross compilation"), but that is the same code packaged multiple ways, not different code per output. A project that genuinely needs different subsets of its own source in different outputs (for example, browser-facing code and server-facing code in one logical project) expresses that as separate Loaves under RFC 117's workspace model — the same idiom Rust uses (separate crates in a workspace) and Python uses (separate sub-packages), not a novel Incan mechanism. This RFC does not add a declaration-level tagging or attribute system for routing individual functions or modules into different carrier outputs from a single Loaf; RFC 117's existing workspace/member model already provides the separation, with a shared common Loaf as a dependency when code needs to be reused across the split. This closes the gap left by RFC 092's rejection using a mechanism this RFC already has, not a new one.
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
    type = "proc-macro"
    ```

    The sibling crate is a `loaf` dependency like every other dependency: there is no separate kind for Rust content, whether the Loaf is this project's own sibling or a package the registry adopted from crates.io. Target predicates *within* one compilation unit (ordinary `#[cfg(...)]` source-level conditionals) remain native Rust and need no manifest declaration at all; this RFC already commits to passing the selected target through so `rustc`'s own `cfg` evaluation works unmodified.
- **Governed-mode containment applies only to the compiler process that hosts procedural macros, and delegates to existing platform-native primitives; Oven does not implement its own sandboxing:** on Linux, containment uses Landlock or delegates to an established userspace sandboxing tool such as bubblewrap rather than hand-rolled namespace/seccomp wiring; on macOS, `sandbox-exec`/Seatbelt profiles; on Windows, AppContainer or Job Objects. These are security-audited, already-existing primitives; Oven integrates with them rather than authoring new OS-level containment code, the same "reuse existing infrastructure" reasoning behind this RFC's proc-macro and generator decisions elsewhere. Where a build host has no viable primitive for a selected policy, governed mode is refused outright rather than silently weakened or emulated, exactly as this RFC's reference text already requires ("if the host cannot enforce the selected policy without changing behavior, Oven denies the governed operation"). Observe mode's Cargo-equivalent behavior is unaffected by this choice: observe mode records effects without enforcing containment, so it has no platform-primitive dependency at all.
- **The native-mode conformance bar is the price of a real benefit, not friction for its own sake, and Cargo cannot provide it:** Oven-native's value over plain Cargo is not "compiles any single cold build faster" — a native bake still invokes the same `rustc` Cargo would. The concrete payoff is avoiding *redundant* rebuild work across environments: reusing a receipt-compatible pre-warmed Loaf/provider/artifact closure across equivalent clean worktrees, CI runners, and implementation slices (see "IDE selection, provenance, and warm reuse") gives a significant speed advantage over raw Cargo under warm circumstances, on top of the receipted, auditable, unified Incan+Rust project model Cargo cannot offer at all. Both benefits come from the Oven-owned graph, receipts, and content-addressed store, which is why Oven never runs Cargo: a project gets them by converting to `loaf.toml` once.
- **A third-party package becomes usable when its adopted Loaf bakes, not by a directive audit:** a package is usable from Oven once the registry has adopted it into a Loaf whose manifest follows "Rust facet grammar" and whose build facts cover the binding a consumer selects. Equivalence to the upstream build is a registry attestation (RFC 125), not a Cargo build on the consumer's side. Unknown required directives refuse at adoption rather than falling through to a directive interpreter. C/C++ compilers and declared tools execute only as their own selected publisher-side units; they never authorize `build.rs` in Oven.
- **Publisher work records declare work; assets declare products:** `[[rust.facts.link]]` and `[[rust.facts.tool]]` are arrays of typed work records. Their executable is identified by a stable name, immutable `sha256:` owner, owner-relative path and executable-byte `sha256:` digest. Link records bind a complete digested source closure, a non-empty list of sorted unique object records and a logical static library. Each object names a portable `.o` file, its `c`, `cpp` or `assembly` language and its ordered compiler arguments; it references at least one declared source and exactly one output equal to its own name. Every declared source is referenced, and source and object names share one namespace. Source paths may overlap, while source-to-object and object-to-object overlaps are refused. The Oven runs the declared compiler once per object, materializes the output reference beneath the product root, verifies exactly those objects, and writes the target-format indexed archive with its in-process archive writer. Dynamic libraries are refused until a record requires them. Tool records bind digested inputs and logical output contracts. Built archives, generated product bytes, product digests and producer receipts are admitted asset-side evidence, not fact-record fields. Link receipts bind each object's logical argv and digest plus the archive digest. Set-like lists are sorted and duplicate-free, omitted optional lists equal empty lists, logical names are unique in their namespace, and no absolute path, host spelling or capture order enters the record.
- **Loaves, not crates; adoption is the registry's (2026-10-06):** Oven has no `crate` dependency kind and never reads crates.io, a crate's `Cargo.toml`, or a `Cargo.lock`, and no Cargo runs on any Oven path. A third-party Rust package is adopted once per version by a Loaf registry (RFC 125) into a Loaf whose manifest follows "Rust facet grammar"; the registry hosts its sources and records its build facts per binding. That grammar is the contract: the registry is its one writer and Oven's resolver its one reader. The resolver's per-domain feature unification must reproduce the feature sets of the registry's recorded bindings, which the registry's existing facts test without running Cargo.
- **Build scripts are inert, and the manifest says what they said (2026-09-13):** earlier drafts planned to execute `build.rs` as a sandboxed host provider and interpret its directives. That design is superseded. A `build.rs` is retained only as source-inventory evidence and produces one warning; it is never a selected unit or execution input. Required compile-time flags, committed generated inputs, linked-library outputs, and generator products are represented by `cfg`, `out`, `link`, and `tool` facts. The registry records those facts when it adopts a package (amended 2026-10-06: by compiling and running the script itself under a declared binding, with no Cargo), but ordinary Oven planning, baking, caching, publication, and consumption never compile, execute, or interpret the script. Proc-macro host execution remains a separate supported contract.
- **No speculative naming for a future registry endpoint:** `incan.pub` is the registry for the foreseeable future. This RFC does not name, brand, or reserve a hypothetical second public endpoint; an unregistered domain speculatively mentioned in earlier drafts is removed rather than carried forward as a design question.
