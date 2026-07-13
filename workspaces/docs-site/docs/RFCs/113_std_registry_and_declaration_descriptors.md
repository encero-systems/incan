# RFC 113: `std.registry` and declaration descriptors

- **Status:** Draft
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
- **Shipped in:** —

## Summary

This RFC introduces `std.registry`: a typed, declaration-centered catalog mechanism. A registry associates a domain-owned descriptor with a declaration, an implicit compilation unit, a future named submodule, or a package, makes the association available to ordinary runtime code when participating modules are loaded, and exposes the same association as a complete compiler-checked projection without executing user modules. `@describe(...)` is the primary declaration-side spelling, but the design is broader than a decorator: registries establish stable identity, provenance, deterministic collection, typed descriptor values, inspection, and package-artifact interchange. This gives libraries one honest pattern for function catalogs, adapters, commands, capabilities, policies, and documentation inventories without creating a second metadata system or asking tools to scrape source.

## Core model

Read this RFC as eight foundations:

1. **A registry is a declaration catalog:** `std.registry` associates descriptors with source subjects. It is not a replacement for ordinary lists, maps, caches, event buses, dependency injection containers, or arbitrary application state.
2. **Descriptors are domain-owned typed values:** a function library may define `FunctionSpec`, a command library may define `CommandSpec`, and a package may define `CapabilityDescriptor`. The registry owns their association and discovery, not their field vocabulary or business semantics.
3. **One declaration has two honest projections:** loading a module may make its descriptions available to runtime code; compiler inspection must expose every checked description independently of which modules a process happened to load.
4. **Static inspection never executes user code:** compiler projection type-checks and validates declaration descriptors, but must not import modules, invoke constructors with user behavior, call decorators, or rely on global registration side effects.
5. **Registry facts have stable subjects and provenance:** every entry records the registry identity, descriptor value, subject identity, source anchor, defining package and module, and whether it is checked, runtime-observed, imported, or derived.
6. **Runtime incompleteness is explicit:** an ordinary runtime registry contains descriptions from modules loaded in that process. It must not claim to enumerate a package's complete catalog unless an explicit package-loading contract makes that claim true.
7. **Description is composable:** a declaration may be described by more than one registry, and a registry may receive entries for declarations, compilation units, named submodules, or packages. Reexports project source-owned entries; they do not create duplicate semantic entries.
8. **The registry is a bridge, not a metadata plane:** contract metadata, evidence attachments, runtime capability authority, package cards, and documentation all retain their existing owners. They may use registries to publish typed descriptors.

## Prior art and concrete lessons

`std.registry` is not a copy of another language's annotation, plugin, or source-generation facility. The following systems establish useful constraints on the design:

- **Dart metadata** permits annotations only as compile-time constants and lets an annotation type declare the source targets on which it is valid. Incan should likewise validate a structural descriptor expression without executing user code, and a registry must explicitly constrain the subject kinds that it accepts. Unlike Dart, descriptor values must be allowed to be ordinary rich Incan structural values rather than only a fixed annotation-argument vocabulary. [Dart metadata](https://dart.dev/language/metadata)
- **Kotlin annotations and KSP** distinguish declaration facts from reflection and let annotation definitions specify targets, retention, repeatability, and documentation participation. Incan should make the checked and runtime projections separate and explicit, make supported subjects and multiplicity part of the contract, and let tools consume compiler-owned facts. It must not require libraries to install a separate source processor to reconstruct the canonical catalog. [Kotlin annotations](https://kotlinlang.org/docs/annotations.html), [Kotlin Symbol Processing](https://kotlinlang.org/docs/ksp-overview.html)
- **C# attributes and Roslyn** demonstrate that an IDE, diagnostics, code generation, and project tooling can share a compiler semantic model rather than scrape source independently. Incan's checked registry projection must be a first-class compiler fact consumed by inspection, documentation, LSP, source graph, and agent tooling. C#'s restricted attribute-argument types are not a fit for domain-owned descriptors such as `FunctionSpec`; Incan should keep the descriptor as a typed structural value. [C# attributes](https://learn.microsoft.com/en-us/dotnet/csharp/advanced-topics/reflection-and-attributes/attribute-tutorial), [Roslyn compiler model](https://learn.microsoft.com/en-us/dotnet/csharp/roslyn-sdk/compiler-api-model)
- **Rust `inventory`** shows the value of registrations distributed across linked dependencies without a central hand-maintained list. It also establishes the boundary: a linked-program collection is not a complete source-package catalog. Incan's runtime API must therefore say that it returns loaded entries, while complete discovery belongs to the checked projection. [Rust `inventory`](https://docs.rs/inventory/latest/inventory/macro.submit.html)
- **Java `ServiceLoader`** shows that a provider can be inspected before it is instantiated, but its provider configuration is still string-addressed and discovery remains runtime and classpath-dependent. Incan must retain the inspect-before-use benefit while rejecting sidecar string configuration, lazy discovery as an authority, and runtime state as a substitute for a checked catalog. [Java `ServiceLoader`](https://docs.oracle.com/en/java/javase/17/docs/api/java.base/java/util/ServiceLoader.html)

These lessons produce three syntax rules:

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
- Define `Registry[T]`, registry entries, registry subjects, and descriptor identity without hardcoding domain fields such as lifecycle, permissions, documentation links, or backend mappings.
- Define `@describe(...)` as a readable declaration-side registration form without introducing a global `feature` keyword or a comment-based mini-language.
- Support descriptions for declarations, modules, and packages without fake functions or sentinel declarations that exist only to carry prose.
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

A registry owner defines the domain descriptor. A function-oriented library can keep its function rules typed and adjacent to the functions they describe. The `Descriptor` derive spelling in this example is illustrative; the required structural descriptor contract is specified below and its final opt-in spelling remains open:

```incan
from std.registry import Registry, describe

@derive(Descriptor)
pub model FunctionSpec:
    canonical_name: str
    lifecycle: Lifecycle
    policy: FunctionPolicy


pub static functions: Registry[FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function, SubjectKind.Method],
)


@describe(functions, FunctionSpec(
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
from std.registry import Registry, RegistrySubject

pub static capabilities: Registry[CapabilityDescriptor] = Registry.define(
    subjects=[SubjectKind.CompilationUnit, SubjectKind.Package],
)


pub static logging_capability: RegistryEntry[CapabilityDescriptor] = capabilities.entry(
    subject=RegistrySubject.current_unit(),
    descriptor=CapabilityDescriptor(
        id=CapabilityId("std.logging"),
        title="Structured logging",
        stability=Stability.Stable,
    ),
)
```

`logging_capability` is a real registry entry value, not a sentinel declaration. It can be inspected or passed to runtime code, and its subject is the defining compilation unit. A corresponding `RegistrySubject.package()` form supports package-wide facts. The exact final spelling of explicit non-declaration subjects remains an unresolved question, but the registry contract must not require a fake callable, a bare `module` marker, or a comment block.

### Use domains without creating a metadata monoculture

The same mechanism supports different domains without merging their meanings:

```incan
from std.registry import Registry, describe

pub static commands: Registry[CommandSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)
pub static adapters: Registry[AdapterSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)


@describe(commands, CommandSpec(name="check", summary="Validate the project"))
pub def check_project(path: Path) -> Result[CheckReport, CheckError]:
    ...


@describe(adapters, AdapterSpec(name="json", media_type="application/json"))
pub def render_json(report: CheckReport) -> bytes:
    ...
```

The registry does not decide what a command, adapter, capability, policy, or function means. It only provides the checked declaration-to-descriptor relationship that lets each domain expose a coherent catalog.

## Reference-level explanation

### Registry types and identities

`std.registry` must provide a generic `Registry[T]` type for a descriptor type `T` that satisfies the descriptor-value contract defined by this RFC. A registry must have a canonical identity derived from its defining package, module, and public binding. A registry may be private, but only a public registry is discoverable outside its defining package by default.

The initial registry-definition form is `pub static name: Registry[T] = Registry.define(subjects=[...])`. `Registry.define` is a compiler-known declarative constructor, not a general mutable-container constructor. The binding supplies the registry identity, and the `subjects` argument declares the kinds of source subject that may be described through it. A description whose target is not in this set is a type-checking error at the description site. The exact subject-kind names and whether a registry may extend them across a compatibility version remain part of the public API design.

A registry entry must contain at least:

- canonical registry identity;
- canonical subject identity;
- subject kind: declaration, compilation unit, named submodule, or package;
- typed descriptor value;
- source anchor for the registration and source anchor for the subject when distinct;
- defining package and module identity;
- declared visibility and reexport projections where applicable;
- origin classification: checked declaration, runtime observation, imported artifact, or derived projection.

The compiler must use the canonical defining declaration as the subject identity. A public reexport may expose an additional import path, but it must not create a second semantic entry or change the entry's origin.

The registry must guarantee deterministic checked output ordering. Ordering must not depend on filesystem traversal, hash-map order, or runtime module initialization. The canonical sort key must include registry identity, subject identity, and a deterministic per-subject description ordinal when a declaration has multiple descriptions in the same registry.

### Descriptor values

The descriptor type is owned by the registry's domain. A descriptor type must opt into structural description through the standard `Descriptor` contract. The contract must reject descriptor shapes that cannot be checked and serialized without executing user code.

A descriptor value may contain safe primitive values, `None`, enum variants, type references, checked constant references, descriptor collections, descriptor maps with stable keys, and constructions of descriptor types whose fields recursively satisfy the descriptor-value contract. A descriptor value must not depend on host state, mutable statics, I/O, reflection over a running program, an arbitrary function call, a secret reveal, or any expression whose value requires user-code execution.

The compiler must type-check a descriptor expression and preserve its typed structural form. It must not claim to have evaluated an unsupported expression. A diagnostic for an unsupported descriptor value must identify the expression and explain that descriptions are compile-time structural facts rather than runtime observations.

Descriptor values are snapshots at the description site. A runtime consumer must not be able to mutate a loaded descriptor in a way that changes the registry's checked projection or causes the registry to misrepresent the declared descriptor. The exact immutability and derive spelling is an unresolved question, but the final contract must make this divergence impossible or explicit.

### Declaration descriptions

`@describe(registry, descriptor)` must be a compiler-known declarative decorator supplied by `std.registry`. It must be valid on the declaration kinds accepted by the active language version. The decorator must preserve the declaration's ordinary runtime type and callable behavior; it must not wrap a declaration with unrelated behavior merely to record a description.

`@describe` deliberately has no domain-specific named arguments. The registry argument identifies a typed catalog and the descriptor argument is one ordinary typed structural value of that catalog's descriptor type. Syntax such as `@describe(functions, FunctionSpec(...))` keeps lifecycle, policy, documentation, and domain keys inside the domain-owned `FunctionSpec` instead of creating a second, string-shaped annotation schema.

The compiler must resolve `describe` by canonical symbol identity, including supported aliases and reexports of the standard decorator. It must not recognize a user-defined function only because it is named `describe`.

The registry argument must resolve to a `Registry[T]`, and the descriptor argument must type-check as `T`. Both must be valid in the defining package's visibility and initialization context. The compiler must reject a description that targets a registry whose descriptor type does not match, whose identity cannot be resolved, or whose registration would introduce an invalid duplicate identity.

A declaration may carry descriptions from multiple registries. A declaration may carry multiple descriptions in one registry only when their deterministic ordinals and domain-level identities are unambiguous. The compiler must preserve source order for those descriptions and report duplicate domain identities at the later source location when the registry or descriptor type declares such an identity rule.

Ordinary decorators and `@describe` may be stacked. The checked description describes the source declaration's canonical public contract, not an accidental wrapper shape. The final decorator-order rule must be explicit and preserve the source declaration's callable signature, aliases, and source anchor.

### Compilation-unit, named-submodule, and package descriptions

The registry surface must support descriptions whose subject is the defining compilation unit, a future named submodule, or the defining package. These descriptions must be represented as real registry entries and must be available to both runtime and checked projections.

A compilation-unit, named-submodule, or package description must carry the corresponding canonical identity as its subject. It must not be modeled by attaching a descriptor to an unrelated function, by parsing a comment block, or by relying on a magic constant name.

The initial surface may use an explicit entry-producing form such as `registry.entry(subject=RegistrySubject.current_unit(), descriptor=...)` and `registry.entry(subject=RegistrySubject.package(), descriptor=...)`. Such a form must have ordinary typed source semantics, return a registry entry value or equivalent typed handle, and be accepted only in a declaration context with deterministic initialization behavior. A future `module name:` declaration may use ordinary `@describe` decorators once it has real name, body, visibility, import, and nesting semantics; this RFC must not introduce a bare `module` marker merely to create a registry subject.

### Runtime projection

Loading a module that contains valid registry descriptions must make its entries available to the target registry's runtime projection. Runtime registration must preserve typed descriptors and canonical subject identity. Repeated loading or equivalent reexport paths must not duplicate an entry.

Runtime registry enumeration must distinguish loaded entries from a complete checked catalog. `loaded_entries()` and similar runtime APIs must not claim package completeness unless the registry has explicitly performed a documented complete-loading operation. A runtime registry may expose its loaded-state or completeness classification so diagnostics do not confuse a missing unloaded module with a missing declaration.

Dynamic runtime additions may be supported by registry APIs, but they must be marked runtime-observed and must not appear in checked package metadata or generated documentation as if they were authored declarations. A package may choose to reject dynamic additions for a particular registry.

### Checked projection and artifacts

The compiler must make checked registry entries available through a stable inspection and metadata contract. The projection must include typed descriptor structure, canonical identities, visibility, provenance, source anchors, import/reexport paths, and explicit unsupported or degraded states where available.

The projection must be available for a source package after checking and for a built package artifact that carries the required checked data. A consumer that only has a runtime registry must report that complete checked discovery is unavailable rather than synthesizing facts from loaded entries.

`incan inspect registry` must support selecting a registry by canonical identity and emitting a deterministic machine-readable representation. The final command spelling and package-closure selection are unresolved, but the output must be suitable for documentation generation, package tooling, LSP features, policy tooling, and compiler-backed agents.

Generated documentation and artifact tooling must consume the checked registry projection when they need completeness. They may use runtime registry APIs for execution behavior, but must not scrape source, parse comments, or inspect generated Rust as an alternate authority.

### Domain validation and duplicate rules

`std.registry` must validate generic registry invariants: registry identity, subject identity, descriptor type compatibility, structural descriptor validity, declaration visibility, deterministic ordering, and duplicate registration of the same subject identity.

Domain-specific invariants remain owned by descriptor types or domain registry constructors. A function catalog may require unique canonical function names; a command catalog may require unique command paths; a capability catalog may require globally stable capability IDs. The domain must be able to declare these rules in a typed, inspectable form so the compiler and runtime can report the same duplicate or compatibility error.

The generic registry must not infer domain uniqueness by searching arbitrary string fields. A descriptor's meaningful key must be explicit in its descriptor contract or domain registry configuration.

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
- Module and package subjects require a first-class source representation that the current declaration surface does not yet provide uniformly.
- A broad registry abstraction could become overused if it is presented as a general application-state container rather than a declaration catalog.

## Implementation architecture

*(Non-normative.)* The implementation should share one normalized registry-entry model between type checking, runtime registration lowering, checked metadata extraction, package artifacts, inspection, documentation consumers, and source-graph export. Runtime registration and checked projection should be generated from the same validated declaration facts, while remaining explicit about their different completeness guarantees. The first delivery should prove the model with one standard-library capability catalog and one existing function-oriented library registry before broadening the supported subject or descriptor forms.

## Layers affected

- **Parser / AST:** needs declaration registration forms and typed explicit compilation-unit/package subjects with source anchors, without introducing a global `feature` keyword or a registry-only bare `module` marker. A later general `module name:` declaration belongs to the module-system surface rather than being invented by this RFC.
- **Typechecker / symbol resolution:** must resolve `std.registry` identities, validate registry and descriptor types, enforce structural descriptor values, preserve declaration subjects, and report duplicate or inaccessible registrations.
- **Runtime / lowering / emission:** must preserve ordinary typed runtime registry behavior for loaded modules without changing described declarations' callable or nominal semantics.
- **Stdlib / Runtime (`incan_stdlib`):** must provide `Registry[T]`, registry-entry handles, `@describe`, explicit compilation-unit/package subject forms, and descriptor-value support appropriate for ordinary Incan authors.
- **Checked metadata and package artifacts:** must carry complete checked registry entries, identities, descriptor structure, visibility, provenance, and degraded states.
- **CLI / inspection tooling:** must provide deterministic registry inspection and actionable diagnostics without invoking user module initialization.
- **Formatter:** must format description decorators and descriptor expressions deterministically.
- **LSP / editor tooling:** must provide completion, hover, go-to-definition, registry-membership navigation, and source diagnostics from checked facts.
- **Documentation and artifact tooling:** must consume checked registry projections for capability inventories and other catalog views rather than source scanners.
- **Source graph / agent tooling:** must expose registry facts and distinguish checked declarations from runtime observations.

## Unresolved questions

- What is the final spelling and lifecycle of explicit compilation-unit and package subject handles? The current illustrative `RegistrySubject.current_unit()` and `RegistrySubject.package()` forms keep those identities typed without pretending that the source file has a bare `module` declaration.
- What is the general submodule declaration design? `module` must retain the meaning of a named, scoped submodule declaration—consistent with the existing `module tests:` shape—and must gain real name, body, visibility, import, and nesting semantics before it can carry ordinary `@describe` decorators. This is a module-system decision, not a registry-specific syntax escape hatch.
- What exact opt-in spelling establishes a descriptor type and its immutable structural serialization contract?
- Which declaration kinds are supported by `@describe` in the first release, and how should descriptor decorators compose with existing user-defined decorators?
- How does a domain declare a typed uniqueness key without requiring the generic registry to inspect arbitrary string fields or execute a user method?
- Should dynamic runtime registration be part of the first `std.registry` surface, and if so, what API makes its non-static status unmistakable?
- How are package-closure selection, private registry access, and optional dependency entries represented by `incan inspect registry`?
- Should registry entries have a versioned standalone artifact encoding in the first implementation, or should package checked-metadata embedding be the only initial artifact form?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
