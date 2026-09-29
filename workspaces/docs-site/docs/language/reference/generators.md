# Generators (Reference)

This page specifies `Generator[T]`, generator functions, generator expressions, the generator methods, and how a generator is consumed. Refusals are reported with `INCAN-T0001`.

## Type

`Generator[T]` is the type of a lazy producer that yields values of type `T`.

A `Generator[T]` value is accepted by a `for` loop and by a parameter of type `Iterable[T]` or `Iterator[T]`.

## Generator functions

A function is a generator function when its body contains `yield` and its declared return type is `Generator[T]`.

```incan
def numbers() -> Generator[int]:
    yield 1
    yield 2
```

Rules:

- `yield expr` requires the type of `expr` to be assignable to `T` as a returned value's type is to a return type; a value of a narrower numeric type is widened to `T` (see [Assignment between numeric types](numeric_semantics.md#assignment-between-numeric-types)).
- A bare `yield`, without a value, is refused in a generator function.
- `yield` is valid only in the body of a generator function or a fixture.
- A bare `return` ends the generator.
- `return value` is refused in a generator function.
- A function declared `-> Generator[T]` without a reachable `yield` is refused, unless it returns an existing generator value.
- Calling a generator function does not run its body. The body runs when a consumer asks for the first item, and each `yield` suspends it until the next item is asked for.

```incan
def words() -> Generator[str]:
    yield "a"                    # accepted
    yield                        # refused: a bare yield has no value
    yield 1                      # refused: 1 is not a str

def count() -> int:
    yield 1                      # refused: count is not a generator function
    return 1
```

## Returning an existing generator

An ordinary function may return an existing generator value without becoming a `yield`-based generator function.

```incan
def positives(xs: list[int]) -> Generator[int]:
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
- A generator expression may have several `for` clauses and trailing `if` filters.
- A generator expression is lazy; a list comprehension builds its list when it is evaluated.

## Methods

```incan
def map[U](self, f: (T) -> U) -> Generator[U]
def filter(self, f: (T) -> bool) -> Generator[T]
def take(self, n: int) -> Generator[T]
def collect(self) -> list[T]
```

- `map`, `filter` and `take` are lazy: each returns a generator and produces no item until that generator is consumed.
- `collect` consumes the generator.

`Generator[T]` satisfies `Iterator[T]`: a generator value has every iterator adapter and consumer of [Collection protocols](stdlib_traits/collection_protocols.md), such as `flat_map`, `skip`, `enumerate`, `zip` and `batch`. Terminal consumers, such as `count`, `fold`, `reduce`, `any`, `all`, `find`, `for_each` and `sum`, consume the generator.

## Consumption

Advancing a generator resumes it until the next yielded value or until it finishes. Exhausting a generator ends iteration normally.

`list(gen)` and `set(gen)` consume a generator and collect the values it yields: `list` keeps every value in the order yielded, `set` keeps one of each distinct value.

```incan
def parities(limit: int) -> Generator[int]:
    for value in range(limit):
        yield value % 2


def main() -> None:
    every: list[int] = list(parities(4))      # accepted
    distinct: set[int] = set(parities(4))     # accepted
```

## See also

- [Generators explained](../explanation/generators.md)
- [Use generators for lazy pipelines](../how-to/generators.md)
