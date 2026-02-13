# RFC 016: `loop` and `break <value>` (Loop Expressions)

**Status:** Planned
**Created:** 2025-12-24  

## Summary

Add a `loop:` keyword for explicit infinite loops, and extend `break` to optionally carry a value: `break <expr>`.

This enables treating `loop:` as an **expression** that can produce a value (similar to Rust’s `loop { ... }`), while
keeping `while` as the general conditional loop construct.

## Motivation

Today, users express infinite loops as `while True:`. The compiler may emit Rust `loop {}` for this pattern, but the
source language has no explicit infinite-loop construct and cannot express “break with a value”.

Adding `loop:` and `break <value>` provides:

- Clearer intent in source (`loop:` reads as “infinite loop”).
- A foundation for expression-oriented control flow without “initialize then mutate” patterns.
- A natural home for “search until found” patterns that return a value.

## Goals

- Introduce `loop:` as an explicit infinite loop construct.
- Allow `break` to optionally carry a value: `break <expr>`.
- Allow `loop:` to be used in expression position (e.g., assignment RHS).
- Keep existing `break` (no value) valid and well-defined.
- Keep `while True:` valid (and optionally desugar `loop:` to `while True:` or vice-versa).

## Non-goals

- Labeled `break` / `continue` syntax changes (may be addressed in a follow-up RFC).
- `break` with multiple values / tuple sugar (users can return tuples explicitly).
- Making `while` an expression (this RFC keeps “value-yielding loops” scoped to `loop:`).

## Proposed syntax

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

## Semantics

### Loop execution

- `loop:` executes its body repeatedly until it is exited via `break` (or an error/abort).
- `continue` skips to the next iteration (existing behavior).

### `break` values

- `break` exits the innermost `loop:`. If it includes a value, that value becomes the value produced by the `loop:` expression.
- `break` without a value is equivalent to `break ()` (i.e., produces `Unit`).

### Expression result type

`loop:` is an expression with a single result type:

- If every reachable `break` in the loop is `break` (no value), the loop’s type is `Unit`.
- If one or more `break` statements include values, the loop’s type is the **least upper bound** (LUB) / unification
  result of all `break` value types.
    - If the compiler cannot unify the break value types, it is a type error.
- If a `loop:` has no reachable `break`, it is considered non-terminating.
    - Initial implementation may treat this as a type error unless we also introduce a `Never`/`!` type.

### Interaction with generators (`yield`)

`break` and `yield` are different control-flow concepts:

- `break` exits a loop.
- `yield` produces one element of an `Iterator[T]` and suspends execution (RFC 006).

Inside a generator function, `loop:` behaves like any other loop construct:

- `yield expr` produces a value and suspends the generator.
- `break` exits the loop; the generator may continue after the loop, or terminate if nothing follows.

Open design question:

- If `loop:` is allowed as an expression inside generator bodies, `break <value>` would produce a value for the `loop:`
  expression, but this value is distinct from generator output (which is produced only by `yield`).
  We can either allow this (it is orthogonal), or restrict “loop-as-expression” in generators initially for simplicity.

### Interaction with async/await

`await` is an async suspension point: it pauses an `async def` until a `Future` is ready.
It does not replace loops.

You typically combine them:

```incan
import std.async

async def wait_until_done() -> None:
    loop:
        if await done():
            break
```

If you need polling/backoff, `await` controls waiting between iterations:

```incan
from std.async.time import sleep

async def wait_with_backoff() -> None:
    loop:
        if done():
            break
        await sleep(0.01)

    return
```

## Examples

### Example 1: Compute a value without external mutation

```incan
answer = loop:
    if some_condition():
        break 42
```

Equivalent with `while True:` (today):

```incan
mut answer = 0
while True:
    # Without `break <value>`, you typically compute a result via external mutation.
    if some_condition():
        answer = 42
        break
```

### Example 2: Search until found

```incan
found = loop:
    item = next_item()
    if item.is_ok():
        break item
```

Equivalent with `while True:` (today):

```incan
mut found = None
while True:
    item = next_item()
    if item.is_ok():
        found = item
        break
```

Alternative with a conditional `while` (works when you can express the loop as “repeat until condition”):

```incan
item = next_item()
while not item.is_ok():
    item = next_item()

# Here: item.is_ok() == true
found = item
```

Why keep `loop:` / `break <value>` anyway?

- `loop:` supports **multiple exit points** naturally (success, timeout, error), without extra state variables.
- `break <value>` makes the loop **expression-oriented**, so you can write `found = loop: ... break value` directly.
- A conditional `while` often forces **pre-initialization** (or a `do-while` construct) to compute the first value.

### Example 3: `break` without value

```incan
loop:
    if done():
        break
```

## Lowering / codegen strategy (Rust backend)

### Desugaring options

The compiler may implement `loop:` in one of two ways:

1. **AST-level sugar**: desugar `loop:` to `while True:` early, and keep codegen optimizations.
2. **Dedicated IR node**: lower `loop:` directly to an IR `Loop` statement/expression, and emit Rust `loop {}`.

If `break <value>` is introduced, a dedicated IR representation is recommended, because Rust requires:

- `loop { ... break expr; }` for value-yielding loops, and
- the `loop` construct (not `while`) to yield a value.

### IR changes

If we treat `loop:` as an expression, IR likely needs:

- An expression form (e.g., `IrExprKind::Loop { body: Vec<IrStmt>, result_ty: IrType }`), or
- A block-expression convention where a `Loop` statement plus `break value` composes into an expression value.

Additionally, `IrStmtKind::Break` should be extended to carry an optional value expression
(and still optionally support labels in the future).

## Backwards compatibility

- Existing programs remain valid.
- `while True:` remains valid and may continue to codegen to Rust `loop {}`.
- `break` without value remains valid.

## Alternatives considered

## Implementation notes (current crate layout)

This RFC introduces new syntax and vocabulary:

- A new keyword: `loop`
- An extended form of `break` (`break <value>`)

In the current workspace, those changes should be implemented in:

- `crates/incan_core/src/lang/keywords.rs`: add `loop` to the keyword registry (with correct RFC provenance)
- `crates/incan_syntax`: lexer emits `TokenKind::Keyword(KeywordId::Loop)` and parser handles `loop:` and `break <expr>`
- `crates/incan_core` docgen/tests: update reference docs + add parity/guardrail tests so registry ↔ lexer stay aligned

### Alternative A: Only keep `while True:`

Pros:

- No new keyword.

Cons:

- Harder to justify `break <value>` on `while`.
- Less clear intent in source.

### Alternative B: Make `while` an expression too

Pros:

- Fewer constructs.

Cons:

- More semantic surface and surprises (“`while` yields a value?”).
- Less aligned with Rust codegen constraints.

## Open questions

- Do we want a `Never`/`!` type for non-terminating `loop:` expressions?
- Do we want labeled loops (and labeled `break`/`continue`) in the same RFC or separately?
- Should `loop:` be allowed in statement position only initially, with expression usage added later?

### Possible future syntax sugar: `loop ... until ...`

We may add a compact, statement-level sugar for “repeat an action until a condition holds”:

```incan
loop item.next() until item.is_ok()
```

Desugaring (action first, then test, then break):

```incan
loop:
    item.next()
    if item.is_ok():
        break
```

Notes:

- `until <expr>` must typecheck to `bool`.
- This form is intended as a **statement** (it does not yield a value by itself).
