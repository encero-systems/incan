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

Use iterator adapters such as `map`, `filter`, and `take` to keep the pipeline lazy until the final `collect()`.

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

Generators support the same adapter and consumer surface as other iterator values, including `flat_map`, `skip`, `enumerate`, `zip`, `batch`, `count`, `fold`, `any`, `all`, `find`, `for_each`, and `sum`.

## Pair, reuse, and materialize iterators

Use the builtin `zip(left, right)` to pair two lists or iterators directly, and the method form `left.iter().zip(right.iter())` when `zip` is one stage in an existing iterator pipeline. Neither form builds a list by itself: iterate the result directly, or call `.collect()` when an owned `list[tuple[T, U]]` is required.

A terminal call such as `count()` or `collect()` consumes the iterator, and an iterator cannot be cloned. When the values are needed more than once, `collect()` them into a list and read the list, or build a second iterator from the source, such as another `.iter()` call on the list or another call of the generator function.

## Change a source after its reader is finished

An iterator or generator expression reads its list when it is consumed, not when it is created. Finish that read before changing, replacing, or moving the source list.

```incan
def main() -> None:
    mut xs = [1, 2, 3]
    it = xs.iter()
    xs.append(4)
    println(it.collect())
```

Checking this program reports the reader's origin, the conflicting change, and the later use that keeps the reader live:

```text
error[INCAN-T0001]: cannot change `xs` while `it` is still reading it
  --> example.incn:4:5
  |
3 |     it = xs.iter()
  |          --------- `it` reads `xs` from here
4 |     xs.append(4)
  |     ^^^^^^^^^^^^ `xs` is changed here
5 |     println(it.collect())
  |             -- `it` is used again here
= hint: finish using `it` before changing `xs`, or write `xs.clone().iter()` to iterate over a copy
```

Move the change after the terminal iterator operation when it should affect only later work. Use `xs.clone().iter()` when the lazy pipeline needs an independent snapshot instead. Apply the same rule to a generator expression: consume it before changing its source, or iterate over a cloned list. When returning or storing a generator, move an owned list into it or clone a borrowed source so the generator does not escape with a read of storage that stays elsewhere.

Control flow does not extend a reader past its last reachable use. A change after `collect()`, `sum()`, or another terminal use is allowed before ordinary calls, `if` or `match` branches, range and collection loops, `while` loops, and comprehensions. Conversely, a reader used again inside a loop remains live across the loop's back edge: a source change in one iteration is reported against the use reached on that or the next iteration.

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
