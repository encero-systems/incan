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

Every read observes the current contents of that storage cell. If another function mutates the static, later reads see the updated value.

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

`API_VERSION` is fixed baked data. `request_count` changes as the program runs.

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

## Reading a dict entry with `get`

`get(key)` returns an `Option[V]` holding the value stored for a key, and `get(key, default)` returns the stored value, or `default` when no entry holds the key, as a `V`. Where the program keeps that result, the result is its own copy of the stored value.

A one-argument `get(key)` on a dict that is not a static reads the entry in place, without a copy, when all of these hold:

- the lookup is the subject of a `match`, `if let` or `while let`;
- each binding its patterns introduce is not used, or is only passed to `len`, `print` or `println`, interpolated in an f-string, or used to call a Rust method with a shared receiver whose result is not kept (discarded, tested as a condition, compared, negated, or itself passed to `len`, `print` or `println` or interpolated);
- no closure captures a binding;
- the name the dict is reached through is not used from the arm's guard up to the last statement that reads a binding.

Every other result is kept: returned, bound to a name, passed on, compared, or read while the dict is used. A `get(key, default)` result is always kept. A lookup on a static dict, on a dict field of a static, or on a local bound to a static is always kept too, because the value is read out of the static's storage.

A value type that cannot be copied, such as a `Generator` or a Rust type without `Clone`, has no copy, so a kept lookup of one is refused with `INCAN-T0118`, while a lookup read in place is accepted:

```incan
def numbers() -> Generator[int]:
    yield 1

def show(streams: dict[str, list[Generator[int]]], key: str) -> None:
    match streams.get(key):
        Some(found) => println(len(found))
        None => println("none")

def pick(streams: dict[str, Generator[int]], key: str) -> Option[Generator[int]]:
    return streams.get(key)
```

`show` reads the entry in place. `pick` returns the lookup, which needs its own copy of a `Generator`, and is refused with `INCAN-T0118`.

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

`static` exists for the cases where the owner really is the module itself. That makes module-level caches, counters, and registries possible without pretending they are compile-time constants.

## See also

- [Const bindings](consts.md)
- [Imports and modules](imports_and_modules.md)
- [Static storage (reference)](../reference/static_storage.md)
- [Module state (how-to)](../how-to/module_state.md)
