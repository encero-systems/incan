# RFC 169: Typed datasets and structured streams

- **Status:** Draft
- **Created:** 2026-11-01
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 094 (context managers and scoped cleanup)
    - RFC 115 (fallible iteration)
    - RFC 122 (model validation)
    - RFC 131 (typed errors)
    - RFC 142 (async composition, iteration, and streams)
    - RFC 164 (explicit ownership and borrowing)
    - RFC 165 (typed embedded storage and Crumb)
    - RFC 166 (indexed graphs and snapshot-bound access)
    - RFC 168 (standard primitives)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce a common typed dataset and structured-stream surface for bounded collections, incremental file readers, and continuing sources. A declared collection may contain several model variants while retaining their concrete types and field identities. Its logical schema may be wide, but individual records need not store placeholders for fields belonging to other variants. Type-aware selection and projected batches expose selected data without requiring full-model decoding or whole-source materialization. Storage and Crumb remain owned by RFC 165, graph structure and traversal by RFC 166, and asynchronous advancement and consumer error policies by RFC 142. This RFC defines their shared data interface, not a query engine or an implemented API.

## Core model

1. **A dataset declares its accepted record types.** A payload enum can identify several model variants without converting records into untyped property bags.
2. **A schema describes possible fields; a record carries its own model data.** Structural absence, an optional field containing `None`, and an available value are distinct.
3. **A projection states which variants and fields are requested.** Field identity includes its declaring model or variant, rather than only its spelling.
4. **A batch bounds delivery and working resources.** Batch boundaries do not imply transactions, commits, time windows, or application-level completeness.
5. **A source declares whether it is finite or continuing.** Async waiting, exhaustion, cancellation, failure, and resource limits retain distinct meanings.
6. **A change stream describes changes, not another snapshot scan.** Inserts, updates, and deletions preserve record identity and source progress.

## Motivation

A compiler stores functions, types, call sites, type uses, and other facts. A graph can connect several of those model families, and a consumer may want all call-site locations without decoding every function body. File ingestion has the same structural problem: a sequence may contain different record variants, optional values, and nested models, while processing must remain bounded even when the source is much larger than RAM.

RFC 165 already requires typed model reconstruction, payload enums, field-aware indexes, and narrow projected reads. Its batch contract describes one family at one snapshot; it does not establish a common heterogeneous dataset interface. RFC 166 needs a graph view over several record and relationship kinds. RFC 142 supplies async advancement and lazy consumers, but an asynchronous iterator alone does not describe a record schema, field applicability, or database change semantics.

The missing surface is shared typed data access. It must keep the model definitions useful during selection and reconstruction, rather than forcing applications to flatten them into thousands of optional fields or repeatedly rebuild dictionaries.

## Goals

- Support explicitly declared heterogeneous datasets and structured streams.
- Preserve concrete model and payload-variant identity through storage, selection, batching, and reconstruction.
- Support wide logical schemas without requiring per-record storage proportional to all fields across all variants.
- Provide type narrowing and bounded field projections with inspectable decoding and allocation behavior.
- Use the same record definitions for bounded snapshots, streamed input, and continuing change sources.
- Compose with existing proposed iteration, borrowing, cleanup, and error-policy contracts.
- Make source ordering, progress, resumption, and completion understandable to both application authors and tooling.

## Non-Goals

- A SQL language, relational optimizer, distributed execution engine, or automatic join planner.
- Redefining Crumb encoding, database transactions, graph identity, or async task scheduling.
- Requiring every database family to accept arbitrary models or weakening existing typed family contracts.
- Treating absent fields as `None`, or accepting undeclared fields without a schema change.
- Promising exactly-once application effects, automatic retries, or universal replayability.
- Specifying event-time windows, watermarks, distributed checkpoints, or streaming aggregation in this RFC. Those can build on this surface with separately reviewed semantics.

## Guide-level explanation

### What is proposed, and where the names come from

All dataset and structured-stream names below are proposed. `std.dataset` is a candidate module, not an existing import. `Dataset`, `StructuredStream`, `BatchLimits`, and `DataError` belong to that candidate module. The constructor, narrowing, and projection spellings are Draft API candidates. `std.db` acquisition and snapshot semantics follow RFC 165; asynchronous loops follow RFC 142. These examples establish behavior to review, not runnable fixture claims.

### Keep different compiler models together

The following application models are shared by the examples in this document. They deliberately simplify compiler identities to strings; a real compiler uses its checked identities rather than names as durable keys.

```incan
from std.dataset import Dataset, StructuredStream, BatchLimits, DataError

model SourceLocation:
    file: str
    line: int

model Calls:
    caller: str
    callee: str
    location: SourceLocation

model Inlines:
    caller: str
    callee: str
    reason: Option[str]

enum CompilerFact:
    Calls(Calls)
    Inlines(Inlines)

def sample_facts() -> Dataset[CompilerFact]:
    return Dataset[CompilerFact].from_values([
        CompilerFact.Calls(Calls(
            caller="app.process",
            callee="text.normalize",
            location=SourceLocation(file="app.incn", line=12),
        )),
        CompilerFact.Inlines(Inlines(
            caller="app.process",
            callee="text.normalize",
            reason=None,
        )),
    ])
```

This small example explicitly allocates a list. Adapting that list must not be confused with reading a database or file into a list. The production source supplies batches incrementally. Whether `from_values` takes ownership or borrows its input must be visible through RFC 164, with ordinary callers relying on duckborrowing.

The logical schema includes `Calls.location.file`, `Calls.location.line`, and `Inlines.reason`. A `Calls` record has no `reason` field. The `Inlines` record above has a `reason` field whose value is `None`. Those facts must survive storage and reconstruction unchanged. Identically spelled `caller` fields remain qualified by their declaring model until explicitly aligned.

### Narrow first, then access ordinary model fields

```incan
def print_call_sites(facts: Dataset[CompilerFact]) -> Result[None, DataError]:
    calls = facts.of_variant(CompilerFact.Calls)

    with calls.cursor(BatchLimits(rows=256, bytes=1_048_576)) as cursor:
        while let Some(batch) = cursor.next_batch()?:
            for call in batch:
                print(f"{call.caller}: {call.location.file}:{call.location.line}")

    return Ok(None)
```

`of_variant(CompilerFact.Calls)` selects and unwraps that variant, yielding a dataset of `Calls` values. It does not add a `reason` field to `Calls`, and attempting to read one must receive a type diagnostic. The source may use a variant index; a sequential source may instead inspect tags while scanning. The adapter must disclose that difference rather than promise indexed access for every source.

The candidate cursor owns bounded scan state. `next_batch()` returns `Result[Option[Batch[Calls]], DataError]`: `Some` supplies a batch, `None` means the finite selection is exhausted, and `Err` reports failure. The configured limits apply to batch delivery and working buffers; a record that cannot be represented within the applicable bound produces a typed limit failure. Limits must not manufacture apparent exhaustion.

### Project a nested field without decoding complete records

The following is an explicit candidate for descriptor-based projection; the exact descriptor spelling remains open. Here `Calls.location.file` and `Calls.location.line` denote checked field descriptors, not attribute reads on a live model instance.

```incan
def print_locations(facts: Dataset[CompilerFact]) -> Result[None, DataError]:
    locations = facts.of_variant(CompilerFact.Calls).select(
        file=Calls.location.file,
        line=Calls.location.line,
    )

    with locations.cursor(BatchLimits(rows=256, bytes=1_048_576)) as cursor:
        while let Some(batch) = cursor.next_batch()?:
            # Only the selected str and int columns are requested.
            for index in range(batch.row_count):
                print(f"{batch.file[index]}:{batch.line[index]}")

    return Ok(None)
```

The projected batch has typed `file` and `line` columns. It does not have to construct a full `Calls` model for each row. An existing in-memory model source already holds decoded data; an encoded source may decode selected fields, or advertise that its codec requires bounded broader decoding. Projecting a field is not a universal zero-copy promise. Column borrows cannot survive advancement if advancement would invalidate their backing buffers; the source must retain those buffers or the checker must reject the conflict under RFC 164.

A cross-variant projection of `Calls.caller` and `Inlines.caller` must explicitly map both descriptors to its output column. Matching spelling alone is insufficient. Access to an inapplicable field must preserve applicability separately from an optional value. The exact authoring surface for such projections remains a review question; narrowing first is already sufficient for the examples above.

### A structured stream may be finite

```incan
import std.async

async def print_stream(
    source: StructuredStream[CompilerFact, DataError]
) -> Result[None, DataError]:
    async for fact in source?:
        match fact:
            CompilerFact.Calls(call) =>
                print(f"Call at {call.location.file}:{call.location.line}")
            CompilerFact.Inlines(inline) =>
                print(f"Inline {inline.caller} -> {inline.callee}")
    return Ok(None)

async def demonstrate_stream() -> Result[None, DataError]:
    # This adapter streams the finite fixture; it does not spawn a task.
    source = sample_facts().stream()
    await print_stream(source)?
    return Ok(None)
```

`Dataset.stream()` is a proposed adapter to RFC 142's fallible async advancement contract. An in-memory batch may be ready immediately; async syntax does not itself imply background work. A file-backed adapter can await input instead. End of a finite source completes the loop; a continuing source with no new records waits. The latter must not return exhaustion merely because it is temporarily caught up.

The same selection and projection behavior must be available to structured streams, preserving source ordering and laziness. Async batch consumers reuse RFC 142 rather than defining another `for_each_batch` or another `ErrorPolicy` enum here. Each source must identify how its resources are acquired and closed; wrapping an iterator does not grant automatic awaited shutdown when a loop breaks.

### Changes are records with explicit meaning

A continuing database source does not repeatedly present the latest table as though every row were newly inserted. The following application-defined envelope demonstrates the required distinctions without settling a universal change-envelope API:

```incan
enum CompilerChange:
    Inserted(key: str, value: CompilerFact)
    Updated(key: str, value: CompilerFact)
    Deleted(key: str)

async def print_changes(
    source: StructuredStream[CompilerChange, DataError]
) -> Result[None, DataError]:
    async for change in source?:
        match change:
            CompilerChange.Inserted(key, value) =>
                print(f"Inserted {key}")
            CompilerChange.Updated(key, value) =>
                print(f"Updated {key}")
            CompilerChange.Deleted(key) =>
                print(f"Deleted {key}")
    return Ok(None)

async def demonstrate_changes() -> Result[None, DataError]:
    changes = Dataset[CompilerChange].from_values([
        CompilerChange.Deleted(key="old-call-site"),
    ])
    # A finite fixture for the consumer, not a live database subscription.
    await print_changes(changes.stream())?
    return Ok(None)
```

`print_changes` receives its source as an explicit dependency. A future database change adapter supplies that same shape, with source positions and transaction grouping supplied by its declared envelope or metadata contract. This RFC does not pretend that such a subscription already exists. RFC 165's bounded snapshot differences are not automatically a retained event log: a source must state whether it emits every committed transition or coalesced changes between selections.

An update carries the new value in this example; it does not promise that a previous value is retained. Deletion preserves identity even when no payload remains. Delivery does not imply that the application's work succeeded. A consumer records resumable progress only according to the source's declared acknowledgment contract, after its intended effects, with duplicate-delivery handling when effects and progress cannot commit together.

## Reference-level explanation

### Declared variants and sparse representation

A dataset must declare the model or finite variant set it accepts. An adapter must validate incoming type and schema identities before exposing values as those types. An unknown variant must produce a typed schema failure or enter an explicitly declared unknown-record representation; it must not be silently dropped or coerced into another model.

The schema may describe fields across many variants. Record storage must not require a slot or placeholder for every field of every other variant. Schema metadata may be shared at collection or batch scope. An implementation may partition by variant, retain tagged records, or combine layouts. It must document and measure its per-record and per-batch overhead rather than infer efficiency from a sparse API.

Structural inapplicability, a declared optional value of `None`, and a present value must remain distinct. A required field missing from a record of its own model is invalid data, not ordinary sparsity. An unselected field in a projection is not evidence that the original model lacked it. A partial projection must not masquerade as a complete reconstructed model.

### Type-aware selection and field identity

Variant narrowing must preserve model types and provenance. Field selection must bind checked declaring-type, variant, and field identities, including nested paths and schema versions where required. Names are presentation, not sufficient identity. Cross-variant alignment must explicitly declare compatible output types and applicability behavior; no automatic stringification or name-based merge is permitted.

Adapters must distinguish indexed selection from sequential filtering and direct projected access from bounded decode-and-project fallback. Work accounting must include rejected candidates and unselected bytes decoded by a codec. A caller must be able to require a supported direct projection and receive a typed refusal when unavailable. Supporting a projection does not imply that every predicate or a query optimizer exists.

### Batches, resources, and reconstruction

A batch must carry its typed projection, row count, and any record/revision or progress metadata promised by its source. Row and byte limits must bound delivery and working state; independently retained consumer batches count against their own live resource owners. Batch size is not a transaction boundary or a global bound on consumer-retained memory.

Sources must not decode the entire dataset merely to open a cursor, determine its variant set, or produce its first batch. A caller stopping early must prevent further demand-driven advancement. Explicit prefetch must have a declared bound. A non-pausable source must declare a bounded buffering or loss/error policy; downstream laziness alone is not proof of upstream backpressure.

RFC 164 governs borrowed columns, exclusive access, and owned retention. Ordinary users must be able to consume batches through duckborrowing. The primitive declarations must expose actual ownership and source dependencies. Adapters must not silently clone an entire batch, acquire indefinite retention, or allow dangling views to make an escaping borrow compile.

Crumb remains responsible for complete typed reconstruction under RFC 165. Dataset adapters must preserve model/variant and schema information needed by that contract. A projected batch need not contain enough data to reconstruct the original model. Reconstruction must not bypass validation or expose protected fields through errors or schema diagnostics.

### Completion, ordering, and errors

Sources must declare finite versus continuing behavior and their ordering guarantees. A finite snapshot-backed dataset observes one consistent selection. A continuing source may span revisions, but its batches and progress metadata must identify the source ordering and consistency boundaries actually guaranteed. There is no inferred total order across independent partitions.

Temporary lack of data must not signal completion. A bounded traversal or scan stopped by a total work limit must return explicit limited progress or a typed limit failure; it must not return ordinary exhaustion. Cancellation and acquisition, advancement, decode, and application callback failures must remain distinguishable. Source-specific typed errors must remain recoverable through any common error umbrella.

RFC 142 owns FailFast, CollectAll, and Ignore consumer behavior. These policies must not turn a damaged cursor into a healthy one or swallow source corruption as exhaustion. A rejected individual record can be delivered as an explicit `Result` item when the source can safely locate and advance past it. A failure to establish the next record boundary is a source-advancement failure. CollectAll must not require retaining an unbounded list of failures for an infinite source; reporting and progress follow the bounded consumer contract.

### Change sources and resumption

A change source must state its key identity, inserted/updated/deleted semantics, value availability, and whether changes are an event history or a coalesced difference. Stable delivery positions must be distinct from record identities, row offsets, and model content hashes. Transactions spanning several batches must retain explicit grouping and completion information where atomic application is required; a consumer must not infer a commit from a batch boundary.

A resumable source must bind its token to source identity, schema/selection, ordering, and retained history. It must define whether resumption begins after or at the acknowledged position and whether duplicate delivery is possible. Unavailable history must produce a typed gap or expiration result; a source must not silently resume at the latest position.

If an adapter offers an initial snapshot followed by changes, it must establish a consistent handoff with a declared boundary, protecting required history or refusing the operation. Reading a snapshot and subsequently starting an unrelated subscription is insufficient because changes can be missed between them. Filtering a stream must still preserve progress over examined but nonmatching events.

This RFC does not promise exactly-once effects. Atomic publication of application changes and source progress requires a compatible transactional mechanism; an arbitrary callback and an external checkpoint cannot gain that guarantee from a stream wrapper.

## Design details

The proposed `Dataset[T]` describes a finite typed source, while `StructuredStream[T, E]` adapts typed records to RFC 142's fallible async protocol. Whether one shared source trait should underlie both public names remains open. Neither requires a new language keyword.

| Owning RFC | Responsibility |
| --- | --- |
| RFC 122 | Model construction and validation |
| RFC 142 | Async advancement, lazy consumers, cancellation composition, and consumer error policies |
| RFC 164 | Borrowing, ownership, and safe retained access |
| RFC 165 | Typed storage, Crumb, indexes, transactions, snapshots, and bounded storage reads |
| RFC 166 | Graph identity, typed relationships, adjacency, and traversal |
| RFC 169 | Shared heterogeneous schemas, type-aware projections, structured batches, and stream data semantics |

RFC 165's existing typed family and single-family projection contracts remain valid independently. RFC 166 may support several model payloads through a declared enum or explicit family adapters without first implementing this entire RFC. This RFC supplies their common dataset and streaming interface; it must not become an accidental prerequisite for basic graph storage or the RFC 165 database primitive.

Module ownership is directional: dataset definitions and consumption protocols must not require a database or a graph package. Database and graph adapters consume those neutral interfaces. Primitive codec and platform layers must not import the higher-level dataset surface.

## Alternatives considered

- **One giant model of optional fields:** confuses fields that do not apply with valid optional values and weakens type-specific validation. It may also impose per-record width unrelated to the actual model.
- **An untyped dictionary for every record:** loses checked field identity and makes model reconstruction and projection errors runtime conventions.
- **Separate unrelated APIs for every model family:** preserves types but forces repeated batching, source-lifetime, and change-processing machinery into each consumer. Separate physical storage remains an acceptable implementation choice beneath a shared interface.
- **Define everything inside the graph RFC:** hides general dataset semantics inside a domain-specific library and makes file and database consumers depend on graph concepts.
- **Adopt a full structured-streaming engine at once:** adds execution planning and event-time semantics before the shared typed data boundary is understood. Such an engine can consume this interface without being required to scan records.

## Drawbacks

Variant-aware schemas and typed projections introduce more descriptor and diagnostic work than homogeneous row iteration. Cheap field access depends on physical encoding, so adapters need observable capabilities rather than blanket performance claims. Resumable change sources impose retention and ordering requirements beyond ordinary iterators. The proposed API must stay small enough that consumers who only want complete models can use ordinary iteration without learning projection internals.

## Layers affected

- **Stdlib and runtime:** neutral dataset/stream definitions, bounded batch access, and adapters over model, file, database, and graph sources.
- **Type checking and descriptors:** checked variant narrowing and field projections, with explicit diagnostics for invalid fields and incompatible alignments.
- **Ownership checking:** batch and column lifetimes, cursor advancement, and explicit retained materialization under RFC 164.
- **Tooling and documentation:** visible source capabilities, schema inspection, concrete examples, and diagnostics distinguishing absence, optionality, limits, and exhaustion.

## Unresolved questions

- Should the public module be `std.dataset`, and should `Dataset[T]` and `StructuredStream[T, E]` be separate public wrappers over a common source protocol?
- What checked field-descriptor and projection spelling should be used, including explicit cross-variant alignment and the representation of field applicability?
- What minimal shared envelope describes change identity, position, transaction grouping, and new/previous-value availability without imposing database semantics on ordinary file or channel streams?
- Which resume and snapshot-to-change adapters must ship with this surface, and which sources explicitly remain non-replayable?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
