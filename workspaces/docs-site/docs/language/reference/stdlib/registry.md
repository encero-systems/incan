# `std.registry`

`std.registry` defines typed declaration catalogues. A registry records a checked descriptor for a function, method, compilation unit, or package. It has two deliberately different views:

- `loaded_entries()` is ordinary runtime state: it contains entries from modules that this process loaded.
- `incan inspect registry` reads the compiler-checked package projection: it can enumerate every checked entry without running user module code.

## Declaration catalogues

Use `@describe` for functions and concrete methods. The decorator does not wrap the declaration or change its callable type.

```incan
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionDescriptor:
    title: str
    stable: bool

pub static functions: Registry[FunctionId, FunctionDescriptor] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(
    functions,
    FunctionId("normalize"),
    FunctionDescriptor(title="Normalize a label", stable=True),
)
pub def normalize(label: str) -> str:
    return label.strip().lower()
```

`FunctionDescriptor` opts into checked structural snapshots with `@derive(Descriptor)`. Descriptor fields may use primitives, concrete Incan type tokens, validated newtypes, fieldless enums, nested descriptor models, `Option`, `FrozenList`, and `FrozenDict`. Mutable containers, functions, Rust handles, open generics, and recursive descriptor graphs are rejected.

## Compilation-unit and package facts

Use a real `RegistryEntry` value when the subject is a compilation unit or package. This avoids fake declarations that exist only to carry metadata.

```incan
from std.registry import Registry, RegistryEntry, RegistrySubject, SubjectKind

@derive(Clone, Eq)
type CapabilityId = newtype str

@derive(Descriptor)
model CapabilityDescriptor:
    title: str

pub static capabilities: Registry[CapabilityId, CapabilityDescriptor] = Registry.define(
    subjects=[SubjectKind.CompilationUnit, SubjectKind.Package],
)

pub static logging: RegistryEntry[CapabilityId, CapabilityDescriptor] = capabilities.entry(
    key=CapabilityId("std.logging"),
    subject=RegistrySubject.current_unit(),
    descriptor=CapabilityDescriptor(title="Structured logging"),
)
```

The compiler replaces `current_unit()` and `package()` with the canonical identity when it lowers the checked entry. `RegistrySubject` remains an ordinary source-owned Incan model; no Rust-authored registry facade is needed.

## Inspecting a complete catalogue

```console
incan inspect registry my_package::functions --project src/main.incn --format json
```

The JSON projection contains checked key and descriptor values, subject and registration anchors, source ownership, visibility, and provenance. Source packages may inspect private registries; library consumers can inspect only public registry facts embedded in the built `.incnlib` artifact.

Reexports preserve the source-owned entry rather than creating a duplicate semantic registration. The checked JSON and codegraph projections expose those additional public paths in `reexport_paths`, each with its facade source anchor; the canonical registry and subject identities remain unchanged.

The initial surface deliberately covers functions and concrete methods plus explicit compilation-unit/package entries. Named modules remain a separate future module-system feature rather than a registry-specific declaration form.
