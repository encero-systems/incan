# Module static storage

`static` gives a module its own **runtime storage cell**.

Use it when state must:

- live for the lifetime of the program
- be shared by multiple functions in the same module
- remain shared when exported via `pub static`

If that is not what you need, `static` is the wrong tool.

## Mental model

A `static` is not “a mutable const”.

It is closer to:

- a module-owned variable with a stable identity
- initialized once in declaration order when the module is initialized
- read and written through compiler-managed access rules

Every read observes the current contents of that storage cell.
If another function mutates the static, later reads see the updated value.

```incan
static counter: int = 0

def next_id() -> int:
    counter += 1
    return counter
```

`next_id()` does not recompute `counter`. It updates the same module-owned storage each time.

## When to use `static`

Reach for `static` when the module owns long-lived runtime state such as:

- counters
- registries
- caches
- accumulated diagnostics or metrics
- shared mutable collections

```incan
static registered_names: list[str] = []

def register_name(name: str) -> None:
    registered_names.append(name)

def count_names() -> int:
    return len(registered_names)
```

## When not to use `static`

Do not use `static` just because:

- a value is “important”
- a value is used in many places
- a value should not be reassigned

Those are usually `const` or plain module helper values.

Use:

- `const` for compile-time, deeply immutable data
- function parameters / return values for short-lived state flow
- models/classes for explicit state carried by an object

## `const` vs `static`

The distinction is semantic:

- `const` is compile-time and deeply immutable
- `static` is runtime-initialized and live

```incan
const API_VERSION: str = "v1"
static request_count: int = 0
```

`API_VERSION` is fixed baked data.
`request_count` changes as the program runs.

See also: [Const bindings](consts.md)

## Aliases are still live

Direct aliases created from a static still refer to the live stored value.

```incan
static items: list[int] = []

def add_pair() -> None:
    let live_items = items
    live_items.append(1)
    live_items.append(2)
```

After `add_pair()`, `items` contains both values because `live_items` refers to the same live storage-backed value.

That is intentional. `static` exists to model module-owned state, not one-time snapshots.

## Exporting shared state

Use `pub static` when another module must observe or mutate the same storage cell:

--8<-- "snippets/module_state_code.md"

This prints `2`.

`hits` in `main.incn` is not a copy. It refers to the same shared module storage declared in `counters.incn`.

## Boundaries and constraints

`static` is intentionally narrow because it represents module-owned state, not general-purpose hidden globals.

That is why the language keeps it at module scope and gives it explicit declaration rules instead of making it a more casual variation of `let`.

For the exact syntax, initialization, and error rules, see: [Static storage (reference)](../reference/static_storage.md)

## Design intent

Incan prefers explicit state ownership.

`static` exists for the cases where the owner really is the module itself.
That makes module-level caches, counters, and registries possible without pretending they are compile-time constants.

## See also

- [Const bindings](consts.md)
- [Imports and modules](imports_and_modules.md)
- [Static storage (reference)](../reference/static_storage.md)
- [Module state (how-to)](../how-to/module_state.md)
