# RFC 109: Receiver chain combinators (`tap` and `then`)

- **Status:** Draft
- **Created:** 2026-06-06
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 028 (trait-based operator overloading)
    - RFC 035 (first-class named function references)
    - RFC 038 (variadic args and unpacking)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 070 (result combinators for `Result[T, E]`)
    - RFC 088 (iterator adapter surface)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC proposes two general receiver-chain combinators, `tap` and `then`, so ordinary values can be observed, configured, and transformed in left-to-right pipelines without temporary variables while preserving Incan's explicit mutability and typed callable contracts.

## Core model

1. **`then` transforms the receiver:** `value.then(f)` evaluates `value`, passes it to `f`, and returns `f(value)`.
2. **`tap` preserves the receiver:** `value.tap(f)` evaluates `value`, passes it to `f` for observation or explicit mutation, and returns the original value.
3. **Mutation stays visible:** mutating taps use a mutable callback parameter such as `(mut value) => value.normalize()` rather than a separate `tap_mut` method.
4. **Callbacks are typed callables:** named functions, closures, and callable objects use the same function-value model as the rest of Incan.
5. **Observer borrowing preserves ownership:** `tap` must not require `Clone` merely because the observer reads the value and the original value is returned afterward.
6. **Container-specific combinators stay distinct:** `Result.inspect`, `Result.and_then`, and iterator `.inspect()` keep their branch-local or item-local semantics; general `tap` and `then` operate on the receiver itself.

## Motivation

Python code often uses temporary variables to inspect or configure an intermediate value before passing it onward. That style is explicit, but it interrupts the value flow and makes simple construction pipelines longer than the work they perform. Rust has a strong combinator culture for `Option`, `Result`, and iterators, but it does not give every ordinary value a standard `tap` or `then` method. Ruby's `tap` and `then` show the ergonomic value of receiver-chain combinators, but Incan should adopt the typed, explicit version rather than dynamic runtime magic.

Incan already has first-class function references, closures, `Result` combinators, iterator adapters, and receiver mutability in method signatures. A small receiver-chain surface can reuse those ingredients. The goal is not to make every program point-free or to hide side effects. The goal is to let authors keep a clear left-to-right flow when an intermediate value needs a small observer, configuration step, or final transformation.

## Goals

- Add a standard `then` combinator for receiver-to-callback transformation.
- Add a standard `tap` combinator for observer/configuration steps that return the original receiver.
- Preserve explicit mutability through callback parameter spelling instead of adding `tap_mut`.
- Preserve non-`Clone` values through observer borrowing where possible.
- Keep `tap` and `then` visually and semantically distinct from `Result.and_then`, `Result.inspect`, and iterator `.inspect()`.
- Define evaluation order, callback typing, return behavior, and diagnostics clearly enough for compiler, stdlib, LSP, and docs support.

## Non-Goals

- This RFC does not add a new pipe operator; RFC 028 already defines operator hooks for `|>` and `<|`.
- This RFC does not replace `Result.map`, `Result.and_then`, `Result.inspect`, iterator adapters, or `?`.
- This RFC does not add statement-bodied closures.
- This RFC does not allow implicit mutation inside `tap`; mutation must remain visible through existing mutability rules.
- This RFC does not define fallible observer helpers such as `try_tap`, although the design should leave room for them.
- This RFC does not require users to rewrite straightforward local-variable code into chains.

## Guide-level explanation

Use `then` when the receiver should become the input to the next operation:

```incan
user = load_user(id)?
    .then(normalize_user)
    .then(save_user)?
```

Use `tap` when a value should be observed or configured while the chain keeps the same value:

```incan
def activate(mut user: User) -> None:
    user.active = true

def log_user(user: User) -> None:
    log.info(user.summary)

saved = User(name="Danny")
    .tap(activate)
    .tap(log_user)
    .then(save_user)?
```

Short closures are useful when the observer is simple:

```incan
config = Config.default()
    .tap((mut c) => c.enable_cache())
    .tap((c) => log.info(c.summary))
    .then(validate_config)?
```

The mutability is visible at the callback parameter. There is no separate `tap_mut` spelling because Incan already has a source-level way to say that a value is being mutated.

`tap` operates on the receiver itself. This is different from `Result.inspect`, which observes only the `Ok` payload while preserving the original `Result`:

```incan
result.inspect(log_success)   # observes Ok(T)
result.tap(log_result)        # observes Result[T, E] itself
```

Likewise, iterator `.inspect()` observes each item in a lazy sequence, while `tap` observes the iterator value itself.

## Reference-level explanation

### Standard surface

The standard surface must provide:

```incan
def then[T, U](self: T, f: (T) -> U) -> U
def tap[T](self: T, f: (T) -> None) -> T
```

The exact declaration form may use traits, compiler-recognized stdlib methods, or another existing method-extension mechanism, but the user-facing behavior must match these signatures. The final prelude/import policy is unresolved.

### `then` semantics

`value.then(f)` must evaluate `value` exactly once, evaluate `f` exactly once, call `f(value)`, and return the callback result. The receiver is passed as the callback argument according to ordinary Incan ownership and lowering rules for function calls.

If `f` returns `Result[U, E]`, then `value.then(f)` returns `Result[U, E]` and callers may use `?` in the ordinary way:

```incan
validated = config.then(validate_config)?
```

`then` does not inspect `Result` or `Option` branches. Calling `result.then(f)` passes the entire `Result` to `f`. Branch-local fallible chaining remains `result.and_then(f)`.

### `tap` semantics

`value.tap(f)` must evaluate `value` exactly once, evaluate `f` exactly once, call `f` with access to the value, and return the original value after the callback completes.

The callback must return `None`. If the callback returns another value, the compiler must reject the call or require an explicit discard form if a future RFC introduces one. This avoids silently dropping meaningful callback results.

For non-mutating observers, `tap` should pass the value by observer borrow when the original value must remain available afterward. A conforming implementation must not require `T with Clone` merely because `tap` observes `T` and returns it.

For mutating observers, the callback parameter must make mutation visible. The source-level spelling should be the ordinary callable mutability spelling:

```incan
value.tap((mut x) => x.normalize())
```

The implementation must preserve the original value identity and return the mutated value after the callback, subject to the same ownership and mutability rules as an explicit local binding followed by a mutating function call.

### Error behavior

`tap` is not fallible in this RFC. A callback returning `Result[None, E]` does not match `tap` unless a future callable-conversion rule explicitly permits it. Authors who need a fallible observer should use `then` with an explicit function that returns the original value on success:

```incan
def log_and_keep(user: User) -> Result[User, LogError]:
    write_audit_log(user)?
    return Ok(user)

user = user.then(log_and_keep)?
```

### Evaluation order

Receiver evaluation must happen before callback evaluation. The callback must run before `tap` returns the receiver or before `then` returns the transformed value. Early returns, `?` propagation, and panics inside the callback follow ordinary function-call behavior.

### Diagnostics

The compiler should diagnose:

- unknown `tap` or `then` when the standard surface is not active;
- callback arity mismatch;
- callback return type mismatch for `tap`;
- attempts to mutate through a non-mut callback parameter;
- attempted branch-local use where `Result.and_then`, `Result.inspect`, or iterator `.inspect()` is likely intended;
- ambiguous resolution if a type defines its own incompatible `tap` or `then` member.

## Design details

### Syntax

This RFC uses ordinary method-call syntax:

```incan
value.tap(observer)
value.then(transform)
```

No new parser token is required unless the final method-extension mechanism requires one. The feature is intended to feel like ordinary Incan method chaining.

### Semantics

`then` is a receiver-threading transform. `tap` is a receiver-preserving observer/configuration step. The two names should remain small and orthogonal. Libraries should not overload `tap` to transform values or `then` to ignore callback results.

### Interaction with existing `Result` combinators

`Result.map`, `Result.map_err`, `Result.and_then`, `Result.or_else`, `Result.inspect`, and `Result.inspect_err` remain the branch-aware API for `Result`. `tap` and `then` operate on the whole receiver, so they compose with `Result` but do not replace its branch-aware methods.

### Interaction with iterator adapters

Iterator `.inspect()` remains an item observer in a lazy pipeline. `iterator.tap(f)` observes the iterator object itself before it is consumed or transformed.

### Interaction with pipe operators

RFC 028 defines `|>` and `<|` operator hooks for libraries that want pipeline-like operators. `then` is the method-chain spelling for ordinary receiver threading. The two surfaces may coexist, but this RFC does not require `a.then(f)` and `a |> f` to be aliases.

### Compatibility and migration

This RFC is additive. Existing values and methods are unaffected unless a user-defined type already exposes a member named `tap` or `then`. If a type defines an incompatible member, ordinary member resolution and diagnostics must make the conflict visible rather than silently selecting the standard combinator.

## Alternatives considered

1. **Temporary variables only.** This is explicit and already works, but it makes simple observer/configuration flows noisier than necessary.
2. **Only a pipe operator.** A pipe operator can thread values through functions, but it does not solve receiver-preserving observation as directly as `tap`.
3. **Separate `tap` and `tap_mut`.** This makes mutation obvious but duplicates a distinction Incan already expresses through `mut` parameters.
4. **Use `inspect` as the universal name.** This collides with established branch-local and iterator-item semantics. `tap` is clearer for the receiver-preserving operation.
5. **Allow callback results to be silently ignored.** This follows Ruby more closely but is less safe in a typed language because accidentally returning meaningful data would disappear.

## Drawbacks

- Universal-looking methods increase the standard surface that users must learn.
- `tap` can encourage side-effect-heavy chains if style guidance is weak.
- Borrow-preserving `tap` requires careful lowering so non-`Clone` values remain ergonomic.
- The relationship between method chaining, pipe operators, and container-specific combinators must be documented clearly.

## Layers affected

- **Typechecker / Symbol resolution:** the standard surface must resolve `tap` and `then` for ordinary receiver values while respecting user-defined members and conflicts.
- **Callable typing:** callbacks must typecheck with receiver, mutability, arity, and return-type contracts.
- **IR Lowering:** `tap` must preserve receiver ownership while allowing observer borrowing and explicit mutable callback access.
- **Emission:** generated code must evaluate the receiver and callback once and preserve the specified control-flow behavior.
- **Stdlib / Runtime (`incan_stdlib`):** the standard library should provide the user-facing combinator declarations or traits.
- **Formatter:** chained `tap` and `then` calls should format like ordinary method chains.
- **LSP / Tooling:** completion and hover should explain the difference between `tap`, `then`, `Result.inspect`, `Result.and_then`, and iterator `.inspect()`.

## Unresolved questions

- Should `tap` and `then` be prelude methods on all values, explicitly imported stdlib extension methods, or compiler-recognized methods backed by the stdlib?
- What exact callable type spelling should represent mutable callback access in the public reference docs?
- Should this RFC include `try_tap` for fallible observers, or should fallible observation remain an explicit `then` pattern?
- Should `a |> f` be documented as equivalent to `a.then(f)` for simple function application, or should pipe operators remain fully library-defined?
- Should `tap` allow observer callbacks that return `None` only, or should there be an explicit discard marker for intentionally ignored results?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
