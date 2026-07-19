# `std.registry`

`std.registry` defines typed declaration catalogues. A registry associates a typed key and descriptor with a function, concrete method, compilation unit, or package. The compiler validates one source-owned catalogue and exposes it as both process-local runtime state and complete checked metadata.

Use `loaded_entries()` when application code needs entries from modules loaded in the current process. Use `incan inspect registry` when tooling needs the complete checked package projection without executing user code.

## Imports

```incan
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind, describe
```

## Public API

| Symbol | Kind | Purpose |
| --- | --- | --- |
| `SubjectKind` | Enum | Declares which source subject categories a registry accepts. |
| `RegistrySubject` | Model | Carries the resolved kind and qualified identity of a loaded entry. |
| `RegistryEntry[K, T]` | Model | Holds one typed key, descriptor, and subject. |
| `Registry[K, T]` | Class | Defines a typed catalogue and its loaded runtime projection. |
| `describe[K, T, F]` | Decorator factory | Marks a function or concrete method as an entry without wrapping the callable. |

## `SubjectKind`

```incan
pub enum SubjectKind:
    Function
    Method
    CompilationUnit
    Package
```

The `subjects` passed to `Registry.define` are an allow-list. Describing a subject that the registry did not declare is a type-checking error.

## `RegistrySubject`

```incan
@derive(Clone)
pub model RegistrySubject:
    pub kind: SubjectKind
    pub qualified_name: str
```

### `RegistrySubject.current_unit()`

```incan
RegistrySubject.current_unit() -> RegistrySubject
```

Returns the explicit placeholder for the compilation unit containing a static `RegistryEntry`. The compiler replaces the placeholder with the checked module identity during lowering.

### `RegistrySubject.package()`

```incan
RegistrySubject.package() -> RegistrySubject
```

Returns the explicit placeholder for the defining package. The compiler supplies the manifest package identity during lowering and checked metadata generation.

The underscore-prefixed checked constructors are compiler implementation surfaces and are not application APIs.

## `RegistryEntry[K, T]`

```incan
@derive(Clone)
pub model RegistryEntry[K, T]:
    pub key: K
    pub descriptor: T
    pub subject: RegistrySubject
```

An entry is the runtime representation of one checked association. Function and method entries are emitted from `@describe` sites. Compilation-unit and package entries are declared explicitly as statics.

## `Registry[K, T]`

`K` is the domain-owned structural key type and `T` is the descriptor type. The registry binding supplies the catalogue identity.

### `Registry.define`

```incan
Registry.define(subjects: list[SubjectKind]) -> Registry[K, T]
```

Defines a declarative registry with the permitted subject kinds:

```incan
pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function, SubjectKind.Method],
)
```

The compiler recognizes this form only in a typed registry static. It validates registry identity and subject constraints; it is not a general mutable-container constructor.

### `Registry.loaded_entries`

```incan
registry.loaded_entries() -> list[RegistryEntry[K, T]]
```

Returns entries contributed by modules initialized in the current process. The result is not a complete package inventory.

### `Registry.entry`

```incan
registry.entry(
    key: K,
    subject: RegistrySubject,
    descriptor: T,
) -> RegistryEntry[K, T]
```

Constructs an explicit compilation-unit or package entry. The call must initialize a deterministic static declaration, and the subject kind must be allowed by the registry.

```incan
pub static package_entry: RegistryEntry[CapabilityId, CapabilitySpec] = capabilities.entry(
    key=CapabilityId("catalogue"),
    subject=RegistrySubject.package(),
    descriptor=CapabilitySpec(summary="Package catalogue"),
)
```

## `@describe`

```incan
@describe(registry, key, descriptor)
```

`@describe` is valid on functions and concrete methods. It preserves the declaration's callable type and runtime behavior; the compiler records the registry fact independently of ordinary decorator application.

```incan
@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize a label", stable=True))
pub def normalize(label: str) -> str:
    return label.strip().lower()
```

Ordinary decorators may be stacked with `@describe`. Placement does not transfer source ownership, create a wrapper signature, or create an additional entry.

## Descriptor contract

Descriptor models opt into immutable structural snapshots with `@derive(Descriptor)`:

```incan
@derive(Descriptor)
pub model FunctionSpec:
    summary: str
    stable: bool
    input_type: type
```

| Accepted structural value | Notes |
| --- | --- |
| `int`, `float`, `bool`, `str`, `bytes`, `None` | Captured directly. |
| Concrete Incan type tokens | Open or unresolved types are rejected. |
| Validated newtypes | Preserve the newtype name and validated underlying value. |
| Fieldless enums | Preserve the typed variant identity. |
| Nested `@derive(Descriptor)` models | Every field must also satisfy the contract. |
| `Option[T]` | The contained value must be structural. |
| `FrozenList[T]` and `FrozenDict[K, V]` | Captured deterministically as immutable values. |
| References to structural `const` values | Expanded into their checked value. |

Mutable containers, functions, Rust handles, open generics, dynamic calls, and recursive descriptor graphs are rejected. Descriptor expressions are checked at the declaration site and are never evaluated by inspection.

Keys follow the same structural principle. A registry rejects duplicate checked keys in its checked declaration scope.

## Checked identities

A registry definition has a module-local canonical identity:

```text
<module>::<binding>
```

For example, a public `functions` static in `src/catalog.incn` has identity `catalog::functions`. The checked package identity is carried separately in metadata.

CLI selection accepts either double-colon or dotted module-local spelling:

```console
incan inspect registry catalog::functions --project . --format json
incan inspect registry catalog.functions --project . --format json
```

When two resolved packages publish the same module-local identity, qualify the selector:

```text
<package>::<module>::<binding>
```

For example:

```console
incan inspect registry analytics-kit::catalog::functions --project . --format json
```

The package-qualified spelling selects a candidate; it does not rewrite the source-owned `registry.identity` field.

## Checked inspection

```console
incan inspect registry IDENTITY [--project PATH] [--format json]
```

| Argument | Contract |
| --- | --- |
| `IDENTITY` | Module-local or package-qualified registry selector. |
| `--project PATH` | Project root or source entry. Defaults to the current directory. |
| `--format json` | Emits the versioned checked JSON projection. JSON is the only v0.5 format. |

Inspection analyzes the source graph through one `CompilationSession`. It does not initialize user modules. Compatible dependency registries are read from their `.incnlib` metadata rather than reconstructed from dependency source.

### JSON shape

The top-level object contains:

| Field | Meaning |
| --- | --- |
| `schema_version` | Checked registry wire-schema version. |
| `provenance` | `"checked"` for this command. |
| `package` | Optional package `name` and `version`. |
| `registry` | Selected definition: identity, binding, visibility, key type, descriptor type, accepted subjects, and registry facade paths. |
| `entries` | Deterministically ordered checked entries belonging to the selected registry. |

Each entry contains `registry_identity`, `registry_public`, `key`, `descriptor`, `subject_kind`, `subject_identity`, `registration_anchor`, `subject_anchor`, `provenance`, and optional `reexport_paths`.

Structural values use a tagged `kind` representation. The v0.5 kinds are `int`, `float`, `bool`, `string`, `bytes`, `none`, `type`, `option`, `list`, `dict`, `const_ref`, `newtype`, and `model`.

Entry provenance is one of `checked_declaration`, `checked_compilation_unit_entry`, or `checked_package_entry`.

## Visibility, packages, and reexports

Source-package inspection may select private registries owned by that package. A built library publishes only public registry definitions and public entries. Consumers therefore cannot inspect a producer's private catalogue through its dependency artifact.

Public reexports add paths to `reexport_paths` with facade source anchors. They never change the canonical registry or subject identity and never create duplicate entries.

If a dependency artifact predates the checked registry schema, inspection reports that the dependency does not publish compatible registry metadata. Rebuild the dependency with a compatible SDK.

## Tooling projections

- `incan inspect codegraph --format jsonl` emits checked registry records and facade paths under codegraph schema v2.
- The LSP uses the checked facts for registry membership hover and navigation.
- Library builds embed public registry metadata in the generated `.incnlib`.
- The generated Incan feature inventory is sourced from the checked `std.capabilities` registry.

All of these projections originate from the same compilation analysis. A tooling consumer must not infer registry meaning by scanning `@describe` syntax or loading runtime state.

## Diagnostics

The compiler reports source-anchored errors for:

- a registry argument that does not resolve to `Registry[K, T]`;
- a key or descriptor whose type does not match `K` or `T`;
- a descriptor type without the structural descriptor contract;
- a dynamic or unsupported structural value;
- a subject kind not declared by the registry;
- a duplicate key;
- an inaccessible registry or declaration;
- an unsupported trait-method or named-module subject;
- an ambiguous or missing inspection selector;
- an incompatible dependency metadata schema.

## Current boundaries

The v0.5 surface supports functions, concrete methods, compilation units, and packages. It does not define a general named-module declaration, dynamic runtime-only registration, remote registry service, dependency injection container, event bus, or authority/capability grant.

Use a custom runtime registry when entries genuinely depend on runtime data and accept that it has no complete checked projection.

## See also

- [Build a typed function catalogue](../../tutorials/typed_registries.md)
- [Work with typed registries](../../how-to/typed_registries.md)
- [Checked catalogues and loaded registries](../../explanation/checked_and_loaded_registries.md)
- [RFC 113: `std.registry` and declaration descriptors](../../../RFCs/closed/implemented/113_std_registry_and_declaration_descriptors.md)
- [Codegraph inspection](../../../tooling/reference/codegraph_inspection.md)
