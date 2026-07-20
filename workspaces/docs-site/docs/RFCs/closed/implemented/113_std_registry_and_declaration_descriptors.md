# RFC 113: `std.registry` and declaration descriptors

- **Status:** Implemented
- **Created:** 2026-07-13
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 082 (checked API documentation generation)
    - RFC 096 (declaration metadata blocks)
    - RFC 102 (Incan semantic layer inspection surface)
    - RFC 104 (ambient runtime capabilities and receipts)
- **Issue:** #575
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** v0.5

## Summary

This RFC introduces `std.registry`: a typed, declaration-centered catalog mechanism. A registry associates a domain-owned descriptor with a declaration, an implicit compilation unit, or a package, makes the association available to ordinary runtime code when participating modules are loaded, and exposes the same association as a complete compiler-checked projection without executing user modules. `@describe(...)` is the primary declaration-side spelling, but the design is broader than a decorator: registries establish stable identity, provenance, deterministic collection, typed descriptor values, inspection, and package-artifact interchange. This gives libraries one honest pattern for function catalogs, adapters, commands, capabilities, policies, and documentation inventories without creating a second metadata system or asking tools to scrape source.

## Core model

Read this RFC as eight foundations:

1. **A registry is a declaration catalog:** `std.registry` associates descriptors with source subjects. It is not a replacement for ordinary lists, maps, caches, event buses, dependency injection containers, or arbitrary application state.
2. **Descriptors are domain-owned typed values:** a function library may define `FunctionSpec`, a command library may define `CommandSpec`, and a package may define `CapabilityDescriptor`. The registry owns their association and discovery, not their field vocabulary or business semantics.
3. **One declaration has two honest projections:** loading a module may make its descriptions available to runtime code; compiler inspection must expose every checked description independently of which modules a process happened to load.
4. **Static inspection never executes user code:** compiler projection type-checks and validates declaration descriptors, but must not import modules, invoke constructors with user behavior, call decorators, or rely on global registration side effects.
5. **Registry facts have stable subjects and provenance:** every entry records the registry identity, descriptor value, subject identity, source anchor, defining package and module, and whether it is checked, runtime-observed, imported, or derived.
6. **Runtime incompleteness is explicit:** an ordinary runtime registry contains descriptions from modules loaded in that process. It must not claim to enumerate a package's complete catalog unless an explicit package-loading contract makes that claim true.
7. **Description is composable:** a declaration may be described by more than one registry, and a registry may receive entries for declarations, compilation units, or packages. Reexports project source-owned entries; they do not create duplicate semantic entries.
8. **The registry is a bridge, not a metadata plane:** contract metadata, evidence attachments, runtime capability authority, package cards, and documentation all retain their existing owners. They may use registries to publish typed descriptors.

## Prior art and concrete lessons

`std.registry` is not a copy of another language's annotation, plugin, or source-generation facility. The following systems establish useful constraints on the design:

- **Dart metadata** permits annotations only as compile-time constants and lets an annotation type declare the source targets on which it is valid. Incan should likewise validate a structural descriptor expression without executing user code, and a registry must explicitly constrain the subject kinds that it accepts. Unlike Dart, descriptor values must be allowed to be ordinary rich Incan structural values rather than only a fixed annotation-argument vocabulary. [Dart metadata](https://dart.dev/language/metadata)
- **Kotlin annotations and KSP** distinguish declaration facts from reflection and let annotation definitions specify targets, retention, repeatability, and documentation participation. Incan should make the checked and runtime projections separate and explicit, make supported subjects and multiplicity part of the contract, and let tools consume compiler-owned facts. It must not require libraries to install a separate source processor to reconstruct the canonical catalog. [Kotlin annotations](https://kotlinlang.org/docs/annotations.html), [Kotlin Symbol Processing](https://kotlinlang.org/docs/ksp-overview.html)
- **C# attributes and Roslyn** demonstrate that an IDE, diagnostics, code generation, and project tooling can share a compiler semantic model rather than scrape source independently. Incan's checked registry projection must be a first-class compiler fact consumed by inspection, documentation, LSP, source graph, and agent tooling. C#'s restricted attribute-argument types are not a fit for domain-owned descriptors such as `FunctionSpec`; Incan should keep the descriptor as a typed structural value. [C# attributes](https://learn.microsoft.com/en-us/dotnet/csharp/advanced-topics/reflection-and-attributes/attribute-tutorial), [Roslyn compiler model](https://learn.microsoft.com/en-us/dotnet/csharp/roslyn-sdk/compiler-api-model)
- **Rust `inventory`** shows the value of registrations distributed across linked dependencies without a central hand-maintained list. It also establishes the boundary: a linked-program collection is not a complete source-package catalog. Incan's runtime API must therefore say that it returns loaded entries, while complete discovery belongs to the checked projection. [Rust `inventory`](https://docs.rs/inventory/latest/inventory/macro.submit.html)
- **Java `ServiceLoader`** shows that a provider can be inspected before it is instantiated, but its provider configuration is still string-addressed and discovery remains runtime and classpath-dependent. Incan must retain the inspect-before-use benefit while rejecting sidecar string configuration, lazy discovery as an authority, and runtime state as a substitute for a checked catalog. [Java `ServiceLoader`](https://docs.oracle.com/en/java/javase/17/docs/api/java.base/java/util/ServiceLoader.html)

These lessons produce four syntax rules:

1. `@describe` takes a registry and one domain-owned descriptor value. It does not grow a registry-specific list of named fields such as `name=`, `summary=`, or `since=`; those fields belong to the descriptor type and remain type-checked by ordinary Incan rules.
2. A registry declaration states the subject kinds it accepts. This makes a misuse such as attaching a function-only function catalog to a model a compiler diagnostic, rather than a convention that a documentation tool discovers later.
3. Runtime enumeration uses an explicit loaded-state spelling such as `loaded_entries()`. It cannot be confused with the complete checked package projection exposed by `incan inspect registry`.
4. Target constraints and repeatability do not establish domain identity. The registry must eventually receive a typed descriptor-key or domain-validator contract; it must not accept a stringly field selector such as `key="canonical_name"` or infer keys by inspecting arbitrary descriptor fields.

## Motivation

Incan libraries already need registries. A query-oriented library can describe public functions with typed lifecycle, policy, and lowering information; a command library can describe commands and their input surface; a package can publish capabilities, adapters, or policies; and the standard library needs a reliable inventory of public capabilities. These descriptions belong beside the declaration or module that owns the behavior, rather than in a central hand-maintained table or a comment format that the compiler does not understand.

Ordinary runtime registries solve one half of the problem. They provide ergonomic lookup and dispatch after modules have loaded. They do not, by themselves, provide a complete package catalog: a documentation generator, registry browser, editor, policy tool, or compiler-backed agent cannot infer absent entries from process-local initialization order. Asking each such consumer to scrape source or recreate a library's decorator convention duplicates semantics and makes completeness accidental.

The desired end-state is one authoring gesture that serves both needs. A library author writes a typed descriptor adjacent to the declaration it describes. Runtime code can enumerate loaded descriptions through the registry. Compiler-backed tooling can inspect all checked descriptions across a package without running user code. Both projections retain stable origin and descriptor identity, so a reexport, generated document, registry artifact, diagnostic, or agent answer can point back to the declaration that owns the fact.

This is deliberately not a proposal for a generic metadata namespace. RFC 048 owns checked contract facts, RFC 096 owns declaration-local field metadata, RFC 104 owns authority capabilities and receipts, RFC 079 owns ecosystem artifact relationships, and RFC 082 owns documentation generation. `std.registry` supplies a common declaration-catalog mechanism that those domains may opt into when they need runtime and static discovery of the same typed descriptor.

## Goals

- Provide a public `std.registry` surface for typed declaration catalogs.
- Define `Registry[K, T]`, registry entries, registry subjects, and descriptor identity without hardcoding domain fields such as lifecycle, permissions, documentation links, or backend mappings.
- Define `@describe(...)` as a readable declaration-side registration form without introducing a global `feature` keyword or a comment-based mini-language.
- Support descriptions for declarations, compilation units, and packages without fake functions or sentinel declarations that exist only to carry prose.
- Define a compile-time descriptor-value contract that can represent typed structural values without executing user code.
- Preserve a runtime registry projection for loaded modules while making its incompleteness explicit.
- Define a complete checked projection for `incan inspect`, checked API metadata, documentation generators, package artifacts, LSP clients, policy tools, and other compiler consumers.
- Preserve canonical source ownership across imports and reexports.
- Make duplicate, malformed, incompatible, inaccessible, and unsupported descriptions fail with source-anchored diagnostics.
- Define deterministic ordering and identity so generated outputs, package artifacts, and diagnostics do not depend on source traversal or module initialization order.

## Non-Goals

- Replacing ordinary application collections, maps, dependency injection, plugin loading, or event dispatch.
- Defining a general macro system or allowing arbitrary decorators to publish compiler facts implicitly.
- Defining contract metadata, evidence attachments, artifact cards, runtime authority grants, receipts, or documentation schemas.
- Executing user code, importing modules, or observing mutable runtime state to discover registry entries.
- Requiring every descriptor to be public, every library to use registries, or every runtime registry to be complete.
- Defining a remote package registry, marketplace, discovery ranking, or package installation protocol.
- Defining an effect system, capability-grant semantics, or policy decision model.
- Replacing RFC 096 field metadata blocks or extending them to arbitrary declarations by implication.
- Promising that a runtime registry can instantiate unavailable optional dependencies or load every package module automatically.

## Guide-level explanation

### Describe a declaration once

A registry owner defines the domain descriptor. A function-oriented library can keep its function rules typed and adjacent to the functions they describe. `@derive(Descriptor)` opts the descriptor model into the structural snapshot contract specified below:

```incan
from std.registry import Registry, SubjectKind, describe

@derive(Descriptor)
pub model FunctionSpec:
    canonical_name: str
    lifecycle: Lifecycle
    policy: FunctionPolicy


pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function, SubjectKind.Method],
)


@describe(functions, FunctionId("normalize"), FunctionSpec(
    canonical_name="normalize",
    lifecycle=Lifecycle.since(Release.v0_5),
    policy=FunctionPolicy.Portable,
))
pub def normalize(value: str) -> str:
    return value.strip().lower()
```

The declaration remains the source of the callable signature and implementation. `FunctionSpec` remains the source of function-specific facts. `functions` owns the association between that declaration and that descriptor.

When the module has loaded, ordinary runtime code can inspect the loaded entries:

```incan
for entry in functions.loaded_entries():
    println(entry.subject.qualified_name)
    println(entry.descriptor.canonical_name)
```

That result is useful for dispatch, execution, and local diagnostics, but it is intentionally process-local. It contains only entries whose defining modules participated in the process.

### Inspect a complete catalog without loading it

Compiler tooling reads the checked projection instead:

```text
incan inspect registry example.functions --format json
```

The result includes every valid description targeting the public registry in the inspected package closure, including source anchor, defining module, descriptor value, canonical subject identity, and reexport projections. It does not run module initialization and does not depend on whether a runtime registry happened to load a helper module.

Documentation, editor, policy, and package tools consume this checked projection. They must not reconstruct it from decorator text, comments, generated Rust, or a running process.

### Describe a compilation unit or package without a fake function

Some facts belong to an implicit compilation unit or package rather than to one function. A logging source unit, adapter package, or command bundle should be able to publish a descriptor without attaching it dishonestly to an arbitrary helper. This is deliberately distinct from a future named `module name:` declaration: `module` is the source form for a real submodule with a name and body, not a bare marker for the file currently being compiled.

```incan
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind

pub static capabilities: Registry[CapabilityId, CapabilityDescriptor] = Registry.define(
    subjects=[SubjectKind.CompilationUnit, SubjectKind.Package],
)


pub static logging_capability: RegistryEntry[CapabilityId, CapabilityDescriptor] = capabilities.entry(
    key=CapabilityId("std.logging"),
    subject=RegistrySubject.current_unit(),
    descriptor=CapabilityDescriptor(
        id=CapabilityId("std.logging"),
        title="Structured logging",
        stability=Stability.Stable,
    ),
)
```

`logging_capability` is a real registry entry value, not a sentinel declaration. It can be inspected or passed to runtime code, and its subject is the defining compilation unit. `RegistrySubject.package()` supports package-wide facts. Neither form requires a fake callable, a bare `module` marker, or a comment block.

### Use domains without creating a metadata monoculture

The same mechanism supports different domains without merging their meanings:

```incan
from std.registry import Registry, SubjectKind, describe

pub static commands: Registry[CommandId, CommandSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)
pub static adapters: Registry[AdapterId, AdapterSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)


@describe(commands, CommandId("check"), CommandSpec(name="check", summary="Validate the project"))
pub def check_project(path: Path) -> Result[CheckReport, CheckError]:
    ...


@describe(adapters, AdapterId("json"), AdapterSpec(name="json", media_type="application/json"))
pub def render_json(report: CheckReport) -> bytes:
    ...
```

The registry does not decide what a command, adapter, capability, policy, or function means. It only provides the checked declaration-to-descriptor relationship that lets each domain expose a coherent catalog.

## Reference-level explanation

### Registry types and identities

`std.registry` must provide a generic `Registry[K, T]` type for a structural key type `K` and descriptor type `T`. `K` gives the domain-owned identity that the registry uses for duplicate validation; `T` satisfies the descriptor-value contract defined by this RFC. A registry must have a canonical identity derived from its defining package, module, and public binding. A registry may be private, but only a public registry is discoverable outside its defining package by default.

The initial registry-definition form is `pub static name: Registry[K, T] = Registry.define(subjects=[...])`. `Registry.define` is a compiler-known declarative constructor, not a general mutable-container constructor. The binding supplies the registry identity, `K` supplies the typed domain-key contract, and the `subjects` argument declares the kinds of source subject that may be described through it. A description whose target is not in this set is a type-checking error at the description site.

A registry entry must contain at least:

- canonical registry identity;
- canonical subject identity;
- subject kind: declaration, compilation unit, or package;
- typed domain key;
- typed descriptor value;
- source anchor for the registration and source anchor for the subject when distinct;
- defining package and module identity;
- declared visibility and reexport projections where applicable;
- origin classification: checked declaration, runtime observation, imported artifact, or derived projection.

The compiler must use the canonical defining declaration as the subject identity. A public reexport may expose an additional import path, but it must not create a second semantic entry or change the entry's origin.

The registry must guarantee deterministic checked output ordering. Ordering must not depend on filesystem traversal, hash-map order, or runtime module initialization. The canonical sort key must include registry identity, subject identity, and a deterministic per-subject description ordinal when a declaration has multiple descriptions in the same registry.

### Descriptor values

The descriptor type is owned by the registry's domain. A descriptor type must opt into structural description with `@derive(Descriptor)`. The compiler-known derive establishes that instances of the model can be type-checked, structurally serialized, and retained as immutable checked snapshots without executing user code. It must reject descriptor shapes that cannot meet that contract.

A descriptor value may contain safe primitive values, `None`, enum variants, type references, checked constant references, descriptor collections, descriptor maps with stable keys, and constructions of descriptor types whose fields recursively satisfy the descriptor-value contract. A descriptor value must not depend on host state, mutable statics, I/O, reflection over a running program, an arbitrary function call, a secret reveal, or any expression whose value requires user-code execution.

The compiler must type-check a descriptor expression and preserve its typed structural form. It must not claim to have evaluated an unsupported expression. A diagnostic for an unsupported descriptor value must identify the expression and explain that descriptions are compile-time structural facts rather than runtime observations.

Descriptor values are immutable checked snapshots at the description site. Lowering reconstructs each loaded entry only from the frontend-approved structural form; it never aliases a dynamic source expression or lets a runtime mutation alter the checked projection. Loaded entries remain ordinary typed runtime values, so consumers must continue to distinguish their process-local state from the immutable compiler-owned catalog.

### Declaration descriptions

`@describe(registry, key, descriptor)` must be a compiler-known declarative decorator supplied by `std.registry`. It is initially valid on functions and methods. The decorator must preserve the declaration's ordinary runtime type and callable behavior; it must not wrap a declaration with unrelated behavior merely to record a description.

`@describe` deliberately has no domain-specific named arguments. The registry argument identifies a typed catalog, the key argument is one ordinary typed structural value of the catalog's domain-key type, and the descriptor argument is one ordinary typed structural value of its descriptor type. Syntax such as `@describe(functions, FunctionId("normalize"), FunctionSpec(...))` keeps lifecycle, policy, and documentation inside the domain-owned `FunctionSpec` while making the catalog identity explicit and typed rather than a string-shaped annotation selector.

The compiler must resolve `describe` by canonical symbol identity, including supported aliases and reexports of the standard decorator. It must not recognize a user-defined function only because it is named `describe`.

The registry argument must resolve to a `Registry[K, T]`, the key argument must type-check as `K`, and the descriptor argument must type-check as `T`. All three must be valid in the defining package's visibility and initialization context. The compiler must reject a description that targets a registry whose descriptor type or key type does not match, whose identity cannot be resolved, or whose registration would introduce a duplicate key.

A declaration may carry descriptions from multiple registries. A declaration may carry multiple descriptions in one registry only when their typed keys are distinct. The compiler must preserve source order for deterministic output and report a duplicate key at the later source location.

Ordinary decorators and `@describe` may be stacked. A description always targets the source declaration's canonical contract, not an accidental wrapper shape. The compiler records `@describe` independently of ordinary decorator application; its placement does not change the declaration's callable signature, aliases, or source anchor.

### Compilation-unit and package descriptions

The registry surface must support descriptions whose subject is the defining compilation unit or defining package. These descriptions must be represented as real registry entries and must be available to both runtime and checked projections.

A compilation-unit or package description must carry the corresponding canonical identity as its subject. It must not be modeled by attaching a descriptor to an unrelated function, by parsing a comment block, or by relying on a magic constant name.

The initial surface uses an explicit entry-producing form such as `registry.entry(key=..., subject=RegistrySubject.current_unit(), descriptor=...)` and `registry.entry(key=..., subject=RegistrySubject.package(), descriptor=...)`. Such a form must have ordinary typed source semantics, return a registry entry value or equivalent typed handle, and be accepted only in a declaration context with deterministic initialization behavior. A future `module name:` declaration may use ordinary `@describe` decorators once a module-system RFC gives it real name, body, visibility, import, and nesting semantics; this RFC must not introduce a bare `module` marker merely to create a registry subject.

### Runtime projection

Loading a module that contains valid registry descriptions must make its entries available to the target registry's runtime projection. Runtime registration must preserve typed descriptors and canonical subject identity. Repeated loading or equivalent reexport paths must not duplicate an entry.

Runtime registry enumeration must distinguish loaded entries from a complete checked catalog. `loaded_entries()` and similar runtime APIs must not claim package completeness unless the registry has explicitly performed a documented complete-loading operation. A runtime registry may expose its loaded-state or completeness classification so diagnostics do not confuse a missing unloaded module with a missing declaration.

Dynamic runtime additions are not part of this RFC's public surface. Libraries that need mutable, process-local registration must use ordinary collections or retain an existing runtime-only registry and must not present it as a checked catalog.

### Checked projection and artifacts

The compiler must make checked registry entries available through a stable inspection and metadata contract. The projection must include typed descriptor structure, canonical identities, visibility, provenance, source anchors, import/reexport paths, and explicit unsupported or degraded states where available.

The projection must be available for a source package after checking and for a built package artifact that embeds the required checked data. A standalone registry-artifact encoding is outside this RFC. A consumer that only has a runtime registry must report that complete checked discovery is unavailable rather than synthesizing facts from loaded entries.

`incan inspect registry <canonical-identity> --format json` must select one public registry from the resolved package closure and emit a deterministic machine-readable representation. Source-package inspection may include private registries when the selected registry belongs to the current package; dependency inspection exposes only public registries. The output must be suitable for documentation generation, package tooling, LSP features, policy tooling, and compiler-backed agents.

Generated documentation and artifact tooling must consume the checked registry projection when they need completeness. They may use runtime registry APIs for execution behavior, but must not scrape source, parse comments, or inspect generated Rust as an alternate authority.

### Domain validation and duplicate rules

`std.registry` must validate generic registry invariants: registry identity, subject identity, key and descriptor type compatibility, structural key and descriptor validity, declaration visibility, deterministic ordering, and duplicate registration of the same key.

Domain-specific invariants remain owned by descriptor and key types. A function catalog may use a `FunctionId`, a command catalog a `CommandId`, and a capability catalog a `CapabilityId`; those types own their validation and compatibility rules. The generic registry provides typed-key uniqueness and reports duplicate keys at the later source location.

The generic registry must not infer domain uniqueness by searching arbitrary string fields, require a conventional descriptor field, or execute a user-defined key method. The key is an explicit structural value at the registration site.

### Visibility, imports, and package boundaries

Private descriptions remain available to local compiler tooling but are not exported as part of a package's public registry projection unless the registry explicitly declares an internal-artifact contract. Public registry entries must preserve the defining package and module as source authority.

Importing or reexporting a described declaration must preserve its source identity and descriptor facts. A facade may add a projection path used by a consumer, but must not silently edit, suppress, or duplicate the source-owned description. A consumer package must be able to inspect registry entries from a dependency artifact without requiring that dependency's source tree.

Package-level inspection must report unresolved, incompatible, or missing descriptor artifacts as explicit diagnostics or degraded facts. It must not silently omit them and present a partial catalog as complete.

### Diagnostics and tooling

Diagnostics must point at the description site, descriptor expression, registry binding, or conflicting entry as appropriate. They should name the registry and source declaration in user-facing terms rather than exposing backend-generated names.

The formatter must preserve `@describe` placement and format descriptor expressions using ordinary Incan expression rules. The LSP should expose registry membership in hover and navigation, offer completion for available registries and descriptor fields where type information permits, and show diagnostics for invalid descriptions without needing to run the program.

The compiler-backed source graph should expose registry, entry, subject, descriptor, and reexport-projection facts with checked provenance. Runtime observations may be linked to the same identities, but must remain distinguishable from declarations.

## Design details

### Relationship to RFC 096

RFC 096 owns declaration-local metadata blocks for fields and constrained primitive options. Its safe, non-executable value discipline and provenance rules are useful precedent for descriptor values. It does not own registry membership, declaration decorators, module/package subjects, runtime registration, or package-wide catalog inspection.

The two RFCs must remain separate. Field metadata may later be represented inside a descriptor when a domain needs a catalog of fields, but a registry entry must not change field semantics or create a second spelling for field metadata.

### Relationship to RFC 048 and RFC 082

RFC 048 owns compiler-checked metadata extraction. This RFC extends the checked fact surface with registry entries; it does not create a parallel source scraper or permit unchecked descriptor strings to become authoritative.

RFC 082 owns generation and validation of public documentation. It may render registry descriptors when a domain chooses to publish them, but it does not define the descriptor schema or registry completeness semantics.

### Relationship to RFC 104

RFC 104 owns the meaning of runtime capabilities, authority grants, and receipts. A capability library may publish `CapabilityDescriptor` entries through `std.registry`, but registry membership does not grant authority, emit a receipt, or change governed-runtime behavior. Runtime receipt observations may reference a capability registry entry by stable identity.

### Relationship to RFC 079

RFC 079 owns artifact graph relationships and remote or private discovery. A package artifact may carry checked registry projections, and an artifact graph may index them, but `std.registry` works for local source and built packages without any remote registry service.

### Compatibility and migration

This RFC is additive. Existing runtime registries and user-defined decorators continue to work. They do not gain checked completeness merely because they have a familiar name or shape.

Libraries may migrate an existing declaration-side runtime registry by replacing its local registration decorator with `@describe` and opting its descriptor type into the descriptor-value contract. Migration must preserve runtime behavior, static inspection, public facade imports, generated artifacts, and package-consumer behavior. A library that requires dynamic-only registration may retain its existing runtime registry and must document that it has no complete static catalog.

The existing generated capability inventory may migrate only after a `CapabilityDescriptor` registry is available and its checked projection preserves public output, validation, and documentation behavior. Comment-based feature blocks are not an acceptable compatibility endpoint.

## Alternatives considered

### Introduce a `feature` keyword

Rejected because feature inventory is only one registry domain. A keyword would embed a documentation concern in the language, would not naturally serve function catalogs or adapters, and would encourage future one-off declaration keywords instead of a coherent catalog model.

### Keep comment-based source scanners

Rejected because comments cannot participate in type checking, symbol resolution, rename, source navigation, descriptor validation, or compiler-backed inspection. A scanner would duplicate parsing and schema rules outside the language while remaining unable to prove that its facts describe a valid Incan declaration.

### Use only runtime registration

Rejected because runtime registration is process-local and initialization-dependent. It is valuable for execution behavior, but it cannot prove a package catalog is complete for documentation, package artifacts, policy tooling, or editor inspection.

### Use only checked metadata and remove runtime registries

Rejected because loaded programs legitimately need runtime lookup, dispatch, and domain-specific behavior. Static inspection should complement runtime registries, not force every library to reconstruct a catalog from a compiler artifact at runtime.

### Make every decorator compiler-visible

Rejected because arbitrary decorators may execute behavior, transform values, depend on runtime state, or have application-specific semantics. Only the canonical `std.registry` declaration forms receive the static descriptor contract.

### Expand RFC 096 into a general declaration metadata system

Rejected because RFC 096 intentionally solves field-local schema and type-option readability. Registry identity, runtime lifecycle, module/package subjects, package artifacts, and complete inspection are a separate system with different users and failure modes.

### Put this surface under `std.metadata`

Rejected because metadata already names stronger contract, evidence, lifecycle, and provenance concepts. Calling a declaration-catalog mechanism metadata would blur its boundary with existing model and evidence work.

## Drawbacks

- The design introduces a new standard-library and compiler contract that must remain stable for library authors and tools.
- Descriptor-value checking adds a constrained compile-time data model that needs careful diagnostics and evolution rules.
- Dual runtime and checked projections create two intentionally different completeness states that documentation and diagnostics must explain well.
- Registry identities and domain-level key rules add authoring discipline compared with ad hoc lists or decorators.
- Compilation-unit and package subjects introduce explicit source identities that authors must choose deliberately instead of attaching descriptors to convenient but unrelated declarations.
- A broad registry abstraction could become overused if it is presented as a general application-state container rather than a declaration catalog.

## Implementation architecture

*(Non-normative.)* The implementation should share one normalized registry-entry model between type checking, runtime registration lowering, checked metadata extraction, package artifacts, inspection, documentation consumers, and source-graph export. Runtime registration and checked projection should be generated from the same validated declaration facts, while remaining explicit about their different completeness guarantees. The first delivery should prove the model with one standard-library capability catalog and one existing function-oriented library registry before broadening the supported subject or descriptor forms.

## Layers affected

- **Parser / AST:** uses the existing decorator and expression forms. It must preserve the source anchors required for descriptions and explicit compilation-unit/package entries without introducing a global `feature` keyword or a registry-only bare `module` marker. A later general `module name:` declaration belongs to the module-system surface rather than being invented by this RFC.
- **Typechecker / symbol resolution:** must resolve `std.registry` identities, validate registry key and descriptor types, enforce structural values, preserve declaration subjects, and report duplicate or inaccessible registrations.
- **Runtime / lowering / emission:** must preserve ordinary typed runtime registry behavior for loaded modules without changing described declarations' callable or nominal semantics.
- **Stdlib / Runtime (`incan_stdlib`):** must provide `Registry[K, T]`, registry-entry handles, `@describe`, explicit compilation-unit/package subject forms, and structural key/descriptor support appropriate for ordinary Incan authors.
- **SDK provider:** `stdlib-core` must publish `std.registry` and the source-owned capability catalogue so minimal, default, and full SDK profiles share the same checked registry contract.
- **Checked metadata and package artifacts:** must carry complete checked registry entries, identities, descriptor structure, visibility, provenance, and degraded states.
- **CLI / inspection tooling:** must provide deterministic registry inspection and actionable diagnostics without invoking user module initialization.
- **Formatter:** must format description decorators and descriptor expressions deterministically.
- **LSP / editor tooling:** must provide completion, hover, go-to-definition, registry-membership navigation, and source diagnostics from checked facts.
- **Documentation and artifact tooling:** must consume checked registry projections for capability inventories and other catalog views rather than source scanners.
- **Source graph / agent tooling:** must expose registry facts and distinguish checked declarations from runtime observations.

## Implementation Plan

### Phase 1: Public source contract and structural values

- Add the `std.registry` public surface: typed registries and entries, subject handles, descriptor derivation, and non-wrapping declaration descriptions.
- Validate structural keys and descriptor values without evaluating user code, and provide source-anchored diagnostics for unsupported values and incompatible registrations.
- Keep the initial declaration matrix bounded to functions, methods, compilation units, and packages.

### Phase 2: Compiler facts and runtime projection

- Record one canonical checked registry fact for every valid description, including its key, descriptor snapshot, subject, provenance, visibility, and reexport paths.
- Carry those facts through semantic snapshots and the package compilation pipeline rather than recreating them in lowering, documentation, or tooling.
- Emit loaded-module runtime entries from the same checked facts while preserving declaration behavior and eliminating duplicate entries from imports or reexports.

### Phase 3: Inspection and package interchange

- Embed checked registry facts in package metadata and expose deterministic JSON inspection for source packages and dependencies.
- Project the same facts into formatter, LSP, source-graph, documentation, and agent-tooling consumers where their existing surfaces can expose them.
- Report unavailable or incompatible dependency metadata explicitly rather than silently presenting an incomplete catalog.

### Phase 4: Capability-inventory migration and consumer proof

- Define the standard-library capability descriptor and move the generated capability inventory to checked registry facts.
- Remove the comment-block scanner and prove output, validation, and generated-reference parity from the new projection.
- Exercise a representative external consumer as a compiler acceptance lane without making its runtime registry the authority for static discovery.

### Phase 5: Documentation and release readiness

- Document authoring, loaded-versus-checked completeness, migration, diagnostics, and inspection behavior in the user-facing reference.
- Update generated references, release notes, rustdocs, and the RFC checklist; verify package, facade, and consumer behavior before presenting the implementation for review.

## Implementation log

### Specification and lifecycle

- [x] Settle subjects, typed registry keys, descriptor derivation, decorator behavior, artifact boundary, and module-system cutoff.
- [x] Record the active implementation and complete all delivery phases in this RFC.

### Source surface and typechecking

- [x] Add `std.registry` names, `Registry[K, T]`, `RegistryEntry[K, T]`, subject handles, and descriptor derivation.
- [x] Recognize canonical `@describe(registry, key, descriptor)` independently of ordinary callable decorators.
- [x] Validate structural keys and descriptors, supported subjects, visibility, duplicate keys, and decorator composition.
- [x] Add source-anchored diagnostics and formatter coverage for valid and invalid registrations.

### Semantic facts, runtime, and emission

- [x] Add structured registry facts with canonical identities, typed snapshots, source anchors, provenance, visibility, and reexport projections.
- [x] Carry registry facts through semantic snapshots and the compilation pipeline.
- [x] Emit typed loaded-entry runtime behavior from checked facts without changing described declaration semantics.
- [x] Prove direct, imported, facade, package-consumer, and test-batch behavior including generated Rust compilation.

### Inspection, package metadata, and tooling

- [x] Embed checked registry facts in package metadata and report degraded dependency metadata explicitly.
- [x] Implement deterministic `incan inspect registry <canonical-identity> --format json`.
- [x] Expose checked registry facts to LSP and source-graph projections with declaration/runtime provenance separation.

### Capability inventory and consumer acceptance

- [x] Add a standard-library capability registry and migrate generated capability inventory input from comment blocks to checked facts.
- [x] Remove the comment-based scanner and prove generated output and validation parity.
- [x] Verify a representative external function-registry consumer against the completed compiler surface without treating its runtime list as static authority.

### Documentation and release

- [x] Add authored reference documentation, diagnostics guidance, migration guidance, rustdocs, generated-reference output, and release-note coverage.
- [x] Bump the active development version, complete the full verification gate, and move this RFC to Implemented only when all checklist items are complete.

## Design Decisions

- **Compilation units and packages are the non-declaration subjects in this RFC.** `RegistrySubject.current_unit()` and `RegistrySubject.package()` are the explicit typed handles. They do not imply a source-level wrapper around a file.
- **A named module remains a future real declaration.** A later module-system RFC may define a form such as `pub module connectors:` with a name, body, visibility, import behavior, and nesting semantics. That declaration may then use `@describe` normally. The existing `module tests:` form remains a test grouping and is not generalized or repurposed by this RFC.
- **Registries have typed keys.** `Registry[K, T]` accepts a structural `K` key and `T` descriptor. `@describe(registry, key, descriptor)` and `registry.entry(key=..., subject=..., descriptor=...)` make the key explicit, allowing the compiler to reject duplicates without a string field selector or user-code key extractor.
- **Descriptor models opt into structural snapshots with `@derive(Descriptor)`.** The derive allows only structurally serializable fields and produces immutable checked snapshots for compiler lowering and inspection.
- **Initial declaration coverage is functions and methods.** Compilation-unit and package descriptions use explicit entry values. Models, classes, enums, traits, and future named modules remain extension points rather than partially supported decorator targets.
- **`@describe` is non-wrapping.** It records a declaration fact independently of ordinary user-defined decorator application. Stacking does not change a declaration's callable type, alias behavior, or source authority.
- **Dynamic registration is outside this public surface.** Runtime-only mutable registries remain ordinary library code and are never represented as a partial checked catalog.
- **Package checked metadata is the initial artifact format.** Source and built-package inspection use that embedded projection; a standalone registry artifact is a follow-on only if a real consumer proves the need.
- **Inspection resolves the package closure through the ordinary project resolver.** Public registries are visible from dependencies; local source inspection may include private registries. Optional or incompatible dependency metadata is an explicit diagnostic or degraded fact, never a silently incomplete catalog.
