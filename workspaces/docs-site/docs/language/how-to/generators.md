# Use generators for lazy pipelines

This guide shows common generator recipes. Use the [Generators reference](../reference/generators.md) when you need exact syntax and typing rules.

## Build a lazy filter

Use a generator function when the filtering logic benefits from names or multiple statements.

```incan
def non_empty(lines: List[str]) -> Generator[str]:
    for line in lines:
        cleaned = line.strip()
        if cleaned != "":
            yield cleaned

def main() -> None:
    for line in non_empty(["", "alpha", "  ", "beta"]):
        println(line)
```

The loop consumes one yielded item at a time. The function does not allocate a list of every non-empty line first.

## Transform and collect

Use `map`, `filter`, and `take` to keep the pipeline lazy until the final `collect()`.

```incan
def square(n: int) -> int:
    return n * n

def large(n: int) -> bool:
    return n > 10

def main() -> None:
    values = (n for n in [1, 2, 3, 4, 5]).map(square).filter(large).take(2).collect()
    println(values[0])
    println(values[1])
```

Use named functions for callbacks when the operation is shared or worth naming. Use a short closure when the logic is local and obvious.

## Limit an unbounded producer

Pair an unbounded generator with `take` before collecting.

```incan
def count_up(start: int) -> Generator[int]:
    mut current = start
    while True:
        yield current
        current += 1

def main() -> None:
    first_five = count_up(10).take(5).collect()
    println(first_five[0])
    println(first_five[4])
```

Do not call `collect()` on an unbounded generator unless an earlier helper limits it.

## Return a generator from a helper

Return a generator when the caller should decide whether to loop, transform, limit, or collect.

```incan
def positive_scores(scores: List[int]) -> Generator[int]:
    return (score for score in scores if score > 0)

def main() -> None:
    top_two = positive_scores([3, -1, 5, 8]).take(2).collect()
    println(top_two[0])
    println(top_two[1])
```

This keeps the helper composable. A caller that only needs iteration can use `for score in positive_scores(scores):` without materializing a list.

## Choose between lists and generators

Use a list comprehension when the next step needs random access, length, or repeated iteration over the full result. Use a generator when the next step can consume items once in order.

```incan
eager = [n * 2 for n in numbers]
lazy = (n * 2 for n in numbers)
```

The first expression builds a list now. The second expression builds a generator that produces doubled values later.

## See also

- [Generators explained](../explanation/generators.md)
- [Collections and iteration](../tutorials/book/08_collections_and_iteration.md)
- [Collection protocols](../reference/stdlib_traits/collection_protocols.md)
