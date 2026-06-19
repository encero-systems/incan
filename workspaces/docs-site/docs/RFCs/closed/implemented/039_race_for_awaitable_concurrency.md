# RFC 039: `race` for awaitable concurrency

- **Status:** Implemented
- **Created:** 2026-03-07
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 023 (Compilable stdlib & Rust module binding)
    - RFC 027 (`incan-vocab`)
    - RFC 028 (Trait-based operator overloading)
    - RFC 029 (Union types and type narrowing)
    - RFC 035 (First-class named function references)
    - RFC 038 (Variadic positional args and keyword-argument capture)
- **Issue:** [#173](https://github.com/encero-systems/incan/issues/173)
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** v0.3

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
- Designing async closures, a full effect system, or unrelated async trait features beyond the `Awaitable[T]` implementation contract required by this RFC.
- Adding Go-style channel `select` as a separate feature in this RFC.
- Adding `default` arms, guarded arms, or fairness controls in this RFC.

## Guide-level explanation

### User model

Think of `race` as the async cousin of `match`.

- `match` says: "inspect one value and choose a branch"
- `race` says: "wait on several awaitables and choose the branch attached to the winner"

The winning branch gets a value binding. The losing branches are cancelled.

That cancellation is part of the contract, not an optimization. Write race arms so that losing arms can be abandoned safely. Do not put required side effects, final writes, channel sends, barrier waits, or other must-run work after an `await` inside an arm unless that operation is independently cancellation-safe or the work has been made durable by spawning it before the race.

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

In this example, when the sleep arm wins, `task` is cancelled. That is the desired timeout contract for ordinary cancel-safe request work. It is not a durable timeout; use a spawned task handle when the timed operation must continue after the timeout result is returned.

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

### Keep loser arms cancellation-safe

Losing race arms are cancelled by dropping their awaitable. If a losing arm needs cleanup, the cleanup must be owned by a cancellation-safe resource or a spawned task that is intentionally durable.

Prefer arms whose awaited operation can be abandoned:

```incan
result = race for value:
    await fetch_primary() => value
    await fetch_replica() => value
```

Avoid relying on loser-arm side effects:

```incan
# Avoid: if `slow_write()` loses, its final write may never happen.
result = race for value:
    await fast_read() => value
    await slow_write() => value
```

For must-run work, spawn it before the race and make the lifetime explicit:

```incan
handle = spawn(slow_write())

result = race for value:
    await fast_read() => value
    await timeout_signal() => fallback_value()

match await handle:
    Ok(_) => println("write finished")
    Err(err) => println(err.message())
```

Dropping the handle would detach the spawned task and lose its result; call `abort()` only when cancellation is intended.

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
    # compiler-known protocol used by `await`
```

This is a language hook. Like the operator protocols of RFC 028, it is specified in Incan terms first and mapped to backend constructs second. Unlike the narrower draft version of this RFC, `Awaitable[T]` is a real protocol surface that user-defined types may satisfy, not merely a compiler-private predicate.

The user-facing rule is:

- `await expr` is valid only if `expr` has some type `F` such that `F with Awaitable[T]` for some `T`
- the result type of `await expr` is `T`

Backends may realize this however they need to. On Rust, that will usually mean a representation equivalent to `Future<Output = T>`, but that is backend guidance, not the language model.

User implementability is still checked, not declarative-only. A type must not be able to write `with Awaitable[T]` and then fail during lowering because the compiler has no await realization for it. Satisfying `Awaitable[T]` therefore requires one of these implementation paths:

- a Rust-backed future-like value with metadata that maps to a future output type
- a stdlib runtime bridge type such as a task handle whose awaited output is known to the compiler
- an Incan wrapper type that delegates its awaitability to a field or method whose await realization is itself known

The protocol is ambitious enough for generic Incan APIs to name awaitability directly, but narrow enough that this RFC does not invent unrelated async closures, effects, or arbitrary polling APIs.

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
2. Every arm accepted by this RFC is an `await` arm.
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

If an implementation detail temporarily needs fixed-arity internal helpers such as `race2` and `race3`, those helpers must remain compiler/runtime plumbing rather than the taught public surface.

They are not the desired public architecture. The public stdlib shape introduced by this RFC is `std.async.race`, not a ladder of fixed-arity variants.

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
- `race` arms that wait on channel receives or one-shot receives are safe to abandon because cancellation does not consume the message
- `race` arms that wait on channel sends are cancel-safe-but-lossy because the unsent value can be dropped
- `race` arms that wait on mutexes, read-write locks, or semaphores are cancel-safe-but-lossy because the waiter loses queue position
- `race` arms that wait at a barrier are cancellation-aware before release: dropping a pending wait withdraws that participant from the generation, but it does not complete the generation, so enough active participants must still arrive or the whole phase must be abandoned deliberately

This is the same semantic territory as runtimes like Tokio, but the language definition stays backend-agnostic.

### Tie-breaking

If more than one arm becomes ready at the same poll point, this RFC chooses the first arm in source order.

This gives deterministic behavior and keeps the feature easy to reason about.

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

This is safe when either request may be abandoned after the other request wins.

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

Existing `std.async.select` surface is replaced by `std.async.race` as part of this RFC. Because Incan is still in its beta-era language-design phase, this RFC does not require compatibility exports or deprecated aliases for `std.async.select`; keeping both names would create legacy before there is a strong user-compatibility reason to preserve it.

This does not rule out a future channel-specialized construct if that later proves worthwhile, but it should not use `select` as a compatibility name for this first-completion awaitable feature.

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
- **Stdlib (`std.async`)** — the module owns the helper surface (`RaceArm[R]`, `arm(awaitable, on_win)`, and `race(*arms: RaceArm[R])`); the older `select` placeholder surface is removed and replaced by `race`.
- **Compilation handoff** — implementations must preserve the contract that `race` maps onto the `std.async` helper model; fixed-arity helpers (`race2`, `race3`) may exist only as internal compiler/runtime plumbing if the implementation needs them.
- **Backend realization** — backends may realize `Awaitable[T]` and `std.async.race(...)` using native async primitives; for Rust that likely means future semantics plus a `tokio::select!`-like strategy, but that is backend guidance, not the normative language definition.

## Implementation Plan

### Phase 1: RFC lifecycle and design lock

- Move this RFC from `Draft` to `In Progress`.
- Record the settled design decisions for `Awaitable[T]`, `std.async.race`, expression-position vocabulary, pattern binding, and deferred branch controls.
- Use the active development version from repository metadata as the implementation baseline.

### Phase 2: Core async model and trait metadata

- Add `Awaitable[T]` to the canonical language trait registry and stdlib protocol surface.
- Teach generic bound checking that `F with Awaitable[T]` is a real awaitability contract.
- Define compiler-known await realization paths for Rust-backed futures, stdlib task handles, and Incan wrapper types that delegate to a known awaitable member.
- Update `await` typechecking so it validates awaitability and returns the protocol output type instead of relying on narrow shape-specific special cases.

### Phase 3: Expression-position vocabulary

- Extend RFC 027 vocabulary support so import-activated syntax can produce expression-position block forms.
- Parse `race for value:` as an expression that is active only after importing `std.async`.
- Keep the binder arm-local and reject non-`await` race arms for this RFC.
- Add parser, formatter, AST bridge, LSP, and desugaring tests for expression-position vocabulary.

### Phase 4: `std.async.race` stdlib and runtime surface

- Replace `std.async.select` with `std.async.race` in stdlib source, runtime exports, prelude exports, docs, and tests.
- Add the public helper surface: `RaceArm[R]`, `arm(awaitable, on_win)`, and `race(*arms: RaceArm[R])`.
- Keep any fixed-arity helpers internal if needed by the runtime/backend implementation.
- Ensure `select_timeout` is removed or renamed rather than kept as a compatibility alias.

### Phase 5: Typechecking, lowering, and emission

- Typecheck each race arm awaited expression as `Awaitable[T_arm]`.
- Typecheck each arm body with the arm-local binder type `T_arm`.
- Compute a common result type `R`, including union result support where RFC 029 permits it.
- Lower `race for value:` to the stdlib helper model or an equivalent internal representation that preserves the helper-shaped public contract.
- Emit Rust that polls all arm awaitables concurrently, evaluates the winning body, drops losing awaitables, and preserves deterministic source-order tie-breaking.

### Phase 6: Docs, versioning, and verification

- Update authored async docs and stdlib reference pages so users learn `Awaitable[T]` and `race`.
- Update release notes for the active development line.
- Bump the active `0.3.0-dev.N` version by one dev increment.
- Run targeted parser/typechecker/codegen/runtime tests, docs checks, and the repository pre-commit gate.

## Implementation log

### Spec / design

- [x] Resolve design questions and move RFC 039 to `In Progress`.
- [x] Add `Awaitable[T]` to canonical language trait and stdlib protocol documentation.
- [x] Document that `Awaitable[T]` is user-facing and user-implementable only through checked await realization paths.
- [x] Document wholesale `std.async.select` replacement by `std.async.race`.

### Core async model / typechecker

- [x] Register `Awaitable[T]` in core trait metadata.
- [x] Model awaitability output types for Rust-backed futures, `JoinHandle[T]`, and wrapper delegation.
- [x] Enforce `await expr` against `Awaitable[T]`.
- [x] Enforce generic bounds that mention `Awaitable[T]`.
- [x] Add diagnostics for declared `Awaitable[T]` implementations that have no valid await realization.

### Vocabulary / parser / AST

- [x] Add expression-position vocabulary block support.
- [x] Parse `race for value:` under `std.async` activation.
- [x] Keep `race` usable as an identifier when `std.async` is not imported.
- [x] Add formatter support for `race for value:` expressions.
- [x] Update public vocab AST bridge support for expression-position race blocks.
- [x] Update LSP syntax/semantic handling for import-activated `race`.

### Stdlib / runtime

- [x] Replace `std.async.select` with `std.async.race`.
- [x] Add `RaceArm[R]`.
- [x] Add `arm(awaitable, on_win)`.
- [x] Add `race(*arms: RaceArm[R])`.
- [x] Remove or rename `select_timeout`; do not keep a compatibility alias by default.
- [x] Update `std.async.prelude` exports.
- [x] Update runtime module exports and Rust-backed helper tests.

### Lowering / IR / emission

- [x] Lower `race for value:` to the `std.async.race` helper model or equivalent internal representation.
- [x] Preserve arm-local binder types through lowering.
- [x] Emit deterministic source-order tie-breaking.
- [x] Ensure losing awaitables are dropped/cancelled after the winner is selected.
- [x] Reject guards, default arms, fairness controls, and pattern-binding arms with clear diagnostics if parsed or encountered.

### Tests

- [x] Parser test for active `race for value:`.
- [x] Parser test proving inactive `race` remains an identifier.
- [x] Formatter round-trip test for race expressions.
- [x] Typechecker test for valid homogeneous race result.
- [x] Typechecker test for valid union race result.
- [x] Typechecker test for invalid non-awaitable arm.
- [x] Typechecker test for invalid non-async context.
- [x] Codegen snapshot test for race expression lowering/emission.
- [x] Runtime/integration test proving first-completion wins.
- [x] Runtime/integration test proving deterministic source-order tie-breaking when both arms are ready.
- [x] Stdlib compile test for `std.async.race`.
- [x] Negative import test proving `std.async.select` is gone.

### Docs / release / version

- [x] Update async programming guide.
- [x] Update stdlib async reference.
- [x] Update import/module reference.
- [x] Update language keyword/builtin surface reference.
- [x] Add active development release-note entry.
- [x] Bump active development version from `0.3.0-dev.41` to `0.3.0-dev.42`.

## Design Decisions

1. `Awaitable[T]` is a real user-facing protocol, not a compiler-private predicate. User-defined types may satisfy it only through checked await realization paths: Rust-backed future metadata, stdlib task-handle semantics, or an Incan wrapper that delegates to a known awaitable member.
2. `std.async.select` is replaced wholesale by `std.async.race`. This RFC does not preserve `std.async.select` compatibility exports by default because the language is still in its beta-era design phase and should avoid creating legacy aliases before there is a real compatibility need.
3. RFC 027 vocabulary support must grow expression-position block forms because `race for value:` is an expression.
4. This RFC keeps `race for value:` plus ordinary `match` as the value-inspection model. More general pattern-binding race arms are deferred to a future RFC.
5. Default arms, guards, and fairness controls are deferred. Rust backends may have native support for those controls, but the Incan surface in this RFC remains deterministic first-completion race with source-order tie-breaking.
