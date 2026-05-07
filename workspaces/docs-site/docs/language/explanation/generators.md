# Generators explained

Generators let a program describe a sequence without building the whole sequence up front. A generator value is a promise to produce items later, one at a time, when something consumes it.

That makes generators different from lists. A list owns all of its elements now. A `Generator[T]` owns the suspended work needed to produce `T` values later. This matters when the sequence is large, when the sequence may be unbounded, or when producing each value involves control flow that is clearer as a step-by-step function.

## The producer and the consumer

A generator has two sides:

- The producer contains `yield` points or a generator expression.
- The consumer is a `for` loop, a helper chain, or a terminal operation such as `collect()`.

```incan
def count_up(start: int) -> Generator[int]:
    mut current = start
    while True:
        yield current
        current += 1

def main() -> None:
    first_three = count_up(10).take(3).collect()
    println(first_three[0])
    println(first_three[1])
    println(first_three[2])
```

Calling `count_up(10)` creates the generator value. The body starts running only when `take(3).collect()` asks for items. Each `yield` returns one item to the consumer and then suspends the producer until the next item is requested.

## Generator functions and generator expressions

Use a generator function when the producer has meaningful statement-level control flow.

```incan
def non_empty(lines: List[str]) -> Generator[str]:
    for line in lines:
        cleaned = line.strip()
        if cleaned != "":
            yield cleaned
```

Use a generator expression when the producer is an inline transform or filter.

```incan
cleaned = (line.strip() for line in lines if line.strip() != "")
```

Both forms produce `Generator[T]`. The difference is how much structure the producer needs. If naming intermediate steps makes the logic easier to read, use a generator function. If the pipeline is short and local, use a generator expression.

## Laziness and helper chains

Iterator adapters such as `map`, `filter`, and `take` preserve laziness. They build another iterator pipeline instead of building intermediate lists.

```incan
def square(n: int) -> int:
    return n * n

def even(n: int) -> bool:
    return n % 2 == 0

def main() -> None:
    values = count_up(1).map(square).filter(even).take(4).collect()
    println(values[0])
    println(values[3])
```

In this example, the generator does not compute every square. It computes only as many values as `take(4)` needs, then `collect()` materializes those four items into a list.

Generators use the same iterator adapter surface as other iterator values. That means broader chains can combine helpers such as `flat_map`, `skip`, `enumerate`, `zip`, `batch`, and terminal consumers such as `count`, `fold`, `any`, `all`, `find`, `for_each`, and `sum`.

## Fixture `yield` and generator `yield`

Fixture `yield` and generator `yield` share the same surface idea: produce a value, suspend, and later resume. The declaration context gives the token its meaning. Fixture declarations use fixture semantics; functions returning `Generator[T]` use lazy iteration semantics.

You do not need a separate style rule for the two forms. Read the enclosing declaration first, then read `yield` as “produce this value and pause here.”

## When not to use a generator

Use a list comprehension when you genuinely want a list immediately. Use an ordinary loop when side effects are the main point and no lazy value needs to leave the function. Use a generator when the sequence itself is the value you want to pass around or compose.

## See also

- [Use generators for lazy pipelines](../how-to/generators.md)
- [Generators reference](../reference/generators.md)
- [Collection protocols](../reference/stdlib_traits/collection_protocols.md)
