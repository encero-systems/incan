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

This RFC proposes two standard receiver-chain combinators, `tap` and `then`, so ordinary values can be observed, configured, and transformed in left-to-right chains without losing Incan's explicit mutability, callable typing, and compile-time ownership guarantees.

The north star is the ergonomic shape Ruby popularized with `tap` and `then`, but specified as a typed Incan surface: receiver evaluation is explicit and single-pass, callbacks are statically typed, read-only observation must not force `Clone`, and mutation is visible at the callback parameter.

## Core model

1. **`then` transforms the receiver:** `value.then(f)` evaluates `value`, passes the receiver value to `f`, and returns `f(value)`.
2. **`tap` preserves the receiver:** `value.tap(f)` evaluates `value`, lets `f` observe or mutate that receiver, requires `f` to return `None`, and returns the same receiver value after the callback.
3. **Read-only taps borrow:** `tap` may adapt a callback to a borrowed receiver boundary, using the same ownership planning already used by `Result.inspect`, so observing a non-`Clone` value does not clone it.
4. **Mutating taps are explicit:** `value.tap((mut x) => x.normalize())` gives the callback mutable access to the receiver and returns the mutated receiver. There is no separate `tap_mut` spelling.
5. **Callbacks are ordinary callables where possible:** named functions, closures, callable objects, and future callable presets should use the same function-value model as the rest of Incan, subject to the borrow and mutability contracts in this RFC.
6. **Container-specific combinators stay distinct:** `Result.inspect`, `Result.and_then`, and iterator `.inspect()` keep their branch-local or item-local semantics; general `tap` and `then` operate on the receiver itself.

## Motivation

Python code often uses temporary variables to inspect or configure an intermediate value before passing it onward. That style is explicit, but it interrupts the value flow and makes simple construction pipelines longer than the work they perform. Rust has a strong combinator culture for `Option`, `Result`, and iterators, and Incan already adopted that direction for `Result` in RFC 070, but Rust does not give every ordinary value a standard `tap` or `then` method. Ruby's `tap` and `then` are strong prior art for the receiver-chain ergonomics; Incan should take the readability lesson without adopting Ruby's dynamic dispatch model.

Incan already has first-class function references, closures, borrowed callable lowering for observer-style combinators, `Result` combinators, iterator adapters, and explicit receiver mutability in method signatures. A small receiver-chain surface can reuse those ingredients. The goal is not to make every program point-free or to hide side effects. The goal is to let authors keep a clear left-to-right flow when an intermediate value needs a small observer, configuration step, or final transformation.

## Goals

- Add a standard `then` combinator for receiver-to-callback transformation.
- Add a standard `tap` combinator for observer/configuration steps that return the original receiver.
- Preserve explicit mutability through callback parameter spelling instead of adding `tap_mut`.
- Preserve non-`Clone` values through observer borrowing where possible.
- State the calling syntax and temporary-variable equivalence clearly enough that `tap` does not become runtime magic.
- Keep `tap` and `then` visually and semantically distinct from `Result.and_then`, `Result.inspect`, and iterator `.inspect()`.
- Define evaluation order, callback typing, return behavior, and diagnostics clearly enough for compiler, stdlib, LSP, and docs support.

## Non-Goals

- This RFC does not add a new pipe operator; RFC 028 already defines operator hooks for `|>` and `<|`.
- This RFC does not replace `Result.map`, `Result.and_then`, `Result.inspect`, iterator adapters, or `?`.
- This RFC does not add statement-bodied closures.
- This RFC does not allow implicit mutation inside `tap`; mutation must remain visible through existing mutability rules.
- This RFC does not require fallible observer helpers such as `try_tap`, although the design should leave room for them.
- This RFC does not require users to rewrite straightforward local-variable code into chains.

## Guide-level explanation

Use `then` when the receiver should become the input to the next operation:

```incan
user = load_user(id)?
    .then(normalize_user)
    .then(save_user)?
```

That chain is equivalent in intent to:

```incan
loaded = load_user(id)?
normalized = normalize_user(loaded)
user = save_user(normalized)?
```

Use `tap` when a value should be observed or configured while the chain keeps the same value:

```incan
saved = User(name="Danny")
    .tap((mut user) => user.activate())
    .tap((user) => log.info(user.summary))
    .then(save_user)?
```

That chain is equivalent in intent to binding the intermediate value, mutating or observing it, and then passing it onward:

```incan
mut user = User(name="Danny")
user.activate()
log.info(user.summary)
saved = save_user(user)?
```

The mutability is visible at the callback parameter. There is no separate `tap_mut` spelling because Incan already has a source-level way to say that a value is being mutated.

Short closures are useful when the observer is local to the chain:

```incan
config = Config.default()
    .tap((mut c) => c.enable_cache())
    .tap((c) => log.info(c.summary))
    .then(validate_config)?
```

Named functions should also work when their callable shape fits the tap contract:

```incan
def log_config(config: Config) -> None:
    log.info(config.summary)

config = Config.default()
    .tap(log_config)
    .then(validate_config)?
```

For read-only taps, the implementation may borrow the receiver while calling the observer so the original value can keep moving through the chain without `Clone`. This matches the existing ownership policy used by observer-style `Result` combinators.

`tap` operates on the receiver itself. This is different from `Result.inspect`, which observes only the `Ok` payload while preserving the original `Result`:

```incan
result.inspect(log_success)   # observes Ok(T)
result.tap(log_result)        # observes Result[T, E] itself
```

Likewise, iterator `.inspect()` observes each item in a lazy sequence, while `tap` observes the iterator value itself.

## Reference-level explanation

### Standard surface

The user-facing surface is ordinary method-call syntax:

```incan
value.then(transform)
value.tap(observer)
value.tap((mut value) => value.normalize())
```

The exact declaration form may use traits, compiler-recognized stdlib methods, a future method-extension mechanism, or a desugared form backed by stdlib callables. The RFC does not require hard-coding magic names into the parser. Whatever implementation path is chosen, the source behavior must match the semantic contracts below.

Semantic shape:

```text
then[T, U](self: T, transform: consumes T -> U) -> U
tap[T](self: T, observer: observes T -> None) -> T
tap[T](mut self: T, observer: mutates T -> None) -> T
```

The `text` block is intentionally not final Incan declaration syntax. Today's public function type spelling, `(T) -> U`, is enough for simple consuming transforms and for read-only observers after borrowed callable adaptation, but it does not by itself describe mutable receiver access. The RFC therefore treats the exact public callable spelling for mutating tap callbacks as a design decision rather than pretending the simple `(T) -> None` signature carries every case.

### `then` semantics

`value.then(f)` must evaluate `value` exactly once, evaluate `f` exactly once, call `f(value)`, and return the callback result. The receiver is passed as the callback argument according to ordinary Incan ownership and lowering rules for function calls.

If `f` returns `Result[U, E]`, then `value.then(f)` returns `Result[U, E]` and callers may use `?` in the ordinary way:

```incan
validated = config.then(validate_config)?
```

`then` does not inspect `Result` or `Option` branches. Calling `result.then(f)` passes the entire `Result` to `f`. Branch-local fallible chaining remains `result.and_then(f)`.

### `tap` semantics

`value.tap(f)` must evaluate `value` exactly once, evaluate `f` exactly once, call `f` with access to the receiver, and return the receiver after the callback completes.

The callback must return `None`. If the callback returns another value, the compiler must reject the call or require an explicit discard form if a future RFC introduces one. This avoids silently dropping meaningful callback results.

For non-mutating observers, `tap` should pass the value by observer borrow when the original value must remain available afterward. A conforming implementation must not require `T with Clone` merely because `tap` observes `T` and returns it. This is the same lowering policy described for observer-style helper functions in the duckborrowing docs and already used by `Result.inspect` / `Result.inspect_err`.

For mutating observers, the callback parameter must make mutation visible:

```incan
value.tap((mut x) => x.normalize())
```

This must lower like an explicit mutable binding followed by a mutating method call on that binding, then returning the binding:

```text
mut x = value
x.normalize()
return x
```

A mutating tap must not be specified as "pass `T` by value into an arbitrary callback and return the original anyway"; that would be incoherent for owned non-`Copy` values. The callback needs mutable access to the receiver that `tap` will return. Inline closures are the clearest source form for this. Named callables should be accepted only when their callable shape can receive the same mutable receiver access without moving the value away from `tap`.

### Error behavior

`tap` is not fallible by default. A callback returning `Result[None, E]` does not match `tap` unless a future callable-conversion rule explicitly permits it. Authors who need a fallible observer can use `then` with an explicit function that returns the original value on success:

```incan
def log_and_keep(user: User) -> Result[User, LogError]:
    write_audit_log(user)?
    return Ok(user)

user = user.then(log_and_keep)?
```

A future `try_tap` helper may be useful, but it should be specified as a separate receiver-preserving fallible combinator rather than smuggling fallibility into ordinary `tap`.

### Evaluation order

Receiver evaluation must happen before callback evaluation. The callback must run before `tap` returns the receiver or before `then` returns the transformed value. Early returns, `?` propagation, and panics inside the callback follow ordinary function-call behavior.

### Diagnostics

The compiler should diagnose:

- unknown `tap` or `then` when the standard surface is not active;
- callback arity mismatch;
- callback return type mismatch for `tap`;
- attempts to mutate through a non-mut callback parameter;
- mutating tap callbacks whose callable shape would move the receiver away from `tap`;
- attempted branch-local use where `Result.and_then`, `Result.inspect`, or iterator `.inspect()` is likely intended;
- ambiguous resolution if a type defines its own incompatible `tap` or `then` member.

## Design details

### Syntax

This RFC uses ordinary method-call syntax:

```incan
value.tap(observer)
value.then(transform)
```

No new parser token is required unless the final method-extension mechanism requires one. The feature is intended to feel like ordinary Incan method chaining, not a special form.

### Calling syntax

The intended calling syntax is:

```incan
value.then(named_transform)
value.then((x) => transform(x))
value.tap(named_observer)
value.tap((x) => log.info(x.summary))
value.tap((mut x) => x.normalize())
```

The named-function forms are ordinary function values. The closure forms are often clearer when the observer is local, when mutation is involved, or when the compiler must infer a borrowed callback boundary.

### Semantics

`then` is a receiver-threading transform. `tap` is a receiver-preserving observer/configuration step. The two names should remain small and orthogonal. Libraries should not overload `tap` to transform values or `then` to ignore callback results.

### Activation and method resolution

The final prelude/import policy is unresolved. The strongest language-level shape is that `tap` and `then` are standard receiver combinators available on all ordinary values once the relevant stdlib/prelude surface is active.

If a type defines its own concrete member named `tap` or `then`, ordinary member resolution should prefer the concrete member because it is part of the type's authored API. The standard combinator should be available through a function fallback, such as `std.chain.tap(value, observer)` or `std.chain.then(value, transform)`, if the method name is shadowed or ambiguous. The exact fallback path is a design decision, but the RFC should not require silent override of user-authored members.

### Relationship to future extension mechanisms

RFC 108 explores import-scoped extension properties, and Incan may later grow a broader extension-method or `vocab` authoring story. RFC 109 should not block that direction. `tap` and `then` may eventually be authored through the same mechanism that lets libraries add statically typed receiver vocabulary, but this RFC is about the standard combinator semantics and calling surface, not about making arbitrary extension methods available for every library.

### Interaction with existing `Result` combinators

`Result.map`, `Result.map_err`, `Result.and_then`, `Result.or_else`, `Result.inspect`, and `Result.inspect_err` remain the branch-aware API for `Result`. `tap` and `then` operate on the whole receiver, so they compose with `Result` but do not replace its branch-aware methods.

### Interaction with iterator adapters

Iterator `.inspect()` remains an item observer in a lazy pipeline. `iterator.tap(f)` observes the iterator object itself before it is consumed or transformed.

### Interaction with pipe operators

RFC 028 defines `|>` and `<|` operator hooks for libraries that want pipeline-like operators. `then` is the method-chain spelling for ordinary receiver threading. The two surfaces may coexist, but this RFC does not require `a.then(f)` and `a |> f` to be aliases.

### Prior art

Ruby provides the most direct prior art for the names and fluent reading style: `tap` observes/configures while returning the receiver, and `then` threads the receiver into a block and returns the block result. Rust provides the stronger safety precedent through owned method receivers, explicit mutable receivers, `Result`/`Option`/iterator combinators, and observer helpers such as `inspect` that preserve the original container. Incan should deliberately combine Ruby's readability with Rust-shaped static guarantees.

### Compatibility and migration

This RFC is additive. Existing values and methods are unaffected unless a user-defined type already exposes a member named `tap` or `then`. If a type defines an incompatible member, ordinary member resolution and diagnostics must make the conflict visible rather than silently selecting the standard combinator.

## Alternatives considered

1. **Temporary variables only.** This is explicit and already works, but it makes simple observer/configuration flows noisier than necessary.
2. **Only a pipe operator.** A pipe operator can thread values through functions, but it does not solve receiver-preserving observation as directly as `tap`.
3. **Separate `tap` and `tap_mut`.** This makes mutation obvious but duplicates a distinction Incan already expresses through `mut` parameters.
4. **Use `inspect` as the universal name.** This collides with established branch-local and iterator-item semantics. `tap` is clearer for the receiver-preserving operation.
5. **Allow callback results to be silently ignored.** This follows Ruby more closely but is less safe in a typed language because accidentally returning meaningful data would disappear.
6. **Model `tap` as an ordinary consuming function only.** This would be simpler to type, but it would either require cloning for read-only observation or make mutating taps incoherent for owned non-`Copy` values.

## Drawbacks

- Universal-looking methods increase the standard surface that users must learn.
- `tap` can encourage side-effect-heavy chains if style guidance is weak.
- Borrow-preserving and mutating `tap` require careful lowering so non-`Clone` values remain ergonomic without hiding moves.
- The relationship between method chaining, pipe operators, and container-specific combinators must be documented clearly.
- The names `tap` and `then` are short enough that shadowing by user-authored methods is plausible.

## Layers affected

- **Typechecker / Symbol resolution:** the standard surface must resolve `tap` and `then` for ordinary receiver values while respecting user-defined members and conflicts.
- **Callable typing:** callbacks must typecheck with receiver, mutability, arity, and return-type contracts.
- **Ownership planning / duckborrowing:** read-only taps must preserve receiver ownership through borrowed callable adaptation instead of clone requirements.
- **IR Lowering:** `tap` must preserve receiver ownership while allowing observer borrowing and explicit mutable callback access.
- **Emission:** generated code must evaluate the receiver and callback once and preserve the specified control-flow behavior.
- **Stdlib / Runtime (`incan_stdlib`):** the standard library should provide the user-facing combinator declarations, traits, or fallback functions chosen by the final design.
- **Formatter:** chained `tap` and `then` calls should format like ordinary method chains.
- **LSP / Tooling:** completion and hover should explain the difference between `tap`, `then`, `Result.inspect`, `Result.and_then`, and iterator `.inspect()`.

## Unresolved questions

- Should `tap` and `then` be prelude methods on all values, explicitly imported stdlib extension methods, compiler-recognized methods backed by the stdlib, or authored through a future extension-method mechanism?
- What exact callable type spelling should represent mutable callback access in the public reference docs?
- Should named mutable callables be accepted for mutating `tap`, or should mutating tap initially require inline closures until callable borrow spelling is explicit?
- Should this RFC include `try_tap` for fallible observers, or should fallible observation remain an explicit `then` pattern?
- Should `a |> f` be documented as equivalent to `a.then(f)` for simple function application, or should pipe operators remain fully library-defined?
- Should `tap` allow observer callbacks that return `None` only, or should there be an explicit discard marker for intentionally ignored results?
- What fallback path should expose the standard combinator when a type already has an authored `tap` or `then` member?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
