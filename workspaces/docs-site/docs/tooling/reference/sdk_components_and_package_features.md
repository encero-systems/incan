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
| `stdlib-codecs` | Checksums, compression, and encoding. | `stdlib-core`, `stdlib-system` |
| `stdlib-data` | Collections, graph, hashing, JSON, math, UUID, datetime, regex, and serialization support. | `stdlib-core`, `stdlib-system` |
| `stdlib-async` | Async runtime-facing standard APIs. | `stdlib-core` |
| `stdlib-observability` | Logging and telemetry data surfaces. | `stdlib-core`, `stdlib-data` |
| `stdlib-web` | Web APIs. | `stdlib-core`, `stdlib-data`, `stdlib-async` |
| `stdlib-testing` | Standard testing APIs. | `stdlib-core` |

`minimal` contains only the mandatory core closure. `default` is used when a project does not declare `[sdk]`. `full` contains every stable official component in that SDK release. In the first v0.5 distribution, `default` and `full` contain the same eight components; they remain distinct profile identities so a future release can evolve the conventional default without changing the meaning of full.

Project selection starts with the profile, adds explicit components, expands dependencies, and then validates exclusions. The command-local `--sdk-profile` override replaces only the base profile for that invocation; explicit project additions and exclusions still apply.

Hashing belongs to `stdlib-data` because collections and UUID reuse it directly. This keeps those implementations Incan-authored without making data consumers link the compression dependencies owned by `stdlib-codecs`.

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
