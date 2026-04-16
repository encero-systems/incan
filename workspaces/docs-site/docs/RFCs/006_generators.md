# RFC 006: Python-style generators

- **Status:** Draft
- **Created:** 2024-12-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 016 (loop and break value), RFC 019 (runner testing)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/324
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** —

## Summary

This RFC introduces Python-style generators to Incan in two connected forms: generator functions that use `yield` inside `def`, and generator expressions that produce lazy generator values inline. Both forms describe the same underlying language model: a resumable producer of `T` exposed as
`Generator[T]`.

## Motivation

Incan currently has eager collections and iterator-shaped loops, but it does not have a first-class way to express lazy, stateful iteration in the language surface itself. That hurts several common cases:

- large or unbounded sequences that should not be materialized eagerly;
- streaming transformations that would otherwise allocate intermediate lists;
- recursive traversals where the natural shape is "produce one value, suspend, resume";
- portability for authors coming from Python, where `yield` and generator
expressions are familiar ways to express lazy iteration.

The feature also fits Incan's backend well. Incan does not need a separate user-facing coroutine model just to support this control-flow shape; the compiler can lower generators to ordinary backend state-machine machinery.

## Goals

- Add a first-class generator model based on `yield` and `Generator[T]`.
- Support both generator functions and generator expressions as part of that model.
- Make lazy iteration explicit in type signatures instead of inferring it from incidental usage.
- Preserve ordinary `for`-loop ergonomics over generator results.
- Distinguish generator `yield` clearly from fixture `yield`.

## Non-Goals

- Async generators in this RFC.
- Bidirectional coroutine features such as Python-style `send()`.
- A wholesale redesign of comprehensions or iterator combinators beyond generator support itself.
- Standardizing every possible `Generator` helper method in the initial draft.

## Guide-level explanation (how users think about it)

### Generator functions

```incan
def count_up(start: int, end: int) -> Generator[int]:
    mut i = start
    while i < end:
        yield i
        i += 1

for n in count_up(0, 1_000_000):
    if n > 100:
        break
    println(n)
```

The function above does not build a list. Each `yield` produces one element for the surrounding iteration and then suspends the function until the next value is requested.

### Generator expressions

```incan
squares = (x * x for x in range(10))

for sq in squares:
    println(sq)
```

Generator expressions are the expression form of the same idea: they produce a lazy `Generator[T]` instead of an eager list.

### Infinite generators

```incan
def fibonacci() -> Generator[int]:
    mut a, b = 0, 1
    while true:
        yield a
        a, b = b, a + b

fibs = fibonacci().take(10).collect()
```

This is the core value proposition: the generator can describe an unbounded stream, while the consumer decides how much to realize.

### Recursive traversal

```incan
def walk_tree(node: Node) -> Generator[Node]:
    yield node
    for child in node.children:
        for descendant in walk_tree(child):
            yield descendant
```

Generators are useful when the control flow is naturally incremental instead of collection-oriented.

## Reference-level explanation (precise rules)

### Generator functions reference

- A function is a generator function when its body contains `yield` and its declared return type is `Generator[T]`.
- `yield expr` produces one element of type `T` for the surrounding generator.
- `yield` must not appear in ordinary functions, except where another RFC
explicitly gives `yield` special meaning for a distinct construct such as fixtures.
- A generator function may use `return` to terminate iteration early, but `return value` is not part of this RFC.

### Generator expressions reference

- A generator expression has the form `(expr for binding in iterable)` and
yields a `Generator[T]`, where `T` is the type of `expr`.
- The iterable source is consumed lazily as the resulting generator is advanced.
- A generator expression is semantically equivalent to an anonymous generator
that iterates the source and yields `expr` for each bound element.
- Generator expressions are lazy; the list-comprehension surface remains the eager collection form.

### Typing

- Every yielded expression must type-check against the element type `T` in `Generator[T]`.
- Declaring `Generator[T]` without any reachable `yield` is a compile-time error.
- Using `yield` without a `Generator[T]` return type is a compile-time error
unless another construct has already claimed `yield` semantics for that context.

### Consumption

- `for` loops must accept generator values anywhere they accept iterable values.
- Generator values may expose chainable helper methods such as `.map()`,
  `.filter()`, `.take()`, and `.collect()`, but this RFC does not yet freeze
the full helper surface.
- Exhausting a generator ends iteration normally.

## Design details

### One generator model, two surfaces

Generator functions and generator expressions are not separate features stitched together for convenience. They are two surfaces over the same language model:

- generator functions are statement-oriented and better for named, reusable, stateful producers;
- generator expressions are expression-oriented and better for inline lazy transforms.

This RFC treats both as first-class parts of Python-style generator support rather than as rollout stages.

### Distinction from fixtures

The language already uses `yield` in fixture-oriented testing flows. That overlap is tolerable only if the surrounding declaration makes the meaning unambiguous:

- fixture declarations keep fixture lifecycle semantics;
- ordinary functions returning `Generator[T]` use lazy iteration semantics.

This RFC therefore treats the declaration context, not the token alone, as the source of truth for `yield` meaning.

### Lowering model

The intended implementation strategy is to lower generator functions and generator expressions through a compiler-owned state-machine transformation or equivalent backend support. That lowering choice is not the language definition; the language contract is only that generators behave as lazy, resumable producers of `T`.

### Interaction with existing features

- `for` loops consume generators the same way they consume other iterable sources.
- Recursive generators are valid as long as the yielded element type remains consistent.
- Generator expressions are the lazy counterpart to eager list-comprehension
syntax rather than a separate collection feature.

### Compatibility / migration

The feature is additive. Existing functions, loops, and comprehensions keep their meaning.

## Alternatives considered

1. **Explicit `gen` keyword**
   - Clear, but more backend-shaped than Incan needs. Requiring `Generator[T]`
     plus `yield` already communicates intent.

2. **Dedicated `generator` declaration form**
   - Avoids overloading ordinary `def`, but splits the function surface for a
     feature that is still "a function producing values over time."

3. **Functions only, expressions later**
   - Not actually more principled. It would make the RFC weaker while still
     aiming at the same north-star generator model.

## Drawbacks

- `yield` now carries two meanings in the language, so diagnostics must be explicit.
- Generators introduce suspension semantics that users must learn alongside ordinary function control flow.
- Generator expressions add grammar and precedence surface that the language and tooling must handle carefully.

## Layers affected

- **Language surface**: `yield` must be valid in generator function bodies, and generator-expression syntax must be recognized.
- **Type system**: yielded expressions must match `Generator[T]`, and generator declarations must remain internally consistent.
- **Execution model**: implementations must preserve suspension points and lazy iteration semantics for both named and anonymous generator forms.
- **Stdlib / surface vocabulary**: the language must define the `Generator` type and any stable helper methods it promises publicly.
- **Formatter / tooling**: multi-line generators should format predictably, and diagnostics should explain generator-specific behavior clearly.

## Unresolved questions

1. Should generator expressions support the full comprehension clause surface
immediately, including multiple `for` clauses and trailing `if` filters, or should this RFC only normatively require the single-`for` core form?
2. What is the minimum `Generator` helper surface that Incan wants to
standardize rather than inheriting opportunistically from backend details?
3. Should `return value` inside a generator be rejected outright, or reserved for a future coroutine-oriented extension?
4. How much of the fixture/generator `yield` distinction should be surfaced in
linting or style guidance so that mixed mental models do not leak into user code?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
