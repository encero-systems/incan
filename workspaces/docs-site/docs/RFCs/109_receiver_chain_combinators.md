# RFC 109: Receiver chain combinators (`tap` and `then`)

- **Status:** Planned
- **Created:** 2026-06-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 028 (trait-based operator overloading)
    - RFC 035 (first-class named function references)
    - RFC 038 (variadic args and unpacking)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 070 (result combinators for `Result[T, E]`)
    - RFC 088 (iterator adapter surface)
    - RFC 108 (import-scoped extensions)
    - RFC 110 (`Ordering.then`)
    - RFC 128 (callable types)
    - RFC 129 (binding and `mut` parameters)
    - RFC 154 (retained capturing callables)
- **Issue:** [#2661](https://github.com/encero-systems/incan/issues/2661)
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

`value.then(f)` passes a value to a function and returns the result, and `value.tap(f)` lets a function look at or change a value and then hands the same value on. Both are available on every value with no import, so a value can flow left to right through small steps without temporary variables. They are ordinary library code: an extension in `std.chain` that the standard prelude activates, with no compiler knowledge of the names. A member a type authors itself always takes precedence.

## Core model

1. **`then` transforms.** `value.then(f)` evaluates `value`, passes it to `f` and returns what `f` returns.
2. **`tap` preserves.** `value.tap(f)` evaluates `value`, gives `f` access to it, and returns the same value. `f` must return `None`.
3. **Looking does not clone.** A read-only `tap` borrows, so observing a value that is not `Clone` works.
4. **Changing is visible.** `value.tap((mut x) => x.normalize())` says so at the callback's parameter, and there is no `tap_mut`.
5. **Always available, not special.** `std.chain` is library code, and the prelude activates it, as the prelude makes `Eq` and `Clone` available.
6. **Authored members win.** A type's own `tap` or `then` is never overridden.
7. **Container combinators stay distinct.** `Result.inspect`, `Result.and_then` and iterator `.inspect()` work on a branch or an item, while `tap` and `then` work on the whole value.

## Motivation

Code often needs one small step on an intermediate value, such as configuring it, logging it or passing it on, and a temporary variable interrupts the flow for that step:

```incan
mut user = User(name="Danny")
user.activate()
log.info(user.summary)
saved = save_user(user)?
```

Ruby's `tap` and `then` show the readability gain. Rust shows how to keep it typed, with explicit mutability and observers such as `inspect` that preserve their receiver. Incan already has the parts: closures, callable types and `mut` in function types.

## Goals

- Provide `then` and `tap` on every value with no import.
- Keep mutation visible at the callback's parameter.
- Let a read-only `tap` work on values that are not `Clone`.
- Keep them as library code, with no compiler knowledge of the names.
- Keep them distinct from branch-local and item-local combinators.

## Non-Goals

- A new pipe operator. RFC 028 defines the hooks for `|>` and `<|`.
- Replacing `Result.map`, `Result.and_then`, `Result.inspect`, iterator adapters or `?`.
- Statement-bodied closures.
- Fallible observers. A fallible step uses `then`.
- Asking anyone to rewrite straightforward local-variable code into chains.

## Guide-level explanation

Use `then` when a value should become the input to the next step:

```incan
user = load_user(id)?
    .then(normalize_user)
    .then(save_user)?
```

Use `tap` when a step should keep the value:

```incan
saved = User(name="Danny")
    .tap((mut user) => user.activate())
    .tap((user) => log.info(user.summary))
    .then(save_user)?
```

`mut` on the parameter is how you say the step changes the value. Named functions work too, read-only or mutating:

```incan
def log_config(config: Config) -> None:
    log.info(config.summary)

def enable_cache(mut config: Config) -> None:
    config.cache = true

config = Config.default()
    .tap(enable_cache)
    .tap(log_config)
    .then(validate_config)?
```

A callback may capture what it needs, under RFC 154, because it finishes within the call.

`tap` works on the whole value, unlike the combinators that look inside one:

```incan
result.inspect(log_success)   # observes the Ok payload
result.tap(log_result)        # observes the Result itself
```

A step that can fail is a `then` that returns the value:

```incan
def log_and_keep(user: User) -> Result[User, LogError]:
    write_audit_log(user)?
    return Ok(user)

user = user.then(log_and_keep)?
```

A type's own member takes precedence, so `cmp(a, b).then(cmp(c, d))` calls RFC 110's `Ordering.then`. When a member shadows the combinator, the plain functions still work: `std.chain.then(value, transform)`.

## Reference-level explanation

### Availability

1. `std.chain` must provide `then` and `tap` as an RFC 108 extension, and as plain functions `then(value, transform)` and `tap(value, observer)`.
2. The standard prelude must activate the extension in every module. It must not need an import, and the compiler must have no knowledge of the names beyond extensions and the prelude.
3. A member a type defines, including through an adopted trait, must take precedence over the extension, under RFC 108. A shadowed call must not be reported as a conflict.

### `then`

4. `value.then(f)` must evaluate `value` once, then `f`, call `f` once with `value` under ordinary ownership rules, and return the result.
5. `then` must not look inside a `Result` or an `Option`. `result.then(f)` passes the whole `Result`, and `?` applies to what `f` returns in the ordinary way.

### `tap`

6. `value.tap(f)` must evaluate `value` once, then `f`, call `f` once with access to the value, and return the value.
7. `f` must return `None`. Any other return type must be refused with a diagnostic pointing at a named function or `then`. `Result[None, E]` is not `None`.
8. A read-only `f`, `Callable[(T) -> None]`, must borrow the value, and `tap` must not require `T with Clone`.
9. A mutating `f`, `Callable[(mut T) -> None]`, whether a closure with a `mut` parameter or a named function declaring one, must change the value `tap` returns. Changing the value through a parameter not marked `mut` must be refused.

### Failure and order

10. The receiver must be evaluated before the callback runs, and the callback must finish before the result is returned. A `return`, `?` or panic inside a callback follows ordinary rules for a function call.

### Diagnostics and tooling

11. Diagnostics must cover arity and return-type mismatches, changing a value through a non-`mut` parameter, and `then` or `tap` where `Result.and_then`, `Result.inspect` or iterator `.inspect()` was likely meant.
12. Completion and hover must show that these come from `std.chain`, and must explain the difference from those combinators. The formatter must treat them as ordinary method chains.

### Conformance workloads

| Workload | Required evidence |
| --- | --- |
| Availability | `then` and `tap` work on models, classes, enums, primitives, `Result` and generic values, with no import. |
| Ownership | A read-only `tap` works on a value that is not `Clone`, and a mutating `tap` returns the changed value. |
| Callables | Closures, named functions, bound methods, callable objects and capturing closures all work, read-only and mutating. |
| Precedence | A type's own `then`, including `Ordering.then`, wins, and `std.chain.then` still reaches the combinator. |
| Refusals | A non-`None` observer and a change through a non-`mut` parameter are refused with their diagnostics. |
| Control flow | `?` on a `then` result, and a panic in a callback, behave as in an ordinary call. |
| Single evaluation | The receiver and callback are each evaluated once. |

## Design details

### Why they are always available

An import to use something as small as `tap` is friction at the moment it would help, and these names are meant as a general way of writing code rather than a specialized library. The prelude is how Incan makes such things available. Activating an ordinary extension there gives that with no compiler knowledge of the names.

### A possible implementation

Non-normatively, the extension could read:

```incan
# std/chain.incn
pub extension chain:
    def then[T, U](self: T, transform: Callable[(T) -> U]) -> U:
        return transform(self)

    def tap[T](self: T, observer: Callable[(T) -> None]) -> T:
        observer(self)
        return self

    def tap[T](mut self: T, observer: Callable[(mut T) -> None]) -> T:
        observer(self)
        return self
```

### Relationship to other surfaces

RFC 028's `|>` remains a hook for libraries that want the operator, and is not an alias of `then`. RFC 122's fieldmethods work on a model being constructed, while `tap` and `then` work on finished values.

### Prior art

Ruby provides the names and the reading style: `tap` observes and returns its receiver, and `then` passes it to a block. Rust provides the safety precedent, with explicit mutable receivers and observers such as `inspect`. Kotlin's `also` and `let` are the same pair, available on every value through its standard library.

### Compatibility

This RFC is additive. A type with its own `tap` or `then` keeps it.

## Alternatives considered

- **Temporary variables only:** explicit and already works, but noisy for small steps.
- **An explicit import:** hides a general-purpose feature behind a line that has to be remembered.
- **Compiler-recognized methods:** would need special handling for two library names, when an extension activated by the prelude needs none.
- **Only a pipe operator:** threads values through functions but has no answer for keeping the value.
- **Separate `tap` and `tap_mut`:** duplicates what `mut` on the parameter already says.
- **`inspect` as the universal name:** already means branch-local and item-local observation.
- **Ignoring callback results:** a meaningful return value would silently disappear.
- **A `try_tap`:** `then` with a function returning the value already says it.
- **Treating `a |> f` as `a.then(f)`:** `|>` is defined by its hook, which a library may give another meaning.

## Drawbacks

Every type gains two names in completion lists, and short names like these are plausible for a type to define with another meaning, as `Ordering.then` does. `tap` can encourage long side-effecting chains if style guidance is weak. The extension depends on RFC 108 supporting methods and receivers of any type.

## Layers affected

- **Standard library:** `std.chain` and activation in the prelude.
- **Extensions:** RFC 108's methods, generic receivers and prelude activation.
- **Ownership:** borrowing for read-only observers and access for mutating ones, under RFC 129.
- **Formatter:** ordinary method-chain layout.
- **Language server:** completion and hover.
- **Documentation:** guidance on when a chain helps and when a local variable reads better.

## Design Decisions

- **Availability:** on every value with no import. `std.chain` is library code, activated by the prelude, and the compiler has no knowledge of the names.
- **Shadowing:** a type's own member always wins, and `std.chain.then` and `std.chain.tap` remain available as plain functions.
- **Callable spelling:** `Callable[(T) -> None]` observes and `Callable[(mut T) -> None]` changes, and named functions work for both.
- **Observers return `None`:** no discard marker. Anything else is a named function or `then`.
- **No `try_tap`:** a fallible step is a `then`.
- **`|>` stays separate:** it is a library hook, not an alias.
