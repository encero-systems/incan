# RFC 117: `loaf.toml` and Oven's language-neutral project model

- **Status:** Planned
- **Created:** 2026-08-04
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies)
    - RFC 015 (hatch-like tooling and project lifecycle CLI)
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 031 (Incan library system)
    - RFC 034 (`incan.pub` package registry)
    - RFC 073 (environment matrices and toolchain constraints)
    - RFC 076 (project mutation policy and recovery)
    - RFC 077 (workspace and multi-package projects)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 097 (Rust-hosted Incan caller)
    - RFC 104 (ambient runtime capabilities and receipts)
    - RFC 112 (crash-safe local publication and file coordination)
    - RFC 114 (compiled providers, SDK components, and package features)
    - RFC 116 (typed C ABI interop)
    - RFC 118 (Incan and Oven command-line surfaces)
    - RFC 119 (Oven-native Rust build facets and Cargo interoperation)
- **Issue:** [#1063](https://github.com/encero-systems/incan/issues/1063)
- **RFC PR:** —
- **Written against:** v0.5
- **Target scope:** v0.6 design and implementation planning; this is not a shipment claim.
- **Shipped in:** —

## Summary

This RFC replaces `incan.toml` with `loaf.toml` as the sole authored manifest for an Oven-managed project. A `loaf.toml` may describe one project, a rooted workspace, or a virtual workspace. Its package, target, lock, cache, and receipt model is language-neutral. Incan and Rust are the built-in authored source facets in v0.6; checked C interop remains under `[interop.c]`; later foreign integrations require their own explicit interop RFC rather than becoming ambient source languages.

`[project]` remains the familiar metadata table. `[workspace]` remains the explicit repository-coordination table. Dependencies use one typed graph whose unit kind and origin are explicit when the defaults are insufficient: Incan Loaf packages default to `incan.pub`, Rust crates default to crates.io, and foreign/system requirements use Oven-managed providers. Oven resolves authored intent into `Oven.lock`, then bakes the selected target into immutable, verified `*.loaf` assets and receipts. Cargo remains a deliberately explicit compatibility mode for repositories that have no `loaf.toml`; it is never inferred as part of a Loaf build.

This RFC also rehomes the authored configuration boundaries of existing environment, matrix, action, workspace, provider, and interop RFCs. It does not define the final command spelling or a general-purpose task shell.

## Core model

Read this RFC as thirteen foundations:

1. **One authored manifest:** `loaf.toml` is the only Incan/Oven project manifest. `incan.toml` is not read as a compatibility format.
2. **A familiar project table:** `[project]` owns package identity and publication metadata. The filename carries the Loaf vocabulary; the metadata table remains ordinary and legible.
3. **A workspace is an optional hierarchy:** `[workspace]` declares explicit members and shared policy. A root with both `[project]` and `[workspace]` is a rooted workspace; a root with only `[workspace]` is virtual. Members may form explicit structural child groups, but the root remains the sole authority for resolution, locking, registry trust, target policy, and receipts.
4. **The package graph is language-neutral:** every dependency is one logical requirement with a typed provider contract. The author does not choose a separate Incan-versus-Rust dependency table.
5. **Origins are configurable:** Loaf packages default to `incan.pub`; crates default to crates.io. A project may explicitly select registered private registries, Git, path, or other supported controlled sources.
6. **Provider semantics stay visible:** an `incan.pub` package contributes checked Incan semantic/provider facts, a Cargo ecosystem crate contributes Rust interop and implementation requirements, and a system/foreign provider contributes a verified target requirement. Oven must not flatten those differences or expose backend details as public Incan API.
7. **Core source facets are convention-first and unambiguous:** a conventional single-language project uses the ordinary `src/` layout. A mixed or nonstandard project declares non-overlapping `[incan.source]` and `[rust.source]` roots; `.incn` and `.rs` files never compete within one undiscriminated source root. C source inputs, headers, shims, and artifacts remain deliberate `[interop.c]` concerns. A Loaf never implicitly consumes a neighbouring `Cargo.toml`.
8. **Targets have three scopes:** a Loaf declares supported target/interface combinations; a workspace declares shared delivery policy; a bake invocation chooses one concrete target, carrier, profile, and feature closure that produces a target-bound `*.loaf` asset.
9. **Plans precede effects:** resolution, import, installation, or the presence of a file must not execute package code. Oven exposes a plan before baking and records the actual execution in a receipt.
10. **Named environments and actions are explicit Loaf intent:** the standard `dev`, `test`, `lint`, and `docs` environments always exist; authored environment and typed-action configuration moves out of `[tool.incan.*]` and into the Loaf model. Environments select named, reproducible command contexts; they do not silently change a production bake.
11. **Derived state is separate:** `Oven.lock`, immutable `*.loaf` assets, the artifact store, selected host toolchains, caches, staged outputs, generated code, and receipts are not authored Loaf configuration.
12. **Distribution assets are verifiable and future-reusable:** a `*.loaf` asset is immutable and target-bound. It identifies the selected project/facet, target, carrier, profile, resolved graph, payload digest, and receipt; those facts reserve the compatibility boundary through which a future `incan.pub` consumer can safely reuse a pre-warmed asset rather than rebuild it. Its wire or bundle format is intentionally deferred.
13. **Cargo compatibility is explicit:** a directory with only `Cargo.toml` may be run by Oven in Cargo-compatibility mode. A directory containing `loaf.toml` is a Loaf project; Cargo files there are ignored with a diagnostic unless the user explicitly invokes Cargo compatibility.

## Motivation

The current `incan.toml` correctly introduced project identity, dependency declarations, SDK selection, build configuration, workspaces, environment configuration, Rust dependency declarations, and Oven interop requirements. It now carries an assumption that no longer fits the intended architecture: that a project's identity is primarily Incan source and that Rust is a special external implementation substrate.

That assumption is constraining in both directions. An Incan-authored package may need a Rust crate, a C ABI library, a C++ shim, a target SDK, or a later JNI/Python carrier without ceasing to be one package. A Rust-authored project that elects to use Oven should be able to adopt the same package, target, receipt, and workspace model without being forced to rewrite its sources into Incan. Cargo must remain runnable where Cargo is explicitly authoritative, but Cargo's implicit build scripts, proc macros, target conventions, and workspace semantics must not become accidental Loaf semantics.

The same pressure exists at the repository root. A separate `Oven.toml` would distinguish package metadata from workspace orchestration, but introduces two canonical files and an ownership/discovery boundary without a demonstrated capability need. Cargo already demonstrates that one manifest can express a root package, a virtual workspace, or both. `loaf.toml` can do the same when its table scopes are explicit.

Oven is more than a package resolver. It must plan target-specific C ABI, JNI, Python, Rust-host, and future carrier builds; select and verify toolchains; materialize controlled environments; protect shared artifact stores; and describe exactly what happened. That requires a manifest that captures desired inputs, not hidden host discovery or arbitrary execution.

## Goals

- Replace `incan.toml` with `loaf.toml` wholesale before Incan 1.0.
- Preserve `[project]` as the familiar package-metadata surface.
- Preserve rooted and virtual workspace forms using optional `[workspace]`.
- Give Incan packages, Rust crates, and foreign/system providers one dependency grammar while retaining their distinct resolver and linkage contracts.
- Make `incan.pub` the default origin for Loaf packages and crates.io the default origin for Rust crates, while allowing registered alternative registries and explicit sources.
- Define safe discovery when `loaf.toml` and `Cargo.toml` coexist.
- Permit explicitly declared hierarchical sub-Loaves under one inherited root-workspace authority.
- Keep source-language choice out of the package identity while avoiding implicit Cargo participation.
- Define a target model that distinguishes platform target, deployable carrier, profile, and delivery policy.
- Distinguish authored intent (`loaf.toml`), resolved graph (`Oven.lock`), and immutable target-bound distribution assets (`*.loaf`) without making any of them hidden state.
- Make plans, declared effects, selected inputs, output artifacts, and receipts inspectable.
- Rehome environment/matrix/action configuration ownership to Oven without re-specifying the semantics already owned by RFCs 073 and 078.
- Rehome package-level C ABI configuration from `[oven.interop]` into `[interop]`, a direct semantic root that future binding kinds may share, while retaining RFC 116 as the owner of C safety semantics.
- Establish `[incan.source]` and `[rust.source]` as the built-in source-facet roots for mixed or nonstandard projects, while retaining convention-first simple projects.
- Reserve `[interop.*]` for future typed binding integrations without defining a plugin system, arbitrary foreign-tool execution, or a Python/JNI/Go/Ruby source model in v0.6.
- Keep locks, caches, credentials, selected SDK paths, staged artifacts, and generated output out of the authored manifest.
- Leave command spelling and CLI ergonomics for a dedicated follow-up RFC.

## Non-Goals

- Defining a separate `Oven.toml` root manifest.
- Preserving `incan.toml`, `incan.lock`, or `[tool.incan.*]` as legacy compatibility formats.
- Defining the final `oven` versus `incan` command-line binary, aliases, or command hierarchy.
- Reimplementing arbitrary Cargo behavior, including implicit `build.rs` execution, proc-macro execution policy, or Cargo workspace discovery, for Loaf projects.
- Defining crates.io's protocol, the full `incan.pub` registry protocol, registry hosting, or publishing UX.
- Defining C ABI safety rules, direct C++ ABI interop, JNI safety rules, Python extension safety rules, or a universal foreign-runtime type system.
- Defining pluggable interop registration, installation, discovery, or execution. Future binding kinds require their own RFC and implementation work.
- Defining remote execution, distributed builds, or a general-purpose shell/task runner.
- Defining the `*.loaf` wire or bundle format, registry transport, or archive layout; a dedicated artifact-format RFC owns those details.
- Allowing packages to run arbitrary installation, import, or lifecycle hooks.
- Giving cold, warm, and hot Loaves normative compiler or package semantics in this RFC.

## Prospective supersession of closed RFC decisions

Closed RFCs remain historical records and are not retroactively edited. This RFC explicitly supersedes the following decisions for implementations that adopt the Loaf/Oven project model:

- **RFC 013 and RFC 031:** separate authored `[dependencies]` and `[rust-dependencies]` tables, and the assumption that a consumer's dependency graph is authored as Cargo wiring. `loaf.toml` has one typed dependency graph; provider-specific Rust requirements remain visible facts rather than a second authoring domain.
- **RFC 015:** `incan.toml` as the project manifest, nearest-manifest root discovery, project generation that writes `incan.toml`, and `[tool.incan.envs]` as the environment-configuration home. `loaf.toml` is the sole authored manifest, while environment semantics remain owned by their dedicated RFCs.
- **RFC 020:** `incan.lock` as the generated resolution identity. The reproducibility and locked/offline requirements remain; their generated artifact is `Oven.lock`.
- **RFC 077:** a flat workspace that forbids nested workspaces. Explicit membership, rooted and virtual workspace forms, and one shared resolution boundary remain; a member may now declare explicit sub-Loaves within a single inherited root-workspace authority.
- **RFC 112:** the public lockfile path used by its publication examples. Crash-safe publication and coordination requirements remain unchanged, but the published lockfile is `Oven.lock` and its companion coordination identity follows that name.
- **RFC 114:** references to the canonical `incan.lock` graph. Its distinction between public Loaf features and provider-specific implementation features remains unchanged; the resolved graph is recorded in `Oven.lock`.

This RFC does not supersede the retained semantic contracts in those RFCs: reproducibility, explicit workspace membership, crash-safe publication, checked library surfaces, and public package-feature meaning remain requirements. It changes the authored manifest, authority hierarchy, dependency presentation, and derived-state names through which those contracts are implemented.

## Terminology and mental model

The user-facing contract is intentionally small: authors write `loaf.toml`; Oven plans and bakes it. The surrounding bread vocabulary is a guide-level model and an Oven inspection affordance, not a second vocabulary users must configure.

```text
loaf.toml + declared sources + declared inputs
  -> Doughball (internal source-and-input mass)
  -> Oven shapes one or more Loaves (resolved logical build units)
  -> Oven bakes a selected Loaf for a selected target
  -> immutable target-bound *.loaf asset + execution receipt
```

- A **project** is the independently named and versioned unit described by `[project]`.
- A **workspace** is the explicit repository coordination boundary described by `[workspace]`; it is not itself a compiled package unless it also has `[project]`.
- A **`loaves/` group** is the repository convention for non-root workspace members. It may be nested, for example `loaves/stdlib/web/`; its path is not a dependency, publication, or Rust-crate identity.
- A **Doughball** is Oven's internal representation of the selected sources, declared dependencies, features, interop inputs, and policy before target shaping.
- A **Loaf** is the resolved logical build unit Oven obtains by shaping a Doughball for a selected package facet and target-compatible plan.
- A **`*.loaf` asset** is the immutable, verified, target-bound build and distribution asset emitted by a bake of a loaf-shaped carrier. It carries the selected Loaf identity, target/carrier/profile, resolved-graph identity, payload digest, and receipt identity. Its archive or wire format is outside this RFC.
- To **bake** is the operation that executes a resolved plan for a selected target/carrier/profile and produces a build artifact plus a receipt. "Bake" is a verb, not a required artifact type. The produced artifact's packaging depends on the selected carrier's shape (see below): a loaf-shaped carrier yields a `*.loaf` asset, a terminal carrier yields a plain build artifact. Both shapes still go through the same plan, policy decision, and receipt; only downstream-reuse packaging differs.
- **Cold**, **warm**, and **hot** Loaves may be shown by Oven's internal inspection and cache-status UX. Their precise state semantics are intentionally not specified here.
- A **carrier** is the target deliverable form: for example, a static library, framework, JNI shared library, Python wheel, Rust-host caller projection, executable, or another declared artifact family. Every carrier has a **shape**: **loaf-shaped** carriers (static library, framework, JNI shared library, Python wheel, Rust-host caller projection) produce a `*.loaf` asset meant for another Oven consumer to depend on or reuse; the **terminal** carrier `executable` produces a plain build artifact meant to be run or deployed directly, never depended on as a package. A carrier's shape is fixed by whichever RFC defines that carrier kind, not chosen per invocation.

## Guide-level explanation

### A single project

An ordinary project needs only `loaf.toml`:

```toml
[project]
name = "weather_service"
version = "0.6.0"
description = "A service that happens to use Incan and Rust sources"
requires-incan = ">=0.6,<0.7"

[dependencies]
web = { loaf = "stdlib-web", version = "^0.6" }
serde = { crate = "serde", version = "1", features = ["derive"] }

[incan.source]
root = "sources/incan"

[rust.source]
root = "sources/rust"
```

`web` defaults to `incan.pub`; `serde` defaults to crates.io. Neither the project identity nor the command to bake it changes because its implementation contains Incan, Rust, or both. The public package feature graph remains an Incan/Loaf contract. A requested Cargo feature is a typed implementation request for `serde`; it does not automatically become a public feature of `weather_service`.

This example is mixed, so it names separate roots. An Incan-only project conventionally uses `src/*.incn`; a Rust-only project conventionally uses `src/lib.rs` or `src/main.rs`. Neither needs a source table. A mixed project must declare separate roots rather than placing `main.incn` and `main.rs` beside one another.

### A workspace of Loaves

The root `loaf.toml` can be a project and a workspace:

```toml
[project]
name = "incan"
version = "0.6.0"

[workspace]
members = [
  "loaves/compiler/syntax",
  "loaves/compiler/semantics",
  "loaves/stdlib/web",
  "loaves/stdlib/io",
]
default-members = [".", "loaves/compiler/semantics"]

[workspace.delivery.mobile]
members = ["loaves/stdlib/web"]
targets = ["aarch64-apple-ios", "aarch64-linux-android"]
profile = "release"
```

A virtual workspace omits `[project]` and retains `[workspace]`. Every member has its own `loaf.toml`; no member is inferred merely because it lives under `loaves/` or a nested group beneath it. A Rust-only member remains a Rust crate in its own source facet; `loaves/` is the repository's package hierarchy, not a replacement name for Rust compilation units.

The directory name is a repository convention, not an identity rule. A member can live anywhere inside the workspace root if it is explicitly included by `members`.

### Nested `loaves/` groups

A workspace can group members recursively without flattening its repository layout. A group directory does not itself need a manifest:

```text
incan/
├── loaf.toml                         # root authority and Oven.lock
└── loaves/
    ├── compiler/
    │   ├── syntax/loaf.toml
    │   └── semantics/loaf.toml
    └── stdlib/
        ├── io/loaf.toml
        └── web/loaf.toml
```

If a group itself needs a local member-selection boundary or narrower local requirements, it may declare a structural parent `loaf.toml` with explicit children. That parent inherits the root's resolved graph, lock, registry trust, target/delivery policy, and receipt boundary. It cannot create a second `Oven.lock`, rebind a registry, widen target policy, or form a second publication authority.

Membership hierarchy is not a dependency graph. Parent/child placement, a shared workspace, and a selected closure create no runtime, linkage, or publication dependency between packages; those relationships exist only through declared typed dependencies. For example, `incql-core`, `incql-db`, and `incql-datafusion` can remain independently versioned packages whether or not one is structurally nested below another; any dependency between them is explicit in the consuming package's dependency table.

This hierarchy also permits efficient cache-status inspection: Oven can compare a receipt-bound aggregate for a parent before it descends into a child's declared closure. The exact cold/warm/hot definitions remain an Oven inspection concern rather than this RFC's package contract.

### Core source facets and deliberate interop

The source-root contract is intentionally small. A conventional single-language project requires no configuration. A mixed root names the two built-in source facets directly:

```toml
[incan.source]
root = "sources/incan"

[rust.source]
root = "sources/rust"
```

The roots must not overlap. Generated output is not an authored source root. The root's package/target authority remains in `loaf.toml`; neither source facet can select a different lock, registry, target, policy, or carrier.

C is not a peer top-level source namespace. When a C integration needs authored shim source, it remains behind the deliberate C interop contract:

```toml
[interop.c.source]
root = "sources/c"
```

`[interop.c]` also owns C headers, ABI declarations, checked shims, linker artifacts, selected toolchain/provider facts, and their receipts. `[interop.python]`, `[interop.jni]`, `[interop.go]`, and similar names are reserved for later concrete interop RFCs; v0.6 does not define their schemas, plugin installation, or arbitrary foreign-language execution.

### Registries without ambient trust

Oven has built-in defaults for the public ecosystems:

```text
Loaf package  -> incan.pub
Rust crate    -> crates.io
```

Users or organizations explicitly register additional registries in Oven-controlled user or organization configuration. Registry configuration identifies the registry kind, endpoint/index, trust policy, and a reference to credentials held outside the project.

```toml
# Illustrative Oven user/organization configuration, not loaf.toml.
[registries.encero]
kind = "loaf"
index = "https://packages.encero.dev/index"
trust = "require-signature"

[registries.internal-cargo]
kind = "crate"
index = "sparse+https://cargo.encero.dev/index"
trust = "organization"
```

The project can select a registered source and the workspace can allow-list source identities:

```toml
[dependencies]
web = { loaf = "web", registry = "encero" }
serde = { crate = "serde", registry = "internal-cargo" }

[workspace.registries]
allow = ["encero", "internal-cargo"]
```

The registry alias is convenience, not the security identity. `Oven.lock` records the canonical registry endpoint and protocol, exact package identity, immutable digest, and observed signature/trust result. Credentials never enter `loaf.toml` or the lock. A project cannot silently register, rebind, or trust a registry on a developer's machine.

### Declaring C ABI and future carriers

A package declares interface intent and physical input requirements. Oven resolves the concrete target toolchain, SDK, artifacts, and deployment placement when it plans a bake.

```toml
[project]
name = "vision_runtime"
version = "0.6.0"

[interop.c]
headers = ["interop/include/vision.h"]
requires = ["vision-runtime"]

[targets.ios]
platforms = ["aarch64-apple-ios"]
carrier = "framework"

[targets.android]
platforms = ["aarch64-linux-android"]
carrier = "jni-library"
```

This example is intentionally schematic. `[interop]` is Loaf-owned requirement data; `[interop.c]` is the direct successor to RFC 116's C envelope. RFC 116 continues to define C declarations, ownership, buffers, verification, and checked shims. A future JNI or Python RFC defines the corresponding language/runtime safety contract. They all reuse the same package-envelope distinctions: declared requirement, selected provider/toolchain, target-compatible carrier, lock identity, plan, `*.loaf` asset, and receipt.

### Baking and inspecting

Before Oven performs external work, it can show a plan containing selected members, dependencies, providers, target, carrier, profile, capabilities, declared inputs/outputs, and expected effect classes. Baking executes only that plan and emits a target-bound build artifact plus a receipt — a `*.loaf` asset for a loaf-shaped carrier, or a plain build artifact for a terminal carrier such as `executable`.

The exact CLI spelling is intentionally outside this RFC. The semantic operation is:

```text
select project/workspace member(s)
select target, carrier, profile, and features
resolve the locked provider graph
show/approve the plan when policy requires it
bake
inspect artifacts and receipt
```

### Cargo compatibility is a different mode

A repository containing only `Cargo.toml` remains runnable by Oven in explicitly selected Cargo-compatibility mode. Cargo remains authoritative for that operation and its own build-script/proc-macro side effects are Cargo-mode behavior.

If both files are present, Oven must continue as a Loaf project and issue a warning equivalent to:

```text
Found loaf.toml and Cargo.toml. loaf.toml is authoritative; Cargo configuration
was ignored. Use an explicit Cargo-compatibility operation to run Cargo semantics.
```

Oven must not parse, merge, or infer dependency, feature, source, workspace, build-script, or target policy from the adjacent Cargo manifest.

## Reference-level explanation

### Manifest discovery and shapes

1. `loaf.toml` is the canonical manifest filename. Project-aware discovery walks upward from the working directory or source path to the nearest directory containing it.
2. A manifest containing `[project]` and no `[workspace]` describes one project.
3. A manifest containing `[project]` and `[workspace]` describes a rooted workspace. The root project is a member without requiring `"."` in `members`.
4. A manifest containing `[workspace]` and no `[project]` describes a virtual workspace. It must declare at least one resolved member.
5. Workspace membership is explicit. The root's selected closure is formed by recursively expanding each selected member's explicit `members` declaration, subject to `exclude`; a path dependency does not imply membership.
6. A member may declare explicit child members, producing hierarchical sub-Loaves. Each non-root member has exactly one direct parent in the selected closure.
7. Membership and parent/child hierarchy are never dependency, linkage, or publication edges. Those edges arise only from declared typed dependency requirements; a path dependency does not imply membership, and membership does not imply a dependency.
8. The root workspace is the sole authority for the resolved graph, `Oven.lock`, registry trust, workspace delivery policy, and receipt boundary. Child workspace declarations may add local membership and narrow local requirements, but may not create a lock, rebind registries, weaken trust, broaden target policy, or establish an independent publication authority.
9. A manifest containing neither `[project]` nor `[workspace]` is invalid.
10. `incan.toml` is not a project manifest after this RFC. A directory that contains it but no `loaf.toml` must receive a targeted diagnostic rather than legacy parsing.
11. If a `loaf.toml` project also contains `Cargo.toml`, Oven must warn and ignore Cargo configuration. The diagnostic must name the ignored file and explain explicit Cargo-compatibility selection.

### Project metadata and source domains

`[project]` retains the project identity and publication fields from RFC 015: name, version, description, authors, maintainers, license, license files, readme, homepage, repository, documentation, issues, keywords, classifiers, toolchain constraint, privacy, entry points, and public features.

`requires-incan` remains an enforceable project compatibility requirement under RFC 073. It is a language/toolchain compatibility fact, not a claim that all implementation sources must be Incan.

An Incan-only or Rust-only project uses its conventional `src/` layout without a source declaration. A project that contains both built-in source languages, uses nonstandard roots, or needs an authored C shim must declare the relevant non-overlapping root: `[incan.source]`, `[rust.source]`, or `[interop.c.source]`. A Loaf must diagnose `.incn` and `.rs` files in one undiscriminated root; it must not apply filename precedence. Generated inputs remain declared provider outputs rather than source roots.

When a Loaf project contains Rust source, Oven determines its compilation/linkage path from the Loaf plan and declared crate/provider requirements. The presence of `Cargo.toml`, `build.rs`, or another Cargo convention does not grant that file execution or planning authority.

### Rust source facet

A Rust-bearing Loaf has a bounded, inspectable Rust facet. `[rust.source]` declares its root only when convention cannot. Rust-specific exceptions live beneath `rust.*`, not in a generic source table. The planner must know, either through the conventional default or an explicit declaration, every compiled crate's package name, crate root, edition, crate type, entry point where applicable, enabled feature set, and linkage/dependency requirements. A conventional `src/lib.rs` or `src/main.rs` may supply a default root only when it yields one unambiguous crate; multiple crates, nonstandard roots, nondefault crate names, nondefault editions, additional crate types, binary entry points, or feature mapping require explicit facet data.

The root spelling is settled; the compact `rust.*` exception fields remain RFC 119 work. No Rust compilation plan may be inferred from Cargo metadata. `Oven.lock` and the receipt must record the selected Rust facet, compiler/toolchain identity, crate dependency closure, feature choices, and all provider-produced inputs that affect the resulting `*.loaf` asset.

The existence of `build.rs` must never execute it. A Loaf may support its intent only through an explicitly selected Oven provider or typed action whose inputs, outputs, capabilities, target/host role, policy decision, and receipt effects are declared and inspectable. Likewise, a procedural macro must be an explicitly resolved compiler-time provider with a recorded host identity and execution policy; an unsupported macro is a diagnostic, not a fallback to Cargo. Cargo compatibility remains the only mode in which Cargo itself is authoritative for these behaviors.

RFC 119 owns the detailed native-Rust facet grammar, crate-provider graph, direct-`rustc` planner, build-time provider envelope, Rust test/IDE projections, and explicit Cargo interoperation. This RFC retains the one-manifest, one-lock, one-workspace-authority boundary those facilities must obey.

### Typed dependencies and features

Every dependency requirement has a **unit kind** and an **origin**.

| Unit kind    | Default origin                      | Contributes                                                                 | Does not become                                 |
| ------------ | ----------------------------------- | --------------------------------------------------------------------------- | ----------------------------------------------- |
| `loaf`       | `incan.pub`                         | checked Incan semantic/provider facts and public package features           | a Cargo crate or generated Rust source contract |
| `crate`      | crates.io                           | Rust interop metadata and backend implementation/linkage requirements       | a public Incan feature namespace by accident    |
| `provider`   | Oven-managed system/foreign catalog | target-specific library, SDK, toolchain, runtime, or deployment requirement | an ambient host discovery result                |

An entry may override its default origin through a registered registry, Git, path, or another supported source form. The resolved provider contract determines the legal source forms; a Loaf registry must not be accepted as a Cargo registry merely because it has a URL.

Project features remain additive public Loaf features as defined by RFC 114. They may enable optional Loaf dependencies and request public features from Loaf dependencies. A project may explicitly request a provider-specific crate feature as part of a crate dependency requirement, but that request is not exposed as a public project feature unless the project explicitly maps it through its public feature definition. Cargo feature names, generated crate names, and backend module paths remain implementation details.

Workspace inheritance remains explicit. A workspace may own a dependency identity and members may opt into it. A member may refine only the usage properties RFC 114 and the selected dependency kind allow; it may not silently replace the source, registry, package identity, or trust policy selected by the workspace.

### Registry registration and trust

1. Oven provides the built-in origins `incan.pub` for `loaf` dependencies and crates.io for `crate` dependencies.
2. Additional registries are registered through Oven-controlled user or organization configuration, never by an unreviewed dependency or an implicit network lookup.
3. A registry registration must declare a stable identity, unit kind/protocol, endpoint/index, and trust policy. Credentials are referenced from secure user/organization storage rather than copied into project files.
4. A Loaf or workspace may allow-list registered registry identities. It may select an allowed alias but may not define credentials, rebind an alias, or weaken a user/organization trust policy.
5. The lock records canonical registry identity, protocol, exact resolved package/crate/provider identity, version, immutable digest, source reference, and signature/trust outcome. An alias alone is insufficient for reproducibility or audit.
6. Resolution in locked or offline mode must not query an unrecorded registry, substitute an ambient local package, or accept a source with different trust facts.

RFC 034 remains the protocol and publication authority for `incan.pub`; this RFC defines how `incan.pub` participates in the generic Loaf graph. A future Cargo-registry adapter must respect Cargo registry identity and integrity semantics without making Cargo the authority for a Loaf bake.

### Targets, carriers, profiles, and delivery policy

Target selection has three distinct levels:

| Level           | Responsibility                                                                                         | Constraint rule                                              |
| --------------- | ------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------ |
| Loaf            | Declares supported platform/interface/carrier combinations and package requirements                    | Defines the maximum compatibility surface the package claims |
| Workspace       | Declares named delivery policy, allowed targets, selected members, shared provider rules, and defaults | May narrow a member's support; must not broaden it           |
| Bake invocation | Selects the concrete target, carrier, profile, feature closure, and optional delivery policy           | Must satisfy both workspace policy and every selected Loaf   |

The resolved plan must distinguish:

- the **build host** on which Oven executes;
- the **platform target** such as `aarch64-apple-ios`;
- the **carrier** such as framework, static library, JNI shared library, Python wheel, executable, or Rust-host caller projection;
- the **profile** such as development or release;
- the enabled public features and selected provider requirements; and
- the destination/deployment policy when relevant.

An explicit invocation has the highest selection precedence but cannot override package support or workspace policy. A workspace delivery policy may narrow a project choice but cannot make a package claim an unsupported target. An unqualified local bake may choose a deterministic host-development target and profile, and the receipt must record that choice. Publication or deployment must require an explicit target/carrier or named delivery policy; it must not silently publish a host-default artifact.

### Carrier shape and bake output

A carrier's **shape** determines what packaging, if any, a bake of it produces:

| Carrier                     | Shape       | Bake output                                                 |
| ---------------------------- | ----------- | -------------------------------------------------------------- |
| static library                | loaf-shaped | `*.loaf` asset, verified and reusable by another Oven bake     |
| framework                     | loaf-shaped | `*.loaf` asset, verified and reusable by another Oven bake     |
| JNI shared library             | loaf-shaped | `*.loaf` asset, verified and reusable by another Oven bake     |
| Python wheel                  | loaf-shaped | `*.loaf` asset, verified and reusable by another Oven bake     |
| Rust-host caller projection    | loaf-shaped | `*.loaf` asset, verified and reusable by another Oven bake     |
| executable                    | terminal    | plain build artifact, no `.loaf` packaging                     |

A loaf-shaped carrier's output is meant for another Oven consumer to depend on or reuse: it is the shape a package published for downstream consumption needs. A terminal carrier's output is meant to be run or deployed directly and is never depended on as a package, so packaging it as a reusable `*.loaf` would carry no meaning — nothing will ever declare a dependency on it. Both shapes go through the same plan, policy decision, and receipt; shape changes packaging for downstream reuse, not plan rigor, policy gating, or audit requirements. A future carrier RFC declares its own carrier's shape alongside its other properties; shape is fixed per carrier kind, not selectable per invocation.

### Plans, effects, extensions, and receipts

Oven uses the following boundary:

```text
authored Loaf configuration
  -> resolution
  -> inspectable plan
  -> policy/capability decision
  -> bake
  -> artifacts and receipt
```

The resolver may fetch, verify, and stage immutable declared inputs according to the selected mode, but it must not execute package-provided code merely to resolve a dependency, inspect an import, discover a source file, or find a manifest.

Every execution edge in a bake must belong to one of these categories:

1. **Oven-managed provider operation:** a known provider kind performs typed work, such as compiling declared Rust sources, verifying C headers, compiling a declared C/C++ shim, or materializing a selected SDK component.
2. **Declared generator operation:** a generator is a typed Incan (or `rust::`-interop) function with typed inputs, outputs, declaring-package identity, target constraints, capability requirements, and generated-output ownership. It may not overwrite authored source through an ordinary bake.
3. **Explicit workflow action:** an action from RFC 078 is selected by the user or an authorized tool and is evaluated under RFC 076 policy before execution.

An extension must have a versioned, typed schema and a known provider/action identity. Unknown extension kinds are invalid; they are not a generic `[tool.*]` escape hatch. A provider or action may advertise a required capability, network requirement, external executable, or mutation class, but it must not gain authority merely by being selected as a dependency.

Before executing a nontrivial plan, Oven must be able to inspect the selected members, dependency/provider identities, target/carrier/profile, toolchains, declared inputs and outputs, execution categories, capabilities, mutation classes, and expected receipt facts. Policy may require approval for a plan according to RFC 076. After execution, the receipt must identify the actual provider/toolchain selections, input identities, output artifacts, declared and observed effects, and deviations or denials.

RFC 104 owns capability-enforcement and receipt semantics. RFC 112 owns crash-safe publication and file coordination. This RFC requires their identities to be included in the Loaf/Oven plan rather than recreated through hidden backend behavior.

### Environments and actions

The following authored concepts move into `loaf.toml` under Oven ownership. Their table roots are intentionally typed and reserved; arbitrary `[tool.*]` namespaces do not carry forward:

| Concept                                       | Semantic owner | Oven responsibility                                                            |
| --------------------------------------------- | -------------- | ------------------------------------------------------------------------------ |
| project and environment toolchain constraints | RFC 073        | resolve and enforce compatible toolchains                                      |
| named environments and matrices               | RFC 073        | materialize the resolved environment and expand matrix cells deterministically |
| typed workflow actions                        | RFC 078        | resolve scope, show plan, policy-gate, and execute explicit actions            |
| mutation policy                               | RFC 076        | evaluate receiver authority over planned changes/effects                       |

RFC 015 is implemented, so RFC 117 supersedes its `incan.toml` discovery and `[tool.incan.envs]` configuration placement. RFCs 073, 076, and 078 are still Draft and must be amended to name the new Loaf tables and terminology rather than preserving legacy configuration compatibility.

The initial table roots are `[envs]`, `[actions]`, `[policy]`, `[mixes]`, and `[templates]`. Their dedicated RFCs own the detailed schema and semantics. No `[oven.*]` prefix is needed: a `loaf.toml` is already Oven's authored project document, and roots should name their semantic domain instead. They must remain explicit, scoped, inspectable, and free of implicit lifecycle execution. RFC 118 owns whether users invoke those semantics through `incan`, `oven`, aliases, or another command arrangement.

### Standard environments

Every Loaf has the standard named environments `dev`, `test`, `lint`, and `docs`, even when no `[envs.*]` tables are authored. `dev` is the local host-development context. `test`, `lint`, and `docs` extend it conceptually and add only the roles and dependencies selected by their operation. A project adds a table only to declare an addition or override:

```toml
[envs.test]
extends = "dev"

[envs.test.dependencies]
assert_cmd = { crate = "assert_cmd", version = "2" }

[envs.docs]
extends = "dev"

[envs.docs.dependencies]
mdbook = { crate = "mdbook", version = "0.4" }
```

An environment is a named, reproducible command context. It may select extra development tools and project features, but it must not silently alter the selected target, carrier, profile, provider closure, or release dependency closure of an ordinary bake. `bake` is therefore not a standard environment: a bake records its target, carrier, profile, and feature selection directly in the plan and receipt. Dependencies needed to compile or execute a provider belong to that provider; target-specific dependencies belong to the target. Environment inheritance must diagnose conflicting bindings in the selected closure rather than silently choose one.

Project scaffolding must generate a valid, minimal `loaf.toml` and include the four standard environments as commented examples. The comments must explain that the defaults already exist and show where test, lint, and documentation dependencies belong; they must not materialize active configuration that merely duplicates the defaults.

### Interop package envelope

RFC 116's current `[oven.interop]` shape is package-owned requirement data, not a record of a host toolchain already selected. Under this RFC it moves to the Loaf-level `[interop]` namespace: C declarations use `[interop.c]`. The file name changes from `incan.toml` to `loaf.toml`; the ownership boundary does not.

The interop envelope must be binding-kind neutral. It represents declared headers, shims, foreign artifacts, toolchain/SDK provider requirements, target variants, carrier requirements, lock identities, plan inputs, generated verification records, artifact staging, and receipts. It does not make C pointer types, C ownership, JNI handles, Python reference counts, or another runtime's safety rules universal. Each binding-kind RFC continues to define its own source vocabulary and checking rules. Loaf-facing configuration uses precise terms such as `interop.c`, `host`, `target`, and declared linker artifact form; it does not introduce a catch-all `native` configuration namespace. Existing RFC 116 source syntax remains historical until a future language-level proposal changes it.

### Locks and derived state

`Oven.lock` is the canonical generated resolution state for a project or workspace. A workspace has one root lock that represents every member's selected dependency/provider graph and target-relevant resolution facts. `incan.lock` is not read after this RFC.

The lock must include every semantic or physical input whose change can alter checking, linking, target compatibility, generated output, or the baked artifact closure. It must not contain credentials, mutable cache paths, arbitrary environment values, or unreviewed host discovery output.

`*.loaf` is the immutable, target-bound distribution/build asset derived from the lock and the selected bake plan for a loaf-shaped carrier. It must identify its project/facet, target, carrier, profile, resolved graph identity, payload digest, and receipt identity; it is not an editable project manifest or a replacement lockfile. A bake of a terminal carrier such as `executable` instead produces a plain build artifact identified by the same lock and receipt, without `.loaf` packaging (see "Carrier shape and bake output"). The artifact store, provider payload store, staged deployment files, generated Rust, target intermediates, and receipts are separate from the lock. They are inspectable and may be content-addressed, but they are not user-authored package configuration or semantic authority.

### Future registry-supplied warm reuse

The long-term advantage of a package published as a Loaf is that `incan.pub` can offer a verified, target-compatible pre-warmed closure rather than only source material. A consumer selects such a closure only when its package identity, trust facts, resolved features and graph, target, carrier, profile, toolchain/ABI, native-link requirements, provider receipts, and policy constraints are compatible with the selected plan. On a match, Oven verifies and materializes the existing assets; it does not repeat the package or dependency compilation merely because the consumer is a different project, clean worktree, or implementation slice. The local store may retain the result under the same receipt-compatible identity.

This is not a promise that one published binary works on every machine, nor that every `incan.pub` package ships a pre-warmed closure for every selection. A mismatch is a normal targeted miss: Oven resolves the checked source/provider closure and builds only the missing compatible units. Cargo crates remain valid ecosystem inputs, but they do not gain this Loaf-native distribution advantage merely by being consumed. This RFC does not require registry-supplied warm closures in v0.6; it requires the identity, trust, receipt, and target model that lets a later registry/artifact protocol provide them without reopening package authority.

## Design details

### Manifest ownership map

`loaf.toml` is the authored envelope. Existing RFCs continue to own the meaning of their respective content:

```text
loaf.toml
├── [project]                 RFC 117
├── [workspace]               RFC 117; prospectively supersedes RFC 077's flat-workspace restriction
├── dependencies/features     RFC 117 + RFC 114
├── registry selection        RFC 117 + RFC 034
├── [envs]                    RFC 073, rehomed by RFC 117
├── [actions]                 RFC 078, rehomed by RFC 117
├── [policy]                  RFC 076, rehomed by RFC 117
├── [mixes]                   RFC 075, governed by RFC 076
├── [templates]               RFC 074, governed by RFC 076
├── [incan.source]            built-in Incan source facet
├── [rust.source] / [rust.*]  built-in Rust source facet; RFC 119 owns exceptions
├── [interop.c]               RFC 116's checked C interop envelope
├── [interop.*]               reserved for later concrete binding RFCs; no v0.6 plugin system
├── Oven.lock                 resolved graph, RFCs 020, 104, 112, and 114
└── *.loaf + receipt          immutable target-bound artifact, RFC 117; wire format deferred
```

This map does not make RFC 117 the owner of every detailed sub-schema. It establishes one manifest root and prevents every feature family from inventing its own root file, discovery rule, or source-language split.

### Source language and authored Rust

The Loaf package model is language-neutral in package and build authority; it does not claim that every language is an equivalent v0.6 source facet or relax Incan's authoring policy. Incan products and dogfooding projects should remain Incan-authored and use `rust::` interop for existing Rust capabilities. New authored Rust still requires a demonstrated Incan limitation and a tracked removal path.

That policy is distinct from the manifest contract. Oven must be capable of baking a Loaf whose implementation is Rust, Incan, or mixed because the build system cannot make a package's identity depend on the language mix. Project governance can constrain which sources its maintainers choose to author.

The Rust facet is the language-neutral planner contract for Rust-authored or mixed Loaves. It does not license authored Rust in Incan products without the demonstrated limitation and tracked removal path required above.

### Cargo compatibility boundary

Cargo compatibility has two valid states:

1. **Cargo project:** a directory has `Cargo.toml` and no `loaf.toml`. An explicitly selected Oven Cargo-compatibility operation delegates to Cargo. Cargo retains authority over its manifest and side effects.
2. **Loaf project:** a directory has `loaf.toml`, with or without `Cargo.toml`. Oven uses only the Loaf graph. Cargo is not an implicit source of build policy.

There is no mixed automatic state. In particular, Oven must not merge dependency tables, inherit Cargo workspace membership, execute a discovered `build.rs`, or allow a Cargo feature to silently change a Loaf's public feature API.

### Inspectability and diagnostics

Oven must make the following facts inspectable without reading generated Rust, private compiler IR, a cache directory, or an implementation-specific backend manifest:

- discovered manifest root, project/workspace shape, members, and selected scope;
- source roots and ignored adjacent `Cargo.toml` files;
- each dependency's unit kind, configured/default origin, resolved canonical source, integrity/trust facts, and public feature closure;
- registry registrations and allow-list/policy decision, with credentials redacted;
- selected target, build host, carrier, profile, delivery policy, toolchain/SDK/provider identities, and compatibility constraints;
- planned execution categories, inputs, outputs, capabilities, mutation classes, policy outcome, and expected receipts;
- actual baked artifacts, receipt identities, cache status, and declared-versus-observed effects.

For each `*.loaf` asset, inspection must report the project/facet, target, carrier, profile, resolved graph identity, payload digest, signature/trust facts where applicable, and receipt identity without requiring an author to inspect a cache path or container implementation.

Diagnostics must distinguish configuration errors, unsupported target/carrier combinations, unmet package requirements, unavailable registered registries, trust failures, stale or incompatible locks, missing provider/toolchain capabilities, explicit Cargo compatibility, and accidental Cargo coexistence. They must explain which declaration or policy constraint led to the outcome.

## Compatibility and migration

This RFC is intentionally breaking and should land before Incan 1.0.

- `incan.toml` is replaced wholesale with `loaf.toml`.
- `incan.lock` is replaced with `Oven.lock`.
- Existing implemented project/workspace/environment code must be updated to discover and write the new schema; it must not retain a legacy `incan.toml` parser.
- Existing Draft RFCs that refer to `[tool.incan.*]` must be amended before their implementation is scheduled.
- Existing project authors must explicitly adopt the Loaf contract; a raw rename is valid only when the resulting tables satisfy the new schema.
- Cargo-only repositories require no adoption to be runnable in explicit Cargo-compatibility mode.

The tool should emit a targeted diagnostic when it finds `incan.toml` without `loaf.toml`, but it must not silently interpret the old file. This keeps the authority transition explicit and avoids supporting every interaction between old Incan configuration, Cargo configuration, and the new Oven model.

## Alternatives considered

### Add `Oven.toml` beside `loaf.toml`

Rejected for now. A separate root file makes the package-versus-workspace vocabulary literal, but it creates two canonical documents, discovery precedence, and synchronization questions. A single `loaf.toml` with explicit `[project]` and `[workspace]` scopes supports single projects, rooted workspaces, and virtual workspaces without losing capability. A future RFC may introduce a separate file only if independently versioned workspace policy demonstrates a real need.

### Keep `incan.toml` and add Loaf concepts inside it

Rejected. The filename embeds a source-language assumption that the package model is deliberately outgrowing. Keeping it would make Rust-authored and mixed projects second-class by name even when the model is technically neutral.

### Keep separate Incan and Rust dependency tables

Rejected. The package graph must be one user-facing graph. Resolver, feature, semantic-provider, and linkage differences remain typed provider facts, but should not force authors to split their logical dependencies by implementation language.

### Treat all registries as equivalent URLs

Rejected. `incan.pub` packages, Cargo ecosystem crates, and foreign/system catalogs have different package formats, integrity, semantic metadata, and execution/linkage contracts. Registry identity requires a kind/protocol and trust policy, not only an endpoint string.

### Infer workspace members or build behavior from the directory tree

Rejected. A `loaves/` directory is a useful convention, not authority. Implicit membership and implicit Cargo conventions make plans surprising and make receipt identity incomplete.

### Run dependency hooks like Cargo `build.rs` or npm lifecycle scripts

Rejected. Oven must know what it intends to execute before it executes it. Typed provider operations, declared generators, and explicit policy-gated actions preserve practical integration without hidden lifecycle behavior.

### Let `[tool.*]` be an unbounded extension namespace

Rejected. Unconstrained tool tables make the canonical manifest a dumping ground and prevent Oven from planning effects or validating ownership. Extensions need versioned schemas and explicit provider/action identities.

## Drawbacks

- This is a breaking pre-1.0 configuration change touching project creation, discovery, documentation, workspaces, locking, environments, package resolution, interop, and tooling.
- Typed dependencies and registered registries are more explicit than one string version requirement; good defaults and concise syntax are necessary for everyday use.
- A strict Cargo boundary means Rust projects that adopt `loaf.toml` cannot rely on arbitrary Cargo manifest behavior without an explicit supported Oven provider/action model.
- Plans, policies, and receipts impose vocabulary and implementation work beyond a simple compiler wrapper.
- One manifest filename means a virtual workspace has a `loaf.toml` without a `[project]` table. The explicit table scopes must make that form obvious.

## Layers affected

- **Manifest discovery and validation** — must recognize only `loaf.toml`, validate rooted/virtual workspace shapes, reject unknown or conflicting scope, and diagnose legacy or coexisting manifests precisely.
- **Workspace resolution** — must preserve explicit membership, shared lock ownership, scoped command selection, and typed dependency inheritance while replacing `incan.toml` paths and tables.
- **Provider and dependency resolution** — must normalize typed dependencies, apply default origins, resolve registered registries, preserve provider-specific semantics, verify trust/integrity, and construct one session-owned plan.
- **Source and backend planning** — must discover one conventional built-in source facet, require `[incan.source]` and `[rust.source]` for mixed or nonstandard roots, diagnose ambiguous mixed roots, define the bounded Rust facet needed to plan Rust compilation, and prevent Cargo conventions from participating in Loaf planning.
- **Target and interop planning** — must resolve host/target/carrier/profile/delivery combinations, select providers/toolchains, and reuse RFC 116's checked `[interop.c]` facts without turning C or a future foreign ecosystem into an ambient top-level source language.
- **Environment and action tooling** — must rehome RFC 073 and RFC 078 configuration to Loaf/Oven while retaining matrix, scope, dry-run, policy, and receipt contracts.
- **Locking, stores, and publication** — must rename the canonical lock to `Oven.lock`, preserve deterministic whole-workspace resolution, reserve immutable target-bound `*.loaf` identities, and keep locks, receipts, caches, and publication artifacts distinct.
- **Cargo compatibility adapter** — must run Cargo only when explicitly selected, report its mode and side-effect boundary, and never act as a fallback inside a Loaf build.
- **CLI and inspection** — must expose manifest shape, plan, target selection, registry/trust facts, effects, `*.loaf` assets, and receipts; RFC 118 owns final binary/subcommand spelling and alias behavior.
- **Documentation, templates, LSP, and IDEs** — must create, discover, edit, display, and diagnose `loaf.toml` consistently without making generated Rust or hidden backend state the public model.

## Acceptance criteria

RFC 117 is ready to move beyond Draft when its normative rules and updated related RFCs establish all of the following:

- A standalone project, rooted workspace, and virtual workspace have unambiguous `loaf.toml` examples and discovery rules.
- The dependency grammar distinguishes unit kind from origin and explains defaults, source overrides, features, integrity, and trust.
- The `incan.pub` and crates.io defaults, explicit registry registration, project allow-list, credential boundary, and lock identity are defined precisely enough to implement.
- An Incan-only, Rust-only, and mixed Incan/Rust Loaf have defined source-root behavior: simple projects are convention-first, mixed roots use `[incan.source]` and `[rust.source]`, ambiguous co-located `.incn`/`.rs` sources diagnose, and no implicit Cargo participation occurs.
- A C integration has a defined `[interop.c]` envelope without promising Python, JNI, Go, Ruby, or a general plugin system in v0.6.
- Target, carrier, profile, workspace delivery policy, and invocation precedence have explicit validation and receipt requirements.
- Provider operations, generators, actions, policy, and receipts have a complete no-hidden-effects boundary.
- The standard `dev`, `test`, `lint`, and `docs` environments, their inheritance/selection rules, their dependency boundaries, and their commented scaffold presentation are explicit enough to implement and explain without treating a bake as an environment.
- RFCs 015, 073, 076, 077, 078, 114, and 116 identify exactly which manifest/discovery clauses RFC 117 supersedes or amends.
- The lock rename and legacy-manifest rejection have a complete pre-1.0 compatibility decision.
- RFC 118 has a clear command-ownership and alias boundary that does not reopen the manifest model.

## Implementation plan

### Phase 1: Manifest and workspace authority

- Introduce `loaf.toml` discovery and validation; remove `incan.toml` discovery from project-aware operations.
- Implement rooted and virtual workspace shapes, explicit member selection, and precise coexisting-Cargo diagnostics.
- Support nested `loaves/` groups, retaining explicit membership and allowing a structural parent manifest only when it has real child-selection or local-requirement work to do.
- Update project creation, documentation, LSP/IDE manifest discovery, and fixtures.

### Phase 2: Typed graph, registries, and lock

- Normalize Loaf, crate, and provider dependencies into one resolver-owned graph.
- Implement default origins, registered registry identities, workspace allow-lists, integrity/trust facts, and the `Oven.lock` format.
- Amend RFC 034 integration and retain RFC 114's public-feature/provider boundaries.

### Phase 3: Target plan and controlled effects

- Implement target/carrier/profile/delivery validation and receipt identity.
- Make provider operations and declared generators plan-visible; connect policy and receipt facts from RFCs 076 and 104.
- Rehome RFC 116 package requirements into the generic interop envelope.

### Phase 4: Environment and action rehoming

- Amend and implement the `loaf.toml` configuration location for RFC 073 environments/matrices and RFC 078 typed actions.
- Materialize the standard `dev`, `test`, `lint`, and `docs` environments; support explicit inheritance, environment dependency closure, conflict diagnostics, and commented scaffold defaults.
- Make Oven materialization, scope selection, dry-run, and policy gating consume the same project/workspace plan without allowing an environment to change an ordinary bake implicitly.

### Phase 5: Explicit Cargo compatibility and documentation

- Implement the distinct Cargo compatibility adapter and receipt/reporting boundary.
- Add compatibility fixtures for Cargo-only projects, convention-first Incan-only and Rust-only Loaves, explicit mixed `[incan.source]`/`[rust.source]` Loaves, nested `loaves/` workspaces, private registries, and C ABI target carriers.
- Document the command-surface handoff to RFC 118 without defining aliases or final command spelling here.

## Design decisions

- **Loafified-package distribution advantage:** the long-term `incan.pub` value proposition is a verified, target-compatible pre-warmed Loaf closure that Oven can reuse across consumers after compatibility and trust verification. It is not universal-binary distribution, an implicit network effect, or a v0.6 release requirement. The v0.6 package/lock/receipt model must nevertheless preserve the identities needed to make it a later registry capability rather than a new authority model.
- **Nested Loaf topology:** `loaves/` is the conventional home for all non-root workspace member projects, including Rust-only members. It supports arbitrary nested groups such as `loaves/stdlib/web`; paths are never implicit membership, dependency, linkage, or publication edges. A group gains a `loaf.toml` only when it needs a real structural parent contract, and it always inherits the root authority.
- **Built-in source facets:** Incan and Rust are the v0.6 built-in authored source facets. Simple one-language projects use `src/` by convention. Mixed or nonstandard projects use `[incan.source]` and `[rust.source]` with non-overlapping roots; the compiler diagnoses co-located `.incn` and `.rs` source files instead of imposing precedence. Rust exceptions remain beneath `rust.*`.
- **Deliberate interop boundary:** C remains behind the existing `[interop.c]` facade, including any authored C shim root. `[interop.*]` is reserved for later concrete integrations such as JNI or Python; v0.6 does not define a plugin system, automatic foreign-tool discovery, or generic foreign-source execution.
- **Precise C configuration terminology:** Loaf configuration names C interop, host, target, and linker artifact form directly. It does not use `native` as an interop configuration keyword or a generic foreign-language category. Any change to RFC 116's existing source-level binding spelling requires a later language proposal rather than a retroactive edit to that closed RFC.
- **`provider` is the third dependency unit kind, not `capability`:** the `[dependencies]` unit kind for target-specific library, SDK, toolchain, runtime, or deployment requirements is named `provider`, matching the vocabulary this RFC already uses pervasively ("provider semantics," "Oven-managed provider operation") rather than inventing a fourth word. `capability` is reserved: RFC 104 already owns it for checked runtime-authority grants, and reusing it here for a build-time provisioning concept created a real three-way terminology collision with RFC 104's runtime capabilities and RFC 075's project-scaffolding descriptor (also renamed, to `mix`, for the same reason). The `[capabilities]` table root this RFC reserved for RFC 075's descriptor-enablement record is renamed to `[mixes]` to match.
- **Provider-kind dependencies have a full and a bare form:** a `provider`-kind requirement that needs real version or feature constraints is declared as an ordinary `[dependencies]` entry, the same shape as `loaf` and `crate` entries: `vision-runtime = { provider = "vision-runtime", version = ">=2.0" }`. A `provider`-kind requirement that only needs to exist, with no constraints, may instead be referenced bare from the owning domain table that needs it, for example `[interop.c].requires = ["vision-runtime"]`. The bare form is implicit shorthand that resolves against a fuller `[dependencies]` declaration when one is present; the two are not competing syntaxes. This mirrors the "simple shorthand plus full explicit form, both valid" shape RFC 118 uses for its CLI surface.
- **Detailed environment/action/policy subtable shapes belong to their owning RFCs:** RFC 117 reserves and rehomes the top-level `[envs]`, `[actions]`, and `[policy]` table roots (see "Environments and actions"), but the detailed subtable schema beneath each root is RFC 073's, RFC 078's, and RFC 076's own decision, respectively. RFC 117 does not need to specify those shapes to reach Planned; it only needs the roots and the standard-environment/explicit-effect boundaries it already defines to hold.
- **Compact `rust.*` exception fields belong to RFC 119:** the core source-root contract (`[incan.source]`, `[rust.source]`, non-overlapping roots, convention-first defaults) is this RFC's own settled surface. The detailed native-Rust facet grammar, including generated-input declarations and the compact `rust.*` exception fields, is RFC 119's work, as this RFC's own "Rust source facet" section already states.
- **Target declarations use `[targets.<name>]` blocks with a list-only `platforms` field and a toolchain-owned `carrier` field:** `platforms` is always a list of literal Rust target triples (for example `aarch64-apple-ios`), reusing Rust's own target vocabulary instead of inventing a parallel one, and matching this RFC's existing list-only convention for `members`/`targets`. `carrier` is a single value naming the deliverable form (static library, framework, JNI shared library, Python wheel, executable, Rust-host caller projection, or another declared family). Carrier kinds are toolchain-owned and extensible only across releases: a new carrier kind lands through an official RFC (RFC 119 for Rust-hosted carriers, a future RFC for others), never through package-defined extension. Whether a given platform/carrier combination is valid (for example, `framework` only makes sense for Apple platforms) is not this RFC's concern; it is owned by whichever RFC defines that carrier kind. Tooling support for validating combinations and offering editor autocomplete is tracked separately (issue #1064) and is not required for this RFC to reach Planned.
- **`*.loaf` wire/container format stays out of scope, as already stated in Non-Goals:** this RFC reserves the identity a future artifact-format RFC must carry — project/facet, target, carrier, profile, resolved-graph identity, payload digest, and receipt identity — but does not define the container, registry transport, or multi-asset publication protocol itself. Asking this RFC to also settle the wire format was a category error: Non-Goals already assigns it to a dedicated RFC.
- **Cold/warm/hot Loaf states stay out of scope, as already stated in Non-Goals and Terminology:** this RFC's Terminology section already flags that their precise state semantics are intentionally not specified here. Giving them normative inspection meaning is a future Oven cache/lifecycle RFC's decision, not a gap in this RFC's own model.
- **Registry trust distribution/administration is out of scope:** this RFC defines the registration contract — registries are declared in Oven-controlled user or organization configuration outside the project, and a Loaf or workspace may only allow-list from what is already registered, never define, rebind, or weaken trust (see "Registry registration and trust"). That non-overwrite guarantee already holds without any additional distribution mechanism, because a project has no registration authority at all. How an organization gets the same trusted-registry configuration onto every developer machine and CI runner — through existing device-management tooling, a self-hosted convention, or another means the operator chooses — is an operational concern for whoever runs the registry, not a capability this RFC or Oven itself needs to provide. `incan.pub` remains the trusted, secure default origin for the open ecosystem; that trust is RFC 034's responsibility.
- **Generators are ordinary typed Incan functions, not a bespoke plugin format:** a generator is not a new sandboxed runtime, wire protocol, or component model. It is a typed Incan (or `rust::`-interop) function with a declared input/output signature that Oven resolves and calls as part of planning or baking, reusing the same execution substrate Incan's own interactive and notebook ambitions already require rather than inventing a second one. Its capability needs — network access, filesystem beyond its declared outputs, a live database, or any other ambient authority — are ordinary RFC 104 capability grants like any other code path; there is no generator-specific security model, because RFC 104 already owns "what can this code touch" regardless of who is asking. A generator's output may legitimately vary between two otherwise-identical builds when it consults an external, changing source; that variability is a capability its declaring package consciously requests and its consumer consciously grants, not a defect this RFC needs to guard against. A generator's declared outputs remain generated, ownership-tracked files that an ordinary bake must not overwrite authored source with.
- **Providers remain closed and toolchain-owned, not a third-party extension point:** unlike generators, providers (compiling Rust, verifying C headers, materializing an SDK component) are deep native toolchain integrations that Oven implements itself, not expressible as a typed function signature. A new provider kind lands only through an official RFC, the same governance already settled for carrier kinds.
- **Carriers have a shape, and a terminal carrier's bake output is not a `*.loaf`:** found while working RFC 118's command-surface decisions and folded back here, since carrier semantics are this RFC's job, not RFC 118's. A `*.loaf`'s entire value proposition, as this RFC's own "Future registry-supplied warm reuse" section describes it, is reuse by another Oven consumer. A terminal artifact such as an executable — for example a data pipeline's final ETL binary — is never depended on as a package by anything, so packaging it as a reusable `*.loaf` carries no meaning. Every carrier therefore has a fixed **shape**: loaf-shaped carriers (static library, framework, JNI shared library, Python wheel, Rust-host caller projection) bake to a `*.loaf` asset; the terminal carrier (`executable`) bakes to a plain build artifact with no `.loaf` packaging. Both shapes still go through the same plan, policy decision, and receipt; shape changes only downstream-reuse packaging, not plan rigor or audit requirements. See "Carrier shape and bake output."
