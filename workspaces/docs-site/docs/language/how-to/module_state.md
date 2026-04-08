# Module state (how-to)

This guide shows how to solve a concrete problem with `static`: keep live module-owned state and optionally share it across files.

If you need the exact rules, see: [Static storage (reference)](../reference/static_storage.md).

## Task: keep a module-local counter

```incan
static counter: int = 0

pub def next_counter() -> int:
    counter += 1
    return counter
```

Use this when the state belongs to the module itself and callers should access it through functions.

## Task: export shared state across files

1. Declare the shared storage with `pub static`.
2. Update it through normal functions in the exporting module.
3. Import the live value from another file.

--8<-- "snippets/module_state.md"

Because `hits` is live module storage, the import sees the updated value after the function call.

## Task: share a mutable collection

1. Export the collection with `pub static`.
2. Mutate it through helper functions.
3. Import it only if callers really need the live collection itself.

```incan
pub static names: list[str] = []

pub def register(name: str) -> None:
    names.append(name)
```

Elsewhere:

```incan
from registry import names, register

def main() -> None:
    register("Ada")
    register("Linus")
    println(len(names))
```

Use this only when the shared collection itself is part of the module’s public surface.

## Task: keep mutation behind functions

Prefer this shape when callers only need behavior, not direct access to the storage:

```incan
static names: list[str] = []

pub def register(name: str) -> None:
    names.append(name)

pub def registered_count() -> int:
    return len(names)
```

That keeps mutation rules in one place and prevents other modules from depending on too much internal detail.

Expose `pub static` only when sharing the actual live state is part of the API.

## Avoid these mistakes

### Mistake: using `const` for runtime state

```incan
const request_count: int = 0
```

This is wrong if the value changes while the program runs: use `static`.

### Mistake: omitting the type annotation

```incan
static request_count = 0
```

This is rejected. `static` always needs an explicit type.

### Mistake: rebinding an imported static

```incan
from stats import request_count

def reset() -> None:
    request_count = 0
```

This is rejected because the imported name still refers to the exporter’s storage.

## Pick the right tool

- Use `const` for fixed compile-time data.
- Use `static` for module-owned runtime state.
- Use local variables and return values for ordinary flow through your program.

## Related pages

- [Module static storage](../explanation/static_storage.md)
- [Static storage (reference)](../reference/static_storage.md)
- [Const bindings](../explanation/consts.md)
