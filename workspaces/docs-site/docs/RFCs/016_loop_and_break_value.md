# RFC 016: `loop` and `break <value>` (Loop Expressions)

- **Status:** Planned
- **Created:** 2025-12-24
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 006 (generators)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/327
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

Add a `loop:` keyword for explicit infinite loops and extend `break` to optionally carry a value (`break <expr>`), so `loop:` can be an expression that produces a value (like Rust’s `loop { ... }`) while `while` remains the general conditional loop construct.

## Motivation

Today, users express infinite loops as `while True:`. The compiler may emit Rust `loop {}` for that pattern, but the source language has no explicit infinite-loop construct and cannot express “break with a value.” Adding `loop:` and `break <value>` gives clearer intent (`loop:` reads as an infinite loop), a foundation for expression-oriented control flow without “initialize then mutate” patterns, and a natural shape for “search until found” loops that return a value.

## Goals

- Introduce `loop:` as an explicit infinite loop construct.
- Allow `break` to optionally carry a value: `break <expr>`.
- Allow `loop:` in expression position (e.g., assignment RHS).
- Keep existing `break` (no value) valid and well-defined.
- Keep `while True:` valid (and optionally desugar between `loop:` and `while True:` in the implementation).

## Non-goals

- Labeled `break` / `continue` syntax (follow-up RFC).
- `break` with multiple values or tuple sugar (callers return tuples explicitly).
- Making `while` an expression (value-yielding loops stay scoped to `loop:`).

## Guide-level explanation (how users think about it)

### `loop:`

```incan
loop:
    # body
    ...
```

### `break` (with optional value)

```incan
break
break some_expr
```

### Compute a value without external mutation

```incan
answer = loop:
    if some_condition():
        break 42
```

Equivalent with `while True:` without `break <value>` typically uses a `mut` accumulator and `break` after assignment.

### Search until found

```incan
found = loop:
    item = next_item()
    if item.is_ok():
        break item
```

### `break` without a value

```incan
loop:
    if done():
        break
```

### Async and `await`

`await` pauses an `async def` until a `Future` is ready; it does not replace loops. They compose naturally:

```incan
import std.async

async def wait_until_done() -> None:
    loop:
        if await done():
            break
```

```incan
from std.async.time import sleep

async def wait_with_backoff() -> None:
    loop:
        if done():
            break
        await sleep(0.01)

    return
```

## Reference-level explanation (precise rules)

### Loop execution

- `loop:` runs its body repeatedly until it exits via `break` (or an error/abort).
- `continue` skips to the next iteration (existing behavior).

### `break` values

- `break` exits the innermost enclosing `loop:`.
- If `break` includes a value, that value is the value of the `loop:` expression.
- `break` without a value is equivalent to `break ()` (the loop’s value is `Unit`).

### Expression result type

`loop:` is an expression with a single result type:

- If every reachable `break` omits a value, the loop’s type is `Unit`.
- If any reachable `break` carries a value, the loop’s type is the least upper bound (unification) of all break value types; if the compiler cannot unify them, it must report a type error.
- If a `loop:` has no reachable `break`, the loop is non-terminating on those paths. Until the language defines a bottom (`Never` / `!`) type, the typechecker must reject such `loop:` expressions where a concrete type is required (see Design Decisions).

### Generators (`yield`)

- `break` exits a loop; `yield` produces one element of an `Iterator` and suspends (RFC 006).
- Inside a generator, `loop:` behaves like any other loop: `yield` suspends; `break` exits the loop and may leave the generator running after the loop or finishing, depending on control flow after the loop.
- When `loop:` appears as an expression in a generator body, `break <value>` completes the loop expression only; it does not contribute to the iterator’s yielded sequence (only `yield` does). This is allowed and orthogonal to generator output (see Design Decisions).

### Backend alignment

The language must be able to lower value-carrying `break` together with `loop:` to the host pattern where an infinite loop is the construct that yields a value via `break` (as in Rust). A lowering that only has conditional `while` loops may need a dedicated representation for “infinite loop with value-carrying break” so the backend can emit the correct shape.

## Design details

### Why keep `loop:` / `break <value>` when `while` exists

- `loop:` supports multiple exit points (success, timeout, error) without extra state variables.
- `break <value>` makes the loop expression-oriented: `found = loop: ... break value` without a pre-declared `mut` holder.
- A purely conditional `while` often forces pre-initialization or a “run once then test” shape that `loop:` avoids.

### Example: conditional `while` alternative

```incan
item = next_item()
while not item.is_ok():
    item = next_item()

found = item
```

This works when the loop is “repeat until condition,” but not always when the first iteration shape differs or there are multiple exits.

## Alternatives considered

1. **Only `while True:`**
   - Rejected: harder to justify `break <value>` on `while`, and less clear intent for infinite loops.

2. **Make `while` an expression too**
   - Rejected: larger semantic surface and surprise (“`while` yields a value?”); weaker alignment with common backend shapes for value-carrying breaks.

## Drawbacks

- Adds a new keyword and parsing surface.
- Unifying all `break` value types can produce dense type errors when branches disagree.
- Generator and async bodies add control-flow combinations that implementers and tests must cover.

## Layers affected

- **Lexer / parser:** `loop` keyword, `loop:` blocks, optional expression after `break`.
- **Typechecker:** treat `loop:` as an expression; unify break value types; rules for non-terminating loops until a bottom type exists.
- **Lowering / IR / emission:** represent infinite loops and value-carrying `break` so the backend can emit the correct infinite-loop + break-value pattern.
- **Formatter / LSP (as applicable):** formatting and keyword-aware tooling for `loop` and extended `break`.

## Design Decisions

1. **Non-terminating `loop:` (no reachable `break`)** — Until the language defines a bottom (`Never` / `!`) type, a `loop:` used as an expression where a concrete type is required must be rejected if there is no reachable `break`. Introducing `Never` is explicitly out of scope for this RFC’s minimum bar; a follow-up may add it and relax this rule.

2. **Labeled `break` / `continue`** — Deferred to a separate RFC (see Non-goals).

3. **Statement-only vs expression `loop:`** — Both forms are in scope: `loop:` may appear as a statement or as an expression (per Goals), not phased as statement-only first.

4. **`loop:` as an expression inside generator bodies** — Allowed. `break <value>` only completes the inner `loop:` expression; iterator consumers still observe output only through `yield`.

## Possible future syntax sugar: `loop ... until ...`

A compact statement-level sugar for “repeat an action until a condition holds” may be added later:

```incan
loop item.next() until item.is_ok()
```

Conceptual desugaring:

```incan
loop:
    item.next()
    if item.is_ok():
        break
```

- `until <expr>` must typecheck to `bool`.
- This form is intended as a **statement** and does not yield a value by itself.
