# Static storage (reference)

This page is the reference for module `static` declarations and `pub static` imports.

For motivation and mental model, see: [Module static storage](../explanation/static_storage.md).  
For a practical walkthrough, see: [Module state (how-to)](../how-to/module_state.md).

## Syntax

```incan
static name: Type = expr
pub static name: Type = expr
```

## Summary

- `static` declares module-owned runtime storage
- `pub static` exports that storage to other modules
- every `static` requires a type annotation
- every `static` requires an initializer
- `static` declarations are allowed only at module scope

## Semantics

### Storage identity

Each `static` creates one compiler-recognized storage cell for that module declaration.

- reads observe the current contents of the cell
- assignments update that same cell
- imported `pub static` names refer to the same cell, not a copied value

### Initialization

Static initialization is runtime-oriented, not const-eval-oriented.

- initialization happens once
- earlier declarations may be referenced when valid in declaration order
- later static references are rejected
- dependency cycles are rejected

The compiler preserves declaration-order initialization semantics.

### Visibility and imports

Private statics are visible only inside their declaring module.

Public statics may be imported:

--8<-- "snippets/module_state_code.md"

The imported name refers to the exported storage cell.

## Allowed operations

### Read a static

```incan
static counter: int = 0

def current() -> int:
    return counter
```

### Assign to a static

```incan
static counter: int = 0

def reset() -> None:
    counter = 0
```

### Compound-assign a static

```incan
static counter: int = 0

def bump() -> None:
    counter += 1
```

### Mutate through a method / field / index path

```incan
static items: list[int] = []
static counts: dict[str, int] = {}

def record(name: str) -> None:
    items.append(len(items))
    counts[name] = counts.get(name, 0) + 1
```

### Bind a direct alias

```incan
static items: list[int] = []

def add_default() -> None:
    let live_items = items
    live_items.append(1)
```

Direct aliases from statics preserve live behavior for ordinary mutation paths.

## Disallowed forms

### Missing type annotation

```incan
static counter = 0
```

*Rejected*: `static` requires an explicit type annotation.

### Missing initializer

```incan
static counter: int
```

*Rejected*: `static` requires an initializer.

### Non-module placement

```incan
def bad() -> None:
    static counter: int = 0
```

*Rejected*: `static` is module-scope only.

### Rebinding an imported static

```incan
from counters import hits

def bad() -> None:
    hits = 0
```

*Rejected*: the imported name is not a new local storage cell. It still refers to the exporting module’s static storage.

Mutation of the live value may still be valid when the value’s API allows it. Rebinding the imported name is rejected.

### Using `const` for runtime state

```incan
const counter: int = 0

def bad() -> None:
    counter += 1
```

*Rejected*: `const` is compile-time data.

## Initialization rules

The initializer:

- runs under static runtime-init rules, not const-eval rules
- must be valid for the declared type
- may reference earlier declarations that are valid in init order
- must not participate in a static dependency cycle

## Import behavior

`pub static` participates in module exports alongside other public declarations.

```incan
from counters import hits
import counters::hits
```

Both import styles refer to the same exported storage when the module exports `pub static hits`.

## Errors

Typical compile-time errors include:

- `static` outside module scope
- missing type annotation
- missing initializer
- initializer references a later static
- static dependency cycle
- assignment to an imported static name
- attempted `const` reassignment where `static` was probably intended

## Related pages

- [Module static storage](../explanation/static_storage.md)
- [Const bindings](../explanation/consts.md)
- [Imports and modules](imports_and_modules.md)
- [Module state (how-to)](../how-to/module_state.md)
