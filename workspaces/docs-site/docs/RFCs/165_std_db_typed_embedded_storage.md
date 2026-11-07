# RFC 165: Typed embedded storage with `std.db`

- **Status:** Planned
- **Created:** 2026-10-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 164 (explicit ownership and borrowing contracts)
    - RFC 024 (extensible derive protocol)
    - RFC 047 (lightweight directed graph types)
    - RFC 085 (field metadata and rename continuity)
    - RFC 086 (schema descriptors and adapters)
    - RFC 094 (context managers)
    - RFC 103 (secret values)
    - RFC 106 (declaration identity and semantic digests)
    - RFC 112 (crash-safe publication and file coordination)
    - RFC 114 (compiled providers and package features)
    - RFC 117 (Loaf project model)
    - RFC 124 (store unit identity and cross-plan artifact sharing)
    - RFC 145 (storage lifecycle, retention and compact evidence)
    - RFC 161 (semantic core IR)
    - RFC 166 (indexed graphs and snapshot-bound graph access)
    - RFC 168 (foundational primitives and dependency direction)
    - RFC 169 (typed record sets and record streams)
    - RFC 173 (walking and reconstructing typed values)
- **Issue:** [#2519](https://github.com/encero-systems/incan/issues/2519)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Introduce `std.db`, an Incan-native embedded, typed, transactional database with persistent disk, temporary spooled-memory and strictly in-memory modes. Incan models define stored values; explicit identities, immutable revisions, indexed relationships and snapshot-bound reads define how those values are retained and connected. The public standard-library API uses one shared platform storage substrate rather than creating a second engine. Compiler tooling, document publishers, interactive applications and higher-level query libraries can share that substrate without sharing domain policy, requiring a server, or importing a query engine. RFC 166 defines the graph-reading interface over these records and ordinary in-memory graphs.

## Primitive lifecycle

The primitive must expose explicit resource acquisition/release, transaction commit/abort and cursor operations independently of RFC 094. RFC 094's `with` adaptation composes those operations: it closes a cursor, aborts an unfinished transaction and releases its resources, while never implicitly committing. Successful explicit commit remains publication; leaving an unfinished transaction cannot publish its writes. The primitive therefore does not depend on context-manager syntax.

Explicit lifecycle operations do not excuse unsafe cleanup on an early return, propagated error or supported unwinding path. The primitive implementation must establish its ownership and cleanup mechanism using available language/runtime facilities and verify failure paths. Fallible finalization must be observable when explicitly requested; destructor fallback must not be used to claim successful commit, confirmed rollback or durable publication. Any unsupported cleanup or borrowing requirement must be tracked as a concrete compiler/runtime gap rather than hidden behind a `with` example.

The public API remains Incan-native and duckborrowed. Explicit ownership and lifetime brands belong at the primitive boundary where they are needed to express resource dependencies. Scoped examples describe the RFC 094 integration; primitive storage operations do not require that syntax.

## Core model

1. A **family** gives an Incan record model a stable storage name and a schema and codec contract.
2. A **record identity** names one logical object in that family. Equal contents do not make two objects the same object.
3. A **revision** is an immutable value or tombstone for that identity, with a canonical Crumb representation when encoded. Keeping a checked typed value in memory does not require encoding it eagerly.
4. A **view** selects family definitions, revisions and index roots. A **snapshot** pins one immutable selection.
5. A **transaction** stages changes across families and indexes and publishes one successor view atomically.
6. A **relationship** is a typed, identified record connecting declared endpoint families. It is not an arbitrary string predicate or a second authoritative graph.
7. A **read trace** records the values, existence tests and selections a computation observed. A **change page** describes committed differences; neither executes application callbacks.

The public layers are:

```text
Application models, producers, query and rendering libraries
                         |
           std.db typed storage and snapshots
                         |
             incan_platform.store substrate
                         |
        std.primitives: Crumb, bytes, file I/O
                         |
          mapping, durable I/O and coordination

std.db graph views implement RFC 166's storage-independent
std.graph reading interface; std.graph does not import std.db.
```

## Motivation

Applications repeatedly need stable records, revision visibility, forward and reverse relationships, reproducible reads and durable publication. Implementing these independently produces incompatible caches, identity schemes and recovery rules. Exporting a complete graph into a second store also creates a synchronization problem and makes small questions proportional to a whole project.

A compiler needs selected declarations and their dependencies. A document publisher needs source, include, parse, route and render relationships. An interactive application needs small state snapshots, draft branches and exact references to the artifacts presented to a user. Analytical and retrieval libraries need inspectable inputs and outputs. These workloads share storage mechanics, not the meaning of their records or the policy governing their use.

The standard library should offer a reusable, native foundation. A database must remain optional: an in-memory graph, a static renderer and an ordinary command-line program must not acquire durable storage merely by using another standard module.

## Goals

- Make typed records and relationships directly usable from Incan without SQL DDL, an ORM or an application-authored serialization layer.
- Provide atomic, consistent, isolated transactions and explicitly qualified durability.
- Support disk, temporary spooled-memory and strictly in-memory storage through the same typed record and transaction operations.
- Keep ordinary access compatible with duckborrowing; explicit lifetime brands belong to the primitive API.
- Share one storage implementation between toolchain and application consumers through a consumer-neutral API.
- Support cheap view forks, bounded snapshot reads, mapped byte access and explicit owned materialization.
- Support typed point access and column-projected batch scans without making an analytical query engine part of storage.
- Preserve observable differences between absent, deleted, inaccessible, unavailable and damaged data.
- Support incremental consumers through precise read evidence and bounded changes, including negative reads and newly matching records.
- Make memory, copying, I/O, retention and failure behavior inspectable and testable.
- Let applications declare how much history each view keeps, prune it by changing that declaration, and reclaim what it releases.
- Serve static publication and interactive applications without making filesystem support or persistence mandatory.

## Non-Goals

- A SQL or graph-query language, relational optimizer, vector-search algorithm or application-specific ontology.
- A compiler scheduler, renderer, UI event loop, synchronization protocol or distributed database.
- Inferring authorization, approval, causality or domain truth from a stored value or edge.
- Persisting live closures, process addresses, borrowed pointers, sockets, window handles or other runtime resources.
- Making all standard-library imports select a database or a large analytical backend.

## Guide-level explanation

All names introduced below are proposed by this RFC; the examples are not claims that these APIs already ship.

### Store models, not storage-shaped dictionaries

`Record` is a proposed standard derive using the extensible derive mechanism. It adds family and retained-reference metadata to what every supported model already has, RFC 173's walk and reconstruction and the Crumb codec described below; it is not a new language declaration form. RFC 086 is design context, not a prerequisite to implement its schema language. This proposal requires a focused record/codec contract derived from checked model declarations, not that broader adapter system.

```incan
from std import db
from std.db import Record
from std.fs import Path

@derive(Record)
model Page:
    route: str
    title: str

def save_page() -> Result[None, db.OperationError]:
    """Publish one typed page and read its committed revision."""
    with db.open(Path("site.db")) as store:
        pages = store.family[Page]("pages")?
        published = store.view("published")?
        mut transaction = published.begin()?
        transaction.declare(pages)?
        page = transaction.insert(pages, Page(route="/guide/", title="Guide"))?
        receipt = transaction.commit()?
        snapshot = receipt.snapshot()?
        match snapshot.get(pages, page)?:
            db.RecordState.Present(version) => print(version.value.title)
            db.RecordState.Deleted(_) => pass
            db.RecordState.Absent => pass
    return Ok(None)
```

Family registration is explicit. Constructing a family handle does not write a schema; `transaction.declare(family)` validates or stages its declaration in the transaction's selected view. Reads validate the handle against the snapshot's descriptor. Reopening an existing family does not replace its schema because the local model changed. Declaring the family with a compatible changed model stages a successor descriptor in that transaction, and declaring it with an incompatible one is a typed refusal naming the fields. A family has no version number to keep: the model hash says whether the shape changed, the field identities say whether the change is compatible, and a new meaning takes a new name. This avoids SQL ceremony without hiding schema evolution.

`db.memory()` creates an isolated memory store with the same logical record and transaction semantics. Its receipts report memory durability. It does not create files or silently spill to disk. `db.open(path)` constructs a lazy database scope descriptor; entering it opens or creates a local-durable store directory after validating its format and capabilities. Opening an existing unrelated directory must fail rather than adopt or overwrite its contents.

### Crumble a model and reconstruct its type

Crumb is the typed encoding contract in `std.primitives.crumb`. It is usable without opening a database. A model with supported fields exposes `.crumble()` and `Model.from_crumb(...)`; RFC 173's `__walk__`, `__shape__` and `__reconstruct__` carry the checked field definitions to library-owned encoding and reconstruction rather than requiring a handwritten serializer for each field. Supported models acquire these protocols automatically, including their private stored fields. `Crumb` is an automatic derive of every model, enum and newtype whose fields all support it, so none of them writes `@derive` for this. `Record` composes that capability with database family and retained-reference metadata. Using Crumb itself must not require importing `std.db` or registering a family.

The `Page` model above provides a complete small example. `CrumbError` is the proposed primitive error umbrella; both conversion operations are fallible:

```incan
from std.primitives.crumb import CrumbError

def restore_page() -> Result[Page, CrumbError]:
    original = Page(route="/guide/", title="Guide")
    crumb = original.crumble()?
    restored = Page.from_crumb(crumb)?
    assert restored.route == original.route
    assert restored.title == original.title
    return Ok(restored)
```

`restored` is a `Page`, not a dictionary or an untyped bag of bytes. Nested models, payload enums, supported collections and optional values retain their declared structure. Reconstruction restores data, not the identity of a former live object or its process resources. An unsupported field must be refused where the value is crumbled, at `@derive(Record)` and as an unmet bound through a type parameter, with a diagnostic that identifies the field path; it must not disappear from the result.

A crumb carries the typed encoded value or owns the backing handles sufficient to reconstruct it. A model representation may compose field crumbs; it is not merely an unretained hash whose bytes might already have been collected. Exporting a representation with external field references must include or explicitly retain the required contents. A hash identifies candidate contents, not their continued availability.

The storage implementation may keep scalars inline, share immutable typed fields in memory, encode field values, or use a combination. Calling `.crumble()` explicitly requests an encoded representation; ordinary model access and database storage do not require repeatedly serializing and deserializing every field. Decoding and copying remain observable work. The format and physical layout must be qualified using composed application and compiler-shaped records, including distinct source locations, rather than assuming all repeated instructions have identical provenance.

A crumb keeps working as its model changes, because it matches fields by a small identity rather than by name or position. The compiler computes a field's identity from the earliest name the field has had, so nobody assigns one:

```incan
# A crumb written with this model
model Invoice:
    id: int
    customer: str
    total: int

# reads with this one, with no migration
model Invoice:
    total: int
    id: int
    buyer [renamed_from="customer"]: str
    note: Option[str]
```

`buyer` has the identity of `customer`, so its values arrive there, and reordering the fields changes nothing. `note` is not in the old crumb, so it reads as `None`, and an older reader given a newer crumb skips `note`. A required field that a crumb does not hold is a typed failure, and so is a value whose written type does not fit its field: an `int` does not read as a `float`.

Additive changes are preferred. Dropping a field does not erase it: its values stay in existing crumbs, and they read again if the name returns, because it is the same field.

A crumb can also say whether two values are the same. Its canonical hash covers the fields that define the value, and its object hash covers every field. A field that records when something arrived rather than what it is can opt out of the first:

```incan
from std.datetime import DateTime

model Reading:
    sensor: str
    value: float
    received_at [canonical=false]: DateTime

def same_reading(morning: DateTime, evening: DateTime) -> Result[None, CrumbError]:
    first = Reading(sensor="a1", value=20.5, received_at=morning).crumble()?
    second = Reading(sensor="a1", value=20.5, received_at=evening).crumble()?
    assert first.canonical_hash() == second.canonical_hash()
    assert first.object_hash() != second.object_hash()
    return Ok(None)
```

`first` and `second` are the same reading, stored at different times.

### Fluent lazy resource scopes

The public `db.open(path).read()` chain constructs a lazy read scope. Constructing the chain does not perform the database acquisition or begin a transaction. Entering `with` opens the database and begins a consistent read; only successful entry binds the reader. The following helper uses the `Page` model declared above and assumes an existing table with string keys and Page values:

```incan
def load_page(path: Path, route: str) -> Result[Option[Page], db.OperationError]:
    with db.open(path).read() as reader:
        pages = reader.table[str, Page]("pages")?
        return Ok(pages.get(route)?)
```

Table lookup and owned point retrieval are ordinary fallible operations. There is no cursor requirement for a single record. `OperationError` denotes the public operation-error umbrella, retaining typed database failures and applicable RFC 094 scope-failure aggregates rather than reducing failures to text. Its detailed variants and signatures belong in implementation design under this typed-failure contract.

The chain's scope owns the database acquisition and read lifetime. Exit releases the reader and its owned database handle. If opening succeeds but beginning the read fails, entry must release the acquired handle before reporting failure; acquisition and cleanup failures must both survive. Independently retained batches may continue owning their backing resources after the root handle closes. Closing that handle does not promise destruction of all independently retained storage resources.

Fallibility is declared by the scope's entry contract and checked under RFC 094. The lack of a header question mark is not permission to unwrap a Result, panic on an acquisition error or report false success. The enclosing function must provide a compatible propagation or handling boundary. Ordinary eager Result-returning acquisitions continue requiring their explicit handling; this fluent spelling is a lazy library scope, not general method forwarding through Result.

The primitive retains explicit, fallible acquisition and release operations independently of context-manager implementation. Public scope construction and primitive acquisition are different operations; the lazy spelling does not change when the primitive performs I/O.

### Read, write and transaction scopes

The fluent public surface distinguishes read consistency from write publication:

| Scope | Contract |
| --- | --- |
| `.read()` | A read-only transaction. All reads observe one consistent snapshot; exit releases its resources without requiring commit. |
| `.write()` | A writer whose successful modifying operations commit independently. It does not group the whole scope into one transaction. |
| `.transaction()` | A read/write transaction. Reads see its own staged writes; explicit `commit()` publishes the grouped changes atomically. Leaving it unfinished aborts its pending changes. |

These selectors compose with the lazy `db.open(path)` descriptor. They describe the public resource scope, not general forwarding of method calls through Result. The primitive provides equivalent explicit acquisition, operation and release mechanisms without requiring `with` syntax.

A transaction may read a record, inspect its current values and then update or delete it before committing. Subsequent reads inside the transaction observe those staged updates or deletions. The compare-and-publish conflict rule below protects that read/modify/write sequence: a changed base view causes conflict rather than silently publishing a decision made against stale state. Recovery starts a fresh transaction and re-evaluates the operation against its new read state; retry must not blindly replay external effects.

A read in `.read()` followed by a separate operation in `.write()` does not form one protected read/modify/write operation. A caller requiring that boundary selects `.transaction()` up front. A read-only scope cannot silently upgrade to a writable transaction. Where the scope type is statically known, unsupported writes receive a diagnostic; dynamically admitted access must return a typed refusal. An explicit read/write transaction may perform only reads; it does not have to modify records merely because it permits writes.

Cursors execute operations and retain scan state; they do not select commit policy. Closing a cursor releases cursor resources without committing or rolling back its enclosing transaction. Successful writes through multiple cursors in one explicit transaction remain staged together. Writes through an ordinary writer retain their independent operation boundaries, and a later failure does not undo earlier committed operations. A rejected operation must not automatically invalidate an ordinary writer when the engine can establish that the operation left it usable; underlying resource or integrity failures may still prevent continued use.

Single-record reads remain available directly through a typed table without constructing a cursor. Bulk-operation boundaries must be specified by their public operation contract, never by buffer size, internal flushes or the spool threshold. An explicit transaction must expose whether it remains usable after an operation failure. If the implementation cannot establish a valid staged state, it must refuse further work except abort or outcome resolution; it must not continue with silently partial changes.

### Choose storage placement without changing the data contract

The same typed Incan records and logical transaction operations are available in three explicit modes:

| Mode | Storage and lifetime contract |
| --- | --- |
| Disk | Persistent records and indexes live on disk, with bounded memory caches and working buffers. The complete database need not fit in RAM. Durable success remains subject to the qualified filesystem profile. |
| Spooled memory | Temporary storage starts in memory and spills to temporary disk storage when its configured memory threshold requires it. It supports incremental write buffering, read-ahead staging and short-lived caches without promising persistence across restarts. |
| Memory | Retained contents stay in memory, and the store has no total size of its own to exhaust. A record or a request over its bound is a typed failure. The database does not silently spill to disk. |

Storage placement does not change table types, record identities, transaction atomicity or a reader's selected contents. Spooling must preserve active snapshots, cursors and borrowed data. Migration must not relocate or release buffers still being borrowed. If retained buffers prevent meeting the memory bound, report resource exhaustion rather than invalidate a borrower or silently exceed the bound. The primitive implementation owns placement and retention mechanics; ordinary callers use duckborrowing without naming lifetime brands.

Spilling and flushing are not committing. A cursor may stage writes incrementally across many input batches while one transaction remains unpublished. Persistent and spooled storage may stage encoded data on disk without retaining the complete decoded input in RAM. A caller may instead commit each batch in its own transaction, deliberately accepting that earlier committed batches survive later failures. No buffer threshold may silently select that policy for the caller. Memory mode still needs memory for its retained contents; incremental input avoids additional whole-input retention, not the storage cost of those contents.

Spooled mode is temporary even after data reaches disk. Normal release reclaims temporary storage once its owners and retained readers release it; active retention must not be revoked merely to remove a spool file. Interrupted processes may leave temporary artifacts requiring recovery cleanup, but those artifacts are not a supported persistent database or a restart-durability promise. Failure to acquire or extend temporary storage is a typed failure, not a silent switch to another storage mode.

Read-ahead is an access optimization that can also serve disk mode. Spooled mode describes retention and placement of temporary data, not exclusive ownership of the read-ahead mechanism. Inspection must distinguish logical stored data, working-memory use, temporary disk use and retained buffers. Buffering may improve throughput for a workload; no universal speedup is claimed.

Detailed spooled-store constructor and tuning signatures belong in implementation design under the three-mode contract. The proposed `db.memory()` and `db.open(path)` names do not imply that these APIs already ship.

### Fork a view without copying its records

```incan
def open_preview(store: db.Store) -> Result[db.View, db.DbError]:
    """Create an independent preview from the currently published selection."""
    published = store.view("published")?
    base = published.snapshot()?
    return store.fork(base, "preview")
```

The preview begins with the same selected records. Editing it does not edit `published`. A preview is not automatically promoted, merged or trusted. The application must choose and validate a publication operation. View identity is not an authorization boundary.

`store.view(name)` opens a named view or atomically creates an empty one if it does not exist. `store.fork(snapshot, name)` requires a new name and must not replace an existing view. These root operations follow the same atomic publication and durability rules as record transactions.

### Keep and reclaim history

Every commit replaces a view's current revision, and the replaced revision stays readable for a while. That history lets a reader time travel to an earlier revision, and lets a consumer that saved its position resume after a restart instead of reloading everything (RFC 169). History costs disk, so each view has a policy that says how much to keep. Without one, a disk store keeps superseded revisions for 24 hours. An application that needs something else declares it. A site that wants to roll back a bad publish might keep a week:

```incan
from std import db
from std.datetime import Duration

def keep_rollback_history(store: db.Store) -> Result[None, db.DbError]:
    """Keep a week of published revisions, and at least the last 50, so a bad publish can be rolled back."""
    published = store.view("published")?
    published.set_history(db.HistoryPolicy(expire_after=Some(Duration.days(7)), keep_last=50))?
    return Ok(None)

def run_nightly_maintenance(store: db.Store) -> Result[None, db.DbError]:
    """Reclaim released history and report anything a live reader is still holding."""
    report = store.compact()?
    print(f"reclaimed {report.reclaimed_bytes} bytes from {report.removed_revisions} revisions")
    for hold in report.held:
        print(f"{hold.holder} still holds revision {hold.revision} of {hold.view}")
    return Ok(None)
```

`expire_after` counts from when a revision was replaced, so a revision that was current for a month still stays readable for a further week. `keep_last` keeps at least that many superseded revisions however old they are; either rule keeps a revision. `expire_after=None` keeps everything until the policy changes, and `expire_after=Some(Duration.seconds(0))` keeps only the current revision. Shortening a policy is how an application prunes history. A released revision cannot be read again, even while its bytes are still on disk.

Memory and spooled-memory stores keep only the current revision unless a view declares otherwise. Their contents do not survive a restart, so there is nothing to resume, and an interactive application publishing many small states must not hold a day of them in working memory.

A live reader keeps what it is reading, past the policy if it has to. A saved position does not: a consumer that stays away longer than the policy gets `DataError.HistoryGap` when it returns, and reloads. `view.history()` reports the oldest readable revision and who is holding anything older, so a reader that never finishes is visible rather than a mystery on a full disk.

Ordinary writes reclaim released history a little at a time, so a store nobody maintains still stays within its policy. `store.compact()` is the vacuum: it reclaims all released history that no live reader holds, rewrites what remains in bounded memory, and reports what it did. Compression happens inside that rewrite; it is not a separate operation and it never changes a record's identity.

### Let records age out

History expires revisions that were replaced, but an event or a login session is often never replaced, so it would stay forever. A family can say how long its records last:

```incan
from std import db
from std.datetime import Duration

def keep_a_day_of_events(store: db.Store) -> Result[None, db.DbError]:
    """Let each event age out a day after it was written."""
    events = store.family[Event]("events")?
    events.set_retention(db.RecordRetention(expire_after=Some(Duration.days(1))))?
    return Ok(None)
```

A write may set a different age for one record, such as `transaction.insert(sessions, session, expire_after=Duration.days(30))`. Writing a record again starts its age over. An expired record reads as deleted, and a consumer following changes sees its deletion, so nothing disappears silently.

### Connect typed families

A publisher can declare a relationship from `Document` records to `Asset` records with an `AssetUse` payload. The relationship descriptor names both endpoint families, its payload schema and uniqueness constraints. Creating an edge with reversed or incompatible endpoints is rejected. Relationships have their own identities, so two different uses of one asset need not collapse into one edge.

The same mechanism can represent a diagnostic referring to a declaration, a surface referring to a model revision, or an activity citing evidence. The database preserves those distinctions; it does not decide that any of them establishes ownership or approval.

RFC 166 exposes indexed traversal of these relationships without exporting every node and edge into a separate graph container.

### Incremental readers and interactive state

A reader pins a snapshot, reads selected fields or records, and optionally captures a read trace. After an edit, it can validate that trace against a successor snapshot or enumerate bounded changes. A renderer may reuse an unchanged projection; a compiler may reuse a derived answer. The caller owns the computation and its semantic fingerprint.

Interactive state may remain in a memory store. A render pass reads one snapshot; a later transaction publishes the next state. Callbacks and target resources remain outside the store and resolve record identities through application-owned bindings. A committed state change does not execute an action, send an event or establish that an external effect succeeded.

### An embedded database without a mandatory query language

An application may use `std.db` directly as its local transactional database: open a disk, temporary spooled-memory or memory store, register typed families, update records, follow indexed relationships and read a consistent snapshot. A query library may instead consume bounded typed batches from that same snapshot and supply joins, grouping, sorting or analytical expressions. Both use the same storage and publication contracts; neither requires SQL.

Model-shaped data does not imply one heap-allocated model per scanned record. Selecting two numeric fields from a wide family must have a batch path that does not decode every unrelated field or construct every complete record. Indexed application reads and analytical scans are complementary access patterns, not separate databases containing duplicate authoritative records.

## Reference-level explanation

### Public surface and identities

The following are required concepts and proposed public names. Generic family and relation arguments retain their model types through the API; implementations must not erase them to strings at the authoring boundary.

| Surface               | Required contract                                                                                                    |
| --------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `Store`               | Own a disk, temporary spooled-memory or memory store; expose placement, capabilities and format identity.                      |
| `Family[T]`           | Bind `T` to one family identity and codec profile; an incompatible model is a typed error.                           |
| `RecordId[T]`         | Name a logical record of `T`; include or retain its originating store and family scope.                              |
| `RevisionRef[T]`      | Pin an exact immutable revision and its logical record identity.                                                     |
| `ObservedRevision[T]` | Record an exact revision identity as evidence without retaining its payload; access reports availability explicitly. |
| `View`                | Name a mutable binding root inside a store; publish record changes only through transactions.                        |
| `Snapshot`            | Hold an immutable selection and a lease over everything required to read it.                                         |
| `Transaction`         | Stage one view's multi-family changes, supporting read-your-writes and atomic commit or abort.                       |
| `RecordState[T]`      | `Present(RecordVersion[T])`, `Deleted(Tombstone)` or `Absent`; failures use `DbError`.                               |
| `Index[T, K]`         | A declared typed key projection and ordering profile over one family, maintained atomically with its records.        |
| `Relation[S, T, P]`   | Identified relationship records from family `S` to family `T` with payload `P`.                                      |
| `PayloadRef`          | An immutable, integrity-checked external payload with explicit ownership and retention references.                   |
| `PayloadIdentity`     | A non-retaining description of immutable payload identity, not a promise that its bytes remain available.            |
| `ReadTrace`           | Snapshot and producer/profile-bound observations suitable for conservative reuse validation.                         |
| `ChangePage`          | Bounded committed differences between two identified snapshots, with explicit continuation.                          |
| `CommitReceipt`       | Transaction identity, prior and successor view revisions, changed roots and achieved durability.                     |
| `Reservation`         | A coordinated, bounded allocation allowance for staging, admission or compaction, released when the operation ends.  |
| `HistoryPolicy`       | How long a view keeps superseded revisions: `expire_after` and `keep_last`; either rule keeps a revision.            |
| `RecordRetention`     | How long a family keeps its records: `expire_after`, counted from the current revision's write, and adjustable per write. |
| `HistoryReport`       | A view's effective policy, oldest readable revision, readable revisions, history bytes and holders of released revisions. |
| `CompactionReport`    | The revisions `compact()` removed, the bytes it reclaimed and each released revision it could not remove, with its holder. |
| `RecordBatch[P]`      | RFC 169's batch type: here a bounded typed field projection from one pinned family snapshot, with explicit buffer ownership and continuation. |

Persistent record identity must not be a process pointer, row offset, index slot, graph-local integer or filesystem path. Physical deduplication must not collapse distinct logical records. Revisions and view revisions must distinguish logical content identity from physical layout generation; compaction must not appear as a logical record edit.

A bare identifier is not authority to read another store or inaccessible root. Lookups must validate scope. Import must preserve origin identity or return an explicit remapping; it must not silently declare two foreign identities equivalent because their local integer or payload digest matches.

### Records, record sets and record streams

A record is one typed value. Collections use RFC 169's `RecordSet[T]` and `RecordStream[T]`: a record set is finite and lazy, while a record stream is asynchronous and may wait for more records. Neither name implies that all records are held in memory. Database adapters expose those interfaces; the Crumb primitive owns the encoded records and must not import `std.data`.

For example, a snapshot of stored customers can be read as a `RecordSet[Customer]`, while a continuing feed of customer changes is a `RecordStream` of change records. Both decode records as reading advances, in bounded portions. A stream need not end before its next record can be validated and delivered. RFC 169 owns batching, completion and stream-error behavior; decoding does not turn a batch boundary into a transaction boundary.

The encoding rules below apply to individual records and to the encoded portions read from a record set or record stream. A codec profile must define their byte boundaries and how model and field metadata are shared. These binary-format details do not introduce another public collection type.

### Typed values, schemas and encoding

`Record` must describe supported fields, their types, field identities and deterministic encoding through `std.primitives.crumb`. Crumb owns encoding, typed reconstruction, schema/version checks and decoding errors. The database owns record identity, field indexes, deduplication policy, transactions and retention. Crumb must not import database or compiler implementation services. Primitive values, enums including `Result`, `Option`, unions, lists, tuples, sets, dicts, newtypes, other record models and typed record references must compose. Unsupported resources, cycles through live heap references and unbound runtime state must produce a declaration or encoding error, not an opaque dump of memory.

A stored value must decode to its declared Incan model. This does not require serialized bytes to have the layout of a native model instance. On-disk encoding must not depend on native struct padding, process addresses, hash-map iteration order or host endianness. A codec profile must define the arrangement of the bytes explicitly and must keep every distinction its hash profile draws, so a value reconstructed with its original written shape has the hash it was written with. Compatible schema adaptation is governed separately below. Encoded records must identify their codec profile, either directly or through shared metadata in the encoded portion, and a reader that does not support that profile must return a typed failure rather than guess. A matching model is not evidence of a matching arrangement.

Crumb conversion must preserve declared model data and validate the requested model/schema before exposing a reconstructed value. Incompatible types or versions, missing referenced contents, malformed lengths, damaged bytes and unsupported resources must return typed failures. Decoding must be bounded, as *Reading untrusted records* sets out, and consume exactly the encoded portion selected for that read. Stored data must not supply executable reconstruction code. Conversion must use RFC 173's model-owned reconstruction contract: restore stored fields and enforce final validation without replaying fieldmethods or virtual construction inputs. Private fields remain part of the stored data; their ordinary constructor-input visibility remains unchanged. Reconstruction is not a bypass for producing an invalid model. `SecretStr` and `SecretBytes` follow RFC 103: a model that holds either, directly or nested, must be refused during checking when it is crumbled or stored, with a diagnostic naming the field path. Encoded records and hashes must therefore never hold a secret's contents or anything derived from them. A model that records that a secret existed declares RFC 103's `RedactedSecret`, which is ordinary, forgeable data and cannot restore the secret.

Field lookup must bind the selected field identity, declared value type and equality profile. A content hash locates candidates; the implementation must verify the actual typed value before treating a candidate as equal. Forced collisions must not alias distinct values or records. Range order must follow the declared value semantics rather than arbitrary digest or encoded-byte order. Field lookup is independent of a particular hash algorithm, and Crumb's format is independent of a mandatory per-field blob layout.

Custom codecs may implement the same typed contract. Their identity, schema, determinism and resource limits must be inspectable. Executable codec code is supplied by the installed program, not loaded and executed from an imported record set.

The record contract must also enumerate retained typed revision and payload references, including those nested in supported containers. Collection must not guess references by scanning arbitrary bytes. Custom codecs must supply the same complete reference description. A reference to a foreign store does not implicitly keep that store alive; imported local contents or an explicit cross-store lease agreement are required before it can supply a retained local read.

Retaining references and historical observations must be distinct types. `ObservedRevision[T]` and `PayloadIdentity` preserve evidence without registering immortal retention roots; using either requires an availability check and acquisition of a live lease before reading. Removing that payload does not rewrite the historical observation. Conversely, the store must not expire an active `RevisionRef[T]` or `PayloadRef` as if it were only an observation.

Changing a schema or codec requires an explicit migration or an explicitly compatible descriptor. A migration names its source and target by their models, and their model hashes identify them. A field renamed with RFC 085's `renamed_from` is such a descriptor, so its stored records need no migration. Reordering fields, dropping a field and adding a field that reads as `None` or empty when absent are compatible in the same way, because a reader matches fields by identity. Migration must stage and validate a successor selection before publication; old leased snapshots must remain readable using their declared profiles, or the operation must refuse. Unsupported versions must never be treated as absent records.

### Field identities

A field's identity is the first 6 bytes of the SHA-256 of the UTF-8 bytes of its earliest name, which is the first name in its RFC 085 `renamed_from` when it has one and its own name otherwise. An enum variant's identity follows the same rule. A model's or an enum's own identity follows it from the declaration's earliest qualified name, taken within its package's canonical identity.

- **The package identity:** a package that stores records declares its canonical identity in `loaf.toml` before it is ever published. That identity is its RFC 125 scoped name (`<scope>/<name>`) and the registry it publishes to, which is `loaves.pub` when it names none.
    - The registry is recorded by its RFC 117 registry id, never by a hostname, an endpoint or a local alias.
    - A package without a scoped name cannot encode records, so no persistent identity is ever produced from an incomplete one.
    - Publishing verifies that the package is published under its declared identity, and refuses rather than selecting another.
- **The hash input:** a model's or an enum's identity is the first 6 bytes of the SHA-256 of three UTF-8 strings, each followed by a zero byte: the registry id, the scoped package name, and the declaration's module path and name joined by dots, such as `customers.Customer`. A dependency key in `loaf.toml` is a namespace a consumer chooses, so it never enters an identity.
- **Collisions:** two packages named `crm`, in two registries or under two scopes, that each declare `customers.Customer` declare two models with two identities, as do two modules in one package.
- **Mirrors:** a mirror serves the same registry identity, so it changes nothing.
- **Renames and moves:** a model that moves to another module, or takes another name, keeps its identity by naming its earlier module path and name in `renamed_from`. A package that is renamed or moves to another registry renames its models. For such a move, each model's nested `renamed_from` block also names the earlier `package` (a scoped name) and the earlier `registry` (a registry id); each defaults to the declaring package's own (RFC 085).

A model identity keeps two models apart. It does not authenticate a record, because anyone can copy an identity into bytes they write (see "Reading untrusted frames"). The compiler derives identities from checked declarations and exposes them through RFC 173, and authors neither write nor assign them.

- Encoded records must identify their models, and reading a record as another model must return a typed failure. A model that moves to another module or package, or takes another name, keeps its identity through `renamed_from`, as "Renames and moves" above describes. A model whose fields are all optional therefore cannot read an unrelated crumb as a value of all `None`s, including one written by a same-named model elsewhere.
- Each written shape must list its model's field identities once, in ascending byte order, rather than tagging every value; records sharing that shape may share its metadata. That order keeps declaration order out of the encoding and lets a reader find a field without scanning the list.
- A reader must match values to fields by identity, never by name or declared position. Renaming a field under RFC 085 or reordering fields must therefore neither misread existing data nor change a value's canonical representation.
- Two fields of one model, or two variants of one enum, with the same identity must produce a declaration error naming both. The author resolves it by naming the newer one differently. Across a model's revisions, a matching identity means the same earliest name, so a reader needs no name to confirm it.
- A value whose identity the reader's model does not declare must be skipped. A field the encoded record does not hold must read as `None` when it is optional and as empty when it is a collection whose declared default is empty. Any other field the encoded record does not hold, including one with another default, must produce a typed failure naming the field, because reading must not depend on a model's current defaults.
- Removing a field's earliest name from `renamed_from` gives the field a new identity, which makes it a dropped field and an added one.
- A name that returns after being dropped is the same field. It has its old identity, and values written before the drop must read again when their type still fits. A field with a new meaning needs a new name. Crumb keeps no history of dropped names; a database built on these primitives may keep that history and refuse a returning name under its own policy.

### What fits a field

Each written shape must say once per field what type its values were written as, in enough detail to skip a value and to refuse a misfit using the encoded data and its shared metadata alone. The following type mappings must preserve the canonical representation of each retained value; nested model adaptation follows the field-compatibility rules below:

- the same type, with an alias read as its target and a newtype as the type it holds;
- a type that is not an `Option` into `Option[U]` when it fits `U`;
- a narrower integer into a wider one that holds all its values, and `f32` into `f64`;
- a list, set, dict, tuple or `Option` whose element types fit, element by element.

An optional written type keeps its layers: it fits an `Option` only through the last rule, matching optional layers from the outside in before any layer is added. An `Option[int]` read as `Option[Option[int]]` therefore keeps `None` as the outer `None`, and `Some(5)` reads as `Some(Some(5))`.

Nothing else fits: `int` does not read as `float`, `str` does not read as `int`, `Option[T]` does not read as `T`, and `i64` does not read as `i32`. A misfit must return a typed failure naming the field. It must not be skipped or read as absent, because the value is there.

A nested model fits when it has the same model identity, and its own fields follow these rules. An enum value whose variant the reader does not declare must return a typed failure naming the field and the variant. A value that fits still goes through RFC 173's model-owned reconstruction and final validation, so a newtype's or a model's constraints can still refuse it. This must not replay constructor fieldmethods or input normalization.

Compatibility does not promise identical contents after a field is dropped. Reading must leave the stored record and its hashes unchanged. A reconstructed model that omits a stored field is an adapted value; hashing or re-encoding it uses the fields it actually retains, including when the adaptation occurs inside a nested model. The original record's hash must not be presented as that adapted value's hash. For example, reading an earlier `Customer(name="Ana", legacy_code="X")` as a later `Customer(name="Ana")` is compatible, but the later value no longer contains `legacy_code` and need not have the same object or canonical hash. The original crumb still retains the old field and its original hashes. Renames, reordering and additions at the defined absent value keep the hash-preservation guarantees stated below.

A reader must decide structural compatibility once for each written shape rather than once per record, and it hashes no names, because identities are constants of the checked model. Value validation still runs for each reconstructed instance.

### Reading untrusted records

A reader must be safe on any bytes, however they were produced, because a crumb can arrive in a file, a message or a store that someone else wrote. A value a reader returns has passed its model's checks, and it is trusted no more than the source of its bytes.

- **Limits belong to the read.** Every decode runs under limits its caller can inspect and set: the most bytes it reads, the deepest nesting it follows and the most memory it allocates. Bytes that decompression produces count toward the same limits. A reader must refuse nesting deeper than its limit rather than exhaust its stack, including through a recursive model. Exceeding a limit must return a typed failure naming it, never a partial value. The limits bound one read, not a store or a family.
- **Memory follows the input.** A reader must check each declared length and count against the bytes available in the selected encoded portion and against its allocation limit before it allocates anything for them. A small encoded record whose list header claims 4 billion elements therefore fails before anything is allocated, instead of aborting the process on a failed allocation. A cyclic reference within the encoded data must be refused.
- **Reading resolves nothing.** Decoding a standalone crumb must not open a file, contact a network, resolve a package or read a store. A payload or record reference it holds decodes as a reference, and following one is a separate operation under that operation's own scope checks. A read from a store resolves only contents held in that same store, under the reader's snapshot.
- **Consistent decoding.** Readers supporting the same codec profile must interpret encoded records consistently. A reader may reject valid data because it exceeds its configured limits or is incompatible with the requested model. Malformed or non-canonical encodings must be rejected, including a field identity listed twice, identities out of ascending order, a non-canonical length or number, and unconsumed bytes inside a declared encoded portion. The next record in a record set or stream is not trailing garbage. Under a deterministic codec profile, decoding and re-encoding with the original written shape and the same encoding boundaries and shared metadata must reproduce the selected encoded portion byte for byte. This guarantee does not apply after adapting data to a different model revision or regrouping records.
- **Decoding proves nothing about the writer.** An encoded record's model identity, field identities and hashes are claims made by whoever wrote its bytes. Decoding a crumb as `crm.customers.Customer` means its contents fit that model and passed its checks, not that the package declaring the model wrote them. Provenance comes from a signature, a digest kept elsewhere or the store's own records. Field identities are therefore not a security boundary: anyone who encodes a record can already put any fitting value in any field.
- **Direct access validates first.** A reader that exposes values in place, without copying them out of the encoded data, must validate everything a view can reach, including the framing that locates it, before handing out the view.

### What counts as the same value

A hash covers a typed value, never the encoded bytes, so one value has one hash under every codec profile. What a hash covers is the value's canonical representation. Every hash names its hash profile and its algorithm, as in `crumb1:sha256:…`, and hashes under different profiles must not be compared. A change to these rules must take a new profile name, and the earlier profile must stay readable, because stored hashes were computed under it.

The profile `crumb1` defines the following:

- **Floats:** `-0.0` and `0.0` are different values. Every NaN is one value, the quiet NaN with a clear sign and a zero payload. Encoding writes that NaN, so a float holding any other NaN reads back as that one.
- **Strings:** a string is not normalized, so two spellings of one character, such as `é` as one code point and as two, are different values.
- **Numbers:** an integer hashes by its numeric value and an `f32` as the `f64` it equals, so widening a field's type moves no hash.
- **Optionals:** a bare `x` in an `Option` slot is `Some(x)`, and a bare `None` in an `Option[Option[T]]` slot is the outer `None`. `None`, `Some(None)` and `Some(Some(x))` are therefore three different values, in field slots and inside containers alike. `Some(x)` for an `x` that is not `None` has the representation of `x`, so making a field optional moves no hash.
- **Unions:** a union value hashes as its member's type spelling and then its value, so the order a union's members are written in changes nothing.
- **Dicts and sets:** entries hash in the profile's own order, which runs no user code: `false` before `true`, integers numerically, strings and bytes byte by byte, `None` before any other value, tuples and lists element by element with a shorter one first, a newtype as the value it holds, enum values by variant identity and then payload, union values by hashed member spelling and then value, and models field by field in identity order.
- **Fields:** a model's fields hash in ascending identity order. A field holding what it would read as when absent, `None` for an optional or empty for a collection whose declared default is empty, contributes nothing, so adding such a field moves no existing record's hash.
- **References:** a typed reference contributes the identity it names and never its target's contents, so a later revision of the target moves no hash.

These rules define what hashes and stored comparisons treat as the same. They do not change the language's `==`, where `-0.0 == 0.0` stays true and `nan == nan` stays false. A hash or a stored comparison must not call a model's `__eq__`. A field index says which of the two it follows through its equality profile.

### Record hashes

A stored record carries three hashes, and each answers its own question:

| Hash           | Covers                                   | Answers                                  |
| -------------- | ---------------------------------------- | ---------------------------------------- |
| Object hash    | Every stored field                       | Is this exactly what was stored?         |
| Canonical hash | The stored fields that define the record | Is this the same thing?                  |
| Model hash     | The stored shape of its model            | Was this written against the same model? |

A field counts toward the canonical hash unless its declaration opts out with RFC 085's `canonical=false`, and a nested model's fields follow that model's own settings.

The model hash covers each field's identity, declared type and `canonical` setting, with nested models and enums folded in. Crumb computes it under its named profile from RFC 173's checked shape and canonical spelling, and it needs no value; the compiler supplies structural facts without owning the codec hash profile. Documentation, methods, declaration order and the model's path must not enter it, so renaming or reordering fields does not move it. A record whose model hash differs from its family's model needs a migration or a compatible descriptor.

The shape folds fields in identity order. A model already being folded contributes a back-reference counting the levels up to it, so a recursive model has a finite hash. An alias folds as its target, a newtype as the type it holds and a generic as its concrete instantiation, with built-in types in one canonical spelling. A union's members fold sorted by their hashed spelling.

The object hash and the canonical hash must not cover the model hash, because adding an optional field would then make every record look changed. A comparison across models must therefore check the model hash as well.

Two records with equal contents have equal hashes and different identities. A hash shows that a record no longer matches its contents, but evidence of tampering also needs a signature or a digest kept elsewhere. Computing a hash needs no store; recording one beside a record is the database's job.

### Names the store reserves

A name that starts with three underscores belongs to the store, the way two mark a language hook such as `__eq__`. A model must not declare a field with such a name, and doing so is a declaration error. The store's names are a closed list, `___id`, `___revision`, `___model_hash`, `___canonical_hash` and `___object_hash`, and they name those facts wherever a stored record is inspected or exported. Importing a record that holds any other such name must return a typed failure rather than skip it as an unknown field. No hash covers one of these names, so a hash never covers itself, and a value that was never stored has none of them.

### Indexes and relationships

Indexes must have named, typed key projections, declared uniqueness and an ordering profile. A projection's semantic identity and configuration must be part of its index definition. An index must not silently reuse entries produced by different code. Index creation and rebuilding must publish a complete generation atomically or remain unavailable; queries must not mix partial and complete coverage without an explicit partial-result contract.

Point and bounded range access must be possible without decoding unrelated family values. Equality, uniqueness and ordering must follow the declared key semantics, not incidental byte ordering. Indexed fields and relationship directions must be discoverable through schema inspection.

Relationships must validate endpoint families, store scope, reference mode and declared uniqueness in the same transaction that writes them. A logical endpoint resolves at the reader's snapshot; an exact endpoint pins a `RevisionRef`. The relation descriptor must choose explicitly which reference mode is allowed for each endpoint. Historical references must retain their target revisions. Deleting a current record must not silently rewrite an exact historical reference.

Strong logical endpoints must exist in the successor selection. Endpoint deletion must either remove dependent live relationships in the same transaction or fail validation. Optional or unresolved external references require a separately declared reference type and must not masquerade as traversable local endpoints. Domain rules such as approval, containment ownership or admissible evidence remain outside generic relationship validation.

Outgoing and incoming indexes must be transactionally consistent with relationship records. They are derived access paths over the same authoritative records, not independently editable copies of the graph.

### Transactions, isolation and publication

Transactions are serializable within one view. The baseline rule is strict compare-and-publish against the transaction's base view revision; a changed base produces `Conflict`, even when a more permissive merge could have succeeded. Implementations may optimize validation only if they preserve the same serializability guarantee, including absent keys, index ranges and uniqueness checks. Forked views are independent branches, not concurrent writes to one logical head.

A transaction must atomically publish its family declarations, record revisions, relationship records, index changes and required payload references. Readers must observe the old complete snapshot or the new complete snapshot, never a mixture. Concurrent processes may prepare independent work; publication must use one coordinated authority. A single publication writer does not require every computation or read to be serial.

Abort and failed pre-publication validation must leave the visible view unchanged. Transaction handles are closed after commit or abort and must reject further operations. An unchanged transaction must preserve the logical view revision and avoid durable growth attributable solely to the no-op.

Each commit has an explicit transaction identity. If I/O or acknowledgment fails after publication may have happened, the result must identify an indeterminate outcome. A resolution operation must distinguish committed, not committed and still unknown. A retry must not silently apply the transaction twice, and no resolution operation may replay application effects.

Local-durable success requires the documented file-data, index, manifest and directory publication ordering for a supported filesystem profile. Unsupported durability must be reported before claiming a durable commit. Memory and spooled-memory success provide atomicity, consistency and isolation but explicitly do not promise survival of process failure. Temporary disk spill does not upgrade that guarantee. There is no silent downgrade between profiles.

### Historical reads and version-bound results

Time travel means explicitly reading an identified retained historical snapshot rather than following the latest view head. The public API must support selecting a retained view revision while preserving the same typed read operations and ordinary duckborrowing. A historical read observes one coherent selection across its selected families and indexes; it must not independently choose incompatible versions of related tables. The exact fluent selector spelling belongs in implementation design; selecting unavailable history must never follow the latest head instead.

New source or data versions do not invalidate an active read of an older snapshot. A compiler may capture snapshot A, finish checking A after snapshot B is published, and store its result together with an explicit association to A and the producer/dependency evidence. This does not grant an old transaction permission to overwrite a changed view head: result storage uses a valid publication transaction or isolated result view under the existing conflict rules. Storing a result for A and selecting it as the current result are distinct operations. Selection for B requires validation of its input/profile evidence; current-head publication remains concurrency-checked. A newer snapshot alone does not require discarding or recomputing every result produced against an older one.

Historical availability follows the view's history policy, not an unlimited-history promise. An active snapshot protects the records and backing resources needed to read it. Opening a revision the policy has released must report an unavailable-snapshot error naming the requested revision and the view's oldest readable one; it must never substitute the latest revision. Memory and spooled-memory modes support retained history within their lifetimes without acquiring restart persistence. Persisted history in disk mode remains subject to the policy and the durability contracts.

Delta Lake's [historical snapshot reads](https://docs.delta.io/delta-batch/) and Apache Iceberg's [time travel](https://iceberg.apache.org/docs/latest/) provide prior art for historical selection. This RFC does not adopt their storage formats, query languages or table-level transaction boundaries. Historical reads and [concurrent commit validation](https://docs.delta.io/concurrency-control/) are separate concerns.

### Snapshots, mapped views and bounded reads

Obtaining a snapshot must atomically protect its selected roots against collection. A caller must not receive an unprotected root and race collection while registering a lease afterward. Snapshot identity includes store, logical view revision and format/profile information; physical generation information may accompany it without becoming semantic identity.

Read APIs must support bounded point, range, projection and relationship access. Pages must bind their cursors to the exact snapshot, selector, ordering and index generation they require. The final page must be distinguishable from a budget-limited page. A partial answer must never be reported as complete, and an expired or incompatible cursor must not resume against the latest view.

Mapped byte views must retain mapping and lease ownership for their entire usable lifetime. Closing a parent handle must not leave dangling bytes or allow collection to remove a live backing object. Slices and scalar reads must be bounds-checked. Producing owned bytes or a decoded model must be explicit and accounted separately. General model decoding is not promised to be zero-copy merely because its source is mapped.

The ordinary `get` operation returns an owned `RecordVersion[T]` inside `RecordState[T]`. An explicit projection/view operation may avoid decoding unrequested fields and borrow mapped bytes; its lifetime and supported representations must be visible in its type contract. Neither spelling may conceal a whole-store or whole-graph materialization.

Holding an idle snapshot must not load or retain all of its records, relationships or decoded payloads. Read and traversal budgets must cover visited data and retained cursor/frontier state, not only the number of values returned.

### Typed batches and storage access

RFC 169 defines the common record-set and record-stream interface, `std.data`, over these storage capabilities, including records of several kinds, cross-kind projections, table change streams and resumption. The single-family batch contract here does not require that whole surface for the database primitive.

`RecordBatch[P]` is RFC 169's batch type. A `std.db` batch represents a bounded, typed field projection `P` from one family under one snapshot. A batch must carry its projection descriptor, record count, stable record/revision associations where requested, and explicit completion or continuation evidence. Field columns must preserve their declared value, optionality and ordering semantics; they must not be untyped dictionaries or a foreign query engine's mandatory public types.

The store must offer a scan path that exposes selected fields in typed batches without constructing an owned full model for every record. A native column-oriented profile must support this for declared scalar and variable-width field types. Custom codecs and other layouts must advertise whether they can supply projected columns directly or require bounded decoding/transposition. That work must be observable, and a caller must be able to reject an unsupported direct-column requirement rather than unknowingly accept a record-by-record fallback. This does not require every family, payload or index to use one physical columnar layout.

Batch buffers and borrowed columns inherit snapshot and mapping lifetimes. Zero-copy is a per-buffer capability, not a universal claim: decompression, schema conversion and layout conversion may allocate and must count those bytes. Record and byte budgets must apply to scan work, decoded buffers and retained continuation state, not only the returned batch size. Releasing a batch must release its temporary buffers and leases when no other consumer retains them.

Storage selectors may use declared indexes, key ranges and supported typed predicates. A scan must report which requested predicates were applied exactly and which remain for the caller; an unsupported predicate must never be silently treated as satisfied. Any pruning must preserve all potentially matching records under the declared value semantics. A query runtime owns residual expression evaluation, join ordering, aggregation, vectorized operators and execution strategy. A graph runtime owns traversal. Neither is required merely to open a store or read a record.

Point reads, graph access and projected batches must agree on the same snapshot's logical contents. Batch access must contribute the same kind of selection and negative-read evidence as other reads, so an incremental consumer cannot reuse an answer after newly matching records appear. Efficient batches are a storage capability; parity with an established analytical engine requires separate query-engine and workload evidence.

### Ownership of projected access

RFC 164 owns explicit borrowing and returned reference contracts. Read-only projected views use `Borrowed[T]`; `Reference[T]` denotes exclusive access, not an independently retained snapshot or lease. A projection borrowing a batch or snapshot must retain its checked source dependency. An independently retained batch or cursor may instead own buffer handles and a lease over its immutable selection; ownership of the handle does not require a complete decoded model per record or exclusive allocation of every page. A view into that batch remains bounded by the buffer owner it actually borrows. The compiler must not silently acquire a lease or materialize records to repair an escaping borrow. Ordinary callers infer these dependencies through duckborrowing. Primitive signatures must expose the owning or borrowed result explicitly under RFC 164; implementation must include compile-pass and compile-fail examples proving that callers cannot return a column beyond its batch, close its backing resource while a borrow remains usable, or use an exclusive reference as a retained snapshot. An explicitly retained batch owns its backing buffers and immutable-selection lease; closing the originating cursor or database handle does not revoke that ownership. Later writes create new revisions rather than mutate the retained batch. Releasing its last owner makes its storage eligible for normal reclamation only when no other retention protects it. Exact accessor names are implementation work within these fixed lifetime rules.

### Read traces, changes and invalidation

A trace must identify its snapshot, schema and index profiles, selectors and the aspects actually read: whole value, declared field projection, existence, ordered range, relationship direction or named configuration. It must include successful reads, absent lookups and selection boundaries. A new record matching a previously empty selector must invalidate an observation of that empty selection.

Read validation returns `Unchanged`, `Changed` or a typed inability to validate; uncertainty must never authorize reuse. Conservative invalidation is permitted. False reuse is not. Producer code/profile identity is a separate required input to reusing a computed result; unchanged data alone is insufficient.

A trace for an incomplete page certifies only the observed prefix and its continuation contract, not a complete predicate result. Lazy readers must record the reads actually consumed. Early termination and limits must remain visible in the result evidence.

Changes must name the before and after snapshots and distinguish inserts, updates and deletions. Unchanged content reached through different physical pages is not a semantic change. Changes may be paged or coalesced by an explicit profile, but a consumer needing every intermediate event must use a separately retained event family rather than infer a delivery log from a snapshot diff. RFC 169's table change streams, `changes(after=)` and `snapshot_then_changes()`, deliver these changes as typed change events.

`std.db` must not invoke callbacks, schedule recomputation or deliver UI events during commit or change enumeration. A reactive runtime owns scheduling, cancellation, reentrancy and back-pressure. A store may expose bounded notifications as wake-up hints only if the retained snapshot/change API remains the recovery authority.

### History policy

Each view must have a `HistoryPolicy(expire_after, keep_last)` that decides how long its superseded revisions stay readable. A superseded revision is kept while it was replaced less than `expire_after` ago, or while it is among the `keep_last` most recently superseded revisions; either rule keeps it. `expire_after=None` means superseded revisions never expire. A view's current revision is always readable.

A view without a declared policy must use its store's default. On a disk store the default is `HistoryPolicy()`, whose `expire_after` is 24 hours and whose `keep_last` is 0. On a memory or spooled-memory store the default is `HistoryPolicy(expire_after=Some(Duration.seconds(0)))`. A fork starts with its store's default, not its source view's policy.

`view.set_history(policy)` must publish under the same atomic and durability rules as other root operations, and must apply to the view's existing history as well as its future revisions. Release is final: a revision the policy has released stays unavailable after the policy is lengthened, and the view's oldest readable revision only moves forward. Age must be measured from when a revision was replaced. Restarts and clock changes must not expire a revision early, and the effective expiry must be inspectable.

Released means unavailable to new reads at once, whether or not its bytes have been reclaimed, so that availability never depends on when compaction ran. A retained snapshot, live reader lease, mapped view, `RevisionRef[T]` or `PayloadRef` must keep what it already reaches readable through that handle after the policy releases it; it does not make the revision readable to anyone else. `ObservedRevision[T]`, `PayloadIdentity` and a saved RFC 169 `ResumePosition` must not hold history.

`view.history()` must return a `HistoryReport` with the effective policy, the oldest readable revision and when it expires, the number of readable revisions, the bytes history occupies, and each holder of a released revision with the revision it holds.

Writes must reclaim released history incrementally, each within a bounded amount of work, so a store whose application never calls `compact()` holds no more than its policies keep and its live readers hold. `store.compact()` must reclaim all released history that no live reader holds and rewrite the remaining contents under the compaction rules below. It must return a `CompactionReport` with the revisions removed, the bytes reclaimed and each released revision it could not remove with its holder. A holder must never fail or block compaction. Compaction may compress what it rewrites under the store's storage profile. Compression is a storage encoding: it must not change a record or revision identity.

### Record retention

A family may declare `RecordRetention(expire_after)`, and a family without one must keep its records until they are deleted. `expire_after` must count from when a record's current revision was written, and a write may set a different value for that one record. Expiry must behave as a deletion: a tombstone appears, the record reads as `RecordState.Deleted`, and RFC 169's change streams deliver it as a deletion marked as an expiry. The replaced revision then follows the view's `HistoryPolicy`. Restarts and clock changes must not expire a record early, and a family's effective retention must be inspectable. Expiring records must take a bounded amount of work per write, so a store nobody maintains still stays within its policy.

### Retention, collection and compaction

Reachability roots include published views, explicitly retained snapshots, live reader and mapped-view leases, the history each view's policy keeps, retained sealed sets and protected in-flight publication results. Root registration, publication and collection must share a race-free coordination protocol. Releasing a view must not collect bytes still reachable through another view, exact revision reference or live lease.

Collection must derive eligibility from reachability, the view's history policy and a family's record retention. Size caps, memory pressure or age outside those policies must not authorize deletion of reachable records. A resource limit may suspend work or return a typed resource error; it must not weaken integrity or retention silently.

Operations that stage significant data must reserve their temporary allocation through shared store coordination. Reservations must identify their owner and ceiling, account for actual use, and be released or recovered after the owner ends. They are admission controls, not permission to evict protected data. A reservation does not promise that a filesystem cannot subsequently return an I/O or space error.

Compaction must preserve every selected value, tombstone and reference and must publish replacement roots atomically. It must support a bounded working-memory budget rather than gather the whole store into owned records. For a fixed canonical format and compaction profile, equal logical input contents must produce byte-identical canonical base contents; mutable manifests, leases and transaction bookkeeping are excluded from that equality. A concurrent writer or interrupted compaction must not make readers observe a partial base.

Physical collection and compaction are not logical erasure or a universal privacy policy. Logical deletion creates a tombstone. Explicit erasure must report which retained roots prevent removal and whether the storage profile can meet the requested removal guarantee; the API must not claim secure erasure merely because a file was unlinked.

### Sealed sets and admission

A sealed set is an inert, versioned envelope of records, descriptors, indexes, payload references and integrity evidence. The envelope is generic; a package or toolchain bundle is one producer, not a required store concept.

Admission must verify the envelope and referenced objects, reject incompatible or damaged inputs, write only missing physical objects and atomically publish the requested selection. Repeating the same admission must not grow the store or change its logical view. Shared contents from different origins may deduplicate physically while their distinct origin and logical identity evidence remains intact.

Verification must bound envelope parsing, decompression, reference depth, object counts and staging allocation. Paths supplied by a producer must not escape the admitted owner or follow untrusted links into another owner. A partly admitted set must not become a visible selection, and a live consumer must lease its verified canonical owner rather than rely on a mutable path alias.

Physical availability, accepted provenance and selection into a view are distinct facts. The substrate must not equate a matching digest with permission or successful application admission. Imported indexes must match their declared schema and projection profiles or be rejected/rebuilt explicitly. No import may execute producer code merely to inspect or install a set.

### Errors, inspection and resource accounting

`DbError` must distinguish scope/schema mismatch, conflict, busy coordination, resource exhaustion, unsupported capability or format, unavailable snapshot, invalid cursor, corruption, content-identity collision, access refusal and indeterminate commit. Error details must preserve the operation and relevant typed identities without requiring callers to parse prose. If existence cannot be disclosed, an access refusal may conceal it; corruption must not become a cache miss.

Inspection must expose capabilities, schemas, view and snapshot identities, index coverage, retained roots, history policies, outstanding leases, transaction outcomes and storage-format profiles. Per-operation counters must distinguish bytes verified, mapped, copied and decoded; index pages touched; records examined and returned; files and bytes written; and budget termination. Shared physical pages must not be counted as separate physical allocations merely because multiple processes map them.

### Conformance workloads

| Workload                 | Required evidence                                                                                                                                                                                                                               |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Historical results | Hold snapshot A, publish B, and finish a computation against A without mixed-version reads. Store the result as belonging to A without overwriting B's current selection. Validate separately before reuse for B; retained history remains readable, while unavailable historical versions fail explicitly. |
| Publication and recovery | Faults before and after each durable publication boundary preserve an old or new complete snapshot; ambiguous acknowledgment resolves by transaction identity. Fault injection and filesystem/power-loss qualification are reported separately. |
| Concurrent preparation   | Competing writers serialize or conflict correctly; readers remain snapshot-consistent; retries do not duplicate a transaction.                                                                                                                  |
| Scope and integrity      | Wrong-store handles, incompatible schemas, damaged pages and forced digest collisions fail distinctly without exposing unrelated data.                                                                                                          |
| Fork and retention       | Forks share contents; edits remain isolated; leases protect selected roots during concurrent collection and compaction.                                                                                                                         |
| Record retention | Records expire by family and per-write age, read as deleted, reach change streams as expiries, and survive restarts and clock changes without expiring early. |
| History policy | Default, age-only, count-only, combined, current-only and never-expiring policies keep exactly the revisions they promise across restart. A released revision is unavailable before and after compaction and stays so when the policy is lengthened. A live reader keeps its revision and is reported as its holder; a saved position does not hold history and resumes with a gap. Memory and spooled-memory stores hold only the current revision by default. A store whose application never calls `compact()` stays within its policy under a sustained write load. |
| Incremental publication  | Body, title, include, template, route, rename and deletion changes produce the same outputs as a clean recomputation, including one source rendered under multiple profiles and routes.                                                         |
| Interactive experience   | A render reads one state snapshot; a draft fork cannot mutate the published view; stale cursors and absent-selection changes are detected; no callback or effect executes inside storage operations.                                            |
| Scale and memory         | Point reads, bounded ranges, idle snapshots and streaming compaction are measured at increasing corpus sizes; counters expose unrelated work and hidden whole-store materialization.                                                            |
| Storage modes | Equivalent typed operations agree across disk, spooled-memory and memory modes. Cross the spool threshold during an uncommitted incremental import and with active readers; verify stable identities, no premature publication, valid borrowed buffers, observable resource bounds and temporary cleanup after retention ends. Memory exhaustion must not create spill files. |
| Crumb reconstruction | Restore composed models, payload enums, collections, optional values and compiler-shaped records after process restart. Test schema mismatches, unavailable field contents, malformed framing, resource limits, collision candidates and protected-field diagnostics. Read crumbs written before a rename, a reorder, an added optional field and a dropped field, and read a dropped field's earlier values when its name returns; refuse a missing required field, colliding identities at declaration and encoded records naming an unknown codec profile. Compare typed sharing, encoded sharing and scalar-inline layouts with allocation/copy/decode measurements; no production performance or durability claim follows from a representation spike alone. |
| Field fit | Read values written as each fitting type and each misfit listed under *What fits a field*: representation-preserving type mappings keep the value hash, and a misfit is a typed failure naming the field. Read a model and a nested model after dropping a non-empty field: the adapted value hashes from its retained fields, while the original record and its hashes remain unchanged. Refuse an encoded record for another model, including when every field of the reader's model is optional. Read increasing record counts of one written shape from a store and from standalone crumbs: fit is decided once for that shape, and no name is hashed. |
| Untrusted records | Decode hostile inputs through individual records, record sets and record streams: small encoded portions claiming huge lengths or counts, huge counts of empty values, nesting past the depth limit and through a recursive model, a reference into itself, a repeated or out-of-order identity, a non-canonical length and trailing bytes. Each is a typed failure naming what it broke, with allocation inside the read's limits. Decoding a standalone crumb opens no file, network connection, package or store. Coverage-guided fuzzing of the decoder finds no crash, hang or allocation beyond the limits, and accepted data under a deterministic profile re-encodes to the same bytes with the original written shape, encoding boundaries and shared metadata, before schema adaptation. Read the same valid record under a sufficient limit and an insufficient limit: one succeeds and the other returns a typed limit failure. Adapt a compatible model revision without requiring identical re-encoded bytes. Advance across consecutive records without treating the next record as trailing garbage; a waiting stream must not be reported as complete. |
| Value identity | `-0.0`, `0.0`, `None`, `Some(None)` and two spellings of one string stay distinct through encoding, reading and hashing. NaNs with different signs and payloads read back as the one NaN and hash alike. A dict or set built in different insertion orders has one hash, and one value has one hash under every codec profile. Renaming a field, reordering fields and adding an optional field move no existing record's canonical or object hash. A rename or reorder leaves the model hash alone, and adding a field moves it. `canonical=false` moves the object hash and not the canonical hash. Checking refuses a `___` field and a secret held directly or nested. |
| Packaging                | A consumer selecting memory graphs alone excludes storage; a storage consumer excludes analytical engines, protocol compilers and renderer runtimes unless explicitly selected.                                                                 |
| Projected batch scans    | A narrow projection over a wide family agrees with point reads; counters prove no full-model construction on the direct-column path, disclose decoded/copied bytes and keep unsupported predicates explicit.                                    |

No elapsed-time improvement is established by a zero-copy counter alone. Cold, warm, single-process and multi-process measurements must report equal workloads, selected profiles, output equivalence, repetitions and observer overhead.

## Design details

### Module and dependency boundaries

`incan_platform.store` owns one generic substrate implementation and consumes `std.primitives.crumb` for encoded typed values. Crumb remains available without this storage substrate, and the primitives dependency graph must not reach back into `std.db`. `std.db` owns the public typed API and does not call compiler services or a higher-level query library. `std.graph` remains storage-independent; the database supplies a graph adapter implementing RFC 166. Generic mechanisms must not contain consumer names, model families or policies.

The database must be a separately selectable standard-library capability. Importing an unrelated standard module must not select it. `incan_platform` ships as a dependency of `std.db` and is not a standard namespace of its own. The dependency closure must not require DataFusion, Substrait, `protoc`, SQL support or a renderer. Build receipts must make the selected closure verifiable.

A compiler may consume the platform storage contract directly. Opening its own stored facts must not depend on compiling a downstream query library with that compiler. Optional query adapters consume the same records and snapshots; they must not maintain a second authoritative copy of the compiler graph. Syntax records are not checked semantic facts: their producer, schema, revision and cross-references must keep that distinction explicit, including RFC 161's checked SCIR authority.

RFC 145 owns Oven's download, development-state and evidence-retention policy. This RFC supplies roots, leases, reservations and collection mechanisms that can enforce that policy; it does not import Oven's expiration policy into application databases. A view's history policy is independent of it, and its 24-hour default only shares a number with RFC 145's diagnostic-history window. Policy may explicitly retire an eligible registration, after which the substrate collects only content no longer reachable from any protected root. Compact historical evidence can use non-retaining observations rather than accidentally pinning every past executable. Adopting the shared substrate must reuse the admitted owner, not copy a closure into a second database.

Incan owns the authored semantics and algorithms. Existing native libraries may supply narrow platform mechanisms through ordinary interop. An implementation must not replace the proposed typed Incan surface with a private Rust API for convenience.

### Targets and application frameworks

Memory-only operation must have an explicit target capability profile independent of durable filesystem support. That independence is measured on what the memory path reaches: the modules it reaches, the file operations it performs at run time, which must be none, and the build receipt listing its closure. Browser or sandbox targets must not receive a fabricated local-durable guarantee, silently contact a server, or acquire host filesystem authority. A target adapter must declare its supported guarantees and pass the matching conformance profile.

Declarative experience frameworks retain ownership of surface meaning, interaction state transitions, authority, accessibility intent, lowering and target effects. They may persist typed state and artifact references, but runtime-resource bindings must remain separately managed. Storage location must not confer semantic authority or turn an unsynchronized local candidate into an accepted domain artifact.

### Compatibility and migration

This RFC adds a standard module. It does not reopen RFC 047 or reinterpret existing graph-local ids as durable record ids. Existing toolchain stores may converge on the shared substrate, but their bytes must not be declared compatible without format verification or explicit migration. Higher-level databases may wrap this API without being renamed or absorbed into the standard library.

## Alternatives considered

- **A separate storage engine per consumer:** rejected because identity, snapshot, durability and collection mechanisms would diverge and require duplicate qualification.
- **Make the platform depend on a query product:** rejected because storage mechanics must be usable without that product's language, planner, backend choices or release process.
- **Only expose an untyped key/value store:** insufficient as the public standard-library contract; every consumer would rebuild schemas, reference validation and revision-safe materialization.
- **Put a complete analytical or graph-query engine in `std.db`:** rejected because storage access must remain independently selectable and domain-neutral. Query libraries can consume typed scans and graph views.
- **Serialize arbitrary native objects:** rejected because process-local layout and resources cannot define portable durable identity or safe recovery.
- **Match encoded fields by position:** rejected because reordering a model's fields would silently misread existing data.
- **Match encoded fields by name:** a reader could translate old names, but a rename would change every record's canonical representation, so every record would look changed.
- **Field numbers assigned by hand, as Protobuf does:** every field would carry a number that its author has to manage and never reuse, when `renamed_from` already says what a rename means.
- **Field IDs assigned in table metadata, as Iceberg does:** Crumb has to work outside a store, in files and messages, where no table exists to hold them.
- **Four-byte identities, with earliest names compared across revisions:** `field_2149` and `field_13701` share the prefix `607c7e12`, so a model that dropped one and later added the other would read old values into the wrong field, and no check inside one declaration can see it. Comparing names closes that gap, but a standalone crumb would then have to carry its names or depend on a descriptor kept elsewhere, and every reader would need a mapping built and retained for each pair of revisions. Six bytes make it negligible with no check to run.
- **A hand-written `schema_version` on each family:** it says nothing the model hash and the field identities do not, and it can be left unchanged when a model changes.
- **Members filled for Crumb alone, or a built-in `from_crumb` body as `from_json` has:** see RFC 173.
- **Treat `-0.0` and `0.0` as one value, as `==` does:** reading a record back would change a value that a program can observe, for example through division.
- **Keep NaN signs and payloads:** two values that print alike would hash differently.
- **Normalize strings:** reading back would change what was written, and a hash would depend on a Unicode version.
- **Order dict and set entries through their own comparison:** hashing would run user code.
- **Use RFC 106's semantic digest as the model hash:** that digest must move on a field reorder and on a changed reference target, and neither changes what a record holds.
- **One hash per record:** it could not say both "this is exactly what was stored" and "this is the same reading, received twice".
- **Leave a secret field out of the encoding and mark it:** a model would read back without a field it declares. RFC 103 refuses a placeholder from a serializer, and an unsupported field must not disappear from the result.
- **Make persistence mandatory for interactive state:** rejected because transient state and pure in-memory graphs are first-class workloads.
- **Keep no history by default, as SQLite does:** a consumer could not resume after a restart without first declaring a policy, so the common case would need configuration before it worked.
- **Keep all history by default, as Datomic does:** the disk of an application that never thought about history would grow for as long as it runs.
- **A separate prune operation:** changing the policy already prunes, and `compact()` already reclaims. A second operation would add a second way to decide what stays.
- **Saved positions that hold history, as Postgres replication slots do:** one abandoned consumer could fill the disk. A consumer that stays away past the policy gets a typed gap instead, as a Kafka consumer does past its topic's retention.
- **Availability that depends on whether compaction has run, as Delta Lake time travel does:** the same read could succeed or fail depending on maintenance timing. A released revision is unavailable from the moment it is released.

## Drawbacks

The standard library would acquire a long-lived storage-format and recovery commitment. Precise identities, leases, cursors and trace semantics add API weight, and typed schemas require explicit evolution. Retention can consume disk until a caller releases roots, and a live reader that never finishes holds history past its policy; bounded resource errors are less convenient than silent eviction. The default history window costs a disk store a day of superseded revisions that an application may never read. Supporting disk, spooled-memory and memory modes requires conformance across different failure models. These costs are justified only if consumers actually share the implementation and its proofs.

Field identities tie encoded data to a field's earliest name, so that name has to stay in source for as long as data written under it is read, and a small encoded record set pays for its shared identity list. A strict reader refuses malformed encoded records that a faulty writer produced even when its value could be guessed, so recovering such data takes a repair tool, not a reader.

A hash profile is a long-lived commitment, because a changed rule needs a new profile name and the old one stays readable. A program that carries data in NaN payloads loses it on encoding.

## Layers affected

- **Standard library and platform:** the shared storage substrate, typed API, graph adapter, capability profiles and independent package selection.
- **Type checking and derive support:** model/codec compatibility, field identity collisions, reserved `___` field names, models holding secrets, typed family and endpoint validation, and lifetime-safe materialization through existing language mechanisms.
- **Tooling and inspection:** stable schemas, identities, read evidence, resource counters and durability qualification.
- **Documentation and contracts:** public API reference and independent storage, graph, publishing and interactive-state conformance cases.

## Design Decisions

- **Incan-native database:** typed point reads, cursor reads/writes and projected batches compose without requiring a query engine or a cursor for a single record.
- **Scope semantics:** `.read()` selects a read-only transaction; `.write()` commits modifying operations independently; `.transaction()` groups reads and writes with explicit commit. Cursor close does not select publication policy.
- **Storage modes:** disk, temporary spooled-memory and memory modes preserve the same logical data contract. Spilling is not committing, and memory mode never silently spills.
- **History:** retained historical snapshots remain coherent and readable; results can be stored against their original input snapshot. Reuse for newer inputs is a separate validation decision.
- **History policy:** each view declares how long superseded revisions stay readable through `HistoryPolicy(expire_after, keep_last)`, where either rule keeps a revision. A disk store defaults to 24 hours; memory and spooled-memory stores default to the current revision only. Live readers hold what they read; saved positions hold nothing. Release is final and takes effect before reclamation.
- **Record retention:** `RecordRetention(expire_after)` lets a family's records age out, because history only expires what was replaced, and an event or a session may never be replaced. A family has no retention unless it declares one.
- **Pruning and vacuuming:** shortening the policy prunes. Writes reclaim released history incrementally, and `store.compact()` reclaims the rest and rewrites what remains in bounded memory, reporting what it removed and what live readers still hold. Compression is a storage encoding inside that rewrite.
- **Crumb:** `std.primitives.crumb` owns typed encoding and reconstruction. `.crumble()` and `Model.from_crumb(...)` are the model-facing pair, on every model with supported fields and with no derive. `Record` adds family and retained-reference metadata and nothing else. The database owns indexing, transactions, physical sharing and retention.
- **Representation freedom:** typed sharing in memory, inline scalar values and encoded field contents are valid physical choices. No raw native struct dump, compulsory eager serialization or universal zero-copy promise is adopted.
- **Field identity and equality:** field-scoped hashes locate candidates; typed equality confirms matches. Logical records remain distinct even when contents share storage.
- **Field identities in encoded values:** the first 6 bytes of the SHA-256 of a field's earliest name, listed once per written shape in ascending byte order. An identity only has to be unique within one model's history, not globally. A collision within one declaration is a declaration error, but a collision across revisions is invisible to any one declaration, so the width has to make it negligible on its own: a model that has ever had 1,000 field names has about a 1 in 560 million chance of one, where 4 bytes would have about a 1 in 8,600 chance. Eight bytes would buy odds no model needs for two more bytes per field in each encoded shape. Crafting a collision takes control of both revisions of the model, whose author can already write anything into it.
- **Encoded records describe their contents:** model identities and field types accompany the records, with shared metadata for records of the same shape, so a standalone crumb can be refused or skipped through with no external lookup. Collections use RFC 169's `RecordSet` and `RecordStream`; the codec defines binary boundaries without adding another public collection type.
- **Model identities are qualified:** a field only has to differ from the other fields of its model, but a model has to differ from every other model a reader might meet, so its identity comes from its earliest qualified name within its package's canonical identity, meaning its registry id and its scoped name. Short names would give two `Customer` models in different packages one identity by construction, and an all-optional one would read the other's crumbs. Package names alone would do the same for two packages named `crm` in two registries. The costs are that moving a model takes a `renamed_from`, and that a package must declare its scoped name before it stores records.
- **What fits a field:** scalar widening and optional wrapping preserve canonical value representation. Nested models follow checked field compatibility; dropping fields produces an adapted value whose hash may differ, without changing the stored record or its hashes. A present field with an incompatible type is a typed failure, never a skip.
- **No schema version:** a family binds a model hash, and the field identities decide compatibility. A new meaning takes a new name, as it does for a field.
- **Memory capacity:** a memory store has no total size to run out of. Records and requests have bounds, and exceeding one is a typed failure. An allocation failure stays what it is everywhere in the runtime, an abort, and nothing here claims otherwise.
- **Readers are strict and bounded:** readers supporting the same codec profile agree on the interpretation of encoded records, while configured limits and target-model compatibility may cause different readers to reject otherwise valid data. Limits bound each read, including incremental reads from a record set or stream, so hostile input is refused without requiring an entire collection in memory. A store still has no total size of its own.
- **Encoded records identify their codec profile:** one model can then use different arrangements of its bytes, such as a lone record in rows and a large table in columns, and an unknown arrangement is refused instead of misread.
- **Hashes cover values under a named profile:** `crumb1` keeps `-0.0` apart from `0.0` and `None` apart from `Some(None)`, treats every NaN as one value and leaves strings as written. It orders dicts and sets itself, so hashing runs no user code, and a field at its absent value contributes nothing, so additive changes move no hash. The language's `==` is unchanged.
- **Three hashes per stored record:** the object hash, the canonical hash and the model hash, with `canonical=false` opting a field out of the second.
- **Reserved names:** `___` names belong to the store, and the list is closed.
- **Secrets are refused rather than dropped:** a model holding `SecretStr` or `SecretBytes` cannot be crumbled or stored, and a hash never covers a secret.
- **Ownership:** ordinary reads use duckborrowing and owned point results. Primitive contracts expose explicit borrowing; retained batches own backing resources and snapshot leases. Borrowed columns remain bounded by their actual owner.
- **Descriptor mechanism:** RFC 173 supplies the public model-owned walk, shape and reconstruction protocols. Crumb implements the format through those protocols; compiler-private descriptor shortcuts are forbidden. Implementation must prove the public protocol sufficient for nested data and final validation.
- **Durability qualification:** each supported target/filesystem profile must provide reproducible publication/recovery evidence before claiming local durability. Unsupported profiles refuse that claim; the representation experiment is not power-loss qualification.
- **Primitive lifecycle:** the primitive works through explicit lifecycle operations independently of RFC 094. Detailed API signatures, physical formats and platform qualification belong in implementation design and conformance work under this contract.
