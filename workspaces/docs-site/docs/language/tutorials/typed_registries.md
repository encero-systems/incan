# Build a typed function catalogue

This tutorial builds a small catalogue of text functions with `std.registry`. You will describe ordinary Incan functions once, read the entries loaded by a running program, and inspect the complete checked catalogue without executing that program.

## Create the project

Create a project with this manifest:

```toml title="incan.toml"
[project]
name = "registry-tour"
version = "0.1.0"
```

Create `src/main.incn` and define the domain-owned key and descriptor types:

```incan title="src/main.incn"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    summary: str
    stable: bool

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)
```

The registry does not prescribe fields such as `summary` or `stable`. Those belong to your `FunctionSpec`, so ordinary Incan type checking protects them. `@derive(Descriptor)` tells the compiler that values of this model may be captured as immutable structural registry facts.

## Describe ordinary functions

Add two functions below the registry:

```incan title="src/main.incn"
@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Trim and lowercase text", stable=True))
pub def normalize(value: str) -> str:
    return value.strip().lower()

@describe(functions, FunctionId("shout"), FunctionSpec(summary="Uppercase text", stable=False))
pub def shout(value: str) -> str:
    return value.upper()
```

`@describe` is non-wrapping. Both functions keep their normal callable types and behavior. The compiler separately validates the registry, key, descriptor, allowed subject kind, duplicate key rules, and structural snapshot.

## Read the loaded runtime view

Add a `main` function:

```incan title="src/main.incn"
def main() -> None:
    entries = functions.loaded_entries()
    print(f"loaded {len(entries)} function descriptions")
    print(normalize("  Incan  "))
```

Run the program:

```console
incan run src/main.incn
```

The process prints that two descriptions are loaded and then prints `incan`. The important word is *loaded*: `loaded_entries()` reports entries contributed by modules initialized in this process. It is useful for runtime dispatch and local discovery, but it does not claim to know every entry in an unloaded package.

## Inspect the complete checked view

Ask the compiler for the complete source catalogue:

```console
incan inspect registry main::functions --project . --format json
```

This command type-checks the project through one compilation session and emits deterministic JSON without running `main` or module initialization. The output identifies the package, the registry definition, both entries, their structural key and descriptor values, subject identities, source anchors, visibility, and checked provenance.

The registry identity is `main::functions`: `main` is the source module and `functions` is the registry binding. If multiple packages in the resolved dependency closure publish that same module-local identity, use a package-qualified selector such as `registry-tour::main::functions`.

## What you built

The same source declarations now support two deliberately different consumers:

- application code uses `loaded_entries()` for the process-local runtime view;
- tools use `incan inspect registry` for the complete compiler-checked view.

Neither view scrapes comments or invents a second registry definition. Both originate from the typed `Registry[FunctionId, FunctionSpec]` declaration and its `@describe` sites.

Next, use the [typed-registry how-to guide](../how-to/typed_registries.md) for methods, compilation-unit and package entries, reexports, migration, and troubleshooting. See the [`std.registry` reference](../reference/stdlib/registry.md) for the complete public API and inspection schema.
