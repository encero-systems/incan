# SDK components and package features

Incan resolves checked libraries and official SDK artifacts through one provider plan. SDK components decide which official providers are enabled for a project, package features select additive package-owned facts and dependencies, imports determine which enabled providers are used, and private implementation facets tell the active backend what to link. These are separate layers.

## Core distinctions

| Concept | Owner | Changes | Does not do |
| --- | --- | --- | --- |
| Provider | A checked library artifact | Contributes namespace claims, semantic facts, generated implementation artifacts, dependencies, provenance, and private implementation facets. | Acquire software or grant itself a reserved namespace. |
| SDK component | The installed SDK release and project `[sdk]` selection | Makes one official provider or provider bundle available and enabled. | Act as a public package feature or runtime flag. |
| SDK profile | The SDK release | Supplies a named base component set such as `minimal`, `default`, or `full`. | Change language semantics, dependency features, optimization, or runtime configuration. |
| Package feature | The package that declares `[project.features]` | Adds optional dependencies, dependency features, conditioned facts, or SDK component requirements. | Install or enable SDK components, expose Cargo flags, or remove unconditional API. |
| Implementation facet | The provider artifact | Maps active semantic requirements to backend work such as Cargo features or linked crates. | Become user-facing package API. |

## From canonical source to a consuming project

The standard library keeps one canonical Incan source tree. Splitting the SDK into components does not copy those modules into nine independently maintained libraries, change their `std.*` import paths, or create nine corresponding Rust crates. Instead, each component has a private Incan producer project that selects a coherent part of the shared source tree and compiles it into one independently addressable provider artifact.

The complete flow is:

```text
canonical Incan stdlib source
    -> private component producer project
    -> checked provider manifest and generated implementation artifact
    -> content-addressed SDK component payload
    -> installed SDK inventory
    -> project component and package-feature resolution
    -> session-owned provider plan
    -> typechecking, LSP, tests, codegraph, and generated Rust
    -> private backend dependencies and implementation features
```

### Producer side

The SDK component catalog assigns stable component identities, component dependencies, authorized namespace roots, and profile membership. Each catalog entry points to a private Incan project whose `source-root` is the shared standard-library tree. Its `src/lib.incn` imports the modules belonging to that component.

Those imports are executable producer input rather than a second compiler-owned module inventory. Building the project makes the compiler resolve and validate the complete import and reexport closure from canonical source. Adding a module to a component means making it reachable through that checked producer graph; changing component ownership requires changing the producer entrypoint or component catalog, where the boundary is explicit and reviewable.

The producer build emits two related forms of information:

1. **Checked provider facts** describe the modules, declarations, exports, types, registry entries, soft syntax, feature requirements, namespace claims, dependencies, and provenance that consumers may use without reparsing provider source.
2. **Implementation artifacts and facets** describe the generated Rust and private backend work required when those facts are used, such as linked crates, Cargo features, generated modules, or future non-Rust backend inputs.

The result receives an immutable identity derived from its checked content and relevant compiler, dependency, feature, and backend inputs. Release tooling publishes it into the shared content-addressed provider store and records its identity in the SDK inventory. Component artifacts are independently addressable and physically excludable, although one release archive may carry several selected artifacts and shared content is stored only once.

The private producer projects are therefore build definitions, not public packages. Applications do not depend on names such as `incan_stdlib_data`, import through those projects, or compile them from source. They consume the published provider artifact through the installed SDK.

### Installation and project selection

An installed toolchain carries an SDK inventory that lists the component payloads physically available in that installation, their immutable provider identities, integrity digests, dependencies, namespace authority, and profile membership. The inventory is discovered relative to the active compiler or explicit toolchain root, so it remains valid when an SDK is relocated.

For each project, resolution starts with its `[sdk]` profile, applies explicit component additions, expands component dependencies, and validates exclusions. Package defaults, command-line feature selections, optional Incan dependencies, and dependency feature requests then form one additive package-feature closure. Resolution does not download or silently enable anything.

The compiler turns this result into one immutable provider plan owned by the compilation session. The plan maps canonical import roots to checked providers and records why every component, package feature, dependency edge, implementation facet, and provider fact participates. Single-project builds, workspace members, test batches, library builds, LSP, inspection, reports, and codegraph projections consume this same plan rather than resolving their own approximation.

### Consumer checking and code generation

When source imports an enabled provider module, the typechecker reads its declarations and types from the checked provider manifest. It does not read or typecheck the producer's `.incn` source. Imports and compiler-required facts determine which enabled providers are actually used; an enabled but unused component does not automatically contribute its complete backend dependency closure.

Lowering and generated-project construction use the same provider plan. For every used fact, the plan exposes the corresponding private implementation facets and generated artifact. The active backend translates those facets into its own dependencies and switches. In the current Rust backend, that can select features on the shared `incan_stdlib` runtime crate or link another Rust dependency, but those Cargo names are not Incan package API and are not written in application manifests.

Because tooling consumes the same plan, hover, navigation, diagnostics, test discovery, build reports, locks, and `incan inspect` describe the same component and feature projection that code generation used. A tool may show inactive syntax for navigation, but it cannot independently make an inactive provider fact typecheck or emit code.

### Example: `std.json`

`std.json` illustrates the boundary:

1. Its implementation remains in the canonical standard-library source tree.
2. The private `stdlib-data` producer project imports `json` alongside the other data modules. The component catalog grants `stdlib-data` authority over the `json` namespace root and declares its dependency on `stdlib-core` and `stdlib-system`.
3. SDK construction compiles that project into the checked `stdlib-data` provider artifact, publishes its immutable payload, and records it in the SDK inventory. Consumer archives do not need the producer project or canonical `.incn` source.
4. A project using the default v0.5 profile already enables `stdlib-data`. A project using `minimal` may add `stdlib-data`, which also enables its required core and system closure. Merely having `stdlib-data` installed does not enable it for a project that deliberately excludes it.
5. `from std.json import dumps` resolves `std.json` through the session provider plan. The typechecker obtains `dumps` and its signature from checked provider facts, while LSP and codegraph use the same identity and provenance.
6. Once the import makes the JSON implementation necessary, generated-project construction activates the provider's private JSON implementation facets. With the current Rust backend this includes the required `incan_stdlib` and serialization support, but the application still expresses only an Incan component selection and a stable `std.json` import.

The architectural shape is therefore one canonical source root, nine private producer projects, nine independently selectable compiled component artifacts, and backend implementation dependencies selected behind those artifacts. The producer-project boundaries are distribution boundaries; they are not new source-language namespaces or a promise that the Rust crate topology must mirror the SDK component graph.

## Available, enabled, and used

Component and provider participation has three states:

1. **Available** means the active SDK installation contains an integrity-checked component payload.
2. **Enabled** means the project profile, explicit additions, exclusions, and component dependencies selected it.
3. **Used** means imports, reexports, soft syntax, compiler runtime requirements, or other active provider facts reached one of its modules.

An import never downloads or silently enables a component. Importing a module from a disabled component reports how to change `[sdk]`; selecting a component that is absent from the active SDK reports an installation problem; importing a path that no provider claims remains an unknown-module error.

## SDK profiles and components

The v0.5 SDK defines these standard-library components:

| Component | Public capability group | Required components |
| --- | --- | --- |
| `stdlib-core` | Prelude, result, reflection, derives, traits, and compiler-required standard contracts. | None; mandatory. |
| `stdlib-system` | Environment, I/O, temporary files, and filesystem APIs. | `stdlib-core` |
| `stdlib-codecs` | Checksums and encoding. | `stdlib-core`, `stdlib-system` |
| `stdlib-compression` | Compression codecs and stream adapters. | `stdlib-core`, `stdlib-system` |
| `stdlib-data` | Collections, graph, hashing, JSON, math, UUID, datetime, regex, and serialization support. | `stdlib-core`, `stdlib-system` |
| `stdlib-async` | Async runtime-facing standard APIs. | `stdlib-core` |
| `stdlib-observability` | Logging and telemetry data surfaces. | `stdlib-core`, `stdlib-data` |
| `stdlib-web` | Web APIs. | `stdlib-core`, `stdlib-data`, `stdlib-async` |
| `stdlib-testing` | Standard testing APIs. | `stdlib-core` |

`minimal` contains only the mandatory core closure. `default` is used when a project does not declare `[sdk]`. `full` contains every stable official component in that SDK release. In the first v0.5 distribution, `default` and `full` contain the same nine components; they remain distinct profile identities so a future release can evolve the conventional default without changing the meaning of full.

Project selection starts with the profile, adds explicit components, expands dependencies, and then validates exclusions. The command-local `--sdk-profile` override replaces only the base profile for that invocation; explicit project additions and exclusions still apply.

Hashing belongs to `stdlib-data` because collections and UUID reuse it directly. Compression is independently selectable so encoding and checksum consumers do not link its native and algorithm-specific dependency closure.

## Package-feature resolution

Root CLI selections, package defaults, dependency declarations, and conditioned dependency requests resolve into one additive graph. Optional Incan dependencies remain inactive until selected by `dep:<name>`. Feature requests from multiple active edges are unioned. Cycles, unknown features, missing dependencies, requests against inactive optional edges, and unmet SDK component requirements are errors.

The Incan flags are:

| Flag | Meaning |
| --- | --- |
| `--features a,b` | Add named public features to the root package projection. |
| `--no-default-features` | Do not select the root package's `default` feature. |
| `--all-features` | Select every feature declared by the root package. |
| `--sdk-profile PROFILE` | Replace the manifest's base SDK profile for this invocation. |

These flags are accepted by `incan build`, `incan check`, `incan run`, `incan test`, and `incan lock`. `incan inspect codegraph`, `incan inspect providers`, and `incan inspect features` accept the same projection flags. Cargo pass-through remains explicitly prefixed as `--cargo-features`, `--cargo-no-default-features`, and `--cargo-all-features` where supported.

`incan test --feature NAME` is unrelated: it supplies a collection-time probe to `std.testing.feature("NAME")`. Use `--features` for public package features.

## Inspection

```bash
incan inspect providers . --format json
incan inspect features . --format json
incan inspect codegraph src --format jsonl --features json --sdk-profile minimal
```

Provider JSON uses `schema_version: 1` and reports the SDK identity and selected profile, every component's availability and enablement, component selection reasons, and separate `available`, `enabled`, and `used` facts for each provider alongside its identity, provenance, canonical namespace claims, used modules, active features, implementation facets, and manifest path.

Feature JSON uses `schema_version: 1` and reports every package's active features, optional dependencies, dependency-feature requests, required SDK components, activation reasons, dependency edges, and feature-conditioned provider facts with their active state.

Codegraph JSONL keeps its source and diagnostic records, while the header's typed `semantic_contexts` array records the same project-aware SDK, component, package-feature, provider, artifact, implementation-facet, and provenance projection that shaped those facts. A directory spanning several project roots emits one deterministic context per project instead of flattening their selections together.

Human output is concise and intended for interactive checks. Use JSON when auditing provenance, comparing projections, or integrating tooling.

## Artifacts, locks, and offline behavior

Official components and ordinary compiled libraries use checked provider manifests. Consumers do not parse or typecheck provider source. Source-checkout development publishes immutable SDK provider identities into the user-shared `$INCAN_HOME/cache/providers/sdk-v2` store, or `~/.incan/cache/providers/sdk-v2` when `INCAN_HOME` is unset. The identity follows compiler content, provider source, the verified dependency lock, and distribution profile, so relocating identical compiler bytes reuses the same artifact instead of duplicating it per checkout. Publication uses advisory locking, private staging, file and directory synchronization, and atomic rename.

An ordinary compiled provider freezes every active Incan dependency edge into its `.incnlib` record, including the provider name and version, exact artifact digest, requested public features, default-feature policy, optional-edge state, and the dependency artifact's relative location. Consumers recursively validate those records before checking source. The complete artifact graph can therefore move as one distribution tree and continue to build without producer manifests or `.incn` source; an absolute path, mismatched identity, changed feature projection, missing child, or digest mismatch is rejected before generated Rust is compiled.

Release packaging copies only the selected, relocatable provider seed into `share/incan/sdk` inside the installed toolchain. It excludes producer source, mutable Cargo targets, per-component lockfile copies, and components outside the distribution profile. Every archive has a `.profile.json` evidence sidecar recording the profile, component count, provider payload bytes, and complete archive bytes. Generated consumer projects link the installed immutable artifacts and shared Cargo lock without copying a standard-library source tree or mutable provider cache into every project.

`incan.lock` format 2 records the expanded SDK component selection, exact provider identities and digests, public feature closure and activation reasons, dependency feature edges, and implementation-facet closure that can affect checking or generated output. `--locked` rejects semantic drift; `--frozen` adds the existing offline and no-mutation policy. Neither mode downloads a missing component.

Inside an RFC 077 workspace, the canonical root lock keeps one member-attributed semantic graph for every project. `incan workspace inspect` exposes those locked member roots, feature closures, SDK profiles, component selections, and provider facts in both its human summary and JSON projection instead of flattening them into one ambient workspace-wide selection.

See [Project configuration](project_configuration.md) for manifest syntax and [Conditional compilation](../../language/reference/conditional_compilation.md) for `when feature(...)` source projection.
