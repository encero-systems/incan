# How SDK components become compiled providers

The standard library keeps one canonical Incan source tree even though the SDK distributes its capabilities as independently selectable components. The split exists at the provider-artifact boundary: it does not copy modules into separately maintained libraries, change stable `std.*` import paths, or require the Rust crate topology to mirror the SDK component graph.

For the exact component inventory, selection states, feature-resolution rules, CLI flags, inspection schemas, and lock behavior, use the [SDK components and package features reference](../reference/sdk_components_and_package_features.md).

## The complete flow

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

Each stage narrows or projects the same source-owned capability graph. The producer establishes a checked distribution boundary, the installed SDK makes that boundary available, the project decides whether it is enabled, and actual source use decides which private implementation work the backend must perform.

## Producer projects define distribution boundaries

The SDK component catalog assigns stable component identities, component dependencies, authorized namespace roots, and profile membership. Each catalog entry points to a private Incan producer project whose `source-root` is the shared standard-library tree. Its `src/lib.incn` imports the modules belonging to that component.

Those imports are executable producer input rather than a second compiler-owned module inventory. Building the project makes the compiler resolve and validate the complete import and reexport closure from canonical source. Adding a module to a component means making it reachable through that checked producer graph; changing component ownership requires changing the producer entrypoint or component catalog, where the boundary is explicit and reviewable.

The producer build emits two related forms of information:

1. **Checked provider facts** describe the modules, declarations, exports, types, registry entries, soft syntax, feature requirements, namespace claims, dependencies, and provenance that consumers may use without reparsing provider source.
2. **Implementation artifacts and facets** describe the generated Rust and private backend work required when those facts are used, such as linked crates, Cargo features, generated modules, or future non-Rust backend inputs.

The result receives an immutable identity derived from its checked content and relevant compiler, dependency, feature, and backend inputs. Release tooling publishes it into the shared content-addressed provider store and records its identity in the SDK inventory. Component artifacts are independently addressable and physically excludable, although one release archive may carry several selected artifacts and shared content is stored only once.

The private producer projects are therefore build definitions, not public packages. Applications do not depend on names such as `incan_stdlib_data`, import through those projects, or compile them from source. They consume the published provider artifact through the installed SDK.

## Installation separates availability from selection

An installed toolchain carries an SDK inventory that lists the component payloads physically available in that installation, their immutable provider identities, integrity digests, dependencies, namespace authority, and profile membership. The inventory is discovered relative to the active compiler or explicit toolchain root, so it remains valid when an SDK is relocated.

For each project, resolution starts with its `[sdk]` profile, applies explicit component additions, expands component dependencies, and validates exclusions. Package defaults, command-line feature selections, optional Incan dependencies, and dependency feature requests then form one additive package-feature closure. Resolution does not download or silently enable anything.

The compiler turns this result into one immutable provider plan owned by the compilation session. The plan maps canonical import roots to checked providers and records why every component, package feature, dependency edge, implementation facet, and provider fact participates. Single-project builds, workspace members, test batches, library builds, LSP, inspection, reports, and codegraph projections consume this same plan rather than resolving their own approximation.

## Consumers use checked facts before backend artifacts

When source imports an enabled provider module, the typechecker reads its declarations and types from the checked provider manifest. It does not read or typecheck the producer's `.incn` source. Imports and compiler-required facts determine which enabled providers are actually used; an enabled but unused component does not automatically contribute its complete backend dependency closure.

Lowering and generated-project construction use the same provider plan. For every used fact, the plan exposes the corresponding private implementation facets and generated artifact. The active backend translates those facets into its own dependencies and switches. In the current Rust backend, that can select features on the shared `incan_stdlib` runtime crate or link another Rust dependency, but those Cargo names are not Incan package API and are not written in application manifests.

Because tooling consumes the same plan, hover, navigation, diagnostics, test discovery, build reports, locks, and `incan inspect` describe the same component and feature projection that code generation used. A tool may show inactive syntax for navigation, but it cannot independently make an inactive provider fact typecheck or emit code.

## Following `std.json` through the flow

`std.json` illustrates the boundary:

1. Its implementation remains in the canonical standard-library source tree.
2. The private `stdlib-data` producer project imports `json` alongside the other data modules. The component catalog grants `stdlib-data` authority over the `json` namespace root and declares its dependency on `stdlib-core` and `stdlib-system`.
3. SDK construction compiles that project into the checked `stdlib-data` provider artifact, publishes its immutable payload, and records it in the SDK inventory. Consumer archives do not need the producer project or canonical `.incn` source.
4. A project using the default v0.5 profile already enables `stdlib-data`. A project using `minimal` may add `stdlib-data`, which also enables its required core and system closure. Merely having `stdlib-data` installed does not enable it for a project that deliberately excludes it.
5. `from std.json import dumps` resolves `std.json` through the session provider plan. The typechecker obtains `dumps` and its signature from checked provider facts, while LSP and codegraph use the same identity and provenance.
6. Once the import makes the JSON implementation necessary, generated-project construction activates the provider's private JSON implementation facets. With the current Rust backend this includes the required `incan_stdlib` and serialization support, but the application still expresses only an Incan component selection and a stable `std.json` import.

The resulting architecture has one canonical source root, nine private producer projects, and nine independently selectable compiled component artifacts. The producer projects express distribution boundaries; they are neither new source-language namespaces nor a commitment to matching Rust crates.
