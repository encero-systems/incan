# RFC 114: Compiled providers, SDK components, and package features

- **Status:** Implemented
- **Created:** 2026-07-17
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 013 (Rust crate dependencies and parked Incan-native feature activation)
    - RFC 015 (project lifecycle and manifest ownership)
    - RFC 020 (offline, locked, and reproducible builds)
    - RFC 022 (stdlib namespacing and compiler handoff)
    - RFC 023 (compilable stdlib and Rust module binding)
    - RFC 031 (Incan library artifacts and the `pub` namespace)
    - RFC 034 (`incan.pub` package distribution)
    - RFC 075 (starter profiles and capability packs)
    - RFC 077 (workspaces and canonical locking)
    - RFC 112 (crash-safe local artifact publication)
- **Issue:** #544
- **RFC PR:** [#839](https://github.com/encero-systems/incan/pull/839)
- **Written against:** v0.5
- **Shipped in:** v0.5

## Summary

This RFC defines one generic compiled-provider model for Incan libraries and official SDK components, uses that model to split the standard library into independently describable artifacts, and activates the previously parked Incan-native package-feature model. An SDK installation advertises which components are available; a project profile and explicit component refinements determine which are enabled; source imports and compiler requirements determine which enabled providers are used. Public package features remain a separate additive dependency-selection mechanism, while Cargo features and other backend switches become private provider implementation details. The compiler resolves these inputs into one deterministic, session-owned provider plan consumed by checking, lowering, generated-project construction, LSP, inspection, and locking. The standard library is the first compiler-injected consumer of this generic machinery, not a permanent special case.

## Core model

Read this RFC as twelve foundations:

1. **The compiler core is not the standard library:** syntax, primitive types, intrinsic operations, and compiler-only namespace roots belong to the language implementation. Importable `std.*` modules belong to compiled providers, even when an official SDK distributes them with the compiler.
2. **A provider is generic:** a compiled provider contributes checked semantic facts, generated Rust implementation artifacts, dependency requirements, optional public features, provenance, and namespace claims. Ordinary Incan libraries and SDK components use the same provider contract.
3. **A component is a distribution unit:** an SDK component is a named, versioned, independently installable provider or provider bundle selected through an SDK profile. A component is not a source-language feature flag and is not a Cargo feature.
4. **A feature is a package-owned additive switch:** public Incan features select optional dependencies, dependency features, and feature-conditioned provider facts. Features compose by union and may add behavior or API surface; they must not subtract or reinterpret behavior that is available without them.
5. **A profile is a named component selection:** `minimal`, `default`, and `full` describe coherent SDK component sets for one SDK release. Profiles do not alter language semantics, dependency feature semantics, optimization level, or runtime configuration.
6. **Availability, enablement, and use are distinct:** an installed SDK may contain a component, a project may enable it, and a compilation may use one of its modules. Tooling and diagnostics must not collapse those three states.
7. **Imports never acquire software:** importing a missing module must not download, install, or silently enable an SDK component or package feature. Compilation diagnoses the unmet requirement; explicit lifecycle tooling changes project or installation state.
8. **Namespace ownership is explicit:** every provider declares the exact import modules it owns, and the resolved provider set must contain at most one owner for each canonical module path. Reserved roots such as `std` require toolchain authority and cannot be claimed by an arbitrary dependency.
9. **Backend switches are derived and private:** a provider may map used modules and public features to Cargo features, cfg values, linked crates, or future backend controls. Those mappings are part of the provider artifact and must not become a second user-facing feature system.
10. **The lock records the resolved semantic graph:** exact provider identities, component selections, public feature closure, implementation requirements, and artifact integrity belong in the canonical lock state whenever they can change checking or generated output.
11. **One provider plan feeds every compiler surface:** build, run, test batches, library builds, LSP, codegraph, documentation, and inspection must agree on provider availability, module ownership, feature closure, and diagnostics.
12. **The current monolith is a migration input, not the architecture:** existing standard-library source, Rust runtime support, and cached artifacts may be repackaged incrementally, but new semantic authority must not be added to a hardcoded stdlib inventory or an independent stdlib-only manifest path.

## Motivation

The standard library is currently special in several overlapping ways. The compiler knows a hand-maintained module inventory, imports activate Rust crate features, generated projects wire a monolithic runtime crate, source modules can be materialized into consumer output, and compiled-library work introduces another artifact path beside ordinary library manifests. Each individual bridge was useful while the language and library system were young, but together they make the stdlib difficult to derive, difficult to slim down, and too easy to evolve through compiler-specific exceptions.

This becomes visible as soon as compiled standard-library artifacts are packaged. A single precompiled artifact can improve build time and fresh-install behavior, but a single artifact containing every standard module also fixes the SDK's size and dependency closure at the largest possible setting. Conversely, splitting files without a provider contract merely moves a hardcoded list into more places. Users should eventually be able to install or enable a small SDK for constrained use, a conventional SDK for ordinary development, or the complete official surface without changing import syntax or teaching the compiler a new path for every component.

The same architectural pressure appears in third-party libraries. RFC 031 already defines checked `.incnlib` facts plus a generated Rust crate, but its first phase assumes one unconditional public surface. RFC 013 parses the seed of `[project.features]` and supports Cargo optional dependencies, while deliberately postponing an Incan-native activation model. If SDK components are implemented as Cargo features, that postponement becomes permanent leakage: users would need to understand backend crate layout to select language libraries, lockfiles would conflate semantic and implementation choices, and a future non-Rust backend would inherit Cargo-shaped product semantics.

The desired end-state is a compiler that receives a resolved set of compiled providers. The official standard library happens to be supplied by the active SDK and authorized to own `std.*`; an ordinary dependency happens to be selected from the project dependency graph and owns `pub.<dependency>.*`. Both contribute the same categories of checked facts and implementation requirements. Components decide which official artifacts can participate, features decide which additive package capabilities participate, imports decide which available modules are actually used, and the backend performs only the resulting implementation work.

## Goals

- Define a generic compiled-provider contract that can represent ordinary Incan libraries and SDK-supplied libraries without a stdlib-only semantic manifest path.
- Define available, enabled, and used provider states and expose them consistently in diagnostics, build reports, inspection, LSP, and codegraph output.
- Define SDK component identities, dependencies, compatibility, profiles, project refinements, and lockfile behavior.
- Preserve stable `std.*` source imports independently of how official modules are packaged into SDK components.
- Define an initial component split for the v0.5 standard-library surface that can be packaged independently without pretending every source module needs its own distribution artifact.
- Activate a first-class Incan package-feature graph based on the existing `[project.features]` direction, including defaults, optional Incan dependencies, dependency feature requests, deterministic additive unification, and targeted diagnostics.
- Keep public Incan features distinct from private Cargo features and other backend implementation controls.
- Extend checked library artifacts so module exports, runtime dependencies, soft syntax, registry facts, and other provider facts can declare feature requirements without requiring consumers to reparse provider source.
- Make provider resolution a session-owned input shared by single-project, workspace, test-batch, library, LSP, documentation, and inspection paths.
- Define deterministic lock and artifact-integrity requirements for provider selection and feature closure.
- Preserve offline compilation: provider resolution may consume installed, workspace, cached, or locked artifacts, but it must not perform implicit network acquisition.
- Leave room for future official and third-party provider families without granting ordinary packages authority over reserved namespace roots.

## Non-Goals

- Defining a managed toolchain installer, remote component repository protocol, delta updater, or garbage collector in this RFC.
- Defining registry hosting, package ranking, signing infrastructure, or trust policy beyond the local authority checks required to protect reserved import roots.
- Turning every standard-library module into a separate package or promising that the initial component boundaries can never be revised before the language and SDK compatibility contract is declared stable.
- Letting projects replace language intrinsics, primitive types, compiler-owned derives, or the `rust` import root through providers.
- Treating RFC 075 starter profiles or capability packs as compile-time features. Those remain explicit project-mutation recipes.
- Treating RFC 073 environment matrices, build optimization profiles, target triples, runtime configuration, or policy capabilities as SDK components or package features.
- Automatically downloading a component because source imported one of its modules.
- Exposing Cargo feature names, generated crate names, generated module paths, or Rust dependency layout as stable Incan package API.
- Defining subtractive, mutually exclusive, value-carrying, target-predicate, or runtime feature flags in the initial package-feature model.
- Defining arbitrary third-party ownership of the `std` namespace.

## Guide-level explanation

### The default project remains boring

A project that does not mention SDK selection uses the release's `default` SDK profile and the default features of its Incan dependencies. Existing `std.*` imports continue to look the same:

```incan
from std.fs import Path
from std.json import JsonValue
```

The important change is underneath the source. The active SDK inventory identifies which components provide `std.fs` and `std.json`; the project selection enables those components; the provider plan loads their checked manifests; and the generated project links only the implementation requirements selected by the used modules and public feature closure.

### Select a smaller SDK surface explicitly

A project that wants a deliberately small standard surface may select `minimal` and add only the components it needs:

```toml
[sdk]
profile = "minimal"
components = ["stdlib-system", "stdlib-data"]
```

Profile expansion is deterministic. `stdlib-core` is always present, selected components bring their declared component dependencies, and the expanded selection is recorded in `incan.lock`. Selecting a component does not promise that the local SDK installation contains it. If `stdlib-data` is enabled by the project but unavailable in the installation, the compiler reports an installation problem rather than pretending the import is unknown.

A compatibility-oriented project may start from the conventional profile and remove components it deliberately forbids:

```toml
[sdk]
profile = "default"
exclude-components = ["stdlib-web", "stdlib-observability"]
```

An exclusion is checked after dependency expansion. Excluding `stdlib-async` while retaining `stdlib-web` is an error because the web component declares an async dependency. The resolver must show the dependency path that made the exclusion invalid.

### Public features belong to packages

An Incan library may define additive public features in its project metadata. The existing list form becomes a parsed reference language rather than an opaque list of strings:

```toml
[project.features]
default = ["json"]
json = ["dep:serializer", "serializer/json"]
full = ["json", "http"]
http = ["dep:http_client", "http_client/tls"]

[dependencies]
serializer = { path = "../serializer", optional = true, default-features = false }
http_client = { path = "../http_client", optional = true, default-features = false }
```

Within a feature entry, a plain name refers to another feature in the same package, `dep:<name>` activates an optional Incan dependency, and `<dependency>/<feature>` requests one public feature from an active Incan dependency. These are checked identifiers with source-anchored manifest diagnostics; they are not arbitrary backend arguments.

A consumer enables dependency features on the dependency declaration:

```toml
[dependencies]
reporting = { path = "../reporting", default-features = false, features = ["json"] }
```

Command-line selection uses Incan-owned flags:

```text
incan build --features json,http
incan test --all-features
incan check --no-default-features
```

Cargo pass-through keeps its existing explicitly prefixed surface such as `--cargo-features`. An Incan `--features` argument must never be forwarded blindly to Cargo.

Features do not silently activate from imports. If `pub.reporting.JsonReport` exists only when `reporting/json` is enabled, importing it without that feature produces a diagnostic that names the disabled feature and the dependency declaration that can enable it. This keeps source, manifest, lock state, CI, and editor behavior aligned.

### Components and features may meet without becoming the same thing

A package feature may expose API that requires an SDK component, but it does not own or install that component. Its provider facts declare the requirement. When the feature is active, provider resolution verifies that the required component is enabled and available. A package should therefore be able to say that its `server` feature requires `stdlib-web`, while the project still controls SDK composition under `[sdk]`.

The initial feature-reference shorthand does not directly add or remove SDK components. This is intentional: a command-line `--features server` must not mutate the installed SDK or silently override a project's explicit component exclusions. Tooling may offer an explicit manifest edit when it diagnoses the missing requirement.

### Inspect what the compiler resolved

Users and tools should not need to infer component or feature state from generated Cargo files:

```text
incan inspect providers --format json
incan inspect features --format json
```

Provider inspection shows the active SDK identity, available components, project-enabled components, transitive component reasons, used modules, provider artifact identities, public features, implementation facets, and source or manifest provenance. Feature inspection shows roots, defaults, dependency requests, unified closure, optional dependencies, feature-conditioned facts, and why each feature became active.

Human diagnostics distinguish the important failure classes:

```text
error: `std.web` is provided by SDK component `stdlib-web`, but that component is disabled for this project
help: add `stdlib-web` to `[sdk].components` or select a profile that includes it
```

```text
error: SDK component `stdlib-web` is enabled but is not available in the active Incan SDK
note: active SDK: 0.5.0 (minimal installation)
help: install an SDK distribution containing `stdlib-web`; compilation will not download it automatically
```

```text
error: `JsonReport` requires feature `reporting/json`
help: add `features = ["json"]` to dependency `reporting`
```

## Reference-level explanation

### Terminology

- A **provider** is one resolved compiled-library input containing checked semantic facts, implementation artifacts, dependency requirements, compatibility metadata, and provenance.
- A **provider identity** is the stable package or SDK artifact identity plus version, artifact digest, and feature projection used for locking and diagnostics.
- A **namespace claim** is the set of canonical import module paths for which a provider supplies semantic facts.
- An **SDK component** is an independently describable official distribution unit whose provider artifacts and component dependencies are listed in the active SDK inventory.
- An **SDK profile** is a release-owned named set of component ids.
- An **available component** is present and integrity-checked in the active SDK installation.
- An **enabled component** belongs to the project selection after profile expansion, explicit additions, exclusions, and transitive component dependency resolution.
- A **used provider module** is reached by source imports, reexports, soft-syntax activation, compiler-required runtime support, or another checked provider fact in the active compilation graph.
- A **public feature** is a package-owned additive selection visible in Incan manifests, checked artifacts, locks, diagnostics, and inspection.
- An **implementation facet** is a provider-owned backend requirement derived from used modules and public features, such as one Cargo feature, linked native library, generated helper, or future backend capability.

### Provider artifact contract

A provider must make the following information available before consumer source is typechecked against it:

- provider name, version, manifest schema version, compiler compatibility, and artifact digest;
- namespace authority and exact canonical module claims;
- checked exports, declaration identities, type facts, trait facts, soft-syntax activations, registry facts, documentation facts, and Rust ABI metadata relevant to the claimed modules;
- public feature declarations, default feature set, and feature requirements attached to provider facts;
- provider dependencies and requested public features;
- required SDK components, optionally conditioned on public features;
- generated Rust artifact identity and relocation information;
- implementation-facet mappings and backend dependency requirements;
- source, package, and generated-artifact provenance suitable for diagnostics and inspection.

The semantic portion extends the RFC 031 `.incnlib` contract. A provider must not require consumers to parse, typecheck, or lower provider `.incn` source. The physical distribution may keep semantic facts and generated Rust in one archive or adjacent files, but the resolver must present one validated provider record to compiler stages.

Provider artifacts must be relocatable within their declared distribution root. Absolute producer paths, workspace-only crate paths, and ambient source checkout assumptions are invalid in published or SDK-installed providers.

### Namespace authority and collision rules

The compiler owns the import roots and grants provider authority as follows:

- ordinary Incan dependencies receive the `pub.<dependency-key>` namespace derived from the consumer's resolved dependency graph;
- official SDK providers may receive exact `std.*` claims only through the integrity-checked active SDK inventory;
- compiler-only roots and modules remain compiler-owned and are not provider claims;
- future provider kinds may receive other explicit roots only through an RFC-defined authority mechanism.

An ordinary package manifest must not self-assert ownership of `std.*`. A copied or modified provider artifact outside an authorized SDK inventory remains an ordinary package and cannot acquire reserved-root authority from metadata alone.

Two enabled providers must not claim the same canonical module path. Parent namespaces may be virtual aggregation nodes, so providers may independently own `std.fs` and `std.web` without either claiming the bare `std` root. A provider that owns a facade module may reexport declarations from provider dependencies, but the reexport retains canonical declaration identity and provenance and does not create a second owner for the source declaration.

Collision validation occurs before consumer typechecking. The diagnostic must identify both provider identities, both claim sources, the conflicting module path, and the project or SDK selections that introduced them.

### SDK inventory

Every component-aware SDK distribution must contain one integrity-checked inventory that declares:

- SDK identity and compiler compatibility;
- every component id and version known to that SDK release, including components omitted from the local installation;
- installation availability for each known component;
- expected component artifact identities and digests, plus local artifact locations only for installed components;
- provider records supplied by each component;
- component dependency edges;
- named profile membership;
- reserved namespace grants;
- whether a component is mandatory for that SDK release;
- enough provenance to distinguish official, locally developed, and overridden artifacts.

The compiler must discover the inventory relative to the active executable or an explicit toolchain root, not from a repository-specific absolute path. An SDK without a component inventory is treated as a legacy monolithic installation during migration and projects using explicit component selection must receive a compatibility diagnostic.

Component dependencies are additive and acyclic. A cycle, unknown component, incompatible component version, digest mismatch, or missing artifact makes the SDK inventory invalid. The compiler must not fall back from an invalid installed component to source material in a nearby checkout.

### Project component selection

The project-level `[sdk]` table has this initial shape:

```toml
[sdk]
profile = "default"
components = ["stdlib-web"]
exclude-components = ["stdlib-observability"]
```

- `profile` selects one named profile from the active SDK and defaults to `default` when omitted.
- `components` adds component ids to the profile selection.
- `exclude-components` rejects component ids from the final selection and exists to make deliberate slimming reviewable.
- the mandatory component set is always included and cannot be excluded;
- transitive component dependencies are added unless explicitly excluded, in which case resolution fails with the dependency path;
- unknown profile and component ids are configuration errors;
- duplicate entries are accepted only after deterministic deduplication and should produce a lint when they obscure intent.

The expanded enabled set is compared with the installed available set. A component may therefore be disabled, enabled and available, or enabled but unavailable. Even a minimal installation must retain the release's small component catalog so the compiler can distinguish a known omitted component from an unknown id. An unavailable component is never silently treated as disabled because the remedies differ.

Profile membership is versioned with the SDK. Lock generation records the expanded component ids and exact provider artifacts, not only the profile name, so an updated SDK profile cannot silently alter an existing locked build. A lock refresh may adopt changed profile membership after presenting the graph change in machine-readable and human output.

### Public package features

`[project.features]` defines named public features for the current Incan package. Each entry is an array whose items use this grammar:

```text
feature_member ::= feature_name
                 | "dep:" dependency_name
                 | dependency_name "/" feature_name
```

The compact list remains the preferred form for small feature graphs. A feature may instead use an expanded table when its different edge kinds benefit from explicit structure:

```toml
[project.features.server]
includes = ["json"]
optional-dependencies = ["http_server"]
dependency-features = { http_server = ["tls"] }
requires-sdk-components = ["stdlib-web"]
```

`includes` names local features, `optional-dependencies` activates optional Incan dependencies, `dependency-features` requests features from active Incan dependencies, and `requires-sdk-components` records checked requirements without enabling or installing those components. Compact and expanded definitions are mutually exclusive for one feature and normalize into the same typed graph before validation.

Feature and dependency names must satisfy the corresponding manifest identifier rules. References are resolved structurally and validated for unknown packages, unknown features, non-optional dependencies behind `dep:`, dependency-feature references to inactive optional dependencies, cycles, and malformed spellings. Diagnostics point to the exact array item.

`default` is the conventional root feature set. It is enabled for a package unless the root command uses `--no-default-features` or the consumer dependency declaration sets `default-features = false`. A dependency declaration may request public features with `features = [...]`. Root commands may add root-package features with `--features`, select every declared root feature with `--all-features`, or suppress the root default with `--no-default-features`.

Feature resolution is additive. The active set for one package is the union of its default selection, every requesting parent edge, root command selection, and recursively enabled local features. Enabling a feature twice has no additional effect. A feature cycle is rejected even when a fixed-point implementation could compute it, because cycles obscure ownership and make diagnostics and future evolution harder.

Features may activate optional Incan dependencies and request features from active Incan dependencies. They do not directly contain Cargo feature names or Rust crate dependency keys. Rust dependency choices remain provider implementation facts or explicit `rust-dependencies` authored by the package itself.

The initial model has no negative features, values, precedence, or mutual exclusion. A feature must not remove a declaration, change the type or meaning of an unconditional declaration, disable another feature, or select one of several incompatible global modes. Those requirements should use separate packages, typed runtime configuration, target configuration, or a future explicitly non-additive mechanism.

### Feature-conditioned provider facts

Checked provider facts may carry a positive requirement predicate consisting of one or more public features owned by that provider. A fact participates only when every required feature is active. This applies uniformly to modules, exports, provider dependencies, SDK component requirements, soft-syntax activations, registry entries, documentation entries, and implementation facets.

The provider artifact must contain enough information to validate and inspect every feature-conditioned public fact without executing provider code. Publishers must verify the supported feature projections before publication. A consumer selects the projection after resolving feature closure and before making provider facts available to parsing, checking, lowering, LSP, or documentation.

Authors attach feature requirements with a general compile-time `when` block whose condition uses the compiler-owned `feature` predicate:

```incan
when feature("json"):
    from std.json import JsonValue

    pub def encode(value: Report) -> JsonValue:
        ...
```

At compilation-unit scope, the block may contain imports, reexports, declarations, registry entries, or other forms that are otherwise valid at that scope. The condition is evaluated while the active source projection is built; it does not execute user code. `when` is the general compile-time conditional form and `feature(...)` is one typed compiler predicate, so the language does not acquire a narrowly scoped `feature` statement. The initial grammar accepts only positive feature predicates and conjunctions of positive feature predicates. Negation, disjunction, target predicates, values, and runtime expressions remain outside this RFC.

### Implementation facets

A provider may declare deterministic mappings from active semantic facts to implementation facets. For example, use of `std.web` may require one web facet, and that facet may map to a Cargo crate feature and several Rust dependencies for the current backend. The mapping belongs to the provider artifact and is validated when the provider is built.

Implementation facets are not public Incan features:

- users do not select them with `[project.features]` or `--features`;
- import resolution may derive them because imports already selected semantically available modules;
- backend changes may replace them without changing source or public package features;
- their resolved values appear in build reports and fingerprints for reproducibility, but generated backend names are not stable package API;
- a provider must not expose a public API only because an undeclared implementation facet happened to be enabled by another dependency.

This supersedes the direction in RFC 022 that treated import-driven `incan_stdlib` Cargo features as the durable feature model. Import-driven backend minimization remains valid, but Cargo feature names become private implementation-facet data rather than compiler-owned stdlib inventory.

### Provider-plan construction

Before semantic checking of consumer bodies, the toolchain constructs one provider plan from:

1. the active SDK inventory and installed component set;
2. the project or workspace SDK profile and component refinements;
3. resolved Incan dependency provider artifacts;
4. root and dependency public feature requests;
5. the canonical lock state and active offline or locked policy;
6. module use discovered through imports, reexports, test context, and compiler-required support facts.

The plan contains provider identities, namespace ownership, active public feature sets, active provider facts, component reasons, used modules, implementation facets, backend dependencies, provenance, and diagnostics. Stages may add use facts as module resolution progresses, but they must update the same plan rather than construct an independent provider interpretation.

Single-file mode uses the active SDK's `default` profile unless the command supplies an explicit non-persistent profile override. It has no project feature graph or Incan package dependencies. A single file that requires a disabled or unavailable component receives the same diagnostic categories as a project build.

### Locking and reproducibility

The canonical `incan.lock` graph must record every resolved input that can change checked semantics or emitted output, including:

- SDK identity and inventory digest;
- selected profile name and expanded component ids;
- exact component and provider artifact identities and digests;
- public feature sets per package;
- optional dependency activation and dependency feature requests;
- provider implementation-facet closure or a stable fingerprint of that closure;
- backend dependency resolution governed by RFC 020;
- workspace member roots whose selections contributed to the graph.

`--locked` fails when current manifest, SDK inventory, component availability, provider artifact, feature request, or implementation requirement differs from the lock. `--frozen` adds the existing no-network and no-mutation guarantees. Neither mode may repair an unavailable SDK component by downloading it.

Lock publication and reusable provider-cache publication use RFC 112's coordinated crash-safe recipe. An interrupted publisher must leave either the previous complete artifact or the new complete artifact visible; readers must not consume a partially replaced manifest or provider archive.

### Workspace behavior

Each workspace member may declare its own SDK profile refinements and root features. The workspace root lock records the resolved graph for every member as required by RFC 077. Feature unification occurs within each selected compilation graph; the lock may share one provider artifact and backend dependency resolution across members, but inspection must retain which member roots requested each feature and component.

Workspace-level defaults may be added through RFC 077's explicit inheritance model. A member must opt into inherited SDK or feature policy and may refine only fields the workspace contract marks refinable. Tooling must not infer one workspace-wide component set merely because members share a lockfile.

### Diagnostics

Provider and feature diagnostics must distinguish at least:

- unknown `std.*` module with no provider claim in the active SDK inventory;
- known module whose component is disabled by project selection;
- enabled component unavailable from the active SDK installation;
- installed artifact missing, corrupt, incompatible, or failing integrity validation;
- conflicting namespace claims;
- package export or module hidden behind a disabled public feature;
- unknown feature, feature cycle, inactive optional dependency, or invalid dependency-feature request;
- component exclusion conflicting with a transitive component requirement;
- provider requirement incompatible with the active SDK or target;
- locked provider, feature, component, or implementation closure differing from current inputs.

Every diagnostic must name the owning provider or component when known, identify the manifest entry or source import that created the requirement, and provide a remedy at the correct layer. A missing installation must not suggest editing an import; a disabled component must not suggest downloading when it is already installed; and a disabled feature must not suggest a Cargo flag.

## Design details

### Why providers, components, and features share one RFC

These concerns are coupled at the checked-artifact boundary, not merely because the standard library happens to use all three. A provider manifest must say which modules exist, which public features condition its facts, which SDK components satisfy its external requirements, which backend facets implement the active projection, and which identities enter the lock. Defining components without package features would either freeze providers to one unconditional surface or leak backend features into the public model. Defining features separately without providers would leave no backend-neutral artifact capable of projecting feature-conditioned semantic facts. One RFC therefore owns the shared resolution and artifact contract while preserving the distinct user meanings of component, profile, feature, and implementation facet.

### Initial official standard-library components

The v0.5 standard-library source graph is grouped into nine component artifacts. The grouping follows public capability and dependency cohesion rather than source-file count:

| Component | Public module ownership | Direct component dependencies | Profile membership in v0.5 |
| --- | --- | --- | --- |
| `stdlib-core` | `std.prelude`, `std.result`, `std.traits.*`, `std.derives.*`, `std.this`, `std.reflection` | — | mandatory, `minimal`, `default`, `full` |
| `stdlib-system` | `std.environ`, `std.io`, `std.fs.*`, `std.tempfile` | `stdlib-core` | `default`, `full` |
| `stdlib-codecs` | `std.encoding.*`, `std.checksum` | `stdlib-core`, `stdlib-system` | `default`, `full` |
| `stdlib-compression` | `std.compression.*` | `stdlib-core`, `stdlib-system` | `default`, `full` |
| `stdlib-data` | `std.collections`, `std.graph`, `std.hash.*`, `std.math`, `std.datetime.*`, `std.uuid`, `std.regex.*`, `std.json`, `std.serde.*` | `stdlib-core`, `stdlib-system` | `default`, `full` |
| `stdlib-async` | `std.async.*` | `stdlib-core` | `default`, `full` |
| `stdlib-observability` | `std.logging`, `std.telemetry.*` | `stdlib-core`, `stdlib-data` | `default`, `full` |
| `stdlib-web` | `std.web.*` | `stdlib-core`, `stdlib-data`, `stdlib-async` | `default`, `full` |
| `stdlib-testing` | `std.testing` | `stdlib-core` | `default`, `full`; activated for test contexts when enabled |

`std.rust` capability traits and `std.builtins` escape calls remain compiler-owned symbolic surfaces and are not provider artifacts. Language primitives and ambient prelude behavior that cannot be expressed as an importable checked provider fact also remain compiler-owned, but their boundary must be documented rather than hidden inside the component inventory.

The v0.5 `default` and `full` profiles intentionally contain the same stable standard-library modules to preserve source compatibility while the component system lands. They are distinct profile identities because later releases may keep the compatibility-oriented default smaller than the complete stable official set. The lock records expanded membership, so that future policy change is explicit rather than silently inherited.

The initial dependency edges describe the intended public provider graph rather than freezing every historical source grouping. `std.collections` uses ordinal hashing and UUID versions 3 and 5 use the public hashing algorithms, so `std.hash.*` belongs to `stdlib-data` with those consumers. Encoding and checksums remain a dependency-light codecs component, while compression is physically separate because its native and algorithm-specific dependency closure must not enter projects that only need base encodings. Disabling `stdlib-codecs` leaves hashing available through `stdlib-data`; disabling `stdlib-compression` independently makes `std.compression.*` unavailable.

Component boundaries are allowed to change while the language and SDK compatibility contract remains explicitly pre-stable, provided the SDK release changes and locks expose the expanded graph. Public import paths, declaration identities, and compatibility promises remain the stable user surface. Once component compatibility is declared stable, moving a module between components requires ordinary compatibility and migration policy even when its `std.*` path stays unchanged, because project exclusions and installation profiles may depend on component identity.

### Profiles are distribution policy, not package API

`minimal` contains only the mandatory compiler and `stdlib-core` contract. It exists for constrained installations, compiler bootstrap, and projects that want every optional standard capability to be deliberate.

`default` is the compatibility-oriented SDK experience used when a project makes no SDK selection. For v0.5 it includes every initial stable standard-library component so existing imports continue to work.

`full` contains every stable official component made available by that SDK release. Experimental, preview, vendor, and locally overridden providers must not enter `full` merely because they are installed.

Profile names are stable concepts, while their expanded membership is release-owned data. Projects that need a permanent exact surface should use explicit exclusions or a minimal profile with explicit additions and commit the lockfile.

### Public feature facts and source authoring

The provider artifact model deliberately supports feature requirements on all checked fact kinds rather than only generated Rust items. A source-level feature mechanism that gates emission but leaves an export visible to checking would be invalid. Likewise, a documentation generator or LSP that ignores the active feature projection would be inconsistent with compilation.

The source form is a top-level `when feature("name"):` block. It may gate declarations, imports, reexports, and registry entries together, which keeps related optional API in one readable unit and avoids requiring a named `module` declaration solely to annotate the current file. Nested `when` blocks form an additive conjunction. The parser retains the condition as typed syntax; semantic collection attaches the resulting positive feature requirement set to every checked fact produced by the block. Inactive blocks remain available to formatting, source navigation, and feature-aware documentation, but do not enter the active typechecking or emission projection.

The compiler-owned `feature` predicate resolves names in the current package only. Dependency features are selected in manifests and appear in that dependency's own source projection; source cannot reach across package ownership with `feature("dependency/name")`. The predicate is not a normal function, cannot be imported or shadowed, and is rejected outside compile-time `when` conditions.

### Artifact publication and caching

Provider artifacts are immutable by identity. A cache key includes provider digest, compiler compatibility, public feature projection when physical specialization is required, target/backend inputs, and implementation-facet fingerprint. Content-addressed storage should allow identical artifacts used by several projects or SDK profiles to share one physical copy.

The SDK inventory must not require a project-local copy of an unchanged provider artifact. Generated consumer projects should reference or stage artifacts from a shared immutable cache using relocation-safe metadata. Packaging may include a compressed seed for offline first use, but extraction must deduplicate by artifact identity and must not reproduce the same generated source tree for every project.

The logical provider contract does not require one permanent archive encoding. A future binary `.incnlib` representation or compressed provider bundle may replace JSON-heavy or duplicated storage as long as inspection remains available and compatibility is versioned. Format choice must be measured against installed size, compressed distribution size, load latency, random access, duplication, and forward-compatible schema evolution.

### Interaction with RFC 075 capability packs

An SDK component changes which compiled providers are available to a project. A package feature changes the additive dependency or checked-fact projection of a package. A capability pack changes project files and manifest entries through a reviewable mutation plan. These mechanisms may cooperate, but they do not substitute for one another.

For example, a web starter may apply a capability pack that adds `stdlib-web` to `[sdk].components`, enables a package's `server` feature, writes an entrypoint, and adds scripts. After the mutation, compilation depends only on the resulting ordinary manifest and source state. The capability provenance is not a hidden feature switch, and the SDK component is not considered enabled merely because the starter once mentioned it.

### Compatibility and migration

Projects without `[sdk]` retain the v0.5 standard-library surface through the `default` profile. Existing `std.*` import syntax does not change. Existing Cargo-oriented stdlib feature wiring may remain as a backend adapter during migration, but its inputs must be derived from provider implementation facets and it must have a named removal condition.

Existing `[project.features]` data has been parsed but has not had a complete public semantic contract. Activating the rules in this RFC may reject malformed or cyclic declarations that were previously ignored. Because ignored feature entries could not reliably affect builds, this is a validation tightening rather than a supported behavior break.

Compiled standard-library artifact work must migrate from one hardcoded built-in manifest injection path to the generic provider plan. The current standard-library source inventory may bootstrap provider construction, but the published provider's checked namespace claims become the consumer authority. A hand-maintained compiler list must not remain necessary for adding or removing an ordinary `std.*` provider module.

## Alternatives considered

### Keep one monolithic standard-library artifact

One artifact is simpler to build and cache, but it fixes every installation and project to the complete dependency closure, prevents meaningful minimal distributions, and encourages compiler code to treat the stdlib as one permanent special dependency. It remains a useful migration bundle, not the target architecture.

### Use Cargo features as SDK components and Incan features

This reuses existing backend behavior but exposes Rust crate topology as language and packaging semantics. It cannot distinguish installed components from enabled package capabilities, gives a future backend the wrong abstraction, and makes diagnostics and locking depend on backend vocabulary. Cargo features remain a valid private implementation mechanism behind providers.

### Give every standard module its own component

This maximizes theoretical choice while producing excessive artifact, lock, compatibility, and dependency-graph overhead. Public modules are not all independent: filesystem, streaming codecs, datetime, UUID, logging, and web currently form real dependency clusters. Coherent capability artifacts provide useful slimming without turning every import into distribution administration.

### Infer provider contents from a source `lib.incn` at consumer time

Deriving an artifact's module graph from checked producer source is valuable during provider construction. Asking consumers to parse provider source violates RFC 031's compiled-library boundary, makes installed SDK behavior depend on source layout, and duplicates checking work. The producer derives and validates the provider manifest once; consumers trust the integrity-checked artifact.

### Automatically enable components or features from imports

Import-driven activation feels convenient but makes lock and project intent incomplete, prevents deliberate exclusions, and turns a typo or dead import into a dependency mutation. Imports determine use after availability and enablement have been resolved. Diagnostics and explicit lifecycle commands handle unmet requirements.

### Treat SDK profiles as RFC 075 starter profiles

Starter profiles create or mutate project structure and leave ordinary files behind. SDK profiles are versioned selections inside an installed toolchain and participate directly in compilation and locking. Reusing the word does not make their lifecycle or authority equivalent.

### Allow subtractive and mutually exclusive features immediately

Negative or exclusive features introduce order, global-choice, and graph-conflict semantics that additive unification avoids. They are also commonly misused for runtime configuration. The initial model stays monotonic; incompatible implementations should be separate providers or typed runtime selections until a stronger compile-time configuration model is justified.

## Drawbacks

This design introduces more named concepts than a monolithic stdlib with Cargo features: providers, components, profiles, public features, implementation facets, and three participation states. The distinctions are necessary for truthful diagnostics and future backends, but documentation and inspection must make them understandable.

Splitting artifacts creates packaging and test-matrix cost. Every component edge, profile, feature projection, relocation path, and missing-artifact state needs verification. Small components may also increase metadata and archive overhead enough to offset their payload savings, so the initial nine-way split must be measured rather than assumed optimal.

Feature-conditioned checked artifacts require richer manifests and a new source-level conditional-compilation form. The shared projection prevents backend leakage, but it also expands parser, formatter, semantic, documentation, and editor scope beyond artifact publication alone.

The compatibility-oriented v0.5 default profile does not immediately reduce the installed size for ordinary users. It creates the ability to ship and select smaller distributions while preserving existing imports; later profile policy and installer work are required to realize the full distribution benefit.

## Implementation architecture

Implementations should introduce a provider-resolution layer above existing library-manifest indexing. It should normalize SDK and project dependencies into provider records, validate namespace authority and feature projection, and publish one immutable plan to compiler stages. Standard-library construction may use toolchain bootstrap knowledge, but consumers should see ordinary validated provider facts with an SDK provenance kind.

The active SDK should publish component artifacts into a shared content-addressed store using RFC 112 coordination. The SDK inventory references immutable identities, while projects and generated builds reference those identities instead of copying complete source trees. Backend adapters translate provider implementation facets into current Cargo configuration at the final project-generation boundary.

Provider construction may use one staging workspace, but the published artifacts must be independently addressable and physically excludable from an SDK profile package. A component need not have a dedicated top-level release archive: one distribution archive may contain several selected component artifacts. Packaging tests must prove that excluded component payloads and their exclusive dependency closure are absent, that common immutable content is stored only once, and that every installed artifact retains relocation and integrity guarantees. Size and dependency measurements are release evidence rather than a discretionary gate that permits shipping a semantic or physical monolith.

## Layers affected

- **Project and workspace manifests:** must parse, validate, and preserve SDK selection, public feature declarations, dependency feature requests, optional Incan dependencies, and source-anchored configuration diagnostics.
- **SDK discovery:** must locate and integrity-check the active SDK inventory relative to the toolchain, distinguish legacy installations, and expose available component artifacts without repository-specific paths.
- **Dependency and lock resolution:** must expand profiles, component dependencies, public feature closure, optional Incan dependencies, provider requirements, and exact artifact identities under offline and locked policy.
- **Library artifact construction:** must derive namespace claims and checked provider facts from producer semantics, encode public feature declarations and requirements, validate relocation, and publish complete immutable artifacts.
- **Parser and module resolution:** must consume the shared provider namespace map and active feature projection, preserve import-driven soft syntax where declared, and emit disabled, unavailable, and unknown-module diagnostics distinctly.
- **Typechecker:** must resolve declarations and types only from active provider facts and retain canonical provider provenance in semantic identities and diagnostics.
- **Lowering and emission:** must consume provider identities and implementation facets from the shared plan and must not independently infer stdlib activation from source paths.
- **Generated-project construction:** must link resolved provider Rust artifacts and translate private implementation facets to backend dependencies without exposing those backend names as Incan API.
- **Test runner:** must use the same provider and feature plan for discovery, package batches, generated Rust, and test-only providers; test context must not silently widen the project's production component set.
- **LSP and formatter:** LSP must use project component and feature state for completion, navigation, hover, diagnostics, and code actions; formatting remains independent of availability and must preserve valid conditional source syntax once defined.
- **Inspection, reports, and codegraph:** must project provider, component, feature, artifact, and provenance facts from the shared plan with deterministic schemas.
- **Documentation and release tooling:** must build references against declared profile and feature projections, identify unavailable optional surfaces honestly, and package the selected SDK artifacts without duplicate mutable caches.

## Implementation Plan

### Phase 1: Shared provider model

- Introduce backend-neutral provider identities, provenance, namespace claims, checked facts, implementation facets, feature requirements, component requirements, and one immutable session-owned provider plan.
- Extend the existing library manifest index instead of creating a second stdlib-only semantic index, and validate namespace collisions and reserved-root authority before consumer typechecking.
- Route build, run, library, test-batch, LSP, diagnostics, codegraph, documentation, and generated-project construction through the shared plan; retain any migration adapter only with a named removal condition.

### Phase 2: SDK inventory, components, and profiles

- Define and validate the relocatable SDK inventory, component catalog, provider artifact locations and digests, component dependencies, mandatory membership, profiles, and reserved namespace grants.
- Discover the active inventory relative to the executable or explicit toolchain root, then resolve project `[sdk]` profile, additions, exclusions, dependency expansion, availability, and lock state without implicit acquisition.
- Publish the nine initial stdlib component artifacts into the shared immutable cache and prove that minimal-profile packages omit excluded payloads and exclusive dependencies.

### Phase 3: Public package-feature graph

- Parse compact and expanded feature declarations, dependency feature requests, optional Incan dependencies, defaults, and root command selections into one typed graph with source-anchored validation.
- Resolve additive feature closure for root and transitive packages, reject cycles and malformed or inactive edges, and record activation reasons in provider plans, locks, reports, and inspection.
- Keep Cargo features and other backend controls private by deriving them only from provider implementation facets after semantic provider resolution.

### Phase 4: Compile-time source projection

- Add parser, AST, formatter, and documentation support for compilation-unit `when feature("name"):` blocks and positive conjunctions.
- Attach positive requirements to every fact collected inside a block, retain inactive syntax for tooling, and project active imports, reexports, declarations, registry entries, documentation facts, and implementation facets consistently.
- Reject unsupported predicates, runtime use, cross-package feature names, and non-additive conditions with targeted diagnostics.

### Phase 5: Artifact, cache, lock, and backend integration

- Extend `.incnlib` provider records and generated Rust artifacts with feature-conditioned facts, component requirements, implementation facets, integrity, relocation, and provenance.
- Reuse RFC 112 atomic publication and advisory locking for shared content-addressed provider artifacts, compact offline seeds, and canonical lock publication without project-local duplication.
- Remove the stdlib-only built-in manifest side channel after every consumer path resolves official providers through the generic plan.

### Phase 6: Parity, documentation, and release proof

- Add direct-import, facade/reexport, package-consumer, library, test-batch, generated-Rust, rust-metadata, LSP, diagnostics, codegraph, formatter, documentation, and inspection coverage across component and feature states.
- Verify unknown, disabled, unavailable, corrupt, incompatible, conflicting, feature-gated, locked, offline, relocated, and concurrent-publication behavior on macOS and Linux.
- Update feature inventory, CLI and manifest references, RFC links, rustdocs, v0.5 release notes, development version, installed-SDK smoke coverage, artifact-size evidence, and an IncQL consumer lane before the RFC becomes Implemented.

## Implementation log

- [x] Shared provider identity, provenance, namespace-claim, checked-fact, implementation-facet, and provider-plan models are implemented.
- [x] `LibraryManifestIndex` and `CompilationSession` consume the generic provider plan without a stdlib-only semantic side channel.
- [x] Reserved namespace authority and duplicate canonical module claims are validated before typechecking.
- [x] Relocatable SDK inventory discovery and integrity validation are implemented.
- [x] `[sdk]` profile, component additions, exclusions, dependencies, mandatory membership, and availability are resolved and locked.
- [x] The nine stdlib components are independently published and physically excludable from SDK profile packages.
- [x] Shared content-addressed cache publication, reuse, locking, relocation, and compact offline seed behavior are verified.
- [x] Compact and expanded package-feature declarations normalize into one typed graph.
- [x] Optional Incan dependencies, dependency feature requests, defaults, and CLI feature flags resolve additively across the package graph.
- [x] Feature cycles, unknown references, inactive dependency edges, and unsupported combinations have source-anchored diagnostics.
- [x] Compilation-unit `when feature(...)` blocks and positive conjunctions parse, format, project, and diagnose correctly.
- [x] Feature requirements project imports, reexports, declarations, registry facts, documentation facts, component requirements, and implementation facets consistently.
- [x] Cargo and other backend switches are derived privately from provider implementation facets.
- [x] `.incnlib`, generated Rust, locks, reports, and inspection preserve feature, component, integrity, relocation, and provenance facts.
- [x] Build, run, library, package, test-batch, LSP, diagnostics, codegraph, docs, and generated-project routes share provider and feature semantics.
- [x] Direct import, facade/reexport, package consumer, generated Rust, rust-metadata, locked, offline, and installed-SDK regressions pass.
- [x] Minimal, default, and full profile packaging proves excluded payload and dependency behavior with measured size evidence.
- [x] CLI, manifest, artifact, feature, component, generated-reference, rustdoc, and v0.5 release documentation are complete.
- [x] IncQL consumer verification, macOS and Linux release lanes, development-version bump, and full repository gates pass.

## Inspectability and tooling surface

- **Artifact or metadata:** the SDK inventory, extended `.incnlib` provider records, canonical `incan.lock`, and build report expose component, provider, feature, implementation-facet, integrity, and provenance state.
- **Inspection command:** `incan inspect providers --format json` shows available, enabled, and used providers and components; `incan inspect features --format json` shows public feature roots, closure, optional dependencies, conditioned facts, and activation reasons.
- **Diagnostics:** unknown, disabled, unavailable, corrupt, incompatible, conflicting, and feature-gated provider states have distinct source-anchored diagnostic codes and remedies.
- **Provenance:** every selection records the SDK inventory entry, project or workspace manifest entry, dependency edge, feature edge, import, reexport, compiler requirement, and provider artifact identity that contributed it.
- **Not implicit:** no network acquisition, project mutation, reserved namespace grant, public feature activation, or backend feature leakage may be inferred only from generated Cargo output or ambient source checkout state.

## Design Decisions

1. Feature-conditioned source uses the general compilation-unit `when feature("name"):` form. `when` establishes compile-time selection and `feature` is one compiler-owned typed predicate, so neither syntax is limited to stdlib metadata or dependent on a future named module declaration.
2. `[project.features]` supports both the compact reference list and an expanded typed table. Both normalize into the same feature graph, and a feature cannot mix both representations.
3. A feature may declare that its active provider facts require SDK components, but it never enables or installs them. `[sdk]` remains the explicit owner of component enablement and tooling may offer a reviewable manifest edit when a requirement is unmet.
4. Project environments and workspace matrices may pass explicit feature and SDK-profile selections into compilation commands. They do not become a second persistent owner: the project manifest and canonical lock remain authoritative, and reports record every transient override.
5. The nine stdlib components must be independently addressable and physically excludable from release-profile packages before implementation is complete. Components do not require one top-level archive each, but packaging must prove that excluded payloads and exclusive dependencies are absent, shared immutable content is not duplicated, and all artifacts remain relocatable and integrity-checked. Size measurements are published as release evidence rather than used to waive those invariants.
