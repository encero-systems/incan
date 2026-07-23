# RFC 115: Fallible iteration and combinators

- **Status:** Implemented
- **Created:** 2026-07-22
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 035 (first-class function references)
    - RFC 066 (`std.http` client surface and explicit retry policy)
    - RFC 068 (protocol hooks for core language syntax)
    - RFC 070 (`Result` combinators)
    - RFC 088 (`Iterator` adapter surface)
- **Issue:** https://github.com/encero-systems/incan/issues/579
- **RFC PR:** —
- **Written against:** v0.5
- **Shipped in:** v0.5

## Summary

This RFC defines `FallibleIterator[T, E]` for single-pass sources whose next poll can produce an item, reach ordinary exhaustion, or fail. It adds the explicit `for item in stream?:` consumption form, a coherent first family of lazy item and error adapters, and fallible consuming terminals. Generic fallible iteration never retries a failed poll: retry remains owned by a source that can define idempotency, cursor advancement, and recovery safely.

## Core model

Read this RFC as one foundation plus four mechanisms:

1. **Foundation:** `FallibleIterator[T, E]` names a single-pass source whose `__next__` hook returns `Result[Option[T], E]`.
2. **Explicit loop consumption:** `for item in stream?:` polls the fallible source and propagates an emitted error through the surrounding function's existing `Result` contract.
3. **Lazy adapters:** item transforms, selection, bounded consumption, observation, and error transforms preserve the source's single-pass behavior without polling eagerly.
4. **Fallible terminals:** `collect` and `fold` consume until exhaustion or return the first emitted error.
5. **Source-owned recovery:** a source may apply a retry policy internally, but generic fallible-iterator syntax and adapters never assume that repeating a failed poll is safe.

## Motivation

Bounded I/O currently requires callers to repeat a low-level `read_bytes`, empty-chunk, and error-propagation loop. A typed chunk stream should make EOF ordinary exhaustion while preserving I/O failures. Modeling each item as `Result[T, E]` would allow an ordinary loop to continue after an unhandled error and would make error propagation a body convention rather than a property of consuming the stream.

The initial reader design also exposes a broader API problem. A public generic protocol with only a concrete `ReaderChunks.map_err` method is not meaningfully reusable. Users need the same basic lazy composition they already expect from ordinary `Iterator`: transforming items, selecting items, flattening page-like batches, bounding work, observing values and errors, mapping domain errors, collecting results, and folding an accumulator.

Retry is deliberately different. After a failed file read, network request, parser poll, or device operation, the generic protocol cannot know whether state advanced, whether repeating the operation duplicates effects, or whether the error is transient. Retry therefore cannot be ambient loop behavior or an unconstrained generic adapter.

## Goals

- Define a statically checked `FallibleIterator[T, E]` protocol without requiring associated types.
- Make fallible loop consumption explicit through `for item in stream?:`.
- Preserve ordinary `for item in iterator:` behavior unchanged.
- Provide generic lazy `map`, `filter`, `flat_map`, `take`, `inspect`, `map_err`, and `inspect_err` adapters.
- Provide `collect` and `fold` terminals that return `Result`.
- Define exact item, exhaustion, error, laziness, ordering, and callback invocation rules.
- Keep retry and recovery source-owned and visible through explicit policy APIs.
- Support Incan-authored reader chunk streams and other domain-specific fallible sources without bespoke native backends.

## Non-Goals

- Async streams or an async iteration syntax.
- Native associated types, generic associated types, or opaque iterator return types.
- Automatic retry, backoff, or error recovery.
- Implementing Draft RFC 066 or standardizing one HTTP pagination framework.
- `try_map`, `or_else`, `chain`, `zip`, or complete parity with every RFC 088 adapter in this first family.
- Python-style truthiness or assignment expressions.
- Changing `Result` combinator semantics from RFC 070.

## Guide-level explanation

### Define a fallible source

A fallible source adopts `FallibleIterator[T, E]` and returns one of three states from `__next__`:

```incan
model RowStream with FallibleIterator[Row, ReadError]:
    cursor: int

    def __next__(mut self) -> Result[Option[Row], ReadError]:
        ...
```

- `Ok(Some(row))` yields one item.
- `Ok(None)` ends the stream normally.
- `Err(error)` reports a failed poll.

### Consume it explicitly

The loop header carries `?` because fetching the next item may fail:

```incan
def import_rows(rows: RowStream) -> Result[int, ReadError]:
    mut count = 0

    for row in rows?:
        import_row(row)
        count += 1

    return Ok(count)
```

The loop returns from `import_rows` when a poll produces `Err(ReadError)`. Ordinary exhaustion simply ends the loop.

### Compose successful items

Adapters are lazy and run only when the stream is polled:

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

`flat_map` expands each page into its records, `filter` selects records, `inspect` observes successful records, and `map_err` converts only source errors. The trailing `?` still propagates the mapped error because the adapters do not make the stream infallible.

### Bound work

`take` counts yielded items, not polling attempts:

```incan
for page in pages.take(100).map_err(ImportError.Fetch)?:
    import_page(page)
```

When the limit is reached, the adapter ends without polling the source again.

### Consume without a loop

Fallible terminals return `Result`:

```incan
records = pages.flat_map(page_records).filter(is_relevant).map_err(SyncError.Fetch).collect()?

total = measurements.map(measurement_value).fold(0, add_measurement)?
```

`collect` returns `Ok(list[T])` at ordinary exhaustion. `fold` returns `Ok(accumulator)` at ordinary exhaustion. Either terminal immediately returns the first `Err(E)` emitted by its source.

### Keep setup failure separate from polling failure

Creating a stream and polling it can fail at different times. Code should make both boundaries visible:

```incan
stream = open_rows(path).map_err(ImportError.Open)?

for row in stream.map_err(ImportError.Read)?:
    import_row(row)
```

The first `?` unwraps the one-time `Result` returned while opening the source. The loop-header `?` propagates errors emitted by later polls.

### Retry belongs to the source

A remote paginator may accept an explicit retry policy because it understands request idempotency and cursor state. Its `__next__` implementation must keep the current cursor unchanged while attempting the request, consult the policy after a failed attempt, wait according to the policy when another attempt is allowed, and advance the cursor only after a successfully decoded page. Errors that are non-retryable or remain after the attempt limit escape from `__next__`; outer `inspect_err`, `map_err`, terminals, and loop propagation see that final emitted error.

An application may then pass that paginator to the complete `sync_records` function above. The generic pipeline does not need to know how HTTP attempts work: it receives successful `Page` values or the final emitted error. Draft RFC 066 owns the future HTTP client and retry-policy vocabulary, so this RFC does not invent a `Client.pages`, `ApiPages`, or `RetryPolicy` API and does not claim those types have shipped.

## Reference-level explanation

### Protocol contract

`FallibleIterator[T, E]` must define:

```incan
pub trait FallibleIterator[T, E]:
    def __iter__(self) -> Self
    def __next__(mut self) -> Result[Option[T], E]
```

`__iter__` must return the same single-pass state. A source must use `Ok(Some(item))` for an item, `Ok(None)` for ordinary exhaustion, and `Err(error)` for a failed poll.

An implementation may define whether manually polling after an emitted error is meaningful. Generic adapters must not poll again on behalf of the caller merely because an error occurred. The loop form and consuming terminals stop at the first emitted error.

### Loop-header resolution

For `for pattern in expression?:`, the compiler must resolve exactly one of these forms:

1. If `expression` has type `Result[I, E]` and `I` supports ordinary iteration, `?` unwraps the setup result once and the loop uses ordinary `Iterator` polling.
2. Otherwise, if `expression` supports `FallibleIterator[T, E]`, `?` selects per-poll fallible iteration.
3. A `Result` whose successful value is itself fallibly iterable must be rejected in the loop header with guidance to unwrap setup into a local before starting the fallible loop.

`for pattern in expression:` without the header `?` must not consume a `FallibleIterator`. `for pattern in expression?:` must not reinterpret an ordinary non-`Result` iterator as fallible.

The surrounding function must return a `Result` with an error type compatible with `E` under the language's existing `?` rules.

### Lazy adapter rules

Every lazy adapter must:

- construct without polling its source;
- preserve source item order;
- poll only as required to satisfy the next adapter poll;
- return `Ok(None)` after its own exhaustion condition;
- forward or map source errors exactly as specified;
- avoid retrying an emitted source error.

Callback parameters use the canonical fixed-arity callable traits, so functions, capturing closures, compatible enum variant constructors, and explicit `Callable1` / `Callable2` adopters can share the same generic adapter surface.

#### `map`

```incan
def map[U with Clone, MapFn with (Clone, Callable1[T, U])](self, f: MapFn) -> FallibleIterator[U, E]
```

For each `Ok(Some(item))`, `map` must invoke `f` exactly once and return `Ok(Some(f(item)))`. It must pass `Ok(None)` and `Err(E)` through without invoking `f`.

#### `filter`

```incan
def filter[Predicate with (Clone, Callable1[T, bool])](self, predicate: Predicate) -> FallibleIterator[T, E]
```

`filter` must poll until it finds an item whose predicate returns `true`, reaches exhaustion, or receives an error. The predicate must run exactly once for each successful source item considered. A rejected item must not be yielded.

#### `flat_map`

```incan
def flat_map[U with Clone, Expand with (Clone, Callable1[T, list[U]])](self, f: Expand) -> FallibleIterator[U, E]
```

`flat_map` must invoke `f` once for each successful outer item, yield the returned list in order, exhaust that list before polling the outer source again, and forward outer exhaustion and errors. The first family deliberately matches RFC 088's list-returning adapter rather than introducing an associated or opaque inner-iterator type.

#### `take`

```incan
def take(self, count: int) -> FallibleIterator[T, E]
```

`take` must yield at most `count` successful items. A non-positive count must produce immediate ordinary exhaustion without polling the source. Errors do not count as yielded items and must be forwarded immediately.

#### `inspect`

```incan
def inspect[Observer with (Clone, Callable1[T, None])](self, f: Observer) -> FallibleIterator[T, E]
```

`inspect` must invoke `f` exactly once for each successful item and then return that item unchanged. It must not invoke `f` for exhaustion or errors.

#### `map_err`

```incan
def map_err[F with Clone, ErrorMap with (Clone, Callable1[E, F])](self, f: ErrorMap) -> FallibleIterator[T, F]
```

`map_err` must invoke `f` exactly once for each emitted `Err(E)` and return `Err(F)`. It must pass successful items and exhaustion through unchanged.

#### `inspect_err`

```incan
def inspect_err[ErrorObserver with (Clone, Callable1[E, None])](self, f: ErrorObserver) -> FallibleIterator[T, E]
```

`inspect_err` must invoke `f` exactly once for each emitted error and then return the same error unchanged. It must not invoke `f` for successful items or exhaustion.

### Terminal rules

Terminals consume the source. They must stop after ordinary exhaustion or the first error and must not poll again afterward.

#### `collect`

```incan
def collect(mut self) -> Result[list[T], E]
```

`collect` must append successful items in order. It must return `Ok(items)` at ordinary exhaustion and `Err(error)` for the first emitted source error without returning the partial list.

#### `fold`

```incan
def fold[U with Clone, Folder with (Clone, Callable2[U, T, U])](mut self, initial: U, f: Folder) -> Result[U, E]
```

`fold` must invoke `f` once per successful item in order, starting with `initial`. It must return `Ok(accumulator)` at ordinary exhaustion and the first emitted source error without invoking `f` for that error.

### Error observation and retry

Generic adapters and terminals must treat an emitted error as source output, not as permission to repeat a poll. A domain source may accept an explicit retry policy and consume eligible transient failures internally. Such a source must document whether a logical position advances, which errors are retryable, its attempt counting, any backoff, and when a final error is emitted.

`inspect_err` outside a retrying source observes only errors the source emits after its internal policy. Instrumenting every internal attempt belongs to the source or retry policy, not the generic iterator adapter.

### Reader chunk validation

`BinaryReader.chunks(size)` returns a lazy stream rather than a setup `Result`. A non-positive size therefore becomes an `Err(IoError(kind="invalid_input"))` on the stream's first poll, before the reader itself is polled. This preserves lazy construction, keeps invalid sizes out of reader I/O, and gives loops and terminals the same typed error path as later read failures.

## Design details

### Relationship to RFC 068

RFC 068 keeps ordinary `__next__ -> Option[T]` iteration unchanged. This RFC adds a separate named capability and an explicit header marker rather than weakening ordinary iteration or treating `Result` items specially.

RFC 068 also names fixed-arity `CallableN` traits as the nominal capability behind `__call__`. This RFC relies on that existing contract for stored generic callbacks: matching functions and closures satisfy the same bound through the backend's callable-value bridge, while explicit callable models continue to satisfy it through ordinary source trait adoption. The public API therefore remains source-owned and does not narrow callbacks to Rust function pointers.

### Relationship to RFC 070

`Result.map_err` transforms one already-produced `Result`. `FallibleIterator.map_err` lazily transforms errors produced by future polls. Both use the same branch-preserving mental model, but neither replaces the other.

### Relationship to RFC 088

The first adapter family follows RFC 088 names and callback shapes where the error channel does not require a different contract. It does not promise immediate parity with every ordinary iterator adapter.

### Setup and polling ambiguity

Requiring a local between `Result[FallibleIterator[T, E1], E2]` and the loop makes both failure phases visible and avoids assigning two meanings to one header `?`. It also gives callers a clear place to map setup and polling errors independently.

### Structural and nominal use

Loop syntax may recognize a compatible structural `__iter__` and `__next__` shape under the same general rules as RFC 068. Generic bounds and public APIs should use the nominal `FallibleIterator[T, E]` trait.

## Alternatives considered

### Iterate over `Result[T, E]` items

This would use ordinary `Iterator[Result[T, E]]`, but an ordinary loop could ignore errors and continue polling. It also puts propagation boilerplate in every loop body and does not express that failure belongs to fetching the next item.

### Ship only `ReaderChunks.map_err`

Rejected because it makes a generic language protocol serve one concrete stdlib type and leaves other fallible sources without a coherent composition surface.

### Add every RFC 088 adapter immediately

Rejected because multi-source adapters, error-unifying transforms, and recovery operations require additional contracts. The first family covers common single-source item transformation, observation, bounded consumption, error mapping, collection, and folding without pretending those harder decisions are mechanical.

### Add generic `retry`

Rejected because the protocol cannot know whether a failed poll advanced state or whether repeating the operation is safe. Source-owned retry can preserve domain semantics and still compose with generic adapters after the retry boundary.

### Require RFC 098 associated types

Rejected for this slice because explicit `FallibleIterator[T, E]` parameters express the item and error types needed by loops, adapters, and terminals today.

## Drawbacks

- The loop grammar gives `?` a contextual per-poll meaning in addition to its existing one-time `Result` propagation meaning.
- The separate trait duplicates part of the ordinary iterator adapter implementation.
- The first `flat_map` accepts lists rather than any iterable, reflecting current RFC 088 capability rather than the broadest possible abstraction.
- Users must split fallible stream setup into a local before fallible polling, which adds one line but makes the two failure phases explicit.
- Source-owned retry means different domains may expose different policy types, though their fallible outputs remain generically composable.

## Implementation architecture

This section is non-normative. The compiler should record whether a checked loop uses ordinary or fallible polling, then lower fallible polling through the same checked propagation mechanism as postfix `?`. The stdlib should implement adapters and terminals as ordinary Incan default methods and concrete generic state models, following the existing source-owned ordinary iterator pattern. A backend may bridge its native function and closure values to the canonical source `CallableN` traits, but native code should not own chunking, adapter, terminal, or retry semantics.

## Layers affected

- **Parser / AST**: preserve the loop-header postfix `?` form and its source span.
- **Typechecker / Symbol resolution**: distinguish one-time `Result` unwrapping from fallible per-poll iteration, validate callback and terminal types, and diagnose ambiguous setup-plus-polling headers.
- **IR Lowering**: lower each fallible poll through checked error propagation while preserving ordinary exhaustion.
- **Emission**: emit valid backend control flow for propagated poll errors without making generated Rust the semantic authority.
- **Stdlib / Runtime (`incan_stdlib`)**: provide the trait, source-authored adapters and terminals, and reader chunk stream.
- **Formatter**: preserve readable loop-header `?` and chained adapter syntax.
- **LSP / Tooling**: expose the selected item/error types and protocol diagnostics through existing checked facts.

## Implementation Plan

### Phase 1: Protocol and loop semantics

- Preserve the loop-header `?` span and distinguish one-time setup propagation from per-poll fallible iteration.
- Validate structural and nominal `FallibleIterator[T, E]` implementations and surrounding error compatibility.
- Lower and emit each fallible poll through checked propagation while leaving ordinary iteration unchanged.

### Phase 2: Generic adapters and terminals

- Add source-authored lazy state for `map`, `filter`, `flat_map`, `take`, `inspect`, `map_err`, and `inspect_err`.
- Add source-authored `collect` and `fold` terminals with first-error propagation.
- Keep retry, recovery, and post-error re-polling out of generic adapter behavior.

### Phase 3: Reader chunk stream and stdlib adoption

- Provide `BinaryReader.chunks(size)` for standard binary readers with non-empty chunks, ordinary EOF exhaustion, and typed invalid-size/read errors.
- Migrate reader-backed hashing to the generic chunk stream.
- Keep chunking and adapter behavior authored in Incan rather than a bespoke native backend.

### Phase 4: Verification and documentation

- Cover direct protocol use, every adapter and terminal, invalid loop forms, generated Rust, reader behavior, and errors after prior successful items.
- Document complete loop pipelines, terminal return types, setup versus polling errors, and source-owned retry.
- Update generated references and the 0.5 release note, then run stable, Rust 1.93, source-pinned stdlib/provider, documentation, review/fix, and full repository gates.

## Progress Checklist

### Spec / design

- [x] Define item, exhaustion, error, laziness, and callback invocation semantics.
- [x] Settle the generic first adapter and terminal family.
- [x] Settle source-owned retry and setup-versus-polling boundaries.

### Parser / AST / formatter

- [x] Preserve the loop-header postfix `?` and source span.
- [x] Verify formatter round-tripping for fallible loops and representative adapter chains.

### Typechecker

- [x] Recognize structural fallible polling and record its item/error types.
- [x] Preserve ordinary `Result[Iterable, E]?` loop behavior.
- [x] Reject a setup `Result` containing a fallible stream in one ambiguous loop header with actionable guidance.
- [x] Validate every generic adapter and terminal signature through source fixtures.

### Lowering / emission

- [x] Lower per-poll errors through checked propagation.
- [x] Preserve ordinary exhaustion and ordinary iteration behavior.
- [x] Compile generated Rust for representative adapter chains and terminals.

### Stdlib / runtime

- [x] Implement generic lazy `map`, `filter`, `flat_map`, `take`, `inspect`, `map_err`, and `inspect_err` in Incan.
- [x] Implement fallible `collect` and `fold` in Incan.
- [x] Remove the concrete `ReaderChunks.map_err` entry point after the generic trait method is available.
- [x] Implement `BinaryReader.chunks(size)` and standard reader adoption in Incan.
- [x] Migrate reader-backed hashing to fallible chunk iteration.

### Tests

- [x] Cover fallible-loop typechecking and generated propagation.
- [x] Cover normal chunks, empty input, final short chunks, invalid sizes, raw errors, and mapped reader errors.
- [x] Cover every generic adapter's laziness, order, exhaustion, callback, and error behavior.
- [x] Cover `collect` and `fold` success plus first-error behavior after prior successful items.
- [x] Cover direct import, generic bound, package/provider, and test-batch visibility where applicable.

### Documentation and release

- [x] Draft and review RFC 115.
- [x] Expand the collection-protocol and `std.io` references with complete generic examples.
- [x] Update generated references.
- [x] Update the 0.5 release note.
- [x] Complete review/fix, stable and Rust 1.93 focused gates, source-pinned stdlib/provider proof, docs gates, and the full repository gate.

## Design Decisions

- The first family includes `map`, `filter`, list-returning `flat_map`, `take`, `inspect`, `map_err`, `inspect_err`, `collect`, and `fold`.
- `collect` and `fold` return `Result` and stop at the first emitted error.
- A setup `Result` containing a fallible stream must be unwrapped into a local before the loop.
- Generic fallible iteration never retries; retry remains source-owned.
- Async iteration, associated types, `try_map`, recovery adapters, and multi-source adapters remain separate future design work.
