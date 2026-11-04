# RFC 169: Typed record sets and record streams

- **Status:** Planned
- **Created:** 2026-11-01
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 094 (context managers and scoped cleanup)
    - RFC 115 (fallible iteration)
    - RFC 122 (model validation)
    - RFC 131 (typed errors)
    - RFC 142 (async composition, iteration, and streams)
    - RFC 146 (typed JSON decoding and JSONL readers)
    - RFC 158 (incremental XML records over split and compressed inputs)
    - RFC 164 (explicit ownership and borrowing)
    - RFC 165 (typed embedded storage and Crumb)
    - RFC 166 (indexed graphs and snapshot-bound access)
    - RFC 168 (standard primitives)
- **Issue:** [#2623](https://github.com/encero-systems/incan/issues/2623)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

`std.data` gives Incan one typed way to read large collections of records, such as an application's event log, a database table or a compiler's facts. A `RecordSet[T]` always ends; a `RecordStream[T]` is asynchronous and may wait for more records forever. A set or stream may hold several model types at once, declared as an enum, and each record stores only its own fields. A reader can keep one kind of record, select a few fields as typed columns, and read in bounded batches. A source that changes over time delivers typed `Change` records with positions and transaction boundaries, and a resumable source continues from a saved position or reports a typed gap. Storage and Crumb stay with RFC 165, graphs with RFC 166, JSONL decoding with RFC 146, and iteration consumers and error policies with RFC 142.

## Core model

1. **A record set declares the record types it accepts.** An enum of models identifies several record kinds without turning records into untyped property bags.
2. **A record carries only its own fields.** "This kind has no such field", "this optional field is `None`" and "this field has a value" stay three different facts.
3. **A selection names its kinds and fields by checked paths.** A field's identity includes the model that declares it, not only its spelling.
4. **A batch bounds delivery and working memory.** Batch boundaries are not transactions, commits or time windows.
5. **A set always ends; a stream may wait.** Waiting, exhaustion, cancellation, failure and resource limits keep distinct meanings.
6. **A change source delivers changes, not repeated snapshots.** Inserts, updates and deletions keep the record's key and the source's position.

## Motivation

A web shop logs page views and purchases to one JSONL file, one event per line. A finance report needs the amount and currency of every purchase in a multi-gigabyte log. Today the program either decodes every line into a full model and keeps its own batching, or flattens both event kinds into one wide model where a page view carries an empty `amount_cents`. The wide model cannot tell "a page view has no amount" from "this purchase's amount is missing", and it weakens each event's own validation.

The same shape appears in a database table read by a dashboard, a compiler's stored facts (call sites, inlining decisions, type uses), and a graph with several node and edge kinds. Each needs typed records of several kinds, bounded reading and selection of a few fields, and each source can be much larger than memory. When the fields are stored separately, as in a `std.db` column profile, selecting two of them must not decode the rest. A dashboard following a live table also needs to know what changed since it last looked, and where to continue after a restart.

RFC 165 supplies typed storage, payload enums and single-family projected batches. RFC 166 needs a view over several node and relationship kinds. RFC 142 supplies iteration consumers and error policies, and RFC 146 supplies lazy JSONL readers. None of them defines the shared contract for typed records of several kinds, field selection across those kinds, or change and resume semantics. Without one, each library builds its own batching, selection and change machinery.

## Goals

- Read typed records of one or several declared model kinds from finite and continuing sources.
- Keep each record's concrete model and kind through storage, selection, batching and reconstruction.
- Support wide schemas without per-record storage for every field of every kind.
- Select kinds and fields with compile-time checks, and make decoding work and source capabilities inspectable.
- Use the same record definitions for a file, a snapshot and a live change feed.
- Compose with RFC 142's consumers, RFC 115's fallible iteration, RFC 094's scopes and RFC 164's borrowing.
- Make ordering, progress, resumption and completion visible to application authors and tooling.

## Non-Goals

- A query language, relational optimizer, join planner, aggregation engine or distributed execution. A query library may consume record sets and supply those.
- Redefining Crumb encoding, JSON decoding, database transactions, graph identity or async scheduling.
- Requiring every database family to accept arbitrary models, or weakening RFC 165's typed family contracts.
- Treating an inapplicable field as `None`, or accepting undeclared fields without a schema change.
- Exactly-once application effects, automatic retries or universal replayability.
- Event-time windows, watermarks, distributed checkpoints or streaming aggregation.

## Guide-level explanation

### Records of several kinds

The web shop's event log holds two kinds of records:

```incan
from std.data import RecordSet, BatchLimits, DataError
from std.serde import json

@derive(json)
model Address:
    country: str
    city: str

@derive(json)
model PageView:
    user: str
    path: str

@derive(json)
model Purchase:
    user: str
    amount_cents: int
    currency: str
    coupon: Option[str]
    shipping: Address

@derive(json)
enum Event:
    PageView(PageView)
    Purchase(Purchase)
```

Each line of the log is one `Event` in its JSON form, such as `{"Purchase": {"user": "ana", "amount_cents": 1999, "currency": "EUR", "coupon": null, "shipping": {"country": "NL", "city": "Utrecht"}}}`. A `RecordSet[Event]` holds both kinds. A `PageView` record has no `amount_cents` field at all; a `Purchase` whose `coupon` is `None` has the field with the value `None`. Both facts survive storage and reconstruction unchanged. A `RecordSet` keeps its source's order and may contain equal records more than once; "set" means a complete collection of records that ends, not a mathematical set or records already held in memory.

### Keep one kind, select a few fields

```incan
def eur_revenue(events: RecordSet[Event]) -> Result[int, DataError]:
    amounts = events.filter_variant(Event.Purchase).select(
        amount=Purchase.amount_cents,
        currency=Purchase.currency,
    )
    mut total_cents = 0
    for batch in amounts.batches(BatchLimits(records=4096, bytes=1_048_576))?:
        for index in range(batch.record_count):
            if batch.currency[index] == "EUR":
                total_cents += batch.amount[index]
    return Ok(total_cents)
```

`filter_variant(Event.Purchase)` keeps only purchases and types them as `Purchase`; reading a `path` field from one is a type error. `select(...)` names fields by their path on the model. `Purchase.amount_cents` is a field path checked at compile time, not an attribute read on a live record, so a misspelled field fails when the program compiles. Nested fields use the same form, such as `Purchase.shipping.country`. Each batch has typed columns `amount: int` and `currency: str` and holds at most 4096 records or 1 MiB. A source that stores fields separately, such as a `std.db` column profile, supplies those two columns without decoding the others or building a `Purchase` per record.

A record set is lazy. Building one, narrowing it with `filter_variant` or choosing fields with `select` reads nothing; records are read only as `records()` or `batches(...)` advance, and stopping early reads no further.

`batches(...)` and `records()` are ordinary fallible iterators, so RFC 142's consumers, such as `for_each_batch` and `collect`, apply to them unchanged.

### Read a large file without loading it

A JSONL log is read lazily through RFC 146's reader and adapted to a record set:

```incan
from std.data import RecordSet, DataError
from std.fs import Path
from std.io import IoError
from std.json import jsonl

error LogError = IoError | DataError

def eur_revenue_from_log(path: Path) -> Result[int, LogError]:
    with path.open("rb")? as file:
        events = RecordSet[Event].from_iter(jsonl[Event](file, max_record_bytes=1_000_000))
        return Ok(eur_revenue(events)?)
```

`from_iter` adapts any iterator of records, or of `Result` records, without reading ahead. JSONL stores whole lines, so this set decodes each line through `Event.from_json` and then projects it; its `capabilities()` say so, and a caller that requires direct projection receives `DataError.ProjectionUnavailable` instead of a silent fallback. By default a bad line stops reading with `DataError.Record`, carrying its record number and the reader's own error with its line number. An import that must survive a crash resumes the reader itself: RFC 146's reader pairs each line with a `JsonlPosition`, which the importer can save in the same `std.db` transaction as the records it wrote. To continue past bad lines, the adapter takes RFC 142's policy:

```incan
from std.validation import ErrorPolicy

def report_bad_line(error: DataError) -> None:
    println(error.message())

events = RecordSet[Event].from_iter(
    jsonl[Event](file, max_record_bytes=1_000_000),
    item_errors=ErrorPolicy.CollectAll,
    report_item_error=report_bad_line,
)
```

### Select across kinds

A selection may span kinds when it says how their fields line up:

```incan
from std.data import align

activity = events.select(
    user=align(PageView.user, Purchase.user),
    amount=Purchase.amount_cents,
    coupon=Purchase.coupon,
)
```

`align(PageView.user, Purchase.user)` maps two declared fields to one `str` column; both must have the same type, and matching spelling alone is never enough. `amount` exists only on purchases, so its column type is `FieldValue[int]`: `NotApplicable` for a page view and `Present(1999)` for a purchase. `coupon` has type `FieldValue[Option[str]]`, which keeps three cases apart: `NotApplicable` for a page view, `Present(None)` for a purchase without a coupon, and `Present(Some("WELCOME10"))` for one with a coupon. A cross-kind batch also has a `variant` column, so `batch.variant[index] == Event.Purchase` tells the record's kind.

### Streams that may wait

```incan
import std.async
from std.data import RecordStream, DataError

async def print_events(source: RecordStream[Event]) -> Result[None, DataError]:
    async for event in source?:
        match event:
            Event.PageView(view) => println(f"{view.user} viewed {view.path}")
            Event.Purchase(purchase) => println(f"{purchase.user} paid {purchase.amount_cents}")
    return Ok(None)
```

A `RecordStream` is an RFC 142 fallible async source. `set.stream()` turns any record set into a stream that ends when the set ends; advancing it runs the set's reads on the current task, so a set over a blocking file blocks that task, exactly as an RFC 142 synchronous callback would. A live source, such as a database change feed, waits when it has caught up and never reports exhaustion merely because no record is ready. A function that needs a finite input takes a `RecordSet`; a function that accepts either takes a `RecordStream`. The same `filter_variant` and `select` operations apply to streams and keep their order and laziness.

### Follow a table's changes

A dashboard shows the revenue of a `std.db` table of orders and follows it while customers check out:

```incan
import std.async
from std import db
from std.data import Change, DataError, ResumePosition
from std.db import Record
from std.fs import Path

@derive(Record)
model Order:
    customer: str
    total_cents: int

error RevenueError = db.OperationError | DataError

class Dashboard:
    revenue_cents: int
    position: Option[ResumePosition]

    def apply(mut self, change: Change[str, Order]) -> None:
        match change:
            Change.Inserted(_, order) => self.revenue_cents += order.total_cents
            Change.Updated(_, order, Some(previous)) => self.revenue_cents += order.total_cents - previous.total_cents
            Change.Deleted(_, Some(previous)) => self.revenue_cents -= previous.total_cents
            _ => pass

    def publish(mut self, position: ResumePosition) -> None:
        self.position = Some(position)
        println(f"Revenue: {self.revenue_cents} cents")

async def follow_revenue(path: Path, mut dashboard: Dashboard) -> Result[None, RevenueError]:
    with db.open(path) as store:
        orders = store.view("published")?.table[str, Order]("orders")?
        async for event in orders.snapshot_then_changes()?:
            dashboard.apply(event.change)
            if event.ends_transaction:
                dashboard.publish(event.position)
    return Ok(None)
```

`snapshot_then_changes()` first delivers every order present at one snapshot as `Change.Inserted`, grouped as one transaction, then every change committed to the `published` view after that snapshot, and none the snapshot already contains. No order placed between the snapshot and the subscription is lost. Each `ChangeEvent` carries a `Change`, its position and its transaction boundary. The dashboard publishes only at the end of a transaction, so a customer's three-order checkout appears all at once, never one order at a time. A table stream always has previous values, so the final `_` arm never runs; it exists because `previous` is an `Option` for sources that lack them.

After a restart, the dashboard continues from its saved position:

```incan
async for event in orders.changes(after=saved_position)?:
    dashboard.apply(event.change)
```

If the view's history policy no longer keeps that position (RFC 165; a disk store keeps 24 hours unless told otherwise), the first advancement fails with `DataError.HistoryGap`, naming the requested position and the oldest available one. The stream never silently continues from the latest revision, which would leave the dashboard wrong without anyone noticing; the dashboard reloads from a fresh `snapshot_then_changes()` instead.

## Reference-level explanation

### Module and layering

The public module must be `std.data`, exporting `RecordSet`, `RecordStream`, `RecordBatch`, `BatchLimits`, `FieldPath`, `FieldValue`, `align`, `SourceCapabilities`, `Change`, `ChangeEvent`, `ResumePosition`, `TransactionId`, `ChangeDelivery`, `PreviousValues` and `DataError`. Under RFC 168, `std.data` is a higher-level composition: it must build on the foundation iteration protocols, consumers and `ErrorPolicy` from `std.primitives` and must not redefine them. Codec and platform layers, including Crumb and `std.json`, must not import `std.data`. `std.data` defines the generic `from_iter` adapter; `std.db` and `std.graph` provide their own adapters by implementing its interfaces, and `std.data` must not require a database or graph package.

### Reading

`RecordSet[T]` and `RecordStream[T]` must share one source contract for kinds, selection, batching, capabilities and progress. There must be no third public name for that contract.

- `RecordSet[T]` must provide `records()`, a `FallibleIterator[T, DataError]`; `batches(limits)`, a `FallibleIterator[RecordBatch[T], DataError]`; `stream()`, a `RecordStream[T]` that spawns no task; and `capabilities()`.
- `RecordStream[T]` must adopt `AsyncFallibleIterator[T, DataError]` and provide async `batches(limits)`, `capabilities()` and, when it is resumable, `position()`, a `ResumePosition` that advances over every examined record, including records a filter rejects.
- Both must provide `filter_variant` and `select`, which return a set or stream of the narrowed or projected type, and `from_iter`, which adapts an iterator of `T` or of `Result[T, F]`.

`capabilities()` must return a `SourceCapabilities` model stating whether the source can be read again, whether it can resume, whether it supports direct projection and indexed kind selection, its ordering guarantee, its prefetch bound and, for a source that cannot pause, its buffering or loss policy; for change sources it adds `ChangeDelivery` and `PreviousValues`. A set built by `from_iter` over a single-pass iterator can be read once; a second `records()` or `batches()` must fail with `DataError.NotReplayable`. Sets over lists and snapshots can be read again.

Sets and streams release the resources they hold, such as snapshot leases, subscriptions and readers, when their last owner drops them. Leaving a loop early stops further advancement and awaits no close. A source that needs an awaited shutdown is entered with `async with` under RFC 094 and RFC 142.

### Errors

`DataError` must be an RFC 131 error family with at least these cases:

```incan
error DataError:
    Source(cause: Error):
        message = "The record source failed."
        caused_by = cause

    Record(number: int, cause: Error):
        message = f"Record {number} could not be read."
        caused_by = cause

    Schema(kind: str) = f"Record kind '{kind}' is not declared by this record set."
    Limit(limits: BatchLimits) = "A record does not fit within the batch limits."
    ProjectionUnavailable(field: str) = f"Field '{field}' cannot be read without decoding whole records."
    HistoryGap(requested: ResumePosition, oldest: Option[ResumePosition]) = "History before the oldest retained position is no longer available."
    NotReplayable = "This source can only be read once."
```

`Source` retains the underlying failure, such as an `IoError` or a database error, so source-specific errors remain recoverable under RFC 131. `Record` numbers records from one in source order and retains the decoder's error, which for JSONL carries the physical line. Cancellation, deadlines and callback failures are not `DataError` cases: RFC 142's consumers report them through `ConsumptionFailure`.

`from_iter` over `Result` records must apply RFC 142's `item_errors` policy and reporter rules, with FailFast as the default: under FailFast the first `Err` item ends reading with `DataError.Record`, and under CollectAll or Ignore the record is reported or skipped and reading continues. A record that can be located and skipped is an item failure; a failure to find the next record boundary is a `Source` failure and stops reading under every policy. A record of an undeclared kind is a `Schema` item failure under the same policy; it must never be silently dropped or coerced into another model.

### Declared kinds and sparse representation

A record set must declare the model or enum of models it accepts, and an adapter must validate incoming type and schema identities before exposing values as those types. The schema may describe fields across many kinds. Record storage must not require a slot or placeholder for every field of every other kind. Schema metadata may be shared at set or batch scope, and an implementation may partition by kind, keep tagged records, or combine layouts.

Structural inapplicability, an optional value of `None`, and a present value must remain distinct. A required field missing from a record of its own model is invalid data, not sparsity. An unselected field in a projection is not evidence that the original model lacked it. A partial projection must not masquerade as a complete reconstructed model.

### Selection and field identity

`filter_variant(E.V)` must narrow to the payload model of variant `V` and preserve its type and provenance. A field path such as `Purchase.amount_cents` must have the type `FieldPath[Purchase, int]`; it names the declaring model and the field, including nested paths such as `Purchase.shipping.country`. When two variants of the enum share one payload model, a path must name the variant, as in `Event.Purchase.amount_cents`. Names are presentation, not identity.

`select(name=path, ...)` must bind each output column to a checked field path. `align(A.f, B.f, ...)` must take field paths of one value type `T` from different kinds and produce one column of type `T`; there is no automatic stringification or name-based merge. A selected field that exists on only some kinds of the selection must have the column type `FieldValue[T]`, with the cases `NotApplicable` and `Present(T)`. A field present on every selected kind has the plain column type `T`. A batch that spans several kinds must have a `variant` column holding each record's enum variant.

A source must state through `capabilities()` whether it selects kinds through an index or by filtering in sequence, and whether it reads selected fields directly or decodes whole records first. `select(..., require_direct=true)` must fail with `DataError.ProjectionUnavailable` when the source cannot read the fields directly. Each batch must report the records examined and the bytes decoded to produce it, including rejected records and unselected bytes a codec decoded. Supporting a projection does not imply that any predicate or query optimizer exists.

### Batches, resources and reconstruction

`RecordBatch[P]` must carry its typed projection, record count, and any record, revision or progress metadata its source promises. For a selection, it holds one typed column per selected field; otherwise it holds the records in order. It is the same type as RFC 165's single-family projected batch; `std.db` produces it rather than defining another. `BatchLimits(records, bytes)` must bound delivery and working state. A record that cannot fit within the applicable bound must fail with `DataError.Limit`, never apparent exhaustion. Batch size is not a transaction boundary or a bound on memory that consumers retain.

Sources must not decode the whole set to open it, determine its kinds, or produce its first batch. A caller stopping early must prevent further demand-driven advancement. Prefetch must have a declared bound. A source that cannot pause must declare a bounded buffering or loss/error policy; downstream laziness alone is not proof of upstream backpressure.

RFC 164 governs borrowed columns, exclusive access and owned retention. Ordinary users consume batches through duckborrowing; `std.data` declarations expose actual ownership and source dependencies. Column borrows must not survive advancement that would invalidate their backing buffers; the source retains those buffers or the checker rejects the conflict. Adapters must not silently clone a batch, acquire indefinite retention, or let a dangling view make an escaping borrow compile.

The source's codec owns complete typed reconstruction: Crumb for `std.db` under RFC 165, and `T.from_json` for JSONL under RFC 146. Adapters must preserve the kind and schema information the codec needs. A projected batch need not hold enough data to reconstruct the original model. Reconstruction must not bypass validation or expose protected fields through errors or schema diagnostics.

### Completion and ordering

A set must declare its ordering; a snapshot-backed set observes one consistent selection. A stream must declare its ordering and the consistency boundaries its batches and positions guarantee. There is no inferred total order across independent partitions. Temporary lack of data must not signal completion. A scan stopped by a work limit must return explicit limited progress or a typed limit failure, never ordinary exhaustion.

### Change sources

A change source must deliver `ChangeEvent[K, T]` values:

```incan
enum Change[K, T]:
    Inserted(key: K, value: T)
    Updated(key: K, value: T, previous: Option[T])
    Deleted(key: K, previous: Option[T])

model ChangeEvent[K, T]:
    change: Change[K, T]
    position: ResumePosition
    transaction: Option[TransactionId]
    ends_transaction: bool
```

A deletion must keep the key even when no payload remains. A change source must declare its `ChangeDelivery`, either `EveryChange` (every committed change in order) or `NetDifference` (the coalesced difference between observed positions), and its `PreviousValues`, one of `Always`, `WhenRetained` or `Never`. Positions must be distinct from record keys, row offsets and content hashes. A transaction that spans several events must keep its grouping, and its last event must set `ends_transaction`; under `NetDifference`, a group is one observed step and may cover several commits. A consumer must not infer a commit from a batch boundary. Only change sources use these types; other streams yield plain records.

`std.db` table handles obtained from a view must provide `changes(after=position)` and `snapshot_then_changes()`, both following the view's head. A table change stream delivers `NetDifference` between consecutive observed revisions with `PreviousValues.Always`, consistent with RFC 165's snapshot changes; a revision whose history is gone is a `HistoryGap`, not a missing previous value. An application that needs every intermediate event keeps its own append-only event family; its `changes(after=)` delivers `EveryChange`, one `Change.Inserted` per appended event. A table change stream waits until the view publishes a new revision and reads the changes from retained snapshots, so RFC 165's snapshot change API stays the recovery authority and `std.db` invokes no callbacks during commit.

Delivery does not mean the application's work succeeded. A consumer saves its position after its intended effects, and handles duplicate delivery when effects and position cannot commit together.

### Resumption

A `ResumePosition` must bind to source identity, schema and selection, ordering and retained history, and must have a stable encoded form, through Crumb and JSON, so a consumer can save it across restarts. A saved position does not hold history; RFC 165's history policy decides how long it stays usable. Unavailable history must fail the stream's first advancement with `DataError.HistoryGap`, naming the requested and oldest available positions; a source must never silently resume at the latest position.

`snapshot_then_changes()` must establish a consistent handoff. It delivers every record of one snapshot as `Change.Inserted` events in one transaction group, then every change committed after that snapshot's revision and none the snapshot already contains. The snapshot's lease protects the history from its revision until the stream has passed it, and is released then. Reading a snapshot and separately starting a subscription is not equivalent, because changes can be missed in between.

| Source | Resumes from | Starts | Duplicates | Unavailable history |
| --- | --- | --- | --- | --- |
| `std.db` table change stream | a saved `ResumePosition` | after the saved position | none at or before it | `HistoryGap` |
| `std.db` application event family | a saved `ResumePosition` | after the saved position | none at or before it | `HistoryGap` |
| RFC 146 JSONL and RFC 158 XML readers | a saved `JsonlPosition` or `XmlPosition` | after the saved position | none at or before it | `SourceChanged` when the input before it changed |
| Sets and streams built with `from_iter` | not resumable; resume the underlying reader and adapt it again | — | — | open the source again |
| Channels, broadcasts and HTTP response streams | not resumable | — | — | declared in `capabilities()` |

Exactly-once effects are not promised. Atomic publication of application changes and source progress requires a compatible transactional mechanism; a callback and an external checkpoint cannot gain that guarantee from a stream wrapper.

## Design details

| Owning RFC | Responsibility |
| --- | --- |
| RFC 122 | Model construction and validation |
| RFC 142 | Async advancement, consumers, cancellation composition and error policies |
| RFC 146 | JSONL decoding, record and line errors |
| RFC 164 | Borrowing, ownership and safe retained access |
| RFC 165 | Typed storage, Crumb, indexes, transactions, snapshots, retention and bounded storage reads |
| RFC 166 | Graph identity, typed relationships, adjacency and traversal |
| RFC 169 | Record kinds, field selection, record batches, change events and resumption |

RFC 165's typed families and single-family projections remain valid on their own terms and produce `RecordBatch[P]`. RFC 166 may store several payload kinds through a declared enum without `std.data`; a graph exposed as record sets uses these interfaces rather than a graph-specific copy.

A compiler's stored facts follow the same shape. Call sites and inlining decisions are two models in one enum. A language server selects the callee, file and line of every call site at startup, follows the compiler's revisions as changes while the user edits, and continues from its saved position after a restart.

## Alternatives considered

- **One wide model of optional fields:** confuses fields that do not apply with optional values that are `None`, weakens each kind's validation, and costs per-record width unrelated to the actual model.
- **An untyped dictionary per record:** loses checked field identity and turns projection errors into runtime conventions.
- **String column names, as in DataFrame libraries:** a misspelled field fails only when the program runs, and two kinds' identically spelled fields merge silently.
- **The names `Dataset` and `DataFrame`:** they describe query-library carriers with filters, joins and aggregations. `std.data` reads records and has none of those.
- **The name `RecordSource`:** suggests the reading half of a source and sink pair; `RecordSet` says that the collection is complete and ends.
- **The name `of_variant`:** it reads as a check that a record has a variant rather than an operation that keeps those records. `filter_variant` keeps the filtering verb and narrows the type, as Kotlin's `filterIsInstance` does.
- **A stream error parameter, `RecordStream[T, E]`:** every failure is a `DataError` that retains its source's typed cause, so a second parameter would only repeat it.
- **A snapshot and a separate stream from `snapshot_then_changes()`:** holding the snapshot while following the stream pins retention for the stream's lifetime and leaves the snapshot's records without keys. One stream that starts with the snapshot's records as inserts has neither problem.
- **A separate API per model family:** keeps types but repeats batching, lifetime and change machinery in every consumer.
- **Defining this inside the graph RFC:** hides general record semantics in one domain library and makes file and database readers depend on graphs.
- **A full streaming engine:** adds planning and event-time semantics before the typed reading boundary exists; such an engine can consume this interface.

## Drawbacks

Kind-aware schemas and checked projections add descriptor and diagnostic work beyond plain record iteration. Cheap field access depends on physical encoding, so adapters need observable capabilities rather than blanket performance claims. Resumable change sources impose retention and ordering requirements beyond ordinary iterators. Consumers who want complete models only can use ordinary iteration over `records()` without learning projections.

## Layers affected

- **Stdlib and runtime:** `std.data` definitions and the generic `from_iter` adapter; `std.db` and `std.graph` provide their adapters.
- **Type checking and descriptors:** checked variant narrowing and field paths, `align` type compatibility and `FieldValue` column types, with diagnostics for invalid fields, ambiguous shared payload models and incompatible alignments.
- **Ownership checking:** batch and column lifetimes, advancement and explicit retention under RFC 164.
- **Tooling and documentation:** visible source capabilities, schema inspection, and diagnostics that distinguish absence, optionality, limits and exhaustion.

## Design Decisions

- **Module and names:** the module is `std.data`, with `RecordSet[T]` for collections that end and `RecordStream[T]` for asynchronous sources that may wait. They share one source contract with no third public name; `set.stream()` converts. `std.data` is an RFC 168 composition built on the foundation iteration consumers.
- **Field paths and applicability:** fields are named by checked paths on the model, such as `Purchase.amount_cents` and `Purchase.shipping.country`, qualified by variant when two variants share a payload model. Cross-kind columns require `align(...)` with identical types. A field on only some selected kinds has column type `FieldValue[T]` with `NotApplicable` and `Present(T)`, distinct from `Option`.
- **Change envelope:** `Change[K, T]` with `Inserted`, `Updated` and `Deleted`, wrapped in `ChangeEvent[K, T]` with position and transaction boundary. Sources declare `ChangeDelivery` and `PreviousValues`. Other streams yield plain records.
- **Resumption:** `std.db` table change streams and event families resume from a saved `ResumePosition`, and `snapshot_then_changes()` hands off from a snapshot to changes in one stream. RFC 146 JSONL and RFC 158 XML readers resume from a saved position over any reader. Sets and streams built with `from_iter`, channels, broadcasts and HTTP streams are not resumable and say so. Unavailable history is always `DataError.HistoryGap`.
