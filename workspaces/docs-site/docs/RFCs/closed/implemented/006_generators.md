# RFC 006: Python-style generators

- **Status:** Implemented
- **Created:** 2024-12-10
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:** RFC 016 (loop and break value), RFC 019 (runner testing), RFC 068 (protocol hooks for core language syntax)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/324
- **Follow-up:** RFC 088 (iterator adapter surface), tracked by https://github.com/dannys-code-corner/incan/issues/127
- **RFC PR:** —
- **Written against:** v0.1
- **Shipped in:** v0.3

## Summary

This RFC introduces Python-style generators to Incan in two connected forms: generator functions that use `yield` inside `def`, and generator expressions that produce lazy generator values inline. Both forms describe the same underlying language model: a resumable producer of `T` exposed as `Generator[T]`.

## Motivation

Incan currently has eager collections and iterator-shaped loops, but it does not have a first-class way to express lazy, stateful iteration in the language surface itself. That hurts several common cases:

- large or unbounded sequences that should not be materialized eagerly;
- streaming transformations that would otherwise allocate intermediate lists;
- recursive traversals where the natural shape is "produce one value, suspend, resume";
- portability for authors coming from Python, where `yield` and generator expressions are familiar ways to express lazy iteration.

The feature also fits Incan's backend well. Incan does not need a separate user-facing coroutine model just to support this control-flow shape; the compiler can lower generators to ordinary backend state-machine machinery.

## Goals

- Add a first-class generator model based on `yield` and `Generator[T]`.
- Support both generator functions and generator expressions as part of that model.
- Make lazy iteration explicit in type signatures instead of inferring it from incidental usage.
- Preserve ordinary `for`-loop ergonomics over generator results.
- Standardize the minimum helper surface needed to transform and realize generator values ergonomically: `.map()`, `.filter()`, `.take()`, and `.collect()`.
- Distinguish generator `yield` clearly from fixture `yield`.

## Non-Goals

- Async generators in this RFC.
- Bidirectional coroutine features such as Python-style `send()`.
- A wholesale redesign of comprehensions or iterator adapters beyond generator support itself.
- Standardizing the broader `Iterator[T]` adapter surface in the initial draft. Methods such as `.flat_map()`, `.skip()`, `.chain()`, `.enumerate()`, `.zip()`, `.take_while()`, `.skip_while()`, `.count()`, `.any()`, `.all()`, `.find()`, `.fold()`, and `.batch()` belong in RFC 088.

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

### Lazy transformations

```incan
def square(n: int) -> int:
    return n * n

def is_even(n: int) -> bool:
    return n % 2 == 0

evens_squared = count_up(0, 1_000_000)
    .filter(is_even)
    .map(square)
    .take(10)
    .collect()
```

Generator helper methods are lazy until a terminal operation consumes them. In the example above, `.filter()`, `.map()`, and `.take()` build a generator pipeline; `.collect()` realizes only the first ten transformed values as a `list[int]`.

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
- `yield` must not appear in ordinary functions, except where another RFC explicitly gives `yield` special meaning for a distinct construct such as fixtures.
- A generator function may use `return` without a value to terminate iteration early.
- `return value` inside a generator is a compile-time error. Generator return values are reserved for a future delegation or coroutine-oriented RFC.

### Generator expressions reference

- A generator expression has the form `(expr for binding in iterable [if condition] ...)` and yields a `Generator[T]`, where `T` is the type of `expr`.
- Generator expressions support the same comprehension-clause shape as list comprehensions: one leading `for` clause followed by zero or more nested `for` clauses and zero or more trailing `if` filters.
- The iterable source is consumed lazily as the resulting generator is advanced.
- A generator expression is semantically equivalent to an anonymous generator that iterates the source and yields `expr` for each bound element.
- Generator expressions are lazy; the list-comprehension surface remains the eager collection form.

### Typing

- Every yielded expression must type-check against the element type `T` in `Generator[T]`.
- Declaring `Generator[T]` without any reachable `yield` is a compile-time error.
- Using `yield` without a `Generator[T]` return type is a compile-time error unless another construct has already claimed `yield` semantics for that context.

### Consumption

- `Generator[T]` is the semantic type for values produced by generator functions and generator expressions.
- `Generator[T]` must satisfy the static iteration protocol from RFC 068: it may be consumed anywhere an `Iterable[T]`/`Iterator[T]` value is accepted, and `for` loops must accept generator values anywhere they accept iterable values.
- Generator values must expose the following minimum helper surface:

```incan
def map[U](self, f: (T) -> U) -> Generator[U]
def filter(self, f: (T) -> bool) -> Generator[T]
def take(self, n: int) -> Generator[T]
def collect(self) -> list[T]
```

- `.map()`, `.filter()`, and `.take()` are lazy: they return new generator values and must not materialize intermediate lists.
- `.collect()` is terminal and eager: it consumes the generator and returns a `list[T]`.
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

No separate lint or style guidance is required. Fixture `yield` and generator `yield` share the same surface mental model: produce a value, suspend, and later resume. The compiler only needs to make invalid context errors precise.

### Lowering model

The intended implementation strategy is to lower generator functions and generator expressions through a compiler-owned state-machine transformation or equivalent backend support. That lowering choice is not the language definition; the language contract is only that generators behave as lazy, resumable producers of `T`.

Generator helper methods should lower to backend-native lazy iterator machinery where the target supports it. For the Rust backend, `.map()`, `.filter()`, `.take()`, and `.collect()` should preserve Rust iterator-chain behavior rather than inserting eager intermediate collections.

### Interaction with existing features

- `for` loops consume generators the same way they consume other iterable sources.
- Recursive generators are valid as long as the yielded element type remains consistent.
- Generator expressions are the lazy counterpart to eager list-comprehension syntax rather than a separate collection feature.
- The broader `Iterator[T]` adapter API is a follow-up surface, not part of this RFC. RFC 088 owns that design and issue #127 tracks it.

### Compatibility / migration

The feature is additive. Existing functions, loops, and comprehensions keep their meaning.

## Alternatives considered

1. **Explicit `gen` keyword**
   - Clear, but more backend-shaped than Incan needs. Requiring `Generator[T]` plus `yield` already communicates intent.

2. **Dedicated `generator` declaration form**
   - Avoids overloading ordinary `def`, but splits the function surface for a feature that is still "a function producing values over time."

3. **Functions only, expressions later**
   - Not actually more principled. It would make the RFC weaker while still aiming at the same north-star generator model.

## Drawbacks

- `yield` now carries two meanings in the language, so diagnostics must be explicit.
- Generators introduce suspension semantics that users must learn alongside ordinary function control flow.
- Generator expressions add grammar and precedence surface that the language and tooling must handle carefully.

## Layers affected

- **Language surface**: `yield` must be valid in generator function bodies, and generator-expression syntax must be recognized.
- **Type system**: yielded expressions must match `Generator[T]`, and generator declarations must remain internally consistent.
- **Execution model**: implementations must preserve suspension points and lazy iteration semantics for both named and anonymous generator forms.
- **Stdlib / surface vocabulary**: the language must define the `Generator` type, its RFC 068 iteration-protocol conformance, and the minimum stable helper methods promised publicly.
- **Formatter / tooling**: multi-line generators should format predictably, and diagnostics should explain generator-specific behavior clearly.

## Implementation Plan

### Phase 1: Spec, parser, and AST

- Keep RFC 006 aligned with the settled design decisions: full generator-expression clause support, no `return value` in generators, and no extra fixture/generator lint policy.
- Extend parser and AST support for generator expressions using the existing comprehension clause surface.
- Preserve existing fixture `yield` parsing while separating generator-context validation into later semantic stages.

### Phase 2: Typechecker and diagnostics

- Add `Generator[T]` as a checked semantic type that satisfies the RFC 068 iteration protocol.
- Validate generator function bodies: yielded expressions must match `Generator[T]`, ordinary functions must reject `yield`, and generator functions must reject `return value`.
- Type-check generator expressions lazily while preserving the same binding and filter semantics as list comprehensions.
- Type-check the minimum generator helper surface: `.map()`, `.filter()`, `.take()`, and `.collect()`.

### Phase 3: Lowering, emission, and runtime surface

- Lower generator functions and generator expressions to lazy backend state-machine or iterator-equivalent behavior.
- Emit Rust code that preserves suspension, ordering, and laziness for generator values.
- Expose the minimum generator helper surface through stdlib-visible declarations and runtime support as needed.
- Ensure generator values work in `for` loops anywhere the RFC 068 iteration protocol is accepted.

### Phase 4: Tooling, docs, and release readiness

- Format generator functions and multi-clause generator expressions predictably.
- Add parser, typechecker, codegen snapshot, integration, and diagnostic tests for the full RFC surface.
- Update authored user-facing docs, release notes, and the active development version when implementation lands.

## Implementation log

### Spec / design

- [x] Decide that generator expressions support the full comprehension-clause surface.
- [x] Decide that `return value` in generators is rejected and reserved for future delegation or coroutine work.
- [x] Decide that fixture and generator `yield` need precise diagnostics but no extra lint/style guidance.

### Parser / AST / formatter

- [x] Parser: parse generator expressions with the full comprehension-clause surface.
- [x] AST: represent generator expressions without confusing them with eager list comprehensions.
- [x] Parser: preserve existing fixture `yield` parsing behavior.
- [x] Formatter: round-trip generator functions and multi-clause generator expressions stably.

### Typechecker / diagnostics

- [x] Type system: represent `Generator[T]` as the checked generator semantic type.
- [x] Protocols: make `Generator[T]` satisfy the RFC 068 iteration protocol.
- [x] Diagnostics: reject `yield` outside generator functions and fixture contexts.
- [x] Diagnostics: reject generator functions whose yielded values do not match `Generator[T]`.
- [x] Diagnostics: reject `return value` inside generator functions.
- [x] Typechecker: validate generator expressions with nested `for` clauses and trailing `if` filters.
- [x] Typechecker: validate `.map()`, `.filter()`, `.take()`, and `.collect()` on generator values.

### Lowering / emission / runtime

- [x] Lower generator functions to lazy resumable producer behavior.
- [x] Lower generator expressions to lazy generator values.
- [x] Emit Rust that preserves generator suspension, ordering, and laziness.
- [x] Runtime/stdlib: expose the `Generator[T]` surface and minimum helper methods.
- [x] Runtime/stdlib: ensure `.map()`, `.filter()`, and `.take()` stay lazy and `.collect()` is terminal.

### Tests

- [x] Parser tests for generator function `yield` and generator-expression clauses.
- [x] Typechecker tests for valid and invalid generator function return/yield combinations.
- [x] Typechecker tests for invalid `return value` in generator functions.
- [x] Typechecker tests for generator helper callback arity and return-type diagnostics.
- [x] Codegen snapshot tests for generator functions, generator expressions, and helper chains.
- [x] Integration tests for `for` loops over generators and `.map().filter().take().collect()` chains.
- [x] Regression tests that fixture `yield` behavior still works.

### Docs / release

- [x] Update generator language reference or tutorial docs.
- [x] Update release notes for the active `0.3` development line.
- [x] Bump the active `0.3.0-dev.N` version.

## Design decisions

1. `Generator[T]` is the user-facing semantic type for `yield`-produced values. It should satisfy the iteration protocol rather than being collapsed into a bare `Iterator[T]` return type.
2. The first stable generator helper surface is intentionally small: `.map()`, `.filter()`, `.take()`, and `.collect()`.
3. The broader iterator adapter surface belongs in RFC 088, tracked by issue #127.
4. Generator expressions support the full comprehension-clause surface rather than only a single `for` clause.
5. `return` without a value may terminate a generator early, but `return value` is rejected in RFC 006 and reserved for future generator delegation or coroutine work.
6. Fixture `yield` and generator `yield` do not need extra lint or style guidance beyond precise context-sensitive diagnostics.
