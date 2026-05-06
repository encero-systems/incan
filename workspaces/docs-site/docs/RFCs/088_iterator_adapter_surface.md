# RFC 088: Iterator adapter surface

- **Status:** In Progress
- **Created:** 2026-05-04
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 006 (Python-style generators)
    - RFC 035 (first-class named function references)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 070 (Result combinators)
- **Issue:** #127
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC standardizes a general iterator adapter surface for `Iterator[T]`, including lazy transformation methods such as `.map()`, `.filter()`, `.flat_map()`, `.skip()`, `.chain()`, `.enumerate()`, `.zip()`, `.take_while()`, `.skip_while()`, and `.batch()`, plus terminal consumers such as `.collect()`, `.count()`, `.any()`, `.all()`, `.find()`, and `.fold()`. RFC 006 defines the minimum helper surface needed for generator ergonomics; this RFC defines the broader API that should apply to all iterator values, including generators, collection iterators, range-like values, and user-defined iterator types.

## Core model

1. **Iterator adapters are protocol-level methods:** values satisfying `Iterator[T]` should expose a common adapter surface rather than relying on ad hoc collection-specific helpers.
2. **Adapters are lazy:** transformation methods return new iterator values and must not allocate intermediate lists as part of their language-level semantics.
3. **Consumers are terminal:** consumer methods consume an iterator and return a realized value such as a list, count, boolean, optional element, or folded accumulator.
4. **Generators are one iterator source:** `Generator[T]` from RFC 006 participates through the same iterator protocol, but this RFC is not generator-specific.
5. **Callable values are the transformation boundary:** adapter callbacks use ordinary Incan callable types and named function references from RFC 035; this RFC does not introduce a second closure syntax.
6. **The backend may optimize freely:** the language contract is lazy, ordered iterator behavior; implementations may lower to target-native iterator chains when semantics match.

## Motivation

RFC 068 gives Incan a static iteration protocol, and RFC 006 gives users lazy producer values through generators. Without a common adapter surface, users can consume those values in `for` loops, but they cannot express ordinary lazy transformation pipelines without returning to mutable accumulators and explicit loop scaffolding.

That gap is especially visible for data transformation code. Filtering rows, mapping values, taking bounded prefixes from large or unbounded streams, zipping related streams, flattening nested iterators, and folding into summaries are standard iterator operations. If each library invents local helper names or materializes intermediate lists, Incan loses the main ergonomic and performance reason to expose lazy iteration in the first place.

The language already has the pieces needed to make the surface coherent: static `Iterator[T]` and `Iterable[T]` vocabulary, first-class callable values, named function references, and a generator model. This RFC connects those pieces into a standard iterator API.

## Goals

- Standardize a core set of lazy iterator adapter methods on `Iterator[T]`.
- Standardize a core set of terminal iterator consumer methods on `Iterator[T]`.
- Make the broader adapter surface apply to generators and non-generator iterator values consistently.
- Preserve left-to-right evaluation order and laziness for adapter chains.
- Keep method names aligned with Rust iterator vocabulary where that vocabulary is already common and precise.
- Define `.batch()` as an Incan data-processing convenience with explicit partial-batch behavior.
- Require useful type diagnostics when callbacks have incompatible arity or return types.

## Non-Goals

- Changing the generator model from RFC 006.
- Changing the iteration protocol from RFC 068.
- Introducing a pipeline operator such as `|>` or `>>`.
- Introducing a new closure syntax or block-lambda syntax.
- Defining async iterator adapters or async generators.
- Defining parallel, concurrent, or distributed iterator execution.
- Guaranteeing a particular Rust implementation type in generated code.
- Standardizing every possible iterator helper, such as `peekable`, `cycle`, `scan`, `partition`, `reduce`, `min`, `max`, or sorting helpers.

## Guide-level explanation

Use iterator adapters when the code describes a value pipeline more clearly than a mutable accumulator.

```incan
def is_active(user: User) -> bool:
    return user.is_active

def user_name(user: User) -> str:
    return user.name.upper()

names: list[str] = users.iter()
    .filter(is_active)
    .map(user_name)
    .collect()
```

Adapters compose without realizing intermediate lists. The example above filters and maps lazily; `.collect()` is the point where the final list is produced.

Iterators over unbounded sources stay bounded when the consumer asks for a bounded prefix:

```incan
first_ten_squares: list[int] = naturals()
    .map((n) => n * n)
    .take(10)
    .collect()
```

Adapters can combine multiple streams:

```incan
pairs: list[tuple[str, int]] = names.iter()
    .zip(scores.iter())
    .collect()
```

Nested iterator-producing callbacks can be flattened:

```incan
words: list[str] = documents.iter()
    .flat_map(document_words)
    .filter(is_searchable)
    .collect()
```

Terminal methods are for summaries or predicates:

```incan
has_admin: bool = users.iter().any(is_admin)
all_valid: bool = rows.iter().all(validate_row)
first_error: Option[Error] = results.iter().find(is_error)
total: int = numbers.iter().fold(0, add)
```

Batching groups adjacent elements into fixed-size lists and preserves a final partial batch:

```incan
batches: list[list[Event]] = events.iter()
    .batch(100)
    .collect()
```

If `events` contains 250 elements, the result contains two batches of 100 and one final batch of 50.

## Reference-level explanation

### Adapter methods

`Iterator[T]` must expose these lazy adapter methods:

```incan
def map[U](self, f: (T) -> U) -> Iterator[U]
def filter(self, f: (T) -> bool) -> Iterator[T]
def flat_map[U](self, f: (T) -> Iterable[U]) -> Iterator[U]
def take(self, n: int) -> Iterator[T]
def skip(self, n: int) -> Iterator[T]
def chain(self, other: Iterator[T]) -> Iterator[T]
def enumerate(self) -> Iterator[tuple[int, T]]
def zip[U](self, other: Iterator[U]) -> Iterator[tuple[T, U]]
def take_while(self, f: (T) -> bool) -> Iterator[T]
def skip_while(self, f: (T) -> bool) -> Iterator[T]
def batch(self, size: int) -> Iterator[list[T]]
```

Adapter methods must be lazy. Calling an adapter must not consume elements beyond the amount required to construct the next output element when the adapted iterator is advanced.

Adapter chains must preserve source order unless the method contract says otherwise. This RFC does not define any reordering adapter.

### Terminal consumer methods

`Iterator[T]` must expose these terminal methods:

```incan
def collect(self) -> list[T]
def count(self) -> int
def any(self, f: (T) -> bool) -> bool
def all(self, f: (T) -> bool) -> bool
def find(self, f: (T) -> bool) -> Option[T]
def reduce[U](self, init: U, f: (U, T) -> U) -> U
def fold[U](self, init: U, f: (U, T) -> U) -> U
def for_each(self, f: (T) -> None) -> None
def sum(self) -> T  # when T supports Sum[T]
```

Terminal methods consume the receiver. After a terminal method consumes an iterator value, the program must not rely on that same iterator value yielding additional elements unless the iterator type documents reusable iteration separately.

If code needs to preserve an iterator value for later use, clone it before the terminal call:

```incan
remaining = rows.iter()
copy = remaining.clone()

error_count: int = copy.filter(is_error).count()

for row in remaining:
    handle(row)
```

### Callback typing

Callback arguments must type-check against the element type yielded by the receiver at that point in the chain.

`map` callbacks must return the output element type. `filter`, `any`, `all`, `find`, `take_while`, and `skip_while` callbacks must return `bool`. `flat_map` callbacks must return `Iterable[U]`, including iterator values and collection-like values that satisfy the iteration protocol. `reduce` and `fold` callbacks must accept the accumulator and current element and must return the next accumulator value.

When callback arity or return type is incompatible with the method contract, the compiler must emit a type error at the adapter call site.

### Evaluation order

The receiver expression must be evaluated before method arguments. Method arguments must be evaluated left to right. Adapter callbacks must be invoked only as the resulting iterator is advanced.

For `.any()`, `.all()`, `.find()`, `.take_while()`, and `.skip_while()`, evaluation must short-circuit according to the method contract.

### Method behavior

`.map(f)` yields `f(item)` for each input item.

`.filter(f)` yields each input item for which `f(item)` returns `true`.

`.flat_map(f)` applies `f` to each input item and yields all elements from each returned iterable before advancing to the next input item.

`.take(n)` yields at most the first `n` elements. If `n` is less than or equal to zero, it yields no elements.

`.skip(n)` discards at most the first `n` elements and yields the rest. If `n` is less than or equal to zero, it yields all elements.

`.chain(other)` yields all remaining elements from the receiver, then all remaining elements from `other`.

`.enumerate()` yields `(index, item)` pairs, starting at index `0` and increasing by one for each yielded item.

`.zip(other)` yields pairs from the receiver and `other` until either iterator is exhausted.

`.take_while(f)` yields input items until `f(item)` first returns `false`, then stops.

`.skip_while(f)` discards input items while `f(item)` returns `true`, then yields that first non-skipped item and all following items.

`.batch(size)` yields adjacent `list[T]` batches with at most `size` elements. The final batch is yielded when it is non-empty even if it contains fewer than `size` elements. A `size` less than or equal to zero must be rejected statically when known at compile time and otherwise must produce the same user-facing error category as other invalid numeric standard-library arguments.

`.collect()` consumes all remaining elements into a `list[T]`. This RFC does not define target-collection overloads for `.collect()`.

`.count()` consumes all remaining elements and returns the number of elements consumed.

`.sum()` consumes all remaining summable elements and returns their sum. This RFC supports `int`, `float`, and newtypes over summable underlying types.

The capability is named by `Sum[T]`, mirroring Rust's `std::iter::Sum`: a type can produce `Self` from an iterator of `T` items. For this RFC's implemented surface, primitive numeric types provide the base behavior. Newtypes lift the underlying summation result through normal construction, so checked newtypes still run their selected `from_underlying`/`from_*` validation hook and may fail at runtime if the summed underlying value violates the newtype invariant.

`.any(f)` returns `true` after the first item for which `f(item)` returns `true`; otherwise it returns `false`.

`.all(f)` returns `false` after the first item for which `f(item)` returns `false`; otherwise it returns `true`.

`.find(f)` returns `Some(item)` for the first item for which `f(item)` returns `true`; otherwise it returns `None`.

`.reduce(init, f)` starts with `init`, applies `f(acc, item)` for each item, and returns the final accumulator.

`.fold(init, f)` starts with `init`, applies `f(acc, item)` for each item, and returns the final accumulator.

`.for_each(f)` calls `f(item)` for each remaining item and returns `None`.

## Design details

### Syntax

This RFC adds no new syntax. Iterator adapters are ordinary method calls on iterator values.

### Semantics

The semantic center is a shared lazy iterator API. The adapter methods transform iteration; terminal methods realize or summarize it. The API must be available through the same method-resolution surface users already use for ordinary values.

### Interaction with generators

RFC 006 standardizes `.map()`, `.filter()`, `.take()`, and `.collect()` as the minimum helper surface needed for generator ergonomics. This RFC generalizes that model to `Iterator[T]` and adds the broader adapter and terminal methods. A `Generator[T]` should therefore support the RFC 006 minimum surface directly and support the full RFC 088 surface through its iterator-protocol participation once this RFC is implemented.

### Interaction with collection iteration

Collections that expose iterator values through `.iter()` should receive the same adapter surface as any other `Iterator[T]`. This RFC does not require every collection type itself to expose adapter methods directly; the canonical entry point for collection pipelines is the collection's iterator-producing operation.

### Interaction with callable values

Callbacks use ordinary callable values. Named function references, closures, callable objects, and future callable-producing features should work when their static type satisfies the adapter contract. This RFC does not require generic function references without explicit instantiation if the existing callable rules reject them.

### Interaction with Rust interop

Rust-backed iterator values may use Rust iterator machinery when their Incan-facing behavior matches this RFC. The public contract remains the Incan method surface and type rules, not the exact generated Rust adapter type.

### Compatibility / migration

This feature is additive. Existing `for` loops, comprehensions, collection methods, and generator usage keep their meaning. Code that already defines methods with the same names on custom iterator types may need to align those methods with this RFC's signatures if the type adopts or is treated as `Iterator[T]`.

## Alternatives considered

- **Keep only the RFC 006 generator helper surface**: rejected because generators are not the only iterator source, and a generator-only API would fragment the language surface.
- **Free functions such as `map(iter, f)` and `filter(iter, f)`**: rejected because method chaining follows the value flow left to right and avoids deeply nested calls in multi-step pipelines.
- **A pipeline operator instead of iterator methods**: rejected because a pipeline operator would still need standard transformation functions or methods underneath it.
- **List comprehensions only**: rejected because comprehensions are eager collection builders and do not express reusable lazy chains, short-circuiting consumers, zipping, flattening, or batching as directly.
- **Backend-specific inheritance from Rust's full iterator API**: rejected because Incan needs a stable language contract and portable diagnostics rather than an opportunistic mirror of one backend's entire method set.

## Drawbacks

- The standard iterator surface becomes larger and users must learn when a chain is clearer than a loop.
- Poor diagnostics around callback types could make adapter chains harder to debug than explicit loops.
- Lazy ownership and borrowing behavior can be subtle when callbacks capture values or when iterator items are expensive to move.
- Method names such as `map`, `filter`, and `fold` are common enough that custom iterator-like APIs may need migration or clearer conformance rules.
- `.batch()` is useful for data processing but is less universal than the other adapters, so it adds policy surface around partial batches and invalid sizes.

## Implementation architecture

*(Non-normative.)* The stdlib iterator protocol should be the source of truth for this surface. `Iterator[T]` should expose Incan default methods and Incan adapter carrier types for the common lazy adapters and terminal consumers, with compiler support only where needed for method resolution, type substitution, consumption diagnostics, builtin collection `.iter()` bridging, and backend emission. The Rust backend may lower straightforward adapter chains to Rust iterator chains when doing so preserves Incan evaluation order, callback behavior, item typing, and error behavior; that lowering is an optimization/bridge, not a second semantic definition.

## Layers affected

- **Typechecker / Symbol resolution**: iterator method lookup, generic substitution, callback arity checks, callback return-type checks, and terminal-consumption rules must be validated.
- **IR Lowering**: adapter and terminal calls must preserve lazy semantics, evaluation order, and short-circuiting behavior.
- **Emission**: generated code should preserve the Incan iterator contract and may use backend-native iterator chains when equivalent.
- **Stdlib / Runtime (`incan_stdlib`)**: iterator trait declarations and any runtime bridge helpers must expose the standard method surface.
- **LSP / Tooling**: completions, hovers, and diagnostics should expose the adapter signatures and explain callback mismatch errors.
- **Documentation**: iterator, generator, collection, and data-transformation docs should teach lazy adapters separately from eager comprehensions.

## Implementation Plan

### Phase 1: Iterator contract and stdlib surface

- Add the RFC 088 adapter and terminal method declarations to the `Iterator[T]` trait surface.
- Dogfood the protocol by providing Incan default bodies and Incan adapter carrier types wherever the behavior can be expressed directly in stdlib code.
- Keep `.batch()` in the core iterator adapter surface with final partial-batch preservation and invalid-size behavior defined by this RFC.
- Keep `.collect()` fixed to `list[T]`; target-specific collection is deferred.
- Treat `flat_map` callbacks as returning `Iterable[U]`, with iterator conversion handled by the normal iteration protocol.

### Phase 2: Typechecker and diagnostics

- Resolve RFC 088 methods on iterator values and values accepted through the iteration protocol.
- Substitute generic method types through adapter chains so each chained receiver has the expected element type.
- Validate callback arity and return type for every adapter and terminal consumer.
- Enforce terminal consumption semantics at the language contract level and reject obvious same-binding reuse when the compiler can do so without broader linear ownership machinery.
- Emit focused diagnostics at the adapter call site for missing iterator capability, incompatible callback shape, invalid batch size, and invalid terminal reuse when detected.

### Phase 3: Lowering, emission, and runtime support

- Lower adapter and terminal calls so lazy adapters stay lazy and terminal consumers consume the iterator.
- Treat compiler-recognized RFC 088 methods as the optimized lowering path for canonical stdlib iterator defaults, not as the sole implementation of the protocol.
- Emit Rust iterator chains when they preserve Incan evaluation order, callback behavior, ownership behavior, and error behavior.
- Add runtime or generated helper support for behavior that is not covered directly by Rust iterator adapters, especially `.batch()`.
- Preserve current `for` loop, comprehension, builtin collection, and generator behavior while adding the adapter surface.

### Phase 4: Tests, docs, and release readiness

- Add typechecker coverage for valid chains, element-type propagation, callback mismatch diagnostics, invalid batch sizes, and terminal consumption behavior.
- Add codegen snapshot and integration coverage for representative adapter chains, terminal consumers, `flat_map` over iterable callback results, and batching.
- Update authored user-facing docs for iterator pipelines and how they differ from eager comprehensions.
- Update release notes and bump the active `0.3.0-dev.N` version when implementation lands.

## Progress Checklist

### Spec / design

- [x] Keep `.batch()` in the core RFC 088 surface.
- [x] Keep `.collect()` fixed to `list[T]` for this RFC.
- [x] Accept `Iterable[U]` results from `flat_map` callbacks.
- [x] Define terminal methods as consuming iterators and defer full linear ownership enforcement beyond obvious compiler-detectable reuse.
- [x] Make the stdlib iterator protocol carry default Incan implementations for the RFC 088 adapter/consumer surface where possible.

### Stdlib / trait surface

- [x] Add lazy adapter declarations to `Iterator[T]`.
- [x] Add terminal consumer declarations to `Iterator[T]`.
- [x] Expose `.batch()` with final partial-batch behavior.
- [x] Keep `flat_map` typed against iterable callback results.

### Typechecker / diagnostics

- [x] Resolve RFC 088 methods on iterator values.
- [x] Propagate element types through adapter chains.
- [x] Validate callback arity and return types.
- [x] Validate `.batch(size)` rejects invalid known sizes and reports runtime invalid sizes consistently.
- [x] Detect and reject obvious same-binding reuse after terminal consumption where feasible.
- [x] Emit adapter-specific diagnostics instead of generic missing-method errors.

### Lowering / emission / runtime

- [x] Lower lazy adapters without intermediate list materialization.
- [x] Lower terminal consumers as consuming operations.
- [x] Emit Rust iterator chains for standard adapters where equivalent.
- [x] Add runtime/generated support for `.batch()`.
- [x] Preserve existing collection iteration, comprehensions, and `for` loops.

### Tests

- [x] Typechecker tests for every adapter and terminal consumer.
- [x] Typechecker tests for callback mismatch diagnostics.
- [x] Typechecker tests for adapter chain element-type propagation.
- [x] Tests for `flat_map` callbacks returning iterable values.
- [x] Tests for `.batch()` partial final batch and invalid size.
- [x] Codegen/emission tests for representative lazy chains.
- [x] Integration tests for end-to-end iterator pipelines.
- [x] Regression tests proving eager list comprehensions keep existing behavior.

### Docs / release

- [x] Update iterator or collection user docs with lazy pipeline examples.
- [ ] Update generator docs only where RFC 088 changes cross-references or examples.
- [x] Add release notes for the active `0.3` development line.
- [x] Bump the active `0.3.0-dev.N` version.

## Design Decisions

- `.batch()` remains part of RFC 088 because fixed-size batching is a first-order data-processing use case and the RFC defines a narrow, predictable contract: preserve a final partial batch and reject invalid sizes.
- `.collect()` returns `list[T]` in this RFC. Target-specific collection, such as collecting into sets, maps, or custom containers, is deferred until the collection-construction story is mature enough to specify cleanly.
- `flat_map` accepts callbacks returning `Iterable[U]`. Requiring callbacks to return only `Iterator[U]` would force noisy `.iter()` calls in ordinary cases such as flattening lists or custom iterable fields.
- Terminal consumers consume the iterator. Authors who need to preserve a reusable value should clone or otherwise create a separate iterator source before consuming. Full linear type-system enforcement is deferred; this RFC still allows the compiler to reject obvious same-binding reuse after a terminal consumer.
