# Collection protocols (Reference)

This page documents stdlib traits that model Python-like collection behavior.

Dunder hooks are the implementation methods. Explicit trait adoption names the capability when a bound, diagnostic, or reference page needs stable vocabulary.

## Contains (membership)

- **Syntax**: `item in collection` / `item not in collection`
- **Hook**: `__contains__(self, item: T) -> bool`
- **Trait**: `Contains[T]`

## Len (length)

- **Syntax**: `len(x)`
- **Hook**: `__len__(self) -> int`
- **Trait**: `Len`

## Iterable / Iterator (iteration)

- **Syntax**: `for x in y:`
- **Hooks**:
    - `__iter__(self) -> Iterator[T]`
    - `__next__(self) -> Option[T]`
- **Traits**:
    - `Iterable[T]`
    - `Iterator[T]`
    - `Sum[T]`

Iterator values also expose the standard lazy adapter surface:

The stdlib provides default Incan implementations for these protocol methods. The compiler may recognize the canonical methods and lower them through backend-native iterator chains when the generated behavior is equivalent.

| Method | Result | Notes |
| ------ | ------ | ----- |
| `.map(f)` | `Iterator[U]` | Yields `f(item)` for each input item. |
| `.filter(f)` | `Iterator[T]` | Keeps items where `f(item)` returns `true`. |
| `.flat_map(f)` | `Iterator[U]` | `f(item)` returns an `Iterable[U]`; each returned iterable is yielded before the next input item. |
| `.take(n)` | `Iterator[T]` | Yields at most the first `n` items. |
| `.skip(n)` | `Iterator[T]` | Drops at most the first `n` items and yields the rest. |
| `.chain(other)` | `Iterator[T]` | Yields the receiver, then `other`. |
| `.enumerate()` | `Iterator[tuple[int, T]]` | Pairs each item with a zero-based index. |
| `.zip(other)` | `Iterator[tuple[T, U]]` | Pairs items until either side is exhausted. |
| `.take_while(f)` | `Iterator[T]` | Stops before the first item where `f(item)` returns `false`. |
| `.skip_while(f)` | `Iterator[T]` | Drops items while `f(item)` returns `true`, then yields the rest. |
| `.batch(size)` | `Iterator[list[T]]` | Yields adjacent batches and keeps a final non-empty partial batch. |

Terminal methods consume the iterator:

| Method | Result | Notes |
| ------ | ------ | ----- |
| `.collect()` | `list[T]` | Collects all remaining items into a list. It does not take a target collection type. |
| `.count()` | `int` | Counts all remaining items. |
| `.any(f)` | `bool` | Short-circuits at the first item where `f(item)` returns `true`. |
| `.all(f)` | `bool` | Short-circuits at the first item where `f(item)` returns `false`. |
| `.find(f)` | `Option[T]` | Returns the first matching item, or `None`. |
| `.reduce(init, f)` | `U` | Repeatedly computes the next accumulator with `f(acc, item)`. |
| `.fold(init, f)` | `U` | Repeatedly computes the next accumulator with `f(acc, item)`. |
| `.for_each(f)` | `None` | Calls `f(item)` for each remaining item. |
| `.sum()` | `T` | Sums items when `T` supports `Sum[T]`. The implemented surface supports `int`, `float`, and newtypes over summable underlying types. Checked newtypes are constructed through their normal validation hook, so invalid summed values fail at runtime in the same way as explicit construction. |

Clone the iterator before a terminal call when the original iterator must still be used later.

`Generator[T]` implements this iteration surface. Generator functions and generator expressions can be used directly in `for` loops or passed to APIs that accept `Iterable[T]` / `Iterator[T]`.

## Bool (truthiness)

- **Syntax**: `if x:` / `while x:`
- **Hook**: `__bool__(self) -> bool`
- **Trait**: `Bool`

`Bool` is available for types whose domain has a clear truth value. It should not replace explicit checks for optionality, errors, emptiness, or named state. Prefer patterns such as `value is Some(x)`, `result is Ok(x)`, `len(items) > 0`, `name != ""`, or `connection.is_open` when those are what the code actually means.
