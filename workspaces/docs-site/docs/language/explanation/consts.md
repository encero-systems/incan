# Const bindings

`const` declares a **compile-time constant**: a name whose value is validated during compilation and can be baked into the output program.

## Why is `const` a thing?

In Incan, `const` exists to make intent explicit and to unlock compiler guarantees:

- **Intent**: communicates “this never changes” to readers and tools.
- **Safety**: prevents accidental mutation/reassignment and enforces **deep immutability** for baked data.
- **Performance**: allows the compiler to bake data into the output (including `'static` backing data when needed).
- **Reproducibility**: keeps “configuration tables” and other fixed data out of runtime control flow.

!!! info "Coming from Python?"
    In Python, “constants” are mostly a convention (e.g. `MAX_RETRIES = 5`) and can still be reassigned at runtime. In Incan, `const` is a language feature with compile-time validation and immutability guarantees.

## How do I use it?

- **Syntax**: `const NAME [: Type] = <expr>`
- **Scope**: `const` is currently **module-level only**
- **Type annotations**: optional; if omitted, the compiler must be able to infer the type
- **Initializer rules**: the initializer must be **const-evaluable** (no runtime calls)

!!! tip "Rule of thumb"
    If you can’t evaluate the initializer without running the program (IO, time, function calls, loops), it doesn’t belong in a `const`.

## How is `const` different from a regular variable?

- **`const`**:
    - evaluated at compile time
    - cannot depend on runtime values or non-const variables
    - deeply immutable (no mutating APIs for baked strings/bytes/collections)
- **regular variables (`let` / bindings)**:
    - computed at runtime
    - can call functions, use loops, read inputs, etc.
    - may be mutable (`mut`) depending on the binding

!!! info "Coming from TS/JS?"
    In TypeScript/JavaScript, `const` mostly means “this binding can’t be reassigned” (the value can still be mutable), and it can be computed at runtime.

    In Incan, `const` means “compile-time constant”: the initializer must be const-evaluable, and the result is exposed as a read-only (frozen) value.

## When should I use `static` instead?

Use `static` when the module owns **mutable runtime state** that must live across calls:

```incan
static registered_names: list[str] = []

def register_name(name: str) -> None:
    registered_names.append(name)
```

The important difference is semantic, not cosmetic:

- `const` is compile-time, deeply immutable, and never reassignable.
- `static` is runtime-initialized, module-owned storage, and reads observe the current live value.

If you catch yourself trying to reassign a `const`, the value probably was not a `const` to begin with.

`static` has its own rules and mental model. Do not treat it as a “mutable const”.

For the full story, see:

- [Module static storage](static_storage.md)
- [Static storage (reference)](../reference/static_storage.md)
- [Module state (how-to)](../how-to/module_state.md)

## What can a `const` initializer do?

*Allowed*:

- literals: `int`, `float`, `bool`, `str`, `bytes`
- simple unary/binary ops on const literals: `+`, `-`, `*`, `/`, `%`, comparisons, boolean ops, string concatenation
- tuple/list/dict/set literals whose elements/keys/values are themselves const-evaluable
- references to other `const` bindings

*Disallowed*:

- function/method calls (including builtins)
- comprehensions, ranges, f-strings
- accessing non-const variables or runtime state

If the initializer is not const-evaluable, the compiler emits an error at the binding site.

## Why are there “Rust-native” vs “Frozen” consts?

Incan supports two const “families” because Rust’s `const` rules are strict *and* because Incan wants **deep immutability** for baked data:

- **Rust-native consts**:
    - map directly to a Rust `const`
    - best for numbers, booleans, and tuples of those
- **Frozen consts**:
    - for data that should be baked into the program but exposed through a **read-only API**
    - includes strings/bytes and common containers (lists/dicts/sets)
    - represented using frozen stdlib wrappers like `FrozenStr`, `FrozenBytes`, `FrozenList[T]`, `FrozenDict[K, V]`, `FrozenSet[T]`
    - the compiler emits baked `'static` backing data and constructs frozen wrappers from it

Important detail: in `const` context, Incan treats `str`/`bytes` and common containers as **frozen** to preserve deep immutability. In other words, `const` is not “just a `let` that runs earlier”.

How that shows up in annotations differs by family. A `str` or `bytes` annotation is accepted as written: the const carries `FrozenStr`/`FrozenBytes`, and those read anywhere `str`/`bytes` is expected, so the annotation holds at every use site. A mutable container annotation (`list[T]`, `dict[K, V]`, `set[T]`) is **rejected at the declaration**, because the `FrozenList[T]`/`FrozenDict[K, V]`/`FrozenSet[T]` the const actually has is a different type wherever the mutable container is expected — accepting the annotation would only move the mismatch to the first call that passes the const along. Write the frozen type, or omit the annotation and let it be inferred.

!!! info "Coming from Rust?"
    This is similar to Rust’s split between “things that can be `const`” and “things that need `'static` backing data and a safe API”.

## Examples

### Rust-native consts

```incan
const MAX_RETRIES: int = 5
const TIMEOUT_SECS: float = 2.5
const IS_DEBUG: bool = False
const PORT = 8080  # type may be inferred
const GREETING = "hello" + " world"  # string concatenation is allowed
```

### Frozen consts (deeply immutable)

```incan
const GREETING: FrozenStr = "hello"
const NUMS: FrozenList[int] = [1, 2, 3]
const HEADERS: FrozenDict[FrozenStr, FrozenStr] = {"User-Agent": "incan"}
const DATA: FrozenBytes = b"\x00\x01"
```

### Consts can reference other consts

```incan
const BASE: int = 10
const LIMIT: int = BASE * 2
```

## Errors

- If the initializer is not const-evaluable (calls, comprehensions, ranges, f-strings, non-const variables).
- If a const dependency cycle exists (consts reference each other in a loop).
- If types do not match (explicit type annotation incompatible with the initializer).
- If a `const` is annotated with a mutable container type (`list[T]`, `dict[K, V]`, `set[T]`); the diagnostic points at the annotation and names the frozen type to write instead.

## See also

- [Frozen collections (reference)](../reference/frozen_collections.md)
- [Module static storage](static_storage.md)
- [Static storage (reference)](../reference/static_storage.md)
