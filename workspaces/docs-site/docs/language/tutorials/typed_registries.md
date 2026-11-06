# Build a typed function catalog

This tutorial builds a small catalog of text functions with `std.registry`. You will describe ordinary Incan functions once, read the entries loaded by a running program, and inspect the complete checked catalog without executing that program.

## Create the project

Create a project with this manifest:

```toml title="loaf.toml"
[project]
name = "registry-tour"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
```

Create `src/main.incn` and define the domain-owned key and descriptor types:

```incan title="src/main.incn"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str
    pub stable: bool

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

The program prints `loaded 2 function descriptions` followed by `incan`. `loaded_entries()` denotes entries contributed by modules initialized in a process, rather than every entry in an unloaded package.

## Inspect the complete checked view

Ask the compiler for the complete source catalog:

```console
incan inspect codegraph . --format jsonl > codegraph.jsonl
jq 'select(.record == "registry" and .registry_identity == "main::functions")' codegraph.jsonl
```

The graph exposes checked registry entries without running `main` or module initialization. Read each entry's registry identity, structural key and descriptor, subject identity, source spans, visibility and provenance. An unchecked-unit warning means the export is incomplete.

The registry identity is `main::functions`: `main` is the source module and `functions` is the registry binding. Keep the graph's package and snapshot context with the entry.

## What you built

The same source declarations now support two deliberately different consumers:

- application code uses `loaded_entries()` for the process-local runtime view;
- tools read the checked graph's registry entries.

Neither view scrapes comments or invents a second registry definition. Both originate from the typed `Registry[FunctionId, FunctionSpec]` declaration and its `@describe` sites.

Next, use the [typed-registry how-to guide](../how-to/typed_registries.md) for methods, compilation-unit and package entries, reexports, migration, and troubleshooting. See the [`std.registry` reference](../reference/stdlib/registry.md) for the complete public API and inspection schema.
