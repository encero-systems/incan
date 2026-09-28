# `std.registry`

`std.registry` declares typed catalogs. A registry associates a typed key and a descriptor with a function, a method, a compilation unit or a package. Each registry has a loaded view, `loaded_entries()`, and a checked view, `incan inspect registry`.

## Imports

```incan
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind, describe
```

## Public API

| Symbol | Kind | Purpose |
| --- | --- | --- |
| `SubjectKind` | Enum | The subject kinds a registry accepts. |
| `RegistrySubject` | Model | The kind and qualified name of an entry's subject. |
| `RegistryEntry[K, T]` | Model | One key, descriptor and subject. |
| `Registry[K, T]` | Class | A typed catalog and its loaded entries. |
| `describe[K, T, F]` | Function | The `@describe` decorator, which makes a function or method an entry. |

## `SubjectKind`

```incan
pub enum SubjectKind:
    Function
    Method
    CompilationUnit
    Package
```

| Variant | Subject | Entry form |
| --- | --- | --- |
| `Function` | A function | `@describe` on the function. |
| `Method` | A method of a class, model, enum or newtype | `@describe` on the method. |
| `CompilationUnit` | The module that declares the entry | A `RegistryEntry` static with `RegistrySubject.current_unit()`. |
| `Package` | The package that declares the entry | A `RegistryEntry` static with `RegistrySubject.package()`. |

## `RegistrySubject`

```incan
@derive(Clone)
pub model RegistrySubject:
    pub kind: SubjectKind
    pub qualified_name: str
```

### `RegistrySubject.current_unit()`

```incan
@staticmethod
def current_unit() -> RegistrySubject
```

As the `subject` argument of `registry.entry(...)` in a `RegistryEntry` static, the entry's subject is the declaring module: `kind` is `SubjectKind.CompilationUnit` and `qualified_name` is the declaring module's name. Elsewhere, returns `RegistrySubject(kind=SubjectKind.CompilationUnit, qualified_name="<current-unit>")`.

### `RegistrySubject.package()`

```incan
@staticmethod
def package() -> RegistrySubject
```

As the `subject` argument of `registry.entry(...)` in a `RegistryEntry` static, the entry's subject is the declaring package: `kind` is `SubjectKind.Package` and `qualified_name` is the package's name. Elsewhere, returns `RegistrySubject(kind=SubjectKind.Package, qualified_name="<package>")`.

A call from source to `RegistrySubject._checked_current_unit(...)` or `RegistrySubject._checked_package(...)` is refused (`INCAN-T0001`).

## `RegistryEntry[K, T]`

```incan
@derive(Clone)
pub model RegistryEntry[K, T]:
    pub key: K
    pub descriptor: T
    pub subject: RegistrySubject
```

A `RegistryEntry` is one entry of a registry: from an `@describe` declaration (`Function` and `Method` subjects), or a `RegistryEntry` static initialized by [`registry.entry(...)`](#registryentry) (`CompilationUnit` and `Package` subjects).

## `Registry[K, T]`

```incan
@derive(Clone)
pub class Registry[K, T]:
    pub subjects: list[SubjectKind]
```

`K` is the key type and `T` is the descriptor type, a model with [`@derive(Descriptor)`](#descriptor-contract). A registry is a module static; the static's module and binding name form its [identity](#checked-identities). `subjects` holds the subject kinds passed to `Registry.define`.

A call from source to `Registry._describe(...)`, `Registry._describe_function(...)` or `Registry._describe_method(...)` is refused (`INCAN-T0001`).

### `Registry.define`

```incan
@staticmethod
def define(subjects: list[SubjectKind]) -> Registry[K, T]
```

A static of type `Registry[K, T]` is initialized by `Registry.define(...)`:

```incan
pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function, SubjectKind.Method],
)
```

- `subjects`, positional or named, is a list literal of `SubjectKind.<Variant>` values.
- Refused (`INCAN-T0001`): a `Registry[K, T]` static with another initializer; no `subjects` argument or more than one argument; a `subjects` value that is not a list literal of `SubjectKind` variants; a list spread; a repeated kind; and an empty list.

### `Registry.loaded_entries`

```incan
def loaded_entries(self) -> list[RegistryEntry[K, T]]
```

Returns the registry's entries from the modules initialized in the current process.

### `Registry.entry`

```incan
def entry(mut self, key: K, subject: RegistrySubject, descriptor: T) -> RegistryEntry[K, T]
```

Declares a `CompilationUnit` or `Package` entry. The call is the initializer of a module static of type `RegistryEntry[K, T]`, and its receiver is a registry static of the same module. The registry static is not required to be `mut`.

```incan
pub static package_entry: RegistryEntry[CapabilityId, CapabilitySpec] = capabilities.entry(
    key=CapabilityId("catalog"),
    subject=RegistrySubject.package(),
    descriptor=CapabilitySpec(summary="Package catalog"),
)
```

- `key`, `subject` and `descriptor` are named arguments. `subject` is `RegistrySubject.current_unit()` or `RegistrySubject.package()`. `key` and `descriptor` are [structural values](#structural-values).
- Refused (`INCAN-T0001`): a call that is not the initializer of a `RegistryEntry` static; a receiver that is not a registry static of the module; a `RegistryEntry[K, T]` whose `K` or `T` differs from the registry's; a positional, unknown, repeated or missing argument; another `subject` expression; a subject kind the registry's `subjects` does not list; a key or descriptor of the wrong type; a value that is not structural; and a key that the registry already has.

## `@describe`

```incan
def describe[K, T, F](registry: Registry[K, T], key: K, descriptor: T) -> (F) -> F
```

```text
@describe(registry, key, descriptor)
```

`@describe` makes a function or a method an entry of `registry`, with `key` and `descriptor`. The declaration keeps its type and behavior. `@describe` stacks with other decorators, and each `@describe` adds one entry.

```incan
@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize a label", stable=True, input_type=str))
pub def normalize(label: str) -> str:
    return label.strip().lower()
```

- The subject of a function is `SubjectKind.Function`; the subject of a method of a class, model, enum or newtype is `SubjectKind.Method`.
- `registry` is the name of a module static of type `Registry[K, T]` initialized by `Registry.define`, in the same module or imported. `key` and `descriptor` are [structural values](#structural-values).
- Refused (`INCAN-T0001`): other than three positional arguments; a `registry` that is not such a static; a subject kind the registry's `subjects` does not list; a key or descriptor of the wrong type; a value that is not structural; a key that the registry already has; and `@describe` on a model, class, enum, newtype, trait, trait method or capability declaration.
- An import of a registry that is not `pub` is refused (`INCAN-I0001`).

## Descriptor contract

`@derive(Descriptor)` on a model makes it a descriptor type:

```incan
@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str
    pub stable: bool
    pub input_type: Type[str]
```

| Field type | Condition |
| --- | --- |
| `int`, `float`, exact-width numeric types, `bool`, `str`, `bytes`, `FrozenStr`, `FrozenBytes`, `None` | |
| `Type[T]` | `T` is a concrete type: not a type parameter, a function type or a `rust::` type. |
| A newtype | Not generic and not `rusttype`; its underlying type is a field type. |
| An enum | Every variant has no fields. |
| A model | Has `@derive(Descriptor)` and is not generic. |
| `Option[T]` | `T` is a field type. |
| `FrozenList[T]` | `T` is a field type. |
| `FrozenDict[K, V]` | `K` is `int`, `float`, an exact-width numeric type, `bool`, `str`, `bytes`, `FrozenStr`, `FrozenBytes`, a fieldless enum or a newtype that is a field type; `V` is a field type. |

- Refused (`INCAN-T0001`): `@derive(Descriptor)` on a class, enum or newtype; a generic descriptor model; and a field of another type, including `list`, `dict`, `set`, `FrozenSet`, tuples, `Result`, functions, classes, type parameters, Rust types, and a model that contains itself.

### Structural values

A key or descriptor expression in `@describe` or `registry.entry(...)` is one of:

- an `int`, `float`, `str`, `bytes`, `bool` or `None` literal;
- a list or dict literal of structural values, without spreads;
- `Some(value)`;
- the name of a `const` whose value is structural, recorded as that value;
- a qualified name that starts with a `const` or an enum, such as an enum variant, recorded as a `const_ref`;
- a type name;
- a newtype call with one positional structural value;
- a model call with named structural values.

Any other expression is refused (`INCAN-T0001`), as is a `const` that refers to itself. A descriptor value is not evaluated by inspection.

## Checked identities

A registry's identity is its module and binding:

```text
<module>::<binding>
```

A public `functions` static in `src/catalog.incn` has identity `catalog::functions`. The package identity is a separate field of the checked metadata.

A registry selector is the identity, with `::` or `.` between segments, optionally prefixed by the package name:

```text
<module>::<binding>
<module>.<binding>
<package>::<module>::<binding>
```

```console
incan inspect registry catalog::functions --project . --format json
incan inspect registry catalog.functions --project . --format json
incan inspect registry analytics-kit::catalog::functions --project . --format json
```

A package-qualified selector selects a candidate; the selected registry's `identity` field is `<module>::<binding>`.

## Checked inspection

```console
incan inspect registry IDENTITY [--project PATH] [--format json]
```

| Argument | Contract |
| --- | --- |
| `IDENTITY` | A registry selector. |
| `--project PATH` | Project root or source entry. Defaults to the current directory. |
| `--format json` | The checked JSON projection. `json` is the only value and the default. |

- Inspection reads the source package and the `.incnlib` metadata of each resolved dependency, and runs no user code.
- A selector that matches no registry fails the command, and the failure lists the available selectors and each dependency whose registry metadata is missing, unreadable or of an unsupported schema version.
- A selector that matches more than one registry fails the command, and the failure lists the matching selectors.

### JSON shape

The top-level object contains:

| Field | Meaning |
| --- | --- |
| `schema_version` | The checked registry schema version, `1`. |
| `provenance` | `"checked"`. |
| `package` | The package's `name` and optional `version`, or `null`. |
| `registry` | The selected definition: `identity`, `binding`, `public`, `key_type`, `descriptor_type`, `subjects`, and `reexport_paths` when non-empty. |
| `entries` | The registry's entries, in a deterministic order. |

Each entry contains `registry_identity`, `registry_public`, `key`, `descriptor`, `subject_kind`, `subject_identity`, `registration_anchor`, `subject_anchor` and `provenance`, and `reexport_paths` when non-empty. A subject kind is `function`, `method`, `compilation_unit` or `package`.

A structural value is an object with a `kind` and, except for `none`, a `value`. The kinds are `int`, `float`, `bool`, `string`, `bytes`, `none`, `type`, `option`, `list`, `dict`, `const_ref`, `newtype` and `model`.

Entry provenance is one of `checked_declaration`, `checked_compilation_unit_entry` or `checked_package_entry`.

## Visibility, packages and reexports

- Inspecting a source package includes its private registries.
- `incan build --lib` publishes the public registry definitions and their public entries in the library's `.incnlib`. A consumer inspects a dependency's public registries only.
- A public reexport adds a path to `reexport_paths`, with the facade's source anchor. The registry identity, the subject identity and the entries are unchanged.

## See also

- [Build a typed function catalog](../../tutorials/typed_registries.md)
- [Work with typed registries](../../how-to/typed_registries.md)
- [Checked catalogs and loaded registries](../../explanation/checked_and_loaded_registries.md)
- [Codegraph inspection](../../../tooling/reference/codegraph_inspection.md)
- [`incan inspect registry` in the CLI reference](../../../tooling/reference/cli_reference.md#incan-inspect-registry)
