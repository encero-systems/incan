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

## FallibleIterator (fallible iteration)

- **Syntax**: `for item in stream?:`
- **Hooks**:
    - `__iter__(self) -> Self`
    - `__next__(mut self) -> Result[Option[T], E]`
- **Trait**: `FallibleIterator[T, E]`

Import the trait when defining a custom fallible source or using it in a generic bound:

```incan
from std.derives.collection import FallibleIterator
```

A poll has three distinct outcomes:

| Result | Meaning |
| ------ | ------- |
| `Ok(Some(item))` | Yield one item. |
| `Ok(None)` | End normally. |
| `Err(error)` | Stop the current loop or terminal with a typed polling error. |

The loop-header `?` is required because the source can fail while the loop asks for its next item. It propagates the first polling error through the enclosing function's existing `Result` return type. It does not retry, discard, or turn errors into ordinary exhaustion.

```incan
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
```

Stream creation and stream polling are separate failure boundaries. Unwrap a setup `Result` before starting a fallible loop:

```incan
enum ImportError:
    Open(str)
    Read(ReadError)


def open_rows(path: str) -> Result[RowStream, str]:
    println(f"opening {path}")
    return Ok(RowStream(rows=[], index=0))


def import_row(row: Row) -> None:
    println(row.id)


def import_path(path: str) -> Result[None, ImportError]:
    stream = open_rows(path).map_err(ImportError.Open)?

    for row in stream.map_err(ImportError.Read)?:
        import_row(row)

    return Ok(None)
```

The combined spelling `for row in open_rows(path)?:` is rejected when `open_rows` returns a `Result` containing a fallible iterator. A `Result` containing an ordinary iterable keeps its established one-time unwrapping behavior.

### Lazy adapters

Every adapter constructs without polling, preserves successful item order, and forwards ordinary exhaustion. Generic adapters never poll again merely because the source emitted an error.

| Method | Result | Notes |
| ------ | ------ | ----- |
| `.map(f)` | `FallibleIterator[U, E]` | Transforms each successful item once. |
| `.filter(predicate)` | `FallibleIterator[T, E]` | Polls until an item passes, the source ends, or the source fails. |
| `.flat_map(f)` | `FallibleIterator[U, E]` | Expands each item into a `list[U]` and yields that list before polling the source again. |
| `.take(n)` | `FallibleIterator[T, E]` | Yields at most `n` successful items; a non-positive limit does not poll the source. |
| `.inspect(f)` | `FallibleIterator[T, E]` | Observes successful items without changing them. |
| `.map_err(f)` | `FallibleIterator[T, F]` | Transforms emitted errors without changing items or exhaustion. |
| `.inspect_err(f)` | `FallibleIterator[T, E]` | Observes emitted errors without changing them. |

Mapped item, mapped error, flattened item, and fold-accumulator types must support `Clone` under the current value-semantics contract.

Adapter callbacks may be ordinary functions, capturing closures, enum variant constructors with a compatible signature, or models that adopt the corresponding `Callable1` / `Callable2` trait.

This complete example defines its application types and callbacks instead of relying on ambient names:

```incan
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
| `.fold(initial, f)` | `Result[U, E]` | Accumulates successful items in order or returns the first source error. |

```incan
def collect_records[Pages with FallibleIterator[Page, str]](pages: Pages) -> Result[list[Record], SyncError]:
    return pages.flat_map(page_records).filter(is_relevant).map_err(SyncError.Fetch).collect()


def add_record_count(count: int, page: Page) -> int:
    return count + len(page.records)


def count_records[Pages with FallibleIterator[Page, str]](pages: Pages) -> Result[int, SyncError]:
    return pages.map_err(SyncError.Fetch).fold(0, add_record_count)
```

### Retry is explicit and source-owned

`FallibleIterator` has no generic retry adapter. Repeating a failed poll is safe only when the source knows whether its cursor advanced, whether the operation is idempotent, which errors are transient, and how attempts and backoff are counted.

A remote paginator that supports retry should therefore accept an explicit application or domain policy. Its own `__next__` implementation keeps the logical cursor unchanged while retrying, advances it only after a successful page, and emits one final error when the policy declines another attempt. `inspect_err`, `map_err`, `collect`, `fold`, and `for ...?:` outside that source observe only the final emitted error. Instrumentation for individual attempts belongs inside the paginator or policy.

The following design sketch is application code, not a shipped `std.http` API. The transport and wait callbacks make every effect visible, while the paginator alone owns cursor safety:

```incan
model FetchError:
    detail: str
    transient: bool


model RetryPolicy:
    max_attempts: int
    backoff_ms: int

    def should_retry(self, error: FetchError, attempt: int) -> bool:
        return error.transient and attempt < self.max_attempts

    def delay_ms(self, attempt: int) -> int:
        return self.backoff_ms * attempt


model RemotePage:
    records: list[Record]
    next_cursor: Option[str]


model RetryingPages with FallibleIterator[RemotePage, FetchError]:
    cursor: str
    done: bool
    retry: RetryPolicy
    fetch: (str) -> Result[RemotePage, FetchError]
    wait: (int) -> None

    def __next__(mut self) -> Result[Option[RemotePage], FetchError]:
        if self.done:
            return Ok(None)

        mut attempt = 1
        while true:
            request_page = self.fetch
            match request_page(self.cursor):
                Ok(page) =>
                    match page.next_cursor:
                        Some(next_cursor) => self.cursor = next_cursor
                        None => self.done = true
                    return Ok(Some(page))
                Err(error) =>
                    if not self.retry.should_retry(error, attempt):
                        return Err(error)
                    wait_before_retry = self.wait
                    wait_before_retry(self.retry.delay_ms(attempt))
                    attempt += 1
```

No retry is magical here. A failed call leaves `self.cursor` untouched. The policy classifies retryable failures and limits attempts, the injected `wait` callback owns backoff, and only a successful page advances or closes the cursor. If the policy declines, `__next__` emits the final `FetchError`; outer combinators and `for ...?:` observe exactly that one error.

## Bool (truthiness)

- **Syntax**: `if x:` / `while x:`
- **Hook**: `__bool__(self) -> bool`
- **Trait**: `Bool`

`Bool` is available for types whose domain has a clear truth value. It should not replace explicit checks for optionality, errors, emptiness, or named state. Prefer patterns such as `value is Some(x)`, `result is Ok(x)`, `len(items) > 0`, `name != ""`, or `connection.is_open` when those are what the code actually means.
