# Static storage (reference)

This page specifies module `static` declarations: their syntax, storage, initialization, the operations on a static, aliases of a static, imports of a `pub static`, and the refusals. Refusals are reported with `INCAN-T0001`, and syntax errors with `INCAN-P0001`, unless a rule below names another code.

For the mental model, see [Module static storage](../explanation/static_storage.md). For a task walkthrough, see [Module state (how-to)](../how-to/module_state.md).

## Syntax

```incan
static name: Type = expr
pub static name: Type = expr
```

- A `static` is declared at module scope, with a type annotation and an initializer.
- `pub static` exports the static from its module.

## Storage

- Each `static` declaration is one storage cell of its module.
- A read of the static observes the cell's current value.
- An assignment, a compound assignment, a mutating method call, and a field or index assignment through the static change the cell.
- An imported `pub static` names the exporting module's cell.

## Initialization

- The initializer's type is the declared type.
- A static is initialized once, in declaration order.
- An initializer may read a `const`, an earlier static, and call functions.
- An initializer may not read a later static, directly or through a function it calls, and may not assign to a static through a function it calls.
- Statics may not depend on each other in a cycle.

## Operations

- `get(key)` on a static `dict[K, V]`, or on a `dict[K, V]` field of a static, returns `Option[V]`: `Some` holding a copy of the stored value when the key is present, `None` otherwise.
- An argument passed to a method on a static is readable after the call.

```incan
static counter: int = 0
static items: list[int] = []
static counts: dict[str, int] = {}

def current() -> int:
    return counter               # accepted

def reset() -> None:
    counter = 0                  # accepted

def bump() -> None:
    counter += 1                 # accepted

def record(name: str) -> None:
    items.append(len(items))     # accepted
    counts[name] = counts.get(name, 0) + 1   # accepted

def lookup(name: str) -> Option[int]:
    return counts.get(name)      # accepted
```

## Aliases

A new local bound directly to a static, `x = s`, `let x = s` or `mut x = s`, is an alias of the static's cell. The same holds when the binding is annotated with the static's type, spelled directly or through a type alias.

- A mutating method call and a field or index assignment through the alias change the static.
- An assignment to the alias rebinds the local; the static keeps its value.
- A binding annotated with another type that accepts the static's value, such as `Option[int]` for an `int` static, holds a copy of the value.

```incan
static items: list[int] = []

def add_defaults() -> None:
    let live_items = items
    live_items.append(1)         # accepted
    annotated: list[int] = items
    annotated.append(2)          # accepted
```

## Imports

```incan
from counters import hits
import counters::hits
```

- Both forms bind the exported static `hits` of `counters`, and name its cell.
- A static declared without `pub` is not exported; importing it is refused (see [Imports and modules](imports_and_modules.md)).
- A mutating method call and a field or index assignment through an imported static change the exporting module's cell.

## Refusals

| Form | Code |
| --- | --- |
| `static counter = 0`: no type annotation | `INCAN-P0001` |
| `static counter: int`: no initializer | `INCAN-P0001` |
| `static` inside a function or other block | `INCAN-P0001` |
| An initializer that reads a later static | `INCAN-T0001` |
| Statics that depend on each other in a cycle | `INCAN-T0001` |
| An initializer that assigns to a static through a function it calls | `INCAN-T0001` |
| An assignment to an imported static's name, `hits = 0` | `INCAN-T0001` |
| An assignment or compound assignment to a `const` | `INCAN-T0001` |
| `get` on a static dict, or on a dict field of a static, whose value type cannot be copied, such as a `Generator` | `INCAN-T0118` |

```incan
static counter = 0               # refused: a static needs a type annotation

def bad() -> None:
    static local: int = 0        # refused: static is declared at module scope
```

```incan
from counters import hits

def bad() -> None:
    hits = 0                     # refused: an imported static cannot be reassigned
```

## Related pages

- [Module static storage](../explanation/static_storage.md)
- [Const bindings](../explanation/consts.md)
- [Frozen collections](frozen_collections.md)
- [Imports and modules](imports_and_modules.md)
- [Module state (how-to)](../how-to/module_state.md)
