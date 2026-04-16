# RFC 039: `race` for awaitable concurrency

- **Status:** Draft
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (Compilable stdlib & Rust module binding)
    - RFC 027 (`incan-vocab`)
    - RFC 028 (Trait-based operator overloading)
    - RFC 029 (Union types and type narrowing)
    - RFC 035 (First-class named function references)
    - RFC 038 (Variadic positional args and keyword-argument capture)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/331
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

Introduce `race` as an import-activated `std.async` vocabulary form for "first-completion wins" concurrency, together with an Incan-native `Awaitable[T]` protocol (trait) that formalizes what `await` means in generic code.

The architecture is deliberately layered:

1. `await` remains a core language feature in semantic terms. It is not replaced by ordinary library calls.
2. `Awaitable[T]` is the Incan-facing protocol behind `await`, in the same language-first spirit that RFC 028 applies to operators.
3. `race` is not an always-on core keyword. It is activated through `import std.async` and introduced through RFC 027's vocabulary/desugaring machinery.
4. `race for value:` is surface sugar over `std.async` helper APIs.
5. The long-term helper shape is variadic, via RFC 038: `std.async.race(*arms: RaceArm[R]) -> R`.

RFC 029 matters here too: when different branches produce different value types, `race` can naturally return a union such as `str | int` instead of forcing every caller through `Either`-style wrappers.

The keyword is `race`, not `select`.

That choice is deliberate:

- `race` matches the semantics: multiple awaitables compete, one completes first, the rest are cancelled
- `race` avoids conflict with future query language surfaces that will use `SELECT`
- `race` is a better name for arbitrary awaitables than Go-style `select`, which is channel-oriented

## Motivation

### The stdlib still lacks an Incan-first way to express generic awaitables

The current `std.async` surface already shows the missing language piece. A timeout helper should be straightforward Incan:

```incan
pub async def timeout_option[T, F with Awaitable[T]](seconds: float, task: F) -> Option[T]:
    ...
```

But today that contract cannot be expressed cleanly enough in the public language. A generic parameter like `TaskFuture` can be named, but not properly constrained as "awaitable yielding `T`" in a way that makes `await task` typecheck as ordinary Incan code.

That leaves stdlib code in an awkward place:

- wrappers that should be ordinary Incan remain placeholders
- or they drop to narrow Rust-backed leaves earlier than they should

This RFC closes that gap by giving the language an explicit awaitable protocol and by giving `std.async` a concise syntax for racing awaitables.

### `await` is the primitive; `race` is composition

Earlier iterations of this idea treated `race` as a new core expression. That is the wrong center of gravity.

The real semantic primitive is `await`.

`await` must remain a core language feature because:

- it participates directly in typechecking
- it defines a calling convention for async code
- it requires the compiler and backend to agree on suspension, resumption, and cancellation semantics

By contrast, `race` is one layer higher. It is a way of composing several awaits. Under RFC 027, that makes it a strong fit for import-activated vocabulary plus helper-shaped expansion rather than a permanently reserved always-on keyword.

### RFC 027, RFC 028, RFC 029, RFC 035, and RFC 038 all point to the same design

This RFC sits at the intersection of several other design decisions:

- **RFC 027** gives Incan a vocabulary path, so `race` does not need to become a one-off always-on keyword.
- **RFC 028** reinforces the language-first rule. Async semantics should be specified in Incan terms just as operators are specified in Incan terms.
- **RFC 029** gives Incan anonymous sum types. That means the common result type of a race can naturally be a union when branch bodies yield different types.
- **RFC 035** makes named functions first-class values. Combined with closures, that makes a helper-shaped model natural.
- **RFC 038** gives the helper surface its right long-term shape: a single variadic `race(*arms)` API instead of a growing `race2` / `race3` / `race4` ladder.

Taken together, these RFCs point toward a cleaner architecture:

- define `await` through an Incan protocol
- define `race` as `std.async` sugar
- package branches as homogeneous `RaceArm[R]` values
- express `race` through a variadic helper over those arm values

### Why not just keep library helpers?

Python and TypeScript prove that helper-style APIs are viable:

- Python has `asyncio.wait(...)` and `asyncio.as_completed(...)`
- TypeScript has `Promise.race(...)`

Those are useful, but they are not the whole answer for Incan.

On their own they miss two things:

1. **Ergonomic surface syntax**: `race for value:` is easier to scan and teach than nested helper calls with inline lambdas.
2. **A first-class awaitable model**: without `Awaitable[T]`, helpers still cannot express "this generic value may be awaited and yields `T`".

So the right answer is not "syntax only" or "helpers only". It is both:

- core `await` semantics via `Awaitable[T]`
- helper functions in `std.async`
- syntax sugar over those helpers

### `race` is the right word

This feature is the async sibling of `match`, but it is not literally `match`.

- `match` chooses a branch based on the shape of one value that already exists
- `race` chooses a branch based on which awaited operation completes first

That difference matters enough that reusing `match` would blur semantics rather than clarify them.

## Goals

- Formalize the Incan-facing protocol behind `await` as `Awaitable[T]`.
- Allow generic APIs to say "this parameter is awaitable and yields `T`".
- Introduce `race for value:` as `std.async` vocabulary syntax activated by importing `std.async`.
- Desugar `race` into ordinary `std.async` helper calls rather than treating it as a one-off backend special form.
- Use RFC 038's variadic capture to shape the long-term helper surface as `race(*arms: RaceArm[R])`.
- Make union return types from RFC 029 a first-class part of the `race` story.
- Specify cancellation and tie-breaking semantics clearly.

## Non-Goals

- Making `race` an always-on core keyword.
- Replacing `await` with a library function. `await` remains a core language feature.
- Exposing Rust's `Future<Output = T>` syntax directly in user-facing Incan.
- Designing a full async trait system, async closures RFC, or effect system.
- Adding Go-style channel `select` as a separate feature in this RFC.
- Adding `default` arms, guarded arms, or fairness controls in v1.

## Guide-level explanation

### User model

Think of `race` as the async cousin of `match`.

- `match` says: "inspect one value and choose a branch"
- `race` says: "wait on several awaitables and choose the branch attached to the winner"

The winning branch gets a value binding. The losing branches are cancelled.

### Basic syntax

```incan
result = race for value:
    await fast() => value
    await slow() => value
```

This reads as:

1. start both awaitables
2. whichever completes first binds its result to `value`
3. evaluate the corresponding branch body
4. cancel the losing awaitable

### Union results fit naturally

RFC 029 removes a lot of wrapper pressure here.

```incan
result: str | int = race for value:
    await fetch_text() => value
    await fetch_count() => value

match result:
    case str(s):
        println(f"text: {s}")
    case int(n):
        println(f"count: {n}")
```

The two arms await different result types, but the branch bodies still agree on one final type: `str | int`.

### Use an enum when provenance matters

If both branches produce the same type and you still need to know which branch won, use an explicit wrapper in the branch bodies:

```incan
pub enum Source:
    Primary(str)
    Replica(str)

result = race for value:
    await fetch_primary() => Source.Primary(value)
    await fetch_replica() => Source.Replica(value)
```

This keeps `race` simple. The syntax decides the winner; ordinary Incan types decide how much provenance you want to carry afterwards.

### Timeout becomes ordinary `std.async` composition

Once `Awaitable[T]` exists, timeout helpers become straightforward:

```incan
pub async def timeout_option[T, F with Awaitable[T]](seconds: float, task: F) -> Option[T]:
    return race for value:
        await task => Some(value)
        await sleep(seconds) => None
```

The public surface is plain Incan. The helper is described in Incan terms, even if its eventual backend realization uses Tokio or another runtime.

### Helper model

The intended long-term helper model is variadic:

```incan
result = race for value:
    await fast() => value
    await slow() => value
```

Conceptually, the surface form corresponds to:

```incan
result = await std.async.race(
    std.async.arm(fast(), (value) => value),
    std.async.arm(slow(), (value) => value),
)
```

RFC 038 matters because without variadics the helper surface tends to fragment into fixed-arity forms. With variadics, the public API stays clean.

### Direct helper use also works

RFC 035 matters here because named function references fit the helper form naturally:

```incan
def on_fast(value: str) -> str:
    return value

def on_slow(value: str) -> str:
    return value

result = await std.async.race(
    std.async.arm(fast(), on_fast),
    std.async.arm(slow(), on_slow),
)
```

Users do not have to write the helper call directly, but when they do, named function references and closures should both work.

### Matching is still done with ordinary `match`

This RFC intentionally does not require pattern bindings inside `race` arms.

If you want to inspect the winner's shape, you do that in ordinary Incan:

```incan
result = race for msg:
    await rx_a.recv() =>
        match msg:
            Some(value) => f"a: {value}"
            None => "a closed"
    await rx_b.recv() =>
        match msg:
            Some(value) => f"b: {value}"
            None => "b closed"
```

That keeps `race` focused on concurrency while letting `match` keep its existing role as the value-shape construct.

## Reference-level explanation

### Activation and status

`race` is not always available.

It becomes active when `std.async` is imported, following RFC 027's unified vocabulary model. A file that never imports `std.async` does not gain `race`.

### Semantic layering

This RFC distinguishes three layers:

1. **Core semantic layer**: `await` and `Awaitable[T]`
2. **Library layer**: helper values and helper functions in `std.async`
3. **Vocabulary layer**: `race for value:` syntax, which maps onto the helper layer

The design intentionally avoids collapsing all three into one special-case compiler feature.

### `Awaitable[T]`

This RFC introduces an Incan-facing protocol:

```incan
trait Awaitable[T]:
    # builtin protocol used by `await`
```

This is a language hook. Like the operator protocols of RFC 028, it is specified in Incan terms first and mapped to backend constructs second.

The user-facing rule is:

- `await expr` is valid only if `expr` has some type `F` such that `F with Awaitable[T]` for some `T`
- the result type of `await expr` is `T`

Backends may realize this however they need to. On Rust, that will likely mean a representation equivalent to `Future<Output = T>`, but that is backend guidance, not the language model.

### Bound syntax

This RFC gives practical meaning to:

```incan
F with Awaitable[T]
```

This means:

- values of type `F` may be awaited
- awaiting them yields a value of type `T`

This is the missing piece that lets generic async wrappers be expressed cleanly in Incan source.

### Surface syntax

The primary surface syntax is:

```text
race_for_expr ::= "race" "for" IDENT ":" NEWLINE INDENT race_for_arm+ DEDENT
race_for_arm  ::= "await" expr "=>" race_body
race_body     ::= expr | NEWLINE INDENT stmt+ DEDENT
```

Example:

```incan
result = race for value:
    await fast() => value
    await slow() => value
```

The binding name after `for` is in scope inside each arm body, but each arm gets its own logically separate binding.

### Context restrictions

1. `race` is only valid inside `async def`.
2. Every arm in v1 is an `await` arm.
3. All arm bodies must produce a single common result type.
4. That common result type may be a union, subject to RFC 029's rules.
5. `race` is expression-position syntax.

### Helper API shape

The long-term helper family is expected to look roughly like this:

```incan
pub type RaceArm[R] = ...

pub def arm[T, R, F with Awaitable[T]](
    awaitable: F,
    on_win: (T) -> R,
) -> RaceArm[R]

pub async def race[R](*arms: RaceArm[R]) -> R
```

The important design choice is that the variadic parameter is homogeneous. Each branch is packaged into a `RaceArm[R]` first, and only then passed through `*arms`. This is what lets RFC 038 solve the arity problem cleanly.

### Surface-to-helper relationship

Conceptually:

```incan
result = race for value:
    await fast() => transform_fast(value)
    await slow() => transform_slow(value)
```

desugars to:

```incan
result = std.async.race(
    std.async.arm(fast(), (value) => transform_fast(value)),
    std.async.arm(slow(), (value) => transform_slow(value)),
)
```

The exact internal representation is an implementation detail. The important contract is that `race` is a library-shaped surface over `std.async` helpers, not a hidden one-off backend primitive.

### Transitional implementation note

If RFC 038 is not available at initial implementation time, fixed-arity helpers such as `race2` and `race3` are acceptable as a stepping stone.

They are not the desired long-term public architecture.

### Type checking rules

For a `race for value:` expression:

1. Each awaited expression must typecheck as some `Awaitable[T_arm]`.
2. Inside that arm body, `value` has type `T_arm`.
3. The binder is arm-local; reusing the same name across arms is legal and does not imply the same type.
4. Every arm body must typecheck to the same result type `R`.
5. `R` may be an ordinary type, an enum, or a union from RFC 029.
6. The overall `race` expression has type `R`.

Example:

```incan
return race for value:
    await fetch_user() => Ok(value)
    await fetch_error_code() => Err(value)
```

This typechecks if both branches produce the same outer type, for example `Result[User, int]`.

### Runtime semantics

When evaluation enters a `race` expression:

1. All awaited arm expressions are started in the current async context.
2. The runtime polls them concurrently.
3. The first arm to complete wins.
4. The winning arm body is evaluated.
5. Losing awaitables are cancelled by being dropped.

This is not the same as spawning detached tasks. `race` multiplexes several awaitables within one async flow.

### Cancellation semantics

Cancellation is cooperative:

- losing arms do not continue running to completion
- dropping a losing awaitable triggers whatever cleanup that awaitable normally performs
- code must not assume side effects after the final suspension point of a losing arm will still happen

This is the same semantic territory as runtimes like Tokio, but the language definition stays backend-agnostic.

### Tie-breaking

If more than one arm becomes ready at the same poll point, v1 chooses the first arm in source order.

This gives deterministic behavior and keeps the first version easy to reason about.

### Backend guidance

The Rust backend will likely realize `Awaitable[T]` in terms equivalent to Rust futures and realize `std.async.race(...)` in terms equivalent to `tokio::select!` or a narrow helper facade.

That is explicitly backend guidance, not the normative language definition.

## Examples

### Fastest mirror wins

```incan
async def fetch_file() -> bytes:
    return race for data:
        await http_get(PRIMARY_URL) => data
        await http_get(MIRROR_URL) => data
```

### Heterogeneous winner

```incan
result: str | int = race for value:
    await fetch_text() => value
    await fetch_count() => value
```

### Direct helper use

```incan
pub async def fastest_text() -> str:
    return await std.async.race(
        std.async.arm(fetch_primary(), (value) => value),
        std.async.arm(fetch_replica(), (value) => value),
        std.async.arm(fetch_cache(), (value) => value),
    )
```

## Why not `select`

`select` was considered and rejected.

Reasons:

- `SELECT` is reserved for future query language surfaces, so reusing the word would create unnecessary ambiguity
- Go-style `select` is channel-oriented, while this RFC is about arbitrary awaitables
- `race` describes the behavior directly and keeps expectations cleaner

This does not rule out a future channel-specialized construct if that later proves worthwhile.

## Why not `async match`

`async match` was also considered.

It sounds attractive at first because `race` is the async cousin of `match`, but the semantics are different enough that overloading `match` would blur the model:

- `match` inspects one value that already exists
- `race` waits on several awaitables and cancels losers

Incan already has a good story for "await one thing, then match it":

```incan
match await rx.recv():
    Some(msg) => handle(msg)
    None => handle_closed()
```

`race` is needed specifically for the multi-await case.

## Alternatives considered

### 1. Make `race` a hard core keyword

Rejected as the preferred framing.

Pros:

- simpler to describe in isolation
- direct compiler ownership of the syntax

Cons:

- misses RFC 027's vocabulary/desugaring architecture
- overstates how special `race` really is compared to the true primitive, `await`
- makes the feature feel more compiler-owned and less stdlib-shaped than necessary

### 2. Fixed-arity helper APIs only

Rejected as the long-term design.

Pros:

- easy stepping stone for implementation
- no dependency on RFC 038

Cons:

- proliferates `race2`, `race3`, `race4`, and so on
- teaches the wrong shape for the public API
- makes syntax sugar less cleanly explainable

### 3. Pure helper APIs with no syntax sugar

Rejected as the user-facing design.

Pros:

- minimal syntax work
- familiar to Python and TypeScript users

Cons:

- clunkier for common first-wins code
- loses the clarity of an arm-oriented surface
- still requires `Awaitable[T]` work anyway

The helpers should exist, but syntax sugar over them is worthwhile.

### 4. Expose Rust-like `Future<Output = T>`

Rejected for Incan source.

Pros:

- maps closely to the Rust backend

Cons:

- leaks Rust concepts into the public language model
- introduces associated-type syntax before users need to think in those terms
- conflicts with RFC 028's language-first philosophy

### 5. Treat `race` as a hidden intrinsic instead of helper sugar

Not preferred.

Pros:

- can simplify an initial backend implementation

Cons:

- obscures the stdlib-facing model
- underuses RFC 035's function-reference story
- no longer benefits as directly from RFC 038's variadic design

An implementation may still use internal specialization, but the public architecture should remain helper-shaped.

## Drawbacks

- `Awaitable[T]` adds a new builtin protocol that the language implementation must understand.
- `race` adds async-specific vocabulary users must learn.
- cancellation semantics require careful documentation and testing.
- RFC 027 may need a small extension if expression-position vocab blocks are not yet covered cleanly enough.
- RFC 038 becomes a meaningful architectural dependency for the ideal helper surface, even if fixed-arity helpers can bridge the gap temporarily.

These costs are acceptable because they buy a much cleaner async story for the stdlib and future libraries.

## Layers affected

- **Core async model** — `Awaitable[T]` is the builtin protocol behind `await`; `await expr` must verify that the awaited expression satisfies `Awaitable[T]` and that the result type follows.
- **Vocabulary activation** — `race for value:` is import-activated syntax through `std.async`, following RFC 027's vocabulary model; expression-position block forms may require a small RFC 027 extension.
- **Stdlib (`std.async`)** — the module owns the helper surface (`RaceArm[R]`, `arm(awaitable, on_win)`, and `race(*arms: RaceArm[R])`); the older `select` placeholder story should converge on `race`.
- **Compilation handoff** — implementations must preserve the contract that `race` maps onto the `std.async` helper model; transitional fixed-arity helpers (`race2`, `race3`) are acceptable until RFC 038 variadics land.
- **Backend realization** — backends may realize `Awaitable[T]` and `std.async.race(...)` using native async primitives; for Rust that likely means future semantics plus a `tokio::select!`-like strategy, but that is backend guidance, not the normative language definition.

## Unresolved questions

1. Should `Awaitable[T]` be user-implementable in v1, or compiler-recognized only?
2. Should `std.async.select` be renamed to `std.async.race`, with compatibility exports left behind?
3. Does RFC 027 need a dedicated expression-block surface kind for `race for value:`?
4. Should a later version add a more general pattern-binding `race` form, or is `race for value:` plus ordinary `match` sufficient?
5. Should a later version add `default` arms, guard expressions, or unbiased scheduling options?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
