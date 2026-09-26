# Generators (Reference)

This page defines the generator syntax, typing rules, and helper surface. For the mental model, see [Generators explained](../explanation/generators.md). For practical recipes, see [Use generators for lazy pipelines](../how-to/generators.md).

## Type

`Generator[T]` is the type of a lazy producer that yields values of type `T`.

Generator values satisfy the collection iteration protocol. They can be consumed by `for` loops and by APIs that accept `Iterable[T]` or `Iterator[T]`.

## Generator functions

A function is a generator function when its body contains `yield` and its declared return type is `Generator[T]`.

```incan
def numbers() -> Generator[int]:
    yield 1
    yield 2
```

Rules:

- `yield expr` requires `expr` to type-check as `T`.
- `yield` is only valid in generator-function bodies and fixture contexts.
- A bare `return` ends the generator early.
- `return value` is rejected in generator functions.
- Declaring `Generator[T]` without a reachable `yield` is rejected unless the function returns an existing generator value.

## Returning an existing generator

An ordinary function may return an existing generator value without becoming a `yield`-based generator function.

```incan
def positives(xs: List[int]) -> Generator[int]:
    return (x for x in xs if x > 0)
```

## Generator expressions

A generator expression has the same clause shape as a comprehension and produces `Generator[T]`.

```incan
(expr for binding in iterable if condition)
```

Rules:

- `expr` determines the yielded element type `T`.
- Clauses run in source order.
- Each `for` clause introduces bindings for later clauses and for `expr`.
- Each `if` clause must type-check as `bool`.
- Nested `for` clauses and trailing `if` filters are supported.
- Generator expressions are lazy; list comprehensions remain the eager collection form.

## Iterator adapter methods

`Generator[T]` satisfies `Iterator[T]`, so generator values support the standard iterator adapter and consumer surface.

```incan
def map[U](self, f: (T) -> U) -> Iterator[U]
def filter(self, f: (T) -> bool) -> Iterator[T]
def take(self, n: int) -> Iterator[T]
def collect(self) -> list[T]
```

Lazy adapters return iterator values and do not materialize intermediate lists. Terminal consumers such as `collect`, `count`, `fold`, `reduce`, `any`, `all`, `find`, `for_each`, and `sum` consume the generator. See [Collection protocols](stdlib_traits/collection_protocols.md) for the full surface.

## Consumption

Advancing a generator resumes it until the next yielded value or until it finishes. Exhausting a generator ends iteration normally.

`list(gen)` and `set(gen)` consume a generator and collect the values it yields: `list` keeps every value in the order yielded, `set` keeps one of each distinct value.

```incan
def parities(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value % 2

println(len(list(parities(4))))  # 4
println(len(set(parities(4))))   # 2
```

Generator functions do not execute their body when the generator value is created. Execution starts when a consumer asks for the first item.
