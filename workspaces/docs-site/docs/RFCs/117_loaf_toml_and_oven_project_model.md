# RFC 117: `Loaf.toml` and Oven's language-neutral project model

- **Status:** Draft
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
- **Issue:** [#1009](https://github.com/encero-systems/incan/issues/1009)
- **RFC PR:** —
- **Written against:** v0.5
- **Target scope:** v0.6 design and implementation planning; this is not a shipment claim.
- **Shipped in:** —

## Summary

This RFC replaces `incan.toml` with `Loaf.toml` as the sole authored manifest for an Oven-managed project. A `Loaf.toml` may describe one project, a rooted workspace, or a virtual workspace. It is language-neutral: a project may contain Incan sources, Rust sources, both, or checked foreign interfaces without making any implementation language the repository's organizing principle.

`[project]` remains the familiar metadata table. `[workspace]` remains the explicit repository-coordination table. Dependencies use one typed graph whose unit kind and origin are explicit when the defaults are insufficient: Incan Loaf packages default to `incan.pub`, Rust crates default to crates.io, and foreign/system requirements use Oven-managed capability providers. Oven resolves authored intent into `Oven.lock`, then bakes the selected target into immutable, verified `*.loaf` assets and receipts. Cargo remains a deliberately explicit compatibility mode for repositories that have no `Loaf.toml`; it is never inferred as part of a Loaf build.

This RFC also rehomes the authored configuration boundaries of existing environment, matrix, action, workspace, provider, and interop RFCs. It does not define the final command spelling or a general-purpose task shell.

## Core model

Read this RFC as thirteen foundations:

1. **One authored manifest:** `Loaf.toml` is the only Incan/Oven project manifest. `incan.toml` is not read as a compatibility format.
2. **A familiar project table:** `[project]` owns package identity and publication metadata. The filename carries the Loaf vocabulary; the metadata table remains ordinary and legible.
3. **A workspace is an optional hierarchy:** `[workspace]` declares explicit members and shared policy. A root with both `[project]` and `[workspace]` is a rooted workspace; a root with only `[workspace]` is virtual. Members may declare sub-Loaves, but the root remains the sole authority for resolution, locking, registry trust, target policy, and receipts.
4. **The package graph is language-neutral:** every dependency is one logical requirement with a typed provider contract. The author does not choose a separate Incan-versus-Rust dependency table.
5. **Origins are configurable:** Loaf packages default to `incan.pub`; crates default to crates.io. A project may explicitly select registered private registries, Git, path, or other supported controlled sources.
6. **Provider semantics stay visible:** an `incan.pub` package contributes checked Incan semantic/provider facts, a Cargo ecosystem crate contributes Rust interop and implementation requirements, and a system/foreign capability contributes a verified target requirement. Oven must not flatten those differences or expose backend details as public Incan API.
7. **Source discovery is convention-first:** Oven discovers supported source files under `src/` by language extension. Nonstandard roots or generated inputs require explicit configuration. A Loaf never implicitly consumes a neighbouring `Cargo.toml`.
8. **Targets have three scopes:** a Loaf declares supported target/interface combinations; a workspace declares shared delivery policy; a bake invocation chooses one concrete target, carrier, profile, and feature closure that produces a target-bound `*.loaf` asset.
9. **Plans precede effects:** resolution, import, installation, or the presence of a file must not execute package code. Oven exposes a plan before baking and records the actual execution in a receipt.
10. **Environments and actions belong to Oven:** existing environment/matrix and typed-action semantics remain, but their authored configuration moves out of `[tool.incan.*]` and into the Loaf model. Oven materializes environments and explicitly runs actions.
11. **Derived state is separate:** `Oven.lock`, immutable `*.loaf` assets, the artifact store, selected host toolchains, caches, staged outputs, generated code, and receipts are not authored Loaf configuration.
12. **Distribution assets are verifiable:** a `*.loaf` asset is immutable and target-bound. It identifies the selected project/facet, target, carrier, profile, resolved graph, payload digest, and receipt; its wire or bundle format is intentionally deferred.
13. **Cargo compatibility is explicit:** a directory with only `Cargo.toml` may be run by Oven in Cargo-compatibility mode. A directory containing `Loaf.toml` is a Loaf project; Cargo files there are ignored with a diagnostic unless the user explicitly invokes Cargo compatibility.

## Motivation

The current `incan.toml` correctly introduced project identity, dependency declarations, SDK selection, build configuration, workspaces, environment configuration, Rust dependency declarations, and Oven interop requirements. It now carries an assumption that no longer fits the intended architecture: that a project's identity is primarily Incan source and that Rust is a special external implementation substrate.

That assumption is constraining in both directions. An Incan-authored package may need a Rust crate, a C ABI library, a C++ shim, a target SDK, or a later JNI/Python carrier without ceasing to be one package. A Rust-authored project that elects to use Oven should be able to adopt the same package, target, receipt, and workspace model without being forced to rewrite its sources into Incan. Cargo must remain runnable where Cargo is explicitly authoritative, but Cargo's implicit build scripts, proc macros, target conventions, and workspace semantics must not become accidental Loaf semantics.

The same pressure exists at the repository root. A separate `Oven.toml` would distinguish package metadata from workspace orchestration, but introduces two canonical files and an ownership/discovery boundary without a demonstrated capability need. Cargo already demonstrates that one manifest can express a root package, a virtual workspace, or both. `Loaf.toml` can do the same when its table scopes are explicit.

Oven is more than a package resolver. It must plan target-specific C ABI, JNI, Python, Rust-host, and future carrier builds; select and verify toolchains; materialize controlled environments; protect shared artifact stores; and describe exactly what happened. That requires a manifest that captures desired inputs, not hidden host discovery or arbitrary execution.

## Goals

- Replace `incan.toml` with `Loaf.toml` wholesale before Incan 1.0.
- Preserve `[project]` as the familiar package-metadata surface.
- Preserve rooted and virtual workspace forms using optional `[workspace]`.
- Give Incan packages, Rust crates, and foreign/system capabilities one dependency grammar while retaining their distinct resolver and linkage contracts.
- Make `incan.pub` the default origin for Loaf packages and crates.io the default origin for Rust crates, while allowing registered alternative registries and explicit sources.
- Define safe discovery when `Loaf.toml` and `Cargo.toml` coexist.
- Permit explicitly declared hierarchical sub-Loaves under one inherited root-workspace authority.
- Keep source-language choice out of the package identity while avoiding implicit Cargo participation.
- Define a target model that distinguishes platform target, deployable carrier, profile, and delivery policy.
- Distinguish authored intent (`Loaf.toml`), resolved graph (`Oven.lock`), and immutable target-bound distribution assets (`*.loaf`) without making any of them hidden state.
- Make plans, declared effects, selected inputs, output artifacts, and receipts inspectable.
- Rehome environment/matrix/action configuration ownership to Oven without re-specifying the semantics already owned by RFCs 073 and 078.
- Rehome package-level C ABI configuration from `[oven.interop]` into an interop section that future binding kinds may share, while retaining RFC 116 as the owner of C safety semantics.
- Keep locks, caches, credentials, selected SDK paths, staged artifacts, and generated output out of the authored manifest.
- Leave command spelling and CLI ergonomics for a dedicated follow-up RFC.

## Non-Goals

- Defining a separate `Oven.toml` root manifest.
- Preserving `incan.toml`, `incan.lock`, or `[tool.incan.*]` as legacy compatibility formats.
- Defining the final `oven` versus `incan` command-line binary, aliases, or command hierarchy.
- Reimplementing arbitrary Cargo behavior, including implicit `build.rs` execution, proc-macro execution policy, or Cargo workspace discovery, for Loaf projects.
- Defining crates.io's protocol, the full `incan.pub` registry protocol, registry hosting, or publishing UX.
- Defining C ABI safety rules, direct C++ ABI interop, JNI safety rules, Python extension safety rules, or a universal foreign-runtime type system.
- Defining remote execution, distributed builds, or a general-purpose shell/task runner.
- Defining the `*.loaf` wire or bundle format, registry transport, or archive layout; a dedicated artifact-format RFC owns those details.
- Allowing packages to run arbitrary installation, import, or lifecycle hooks.
- Giving cold, warm, and hot Loaves normative compiler or package semantics in this RFC.

## Prospective supersession of closed RFC decisions

Closed RFCs remain historical records and are not retroactively edited. This RFC explicitly supersedes the following decisions for implementations that adopt the Loaf/Oven project model:

- **RFC 013 and RFC 031:** separate authored `[dependencies]` and `[rust-dependencies]` tables, and the assumption that a consumer's dependency graph is authored as Cargo wiring. `Loaf.toml` has one typed dependency graph; provider-specific Rust requirements remain visible facts rather than a second authoring domain.
- **RFC 015:** `incan.toml` as the project manifest, nearest-manifest root discovery, project generation that writes `incan.toml`, and `[tool.incan.envs]` as the environment-configuration home. `Loaf.toml` is the sole authored manifest, while environment semantics remain owned by their dedicated RFCs.
- **RFC 020:** `incan.lock` as the generated resolution identity. The reproducibility and locked/offline requirements remain; their generated artifact is `Oven.lock`.
- **RFC 077:** a flat workspace that forbids nested workspaces. Explicit membership, rooted and virtual workspace forms, and one shared resolution boundary remain; a member may now declare explicit sub-Loaves within a single inherited root-workspace authority.
- **RFC 112:** the public lockfile path used by its publication examples. Crash-safe publication and coordination requirements remain unchanged, but the published lockfile is `Oven.lock` and its companion coordination identity follows that name.
- **RFC 114:** references to the canonical `incan.lock` graph. Its distinction between public Loaf features and provider-specific implementation features remains unchanged; the resolved graph is recorded in `Oven.lock`.

This RFC does not supersede the retained semantic contracts in those RFCs: reproducibility, explicit workspace membership, crash-safe publication, checked library surfaces, and public package-feature meaning remain requirements. It changes the authored manifest, authority hierarchy, dependency presentation, and derived-state names through which those contracts are implemented.

## Terminology and mental model

The user-facing contract is intentionally small: authors write `Loaf.toml`; Oven plans and bakes it. The surrounding bread vocabulary is a guide-level model and an Oven inspection affordance, not a second vocabulary users must configure.

```text
Loaf.toml + declared sources + declared inputs
  -> Doughball (internal source-and-input mass)
  -> Oven shapes one or more Loaves (resolved logical build units)
  -> Oven bakes a selected Loaf for a selected target
  -> immutable target-bound *.loaf asset + execution receipt
```

- A **project** is the independently named and versioned unit described by `[project]`.
- A **workspace** is the explicit repository coordination boundary described by `[workspace]`; it is not itself a compiled package unless it also has `[project]`.
- A **Doughball** is Oven's internal representation of the selected sources, declared dependencies, features, interop inputs, and policy before target shaping.
- A **Loaf** is the resolved logical build unit Oven obtains by shaping a Doughball for a selected package facet and target-compatible plan.
- A **`*.loaf` asset** is the immutable, verified, target-bound build and distribution asset emitted by a bake. It carries the selected Loaf identity, target/carrier/profile, resolved-graph identity, payload digest, and receipt identity. Its archive or wire format is outside this RFC.
- To **bake** is the operation that executes a resolved plan for a selected target/carrier/profile and produces a `*.loaf` asset plus a receipt. "Bake" is a verb, not a required artifact type.
- **Cold**, **warm**, and **hot** Loaves may be shown by Oven's internal inspection and cache-status UX. Their precise state semantics are intentionally not specified here.
- A **carrier** is the target deliverable form: for example, a static library, framework, JNI shared library, Python wheel, Rust-host caller projection, executable, or another declared artifact family.

## Guide-level explanation

### A single project

An ordinary project needs only `Loaf.toml`:

```toml
[project]
name = "weather_service"
version = "0.6.0"
description = "A service that happens to use Incan and Rust sources"
requires-incan = ">=0.6,<0.7"

[dependencies]
web = { loaf = "stdlib-web", version = "^0.6" }
serde = { crate = "serde", version = "1", features = ["derive"] }
```

`web` defaults to `incan.pub`; `serde` defaults to crates.io. Neither the project identity nor the command to bake it changes because its implementation contains Incan, Rust, or both. The public package feature graph remains an Incan/Loaf contract. A requested Cargo feature is a typed implementation request for `serde`; it does not automatically become a public feature of `weather_service`.

Oven discovers supported source files below `src/`. A project with only conventional source roots does not need to list its implementation languages.

### A workspace of Loaves

The root `Loaf.toml` can be a project and a workspace:

```toml
[project]
name = "incan"
version = "0.6.0"

[workspace]
members = ["loaves/*"]
default-members = [".", "loaves/incan_lsp"]

[workspace.delivery.mobile]
members = ["loaves/stdlib_interop", "loaves/stdlib_web"]
targets = ["aarch64-apple-ios", "aarch64-linux-android"]
profile = "release"
```

A virtual workspace omits `[project]` and retains `[workspace]`. Every member has its own `Loaf.toml`; no member is inferred merely because it lives under `loaves/`.

The directory name is a repository convention, not an identity rule. A member can live anywhere inside the workspace root if it is explicitly included by `members`.

### Hierarchical sub-Loaves

A member may itself declare explicit child members. This is a structural hierarchy, not a nested independent workspace:

```text
incan/
├── Loaf.toml                         # root authority and Oven.lock
└── loaves/
    └── incan_oven/
        ├── Loaf.toml                 # an Incan sub-Loaf
        └── loaves/
            ├── oven_store/Loaf.toml
            └── oven_provider_cargo/Loaf.toml
```

The root's selected closure contains `incan_oven` and its declared children. A sub-Loaf may declare local requirements and local member selection, but it inherits the root's resolved graph, lock, registry trust, target/delivery policy, and receipt boundary. It cannot create a second `Oven.lock`, rebind a registry, widen target policy, or form a second publication authority.

Membership hierarchy is not a dependency graph. Parent/child placement, a shared workspace, and a selected closure create no runtime, linkage, or publication dependency between packages; those relationships exist only through declared typed dependencies. For example, `incql-core`, `incql-db`, and `incql-datafusion` can remain independently versioned packages whether or not one is structurally nested below another; any dependency between them is explicit in the consuming package's dependency table.

This hierarchy also permits efficient cache-status inspection: Oven can compare a receipt-bound aggregate for a parent before it descends into a child's declared closure. The exact cold/warm/hot definitions remain an Oven inspection concern rather than this RFC's package contract.

### Registries without ambient trust

Oven has built-in defaults for the public ecosystems:

```text
Loaf package  -> incan.pub
Rust crate    -> crates.io
```

Users or organizations explicitly register additional registries in Oven-controlled user or organization configuration. Registry configuration identifies the registry kind, endpoint/index, trust policy, and a reference to credentials held outside the project.

```toml
# Illustrative Oven user/organization configuration, not Loaf.toml.
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

The registry alias is convenience, not the security identity. `Oven.lock` records the canonical registry endpoint and protocol, exact package identity, immutable digest, and observed signature/trust result. Credentials never enter `Loaf.toml` or the lock. A project cannot silently register, rebind, or trust a registry on a developer's machine.

### Declaring C ABI and future carriers

A package declares interface intent and physical input requirements. Oven resolves the concrete target toolchain, SDK, artifacts, and deployment placement when it plans a bake.

```toml
[project]
name = "vision_runtime"
version = "0.6.0"

[oven.interop.c]
headers = ["interop/include/vision.h"]
requires = ["vision-runtime"]

[targets.ios]
platform = "aarch64-apple-ios"
carrier = "framework"

[targets.android]
platform = "aarch64-linux-android"
carrier = "jni-library"
```

This example is intentionally schematic. `[oven.interop]` remains Oven-owned in `Loaf.toml`; `[oven.interop.c]` is the direct successor to RFC 116's C envelope. RFC 116 continues to define C declarations, ownership, buffers, verification, and checked shims. A future JNI or Python RFC defines the corresponding language/runtime safety contract. They all reuse the same package-envelope distinctions: declared requirement, selected provider/toolchain, target-compatible carrier, lock identity, plan, `*.loaf` asset, and receipt.

### Baking and inspecting

Before Oven performs external work, it can show a plan containing selected members, dependencies, providers, target, carrier, profile, capabilities, declared inputs/outputs, and expected effect classes. Baking executes only that plan and emits a target-bound `*.loaf` asset plus a receipt.

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
Found Loaf.toml and Cargo.toml. Loaf.toml is authoritative; Cargo configuration
was ignored. Use an explicit Cargo-compatibility operation to run Cargo semantics.
```

Oven must not parse, merge, or infer dependency, feature, source, workspace, build-script, or target policy from the adjacent Cargo manifest.

## Reference-level explanation

### Manifest discovery and shapes

1. `Loaf.toml` is the canonical manifest filename. Project-aware discovery walks upward from the working directory or source path to the nearest directory containing it.
2. A manifest containing `[project]` and no `[workspace]` describes one project.
3. A manifest containing `[project]` and `[workspace]` describes a rooted workspace. The root project is a member without requiring `"."` in `members`.
4. A manifest containing `[workspace]` and no `[project]` describes a virtual workspace. It must declare at least one resolved member.
5. Workspace membership is explicit. The root's selected closure is formed by recursively expanding each selected member's explicit `members` declaration, subject to `exclude`; a path dependency does not imply membership.
6. A member may declare explicit child members, producing hierarchical sub-Loaves. Each non-root member has exactly one direct parent in the selected closure.
7. Membership and parent/child hierarchy are never dependency, linkage, or publication edges. Those edges arise only from declared typed dependency requirements; a path dependency does not imply membership, and membership does not imply a dependency.
8. The root workspace is the sole authority for the resolved graph, `Oven.lock`, registry trust, workspace delivery policy, and receipt boundary. Child workspace declarations may add local membership and narrow local requirements, but may not create a lock, rebind registries, weaken trust, broaden target policy, or establish an independent publication authority.
9. A manifest containing neither `[project]` nor `[workspace]` is invalid.
10. `incan.toml` is not a project manifest after this RFC. A directory that contains it but no `Loaf.toml` must receive a targeted diagnostic rather than legacy parsing.
11. If a `Loaf.toml` project also contains `Cargo.toml`, Oven must warn and ignore Cargo configuration. The diagnostic must name the ignored file and explain explicit Cargo-compatibility selection.

### Project metadata and source domains

`[project]` retains the project identity and publication fields from RFC 015: name, version, description, authors, maintainers, license, license files, readme, homepage, repository, documentation, issues, keywords, classifiers, toolchain constraint, privacy, entry points, and public features.

`requires-incan` remains an enforceable project compatibility requirement under RFC 073. It is a language/toolchain compatibility fact, not a claim that all implementation sources must be Incan.

Supported source files below the conventional `src/` root participate according to their language extension and declared package facet. An implementation must not require an author to declare `incan = [...]` or `rust = [...]` merely because both file kinds appear under conventional roots. A later concrete schema may provide an optional source-root table for nonstandard roots, generated inputs, exclusions, or explicit source-domain policy.

When a Loaf project contains Rust source, Oven determines its compilation/linkage path from the Loaf plan and declared crate/provider requirements. The presence of `Cargo.toml`, `build.rs`, or another Cargo convention does not grant that file execution or planning authority.

### Rust source facet

A Rust-bearing Loaf has a bounded, inspectable Rust facet. The planner must know, either through the conventional default or an explicit declaration, every compiled crate's package name, crate root, edition, crate type, entry point where applicable, enabled feature set, and linkage/dependency requirements. A conventional `src/lib.rs` or `src/main.rs` may supply a default root only when it yields one unambiguous crate; multiple crates, nonstandard roots, nondefault crate names, nondefault editions, additional crate types, binary entry points, or feature mapping require explicit facet data.

The exact TOML table spelling remains an RFC 117 syntax decision, but no Rust compilation plan may be inferred from Cargo metadata. `Oven.lock` and the receipt must record the selected Rust facet, compiler/toolchain identity, crate dependency closure, feature choices, and all provider-produced inputs that affect the resulting `*.loaf` asset.

The existence of `build.rs` must never execute it. A Loaf may support its intent only through an explicitly selected Oven provider or typed action whose inputs, outputs, capabilities, target/host role, policy decision, and receipt effects are declared and inspectable. Likewise, a procedural macro must be an explicitly resolved compiler-time provider with a recorded host identity and execution policy; an unsupported macro is a diagnostic, not a fallback to Cargo. Cargo compatibility remains the only mode in which Cargo itself is authoritative for these behaviors.

RFC 119 owns the detailed native-Rust facet grammar, crate-provider graph, direct-`rustc` planner, build-time provider envelope, Rust test/IDE projections, and explicit Cargo interoperation. This RFC retains the one-manifest, one-lock, one-workspace-authority boundary those facilities must obey.

### Typed dependencies and features

Every dependency requirement has a **unit kind** and an **origin**.

| Unit kind | Default origin | Contributes | Does not become |
| --- | --- | --- | --- |
| `loaf` | `incan.pub` | checked Incan semantic/provider facts and public package features | a Cargo crate or generated Rust source contract |
| `crate` | crates.io | Rust interop metadata and backend implementation/linkage requirements | a public Incan feature namespace by accident |
| `capability` | Oven-managed system/foreign catalog | target-specific library, SDK, toolchain, runtime, or deployment requirement | an ambient host discovery result |

An entry may override its default origin through a registered registry, Git, path, or another supported source form. The resolved provider contract determines the legal source forms; a Loaf registry must not be accepted as a Cargo registry merely because it has a URL.

Project features remain additive public Loaf capabilities as defined by RFC 114. They may enable optional Loaf dependencies and request public features from Loaf dependencies. A project may explicitly request a provider-specific crate feature as part of a crate dependency requirement, but that request is not exposed as a public project feature unless the project explicitly maps it through its public feature definition. Cargo feature names, generated crate names, and backend module paths remain implementation details.

Workspace inheritance remains explicit. A workspace may own a dependency identity and members may opt into it. A member may refine only the usage properties RFC 114 and the selected dependency kind allow; it may not silently replace the source, registry, package identity, or trust policy selected by the workspace.

### Registry registration and trust

1. Oven provides the built-in origins `incan.pub` for `loaf` dependencies and crates.io for `crate` dependencies.
2. Additional registries are registered through Oven-controlled user or organization configuration, never by an unreviewed dependency or an implicit network lookup.
3. A registry registration must declare a stable identity, unit kind/protocol, endpoint/index, and trust policy. Credentials are referenced from secure user/organization storage rather than copied into project files.
4. A Loaf or workspace may allow-list registered registry identities. It may select an allowed alias but may not define credentials, rebind an alias, or weaken a user/organization trust policy.
5. The lock records canonical registry identity, protocol, exact resolved package/crate/capability identity, version, immutable digest, source reference, and signature/trust outcome. An alias alone is insufficient for reproducibility or audit.
6. Resolution in locked or offline mode must not query an unrecorded registry, substitute an ambient local package, or accept a source with different trust facts.

RFC 034 remains the protocol and publication authority for `incan.pub`; this RFC defines how `incan.pub` participates in the generic Loaf graph. A future Cargo-registry adapter must respect Cargo registry identity and integrity semantics without making Cargo the authority for a Loaf bake.

### Targets, carriers, profiles, and delivery policy

Target selection has three distinct levels:

| Level | Responsibility | Constraint rule |
| --- | --- | --- |
| Loaf | Declares supported platform/interface/carrier combinations and package requirements | Defines the maximum compatibility surface the package claims |
| Workspace | Declares named delivery policy, allowed targets, selected members, shared provider rules, and defaults | May narrow a member's support; must not broaden it |
| Bake invocation | Selects the concrete target, carrier, profile, feature closure, and optional delivery policy | Must satisfy both workspace policy and every selected Loaf |

The resolved plan must distinguish:

- the **build host** on which Oven executes;
- the **platform target** such as `aarch64-apple-ios`;
- the **carrier** such as framework, static library, JNI shared library, Python wheel, executable, or Rust-host caller projection;
- the **profile** such as development or release;
- the enabled public features and selected provider requirements; and
- the destination/deployment policy when relevant.

An explicit invocation has the highest selection precedence but cannot override package support or workspace policy. A workspace delivery policy may narrow a project choice but cannot make a package claim an unsupported target. An unqualified local bake may choose a deterministic host-development target and profile, and the receipt must record that choice. Publication or deployment must require an explicit target/carrier or named delivery policy; it must not silently publish a host-default artifact.

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
2. **Declared generator operation:** a generator has typed inputs, outputs, provider identity, target constraints, capability requirements, and generated-output ownership. It may not overwrite authored source through an ordinary bake.
3. **Explicit workflow action:** an action from RFC 078 is selected by the user or an authorized tool and is evaluated under RFC 076 policy before execution.

An extension must have a versioned, typed schema and a known provider/action identity. Unknown extension kinds are invalid; they are not a generic `[tool.*]` escape hatch. A provider or action may advertise a required capability, network requirement, external executable, or mutation class, but it must not gain authority merely by being selected as a dependency.

Before executing a nontrivial plan, Oven must be able to inspect the selected members, dependency/provider identities, target/carrier/profile, toolchains, declared inputs and outputs, execution categories, capabilities, mutation classes, and expected receipt facts. Policy may require approval for a plan according to RFC 076. After execution, the receipt must identify the actual provider/toolchain selections, input identities, output artifacts, declared and observed effects, and deviations or denials.

RFC 104 owns capability-enforcement and receipt semantics. RFC 112 owns crash-safe publication and file coordination. This RFC requires their identities to be included in the Loaf/Oven plan rather than recreated through hidden backend behavior.

### Environments and actions

The following authored concepts move into `Loaf.toml` under Oven ownership. Their table roots are intentionally typed and reserved; arbitrary `[tool.*]` namespaces do not carry forward:

| Concept | Semantic owner | Oven responsibility |
| --- | --- | --- |
| project and environment toolchain constraints | RFC 073 | resolve and enforce compatible toolchains |
| named environments and matrices | RFC 073 | materialize the resolved environment and expand matrix cells deterministically |
| typed workflow actions | RFC 078 | resolve scope, show plan, policy-gate, and execute explicit actions |
| mutation policy | RFC 076 | evaluate receiver authority over planned changes/effects |

RFC 015 is implemented, so RFC 117 supersedes its `incan.toml` discovery and `[tool.incan.envs]` configuration placement. RFCs 073, 076, and 078 are still Draft and must be amended to name the new Loaf/Oven tables and terminology rather than preserving legacy configuration compatibility.

The initial table roots are `[oven.envs]`, `[oven.actions]`, `[oven.policy]`, `[oven.capabilities]`, and `[oven.templates]`. Their dedicated RFCs own the detailed schema and semantics. They must remain explicit, scoped, inspectable, and free of implicit lifecycle execution. RFC 118 owns whether users invoke those semantics through `incan`, `oven`, aliases, or another command arrangement.

### Interop package envelope

RFC 116's current `[oven.interop]` shape is package-owned requirement data, not a record of a host toolchain already selected. Under this RFC it remains Oven-owned in the Loaf-level `[oven.interop]` namespace: C declarations use `[oven.interop.c]`. The file name changes from `incan.toml` to `Loaf.toml`; the ownership boundary does not.

The interop envelope must be binding-kind neutral. It represents declared headers, shims, foreign artifacts, toolchain/SDK capabilities, target variants, carrier requirements, lock identities, plan inputs, generated verification records, artifact staging, and receipts. It does not make C pointer types, C ownership, JNI handles, Python reference counts, or another runtime's safety rules universal. Each binding-kind RFC continues to define its own source vocabulary and checking rules.

### Locks and derived state

`Oven.lock` is the canonical generated resolution state for a project or workspace. A workspace has one root lock that represents every member's selected dependency/provider graph and target-relevant resolution facts. `incan.lock` is not read after this RFC.

The lock must include every semantic or physical input whose change can alter checking, linking, target compatibility, generated output, or the baked artifact closure. It must not contain credentials, mutable cache paths, arbitrary environment values, or unreviewed host discovery output.

`*.loaf` is the immutable, target-bound distribution/build asset derived from the lock and the selected bake plan. It must identify its project/facet, target, carrier, profile, resolved graph identity, payload digest, and receipt identity; it is not an editable project manifest or a replacement lockfile. The artifact store, provider payload store, staged deployment files, generated Rust, target intermediates, and receipts are separate from the lock. They are inspectable and may be content-addressed, but they are not user-authored package configuration or semantic authority.

## Design details

### Manifest ownership map

`Loaf.toml` is the authored envelope. Existing RFCs continue to own the meaning of their respective content:

```text
Loaf.toml
├── [project]                 RFC 117
├── [workspace]               RFC 117; prospectively supersedes RFC 077's flat-workspace restriction
├── dependencies/features     RFC 117 + RFC 114
├── registry selection        RFC 117 + RFC 034
├── [oven.envs]               RFC 073, rehomed by RFC 117
├── [oven.actions]            RFC 078, rehomed by RFC 117
├── [oven.policy]             RFC 076, rehomed by RFC 117
├── [oven.capabilities]       RFC 075, governed by RFC 076
├── [oven.templates]          RFC 074, governed by RFC 076
├── [oven.interop]            RFC 116 now; future binding RFCs later
├── Oven.lock                 resolved graph, RFCs 020, 104, 112, and 114
└── *.loaf + receipt          immutable target-bound artifact, RFC 117; wire format deferred
```

This map does not make RFC 117 the owner of every detailed sub-schema. It establishes one manifest root and prevents every feature family from inventing its own root file, discovery rule, or source-language split.

### Source language and authored Rust

The Loaf package model is language-neutral; it does not relax Incan's authoring policy. Incan products and dogfooding projects should remain Incan-authored and use `rust::` interop for existing Rust capabilities. New authored Rust still requires a demonstrated Incan limitation and a tracked removal path.

That policy is distinct from the manifest contract. Oven must be capable of baking a Loaf whose implementation is Rust, Incan, or mixed because the build system cannot make a package's identity depend on the language mix. Project governance can constrain which sources its maintainers choose to author.

The Rust facet is the language-neutral planner contract for Rust-authored or mixed Loaves. It does not license authored Rust in Incan products without the demonstrated limitation and tracked removal path required above.

### Cargo compatibility boundary

Cargo compatibility has two valid states:

1. **Cargo project:** a directory has `Cargo.toml` and no `Loaf.toml`. An explicitly selected Oven Cargo-compatibility operation delegates to Cargo. Cargo retains authority over its manifest and side effects.
2. **Loaf project:** a directory has `Loaf.toml`, with or without `Cargo.toml`. Oven uses only the Loaf graph. Cargo is not an implicit source of build policy.

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

- `incan.toml` is replaced wholesale with `Loaf.toml`.
- `incan.lock` is replaced with `Oven.lock`.
- Existing implemented project/workspace/environment code must be updated to discover and write the new schema; it must not retain a legacy `incan.toml` parser.
- Existing Draft RFCs that refer to `[tool.incan.*]` must be amended before their implementation is scheduled.
- Existing project authors must explicitly adopt the Loaf contract; a raw rename is valid only when the resulting tables satisfy the new schema.
- Cargo-only repositories require no adoption to be runnable in explicit Cargo-compatibility mode.

The tool should emit a targeted diagnostic when it finds `incan.toml` without `Loaf.toml`, but it must not silently interpret the old file. This keeps the authority transition explicit and avoids supporting every interaction between old Incan configuration, Cargo configuration, and the new Oven model.

## Alternatives considered

### Add `Oven.toml` beside `Loaf.toml`

Rejected for now. A separate root file makes the package-versus-workspace vocabulary literal, but it creates two canonical documents, discovery precedence, and synchronization questions. A single `Loaf.toml` with explicit `[project]` and `[workspace]` scopes supports single projects, rooted workspaces, and virtual workspaces without losing capability. A future RFC may introduce a separate file only if independently versioned workspace policy demonstrates a real need.

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
- A strict Cargo boundary means Rust projects that adopt `Loaf.toml` cannot rely on arbitrary Cargo manifest behavior without an explicit supported Oven provider/action model.
- Plans, policies, and receipts impose vocabulary and implementation work beyond a simple compiler wrapper.
- One manifest filename means a virtual workspace has a `Loaf.toml` without a `[project]` table. The explicit table scopes must make that form obvious.

## Layers affected

- **Manifest discovery and validation** — must recognize only `Loaf.toml`, validate rooted/virtual workspace shapes, reject unknown or conflicting scope, and diagnose legacy or coexisting manifests precisely.
- **Workspace resolution** — must preserve explicit membership, shared lock ownership, scoped command selection, and typed dependency inheritance while replacing `incan.toml` paths and tables.
- **Provider and dependency resolution** — must normalize typed dependencies, apply default origins, resolve registered registries, preserve provider-specific semantics, verify trust/integrity, and construct one session-owned plan.
- **Source and backend planning** — must discover conventional supported sources, require explicit nonstandard declarations, define the bounded Rust facet needed to plan Rust compilation, and prevent Cargo conventions from participating in Loaf planning.
- **Target and interop planning** — must resolve host/target/carrier/profile/delivery combinations, select providers/toolchains, and reuse RFC 116's checked C package facts without C-specific assumptions in the envelope.
- **Environment and action tooling** — must rehome RFC 073 and RFC 078 configuration to Loaf/Oven while retaining matrix, scope, dry-run, policy, and receipt contracts.
- **Locking, stores, and publication** — must rename the canonical lock to `Oven.lock`, preserve deterministic whole-workspace resolution, reserve immutable target-bound `*.loaf` identities, and keep locks, receipts, caches, and publication artifacts distinct.
- **Cargo compatibility adapter** — must run Cargo only when explicitly selected, report its mode and side-effect boundary, and never act as a fallback inside a Loaf build.
- **CLI and inspection** — must expose manifest shape, plan, target selection, registry/trust facts, effects, `*.loaf` assets, and receipts; RFC 118 owns final binary/subcommand spelling and alias behavior.
- **Documentation, templates, LSP, and IDEs** — must create, discover, edit, display, and diagnose `Loaf.toml` consistently without making generated Rust or hidden backend state the public model.

## Acceptance criteria

RFC 117 is ready to move beyond Draft when its normative rules and updated related RFCs establish all of the following:

- A standalone project, rooted workspace, and virtual workspace have unambiguous `Loaf.toml` examples and discovery rules.
- The dependency grammar distinguishes unit kind from origin and explains defaults, source overrides, features, integrity, and trust.
- The `incan.pub` and crates.io defaults, explicit registry registration, project allow-list, credential boundary, and lock identity are defined precisely enough to implement.
- A Loaf containing Incan, Rust, and mixed conventional sources has defined planning behavior, including bounded Rust-facet inputs and no implicit Cargo participation.
- Target, carrier, profile, workspace delivery policy, and invocation precedence have explicit validation and receipt requirements.
- Provider operations, generators, actions, policy, and receipts have a complete no-hidden-effects boundary.
- RFCs 015, 073, 076, 077, 078, 114, and 116 identify exactly which manifest/discovery clauses RFC 117 supersedes or amends.
- The lock rename and legacy-manifest rejection have a complete pre-1.0 compatibility decision.
- RFC 118 has a clear command-ownership and alias boundary that does not reopen the manifest model.

## Implementation plan

### Phase 1: Manifest and workspace authority

- Introduce `Loaf.toml` discovery and validation; remove `incan.toml` discovery from project-aware operations.
- Implement rooted and virtual workspace shapes, explicit member selection, and precise coexisting-Cargo diagnostics.
- Update project creation, documentation, LSP/IDE manifest discovery, and fixtures.

### Phase 2: Typed graph, registries, and lock

- Normalize Loaf, crate, and capability dependencies into one resolver-owned graph.
- Implement default origins, registered registry identities, workspace allow-lists, integrity/trust facts, and the `Oven.lock` format.
- Amend RFC 034 integration and retain RFC 114's public-feature/provider boundaries.

### Phase 3: Target plan and controlled effects

- Implement target/carrier/profile/delivery validation and receipt identity.
- Make provider operations and declared generators plan-visible; connect policy and receipt facts from RFCs 076 and 104.
- Rehome RFC 116 package requirements into the generic interop envelope.

### Phase 4: Environment and action rehoming

- Amend and implement the `Loaf.toml` configuration location for RFC 073 environments/matrices and RFC 078 typed actions.
- Make Oven materialization, scope selection, dry-run, and policy gating consume the same project/workspace plan.

### Phase 5: Explicit Cargo compatibility and documentation

- Implement the distinct Cargo compatibility adapter and receipt/reporting boundary.
- Add compatibility fixtures for Cargo-only projects, Loaf-only Rust projects, mixed-source Loaves, workspaces, private registries, and C ABI target carriers.
- Document the command-surface handoff to RFC 118 without defining aliases or final command spelling here.

## Unresolved questions

1. What is the final compact TOML syntax for typed dependency entries and provider-specific feature requests, while preserving the unit-kind/origin distinction?
2. What exact table names should environments, matrices, actions, policy, and interop use under `Loaf.toml` after RFCs 073, 076, 078, and 116 are amended?
3. Which nonstandard source-root and generated-input declarations are necessary for the first 0.6 implementation, beyond convention-first `src/` discovery?
4. How should an organization distribute and administer trusted registry registrations without letting a checked-out project overwrite user or organization trust policy?
5. Which target/carrier combinations and named delivery-policy grammar constitute the first 0.6 proof set?
6. What is the smallest typed generator/provider extension API that supports real code generation and native shims without becoming a generic script hook?
7. Which `*.loaf` container or wire format, registry transport, and multi-asset publication protocol best carries the already-reserved immutable target-bound identity without changing its authored/lock/receipt contract?
8. What precise inspection meaning, if any, should cold, warm, and hot Loaves have after the Oven's cache and lifecycle model is settled?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. -->
