# Collection protocols (Reference)

This page specifies the standard-library traits behind membership, length, iteration, fallible iteration and truthiness: each protocol's syntax, hook methods, trait, and the methods the trait provides. The traits are declared in `std.derives.collection`.

A type supports a protocol's syntax by defining the protocol's hook methods; adopting the protocol's trait is not required for the syntax.

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
    - `__next__(mut self) -> Option[T]`
- **Traits**:
    - `Iterable[T]`
    - `Iterator[T]`
    - `Sum[T]`

`Iterable[T]` and `Iterator[T]` provide `iter(self) -> Iterator[T]`. `Iterator[T]` provides these lazy adapters as default methods:

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
| `.batch(size)` | `Iterator[list[T]]` | Yields adjacent batches and keeps a final non-empty partial batch. A literal `size` that is not greater than zero is refused (`INCAN-T0001`). |

The builtin `zip(left, right)`, also spelled `std.builtins.zip(left, right)`, pairs two lists, frozen lists or `Iterator[T]` values. It returns the same lazy `Iterator[tuple[T, U]]` as `left.iter().zip(right.iter())` and stops as soon as either input is exhausted. Neither form builds a list.

```incan
def is_visible(name: str) -> bool:
    return name != ""

def main() -> None:
    names = ["ada", "lin"]
    scores = [3, 5]
    for name, score in zip(names, scores):
        println(f"{name}: {score}")

    ranked: list[tuple[str, int]] = names.iter()
        .filter(is_visible)
        .zip(scores.iter())
        .collect()
```

Terminal methods consume the iterator. Reading an iterator binding again after a terminal call is refused (`INCAN-T0001`).

| Method | Result | Notes |
| ------ | ------ | ----- |
| `.collect()` | `list[T]` | Collects all remaining items into a list. It takes no argument. |
| `.count()` | `int` | Counts all remaining items. |
| `.any(f)` | `bool` | Short-circuits at the first item where `f(item)` returns `true`. |
| `.all(f)` | `bool` | Short-circuits at the first item where `f(item)` returns `false`. |
| `.find(f)` | `Option[T]` | Returns the first matching item, or `None`. |
| `.reduce(init, f)` | `U` | Repeatedly computes the next accumulator with `f(acc, item)`. |
| `.fold(init, f)` | `U` | Repeatedly computes the next accumulator with `f(acc, item)`. |
| `.for_each(f)` | `None` | Calls `f(item)` for each remaining item. |
| `.sum()` | `T` | Sums the items of an `int`, `float` or summable-newtype iterator, or of a type that adopts `Sum[T]`. Another item type is refused (`INCAN-T0001`). A checked newtype's sum is constructed through its validation hook. |

`Sum[T]` declares `sum(cls, items: Iterator[T]) -> Self`.

`Generator[T]` implements this iteration surface: a generator is iterated by `for`, is accepted where an `Iterable[T]` or `Iterator[T]` is expected, and has the adapters and terminals above.

## FallibleIterator (fallible iteration)

- **Syntax**: `for item in stream?:`
- **Hooks**:
    - `__iter__(self) -> Self`, with a default body
    - `__next__(mut self) -> Result[Option[T], E]`
- **Trait**: `FallibleIterator[T, E]`

The trait is not in the prelude. Adopting it (`with FallibleIterator[T, E]`) or naming it in a generic bound needs it imported:

```incan
from std.derives.collection import FallibleIterator
```

Adopting it without the import is refused (`INCAN-T0001`).

Calling an adapter or a terminal needs no import: the methods are available on an adopter and on the `FallibleIterator[U, E]` value an adapter returns, so `numbers().map(double).collect()` is accepted in a module that does not import `FallibleIterator`.

A poll has three outcomes:

| Result | Meaning |
| ------ | ------- |
| `Ok(Some(item))` | Yield one item. |
| `Ok(None)` | End normally. |
| `Err(error)` | Stop the current loop or terminal with a typed polling error. |

`for item in stream?:` loops over a fallible source. At the first `Err(error)`, the loop returns `Err(error)` from the enclosing function. The loop does not retry, discard an error, or treat it as the end of the stream. Each of these is refused (`INCAN-T0001`):

- `for item in stream:` without the `?` over a fallible source;
- `for item in stream?:` in a function that does not return a `Result`;
- `for item in stream?:` in a function whose `Result` error type is not the stream's error type;
- `for row in open_rows(path)?:` where `open_rows` returns a `Result` holding a fallible iterator.

`for x in make()?:`, where `make` returns a `Result` holding an ordinary iterable, unwraps the `Result` once.

```incan
from std.derives.collection import FallibleIterator

model Row:
    id: int


enum ReadError:
    Source(str)


model RowStream with FallibleIterator[Row, ReadError]:
    rows: list[Row]
    index: int

    def __next__(mut self) -> Result[Option[Row], ReadError]:
        if self.index >= len(self.rows):
            return Ok(None)
        row = self.rows[self.index]
        self.index += 1
        return Ok(Some(row))


def process(row: Row) -> None:
    println(row.id)


def consume[Rows with FallibleIterator[Row, ReadError]](rows: Rows) -> Result[int, ReadError]:
    mut count = 0
    for row in rows?:
        process(row)
        count += 1
    return Ok(count)


enum ImportError:
    Open(str)
    Read(ReadError)


def open_rows(path: str) -> Result[RowStream, str]:
    println(f"opening {path}")
    return Ok(RowStream(rows=[], index=0))


def import_path(path: str) -> Result[None, ImportError]:
    stream = open_rows(path).map_err(ImportError.Open)?

    for row in stream.map_err(ImportError.Read)?:
        process(row)

    return Ok(None)
```

### Lazy adapters

An adapter polls nothing when it is created. It keeps the order of successful items and ends when its source ends. After the source emits an error, an adapter does not poll it again.

| Method | Result | Notes |
| ------ | ------ | ----- |
| `.map(f)` | `FallibleIterator[U, E]` | Transforms each successful item once. |
| `.filter(f)` | `FallibleIterator[T, E]` | Polls until an item passes, the source ends, or the source fails. |
| `.flat_map(f)` | `FallibleIterator[U, E]` | Expands each item into a `list[U]` and yields that list before polling the source again. |
| `.take(n)` | `FallibleIterator[T, E]` | Yields at most `n` successful items; a limit that is not positive does not poll the source. |
| `.inspect(f)` | `FallibleIterator[T, E]` | Observes successful items without changing them. |
| `.map_err(f)` | `FallibleIterator[T, F]` | Transforms emitted errors without changing items or exhaustion. |
| `.inspect_err(f)` | `FallibleIterator[T, E]` | Observes emitted errors without changing them. |

The mapped item type, the mapped error type, the flattened item type and the fold accumulator type implement `Clone`.

A callback is an ordinary function, a capturing closure, an enum variant constructor with a compatible signature, or a model that adopts the corresponding `Callable1` or `Callable2` trait.

```incan
from std.derives.collection import FallibleIterator

model Record:
    id: str
    active: bool


model Page:
    records: list[Record]


enum SyncError:
    Fetch(str)
    Store(str)


def page_records(page: Page) -> list[Record]:
    return page.records


def is_relevant(record: Record) -> bool:
    return record.active


def record_seen(record: Record) -> None:
    println(f"received {record.id}")


def store(record: Record) -> Result[None, str]:
    println(f"stored {record.id}")
    return Ok(None)


def sync_records[Pages with FallibleIterator[Page, str]](pages: Pages) -> Result[int, SyncError]:
    mut stored = 0

    for record in pages.flat_map(page_records).filter(is_relevant).inspect(record_seen).map_err(SyncError.Fetch)?:
        store(record).map_err(SyncError.Store)?
        stored += 1

    return Ok(stored)
```

### Fallible terminals

| Method | Result | Notes |
| ------ | ------ | ----- |
| `.collect()` | `Result[list[T], E]` | Returns all successful items in order or the first source error; a partial list is not returned. |
| `.fold(init, f)` | `Result[U, E]` | Accumulates successful items in order or returns the first source error. |

With the declarations of the previous example:

```incan
def collect_records[Pages with FallibleIterator[Page, str]](pages: Pages) -> Result[list[Record], SyncError]:
    return pages.flat_map(page_records).filter(is_relevant).map_err(SyncError.Fetch).collect()


def add_record_count(count: int, page: Page) -> int:
    return count + len(page.records)


def count_records[Pages with FallibleIterator[Page, str]](pages: Pages) -> Result[int, SyncError]:
    return pages.map_err(SyncError.Fetch).fold(0, add_record_count)
```

`FallibleIterator` has no retry adapter. An adapter, a terminal or a `for ...?:` loop sees only the errors the source emits. For a source that retries its own polls, see [Retry inside a fallible iterator](../../how-to/error_handling_recipes.md#pattern-retry-inside-a-fallible-iterator).

## Bool (truthiness)

- **Syntax**: `if x:` / `while x:`
- **Hook**: `__bool__(self) -> bool`
- **Trait**: `Bool`

A condition over an `Option` or a `Result` is refused (`INCAN-T0001`).
