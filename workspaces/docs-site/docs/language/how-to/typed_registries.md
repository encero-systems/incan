# Work with typed registries

Use this guide when you already understand the basic `std.registry` model and need to choose a subject, migrate an existing catalogue, publish it through a facade, inspect dependency metadata, or diagnose a rejected entry.

## Choose the subject kind

Declare every subject kind the registry accepts:

```incan
pub static commands: Registry[CommandId, CommandSpec] = Registry.define(
    subjects=[SubjectKind.Function, SubjectKind.Method],
)
```

Use the narrowest set that matches the domain. A function-only catalogue should not admit package entries merely because another registry might need them.

| Subject | Source form | Use it for |
| --- | --- | --- |
| `Function` | `@describe(...)` on a function | Commands, transforms, handlers, adapters |
| `Method` | `@describe(...)` on a concrete method | Operations whose identity includes an owning type |
| `CompilationUnit` | A static `registry.entry(...)` with `RegistrySubject.current_unit()` | Facts owned by one source module |
| `Package` | A static `registry.entry(...)` with `RegistrySubject.package()` | Facts owned by the package as a whole |

Traits are not method subjects in the initial surface because a trait declaration does not identify one concrete runtime method. Named modules are also outside the initial surface; `module tests:` remains a test grouping rather than a general module declaration.

## Describe a method

The registry remains a module static. Apply `@describe` to a concrete class, model, enum, or newtype method accepted by the compiler:

```incan
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FormatterId = newtype str

@derive(Descriptor)
pub model FormatterSpec:
    media_type: str

pub static formatters: Registry[FormatterId, FormatterSpec] = Registry.define(
    subjects=[SubjectKind.Method],
)

class JsonFormatter:
    @describe(formatters, FormatterId("json"), FormatterSpec(media_type="application/json"))
    pub def render(self, value: str) -> str:
        return value
```

The subject identity includes the owning type and method. Importing or reexporting the type does not manufacture a second registry entry.

## Describe a compilation unit or package

Do not create a fake function just to attach metadata. Construct an explicit static entry:

```incan
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind

@derive(Clone, Eq)
pub type CapabilityId = newtype str

@derive(Descriptor)
pub model CapabilitySpec:
    summary: str

pub static capabilities: Registry[CapabilityId, CapabilitySpec] = Registry.define(
    subjects=[SubjectKind.CompilationUnit, SubjectKind.Package],
)

pub static module_capability: RegistryEntry[CapabilityId, CapabilitySpec] = capabilities.entry(
    key=CapabilityId("text.normalization"),
    subject=RegistrySubject.current_unit(),
    descriptor=CapabilitySpec(summary="Text normalization supplied by this module"),
)

pub static package_capability: RegistryEntry[CapabilityId, CapabilitySpec] = capabilities.entry(
    key=CapabilityId("catalogue"),
    subject=RegistrySubject.package(),
    descriptor=CapabilitySpec(summary="Package-wide function catalogue"),
)
```

The compiler replaces the subject placeholder with the canonical checked module or package identity during lowering. Runtime code receives the same resolved identity; it does not discover package ownership dynamically.

## Reexport a registry and its subjects

Keep the registry and described declarations source-owned, then expose them through an ordinary public facade:

```incan title="src/catalog.incn"
pub from crate.text import functions, normalize
```

The checked entry retains its original registry and subject identity. Inspection and codegraph output add the facade paths under `reexport_paths`; they do not duplicate the entry or transfer ownership to the facade.

## Inspect a local or dependency registry

Inspect a local module-level identity:

```console
incan inspect registry text::functions --project . --format json
```

The selector may also use dots: `text.functions`. If two packages publish the same module-level identity, qualify the selector with the package:

```console
incan inspect registry analytics-kit::text::functions --project . --format json
```

Dependency inspection reads the checked registry projection embedded in the dependency's `.incnlib`; it does not load dependency source or run dependency initialization. Only public registries and public entries are visible to consumers. Local source inspection may include private registries owned by the selected package.

## Migrate a custom runtime registry

Use this sequence when a library currently registers functions through its own decorator or mutable queue:

1. Define a domain key type and a descriptor model. Put domain fields in that model instead of adding registry-specific keyword arguments.
2. Add `@derive(Descriptor)` to the descriptor model and replace dynamic descriptor expressions with structural values.
3. Define one `Registry[K, T]` as the canonical declaration authority.
4. Replace the custom registration decorator with `@describe(registry, key, descriptor)`.
5. Replace runtime enumeration with `loaded_entries()` where process-local behavior is intended.
6. Replace source scanning or documentation extraction with `incan inspect registry`.
7. Remove the old mutable or queued registry. Keeping it as a fallback would create two authorities that can disagree.
8. Test direct imports, facade reexports, package consumers, compiled test batches, inspection JSON, and generated Rust.

Dynamic-only registration may remain custom when entries genuinely depend on runtime values. Document that such a catalogue has no complete static projection rather than presenting it as equivalent to `std.registry`.

## Diagnose rejected entries

| Failure | Check |
| --- | --- |
| Registry argument is rejected | It must resolve to the declared `Registry[K, T]`, not another value with a similar name. |
| Key or descriptor type mismatch | The key must be `K` and the descriptor must be `T`; implicit string-shaped metadata is not accepted. |
| Descriptor is not structural | Add `@derive(Descriptor)` and remove mutable containers, functions, Rust handles, open generics, or recursive descriptor graphs. |
| Subject kind is rejected | Add the required `SubjectKind` to `Registry.define(...)` or choose the correct registry. |
| Duplicate key is rejected | Give each entry a unique typed key within that registry. Reexports do not require a new entry. |
| Dependency registry is missing | Rebuild the dependency with an SDK that publishes RFC 113 metadata; older `.incnlib` artifacts do not contain the projection. |
| Selector is ambiguous | Use `package::module::registry` instead of the shorter module-local identity. |

For the complete type and JSON contracts, see the [`std.registry` reference](../reference/stdlib/registry.md). For the design boundary between loaded and checked views, see [Checked catalogues and loaded registries](../explanation/checked_and_loaded_registries.md).
