# RFC 164: Typed embedded storage with `std.db`

- **Status:** Draft
- **Created:** 2026-10-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 024 (extensible derive protocol)
    - RFC 047 (lightweight directed graph types)
    - RFC 086 (schema descriptors and adapters)
    - RFC 112 (crash-safe publication and file coordination)
    - RFC 114 (compiled providers and package features)
    - RFC 117 (Loaf project model)
    - RFC 124 (store unit identity and cross-plan artifact sharing)
    - RFC 145 (storage lifecycle, retention and compact evidence)
    - RFC 161 (semantic core IR)
    - RFC 165 (indexed graphs and snapshot-bound graph access)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6.0-dev.9
- **Shipped in:** —

## Summary

Introduce `std.db`, an embedded, typed, transactional record store with memory-only and local-durable profiles. Incan models define stored values; explicit identities, immutable revisions, indexed relationships and snapshot-bound reads define how those values are retained and connected. The public standard-library API wraps one generic platform storage substrate rather than creating a second engine. Compiler tooling, document publishers, interactive applications and higher-level query libraries can share that substrate without sharing domain policy, requiring a server, or importing a query engine. RFC 165 defines the graph-reading interface over these records and ordinary in-memory graphs.

## Core model

1. A **family** gives an Incan record model a stable storage name and a versioned schema and codec contract.
2. A **record identity** names one logical object in that family. Equal contents do not make two objects the same object.
3. A **revision** is an immutable, canonically encoded value or tombstone for that identity.
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
          mapping, durable I/O and coordination

std.db graph views implement RFC 165's storage-independent
std.graph reading interface; std.graph does not import std.db.
```

## Motivation

Applications repeatedly need stable records, revision visibility, forward and reverse relationships, reproducible reads and durable publication. Implementing these independently produces incompatible caches, identity schemes and recovery rules. Exporting a complete graph into a second store also creates a synchronization problem and makes small questions proportional to a whole project.

A compiler needs selected declarations and their dependencies. A document publisher needs source, include, parse, route and render relationships. An interactive application needs small state snapshots, draft branches and exact references to the artifacts presented to a user. Analytical and retrieval libraries need inspectable inputs and outputs. These workloads share storage mechanics, not the meaning of their records or the policy governing their use.

The standard library should offer a reusable, native foundation. A database must remain optional: an in-memory graph, a static renderer and an ordinary command-line program must not acquire durable storage merely by using another standard module.

## Goals

- Make typed records and relationships directly usable from Incan without SQL DDL, an ORM or an application-authored serialization layer.
- Provide atomic, consistent, isolated transactions and explicitly qualified durability.
- Share one storage implementation between toolchain and application consumers through a consumer-neutral API.
- Support cheap view forks, bounded snapshot reads, mapped byte access and explicit owned materialization.
- Support typed point access and column-projected batch scans without making an analytical query engine part of storage.
- Preserve observable differences between absent, deleted, inaccessible, unavailable and damaged data.
- Support incremental consumers through precise read evidence and bounded changes, including negative reads and newly matching records.
- Make memory, copying, I/O, retention and failure behavior inspectable and testable.
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

`Record` is a proposed standard derive using the extensible derive mechanism. It supplies the model descriptor and canonical record codec described below; it is not a new language declaration form. RFC 086 is deferred design context, not a prerequisite to implement its schema language. This proposal requires a focused record/codec contract derived from checked model declarations, not that broader adapter system.

```incan
from std import db
from std.db import Record
from std.fs import Path

@derive(Record)
model Page:
    route: str
    title: str

def save_page() -> Result[None, db.DbError]:
    """Publish one typed page and read its committed revision."""
    store = db.open(Path("site.db"))?
    pages = store.family[Page]("pages", schema_version=1)?
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

Family registration is explicit. Constructing a family handle does not write a schema; `transaction.declare(family)` validates or stages its declaration in the transaction's selected view. Reads validate the handle against the snapshot's descriptor. Reopening an existing family does not replace its schema because the local model changed. This avoids SQL ceremony without hiding schema evolution.

`db.memory()` creates an isolated memory store with the same logical record and transaction semantics. Its receipts report memory durability. It does not create files or silently spill to disk. `db.open(path)` opens or creates a local-durable store directory, validating its format and capabilities first. Opening an existing unrelated directory must fail rather than adopt or overwrite its contents.

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

### Connect typed families

A publisher can declare a relationship from `Document` records to `Asset` records with an `AssetUse` payload. The relationship descriptor names both endpoint families, its payload schema and uniqueness constraints. Creating an edge with reversed or incompatible endpoints is rejected. Relationships have their own identities, so two different uses of one asset need not collapse into one edge.

The same mechanism can represent a diagnostic referring to a declaration, a surface referring to a model revision, or an activity citing evidence. The database preserves those distinctions; it does not decide that any of them establishes ownership or approval.

RFC 165 exposes indexed traversal of these relationships without exporting every node and edge into a separate graph container.

### Incremental readers and interactive state

A reader pins a snapshot, reads selected fields or records, and optionally captures a read trace. After an edit, it can validate that trace against a successor snapshot or enumerate bounded changes. A renderer may reuse an unchanged projection; a compiler may reuse a derived answer. The caller owns the computation and its semantic fingerprint.

Interactive state may remain in a memory store. A render pass reads one snapshot; a later transaction publishes the next state. Callbacks and target resources remain outside the store and resolve record identities through application-owned bindings. A committed state change does not execute an action, send an event or establish that an external effect succeeded.

### An embedded database without a mandatory query language

An application may use `std.db` directly as its local transactional database: open a directory or memory store, register typed families, update records, follow indexed relationships and read a consistent snapshot. A query library may instead consume bounded typed batches from that same snapshot and supply joins, grouping, sorting or analytical expressions. Both use the same storage and publication contracts; neither requires SQL.

Model-shaped data does not imply one heap-allocated model per scanned row. Selecting two numeric fields from a wide family must have a batch path that does not decode every unrelated field or construct every complete record. Indexed application reads and analytical scans are complementary access patterns, not separate databases containing duplicate authoritative records.

## Reference-level explanation

### Public surface and identities

The following are required concepts and proposed public names. Generic family and relation arguments retain their model types through the API; implementations must not erase them to strings at the authoring boundary.

| Surface               | Required contract                                                                                                    |
| --------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `Store`               | Own one memory store or open one physical local store; expose capabilities and format identity.                      |
| `Family[T]`           | Bind `T` to one family identity, schema version and codec profile; mismatches are typed errors.                      |
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
| `RecordBatch[P]`      | A bounded typed field projection from one pinned family snapshot, with explicit buffer ownership and continuation.   |

Persistent record identity must not be a process pointer, row offset, index slot, graph-local integer or filesystem path. Physical deduplication must not collapse distinct logical records. Revisions and view revisions must distinguish logical content identity from physical layout generation; compaction must not appear as a logical record edit.

A bare identifier is not authority to read another store or inaccessible root. Lookups must validate scope. Import must preserve origin identity or return an explicit remapping; it must not silently declare two foreign identities equivalent because their local integer or payload digest matches.

### Typed values, schemas and encoding

`Record` must describe supported fields, their types, field identities and deterministic encoding. Primitive values, enums, supported containers, other record models and typed record references must compose. Unsupported resources, cycles through live heap references and unbound runtime state must produce a declaration or encoding error, not an opaque dump of memory.

A stored value must decode to its declared Incan model. This does not require serialized bytes to have the layout of a native model instance. On-disk encoding must not depend on native struct padding, process addresses, hash-map iteration order or host endianness. A codec profile must define ordering, floating-point representation and canonicalization explicitly; it must not silently merge distinctions that the declared value semantics preserve.

Custom codecs may implement the same typed contract. Their identity, schema, determinism and resource limits must be inspectable. Executable codec code is supplied by the installed program, not loaded and executed from an imported record set.

The record contract must also enumerate retained typed revision and payload references, including those nested in supported containers. Collection must not guess references by scanning arbitrary bytes. Custom codecs must supply the same complete reference description. A reference to a foreign store does not implicitly keep that store alive; imported local contents or an explicit cross-store lease agreement are required before it can supply a retained local read.

Retaining references and historical observations must be distinct types. `ObservedRevision[T]` and `PayloadIdentity` preserve evidence without registering immortal retention roots; using either requires an availability check and acquisition of a live lease before reading. Removing that payload does not rewrite the historical observation. Conversely, the store must not expire an active `RevisionRef[T]` or `PayloadRef` as if it were only an observation.

Changing a schema or codec requires an explicit versioned migration or an explicitly compatible descriptor. Migration must stage and validate a successor selection before publication; old leased snapshots must remain readable using their declared profiles, or the operation must refuse. Unsupported versions must never be treated as absent records.

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

Local-durable success requires the documented file-data, index, manifest and directory publication ordering for a supported filesystem profile. Unsupported durability must be reported before claiming a durable commit. Memory-store success provides atomicity, consistency and isolation but explicitly does not promise survival of process failure. There is no silent downgrade between profiles.

### Snapshots, mapped views and bounded reads

Obtaining a snapshot must atomically protect its selected roots against collection. A caller must not receive an unprotected root and race collection while registering a lease afterward. Snapshot identity includes store, logical view revision and format/profile information; physical generation information may accompany it without becoming semantic identity.

Read APIs must support bounded point, range, projection and relationship access. Pages must bind their cursors to the exact snapshot, selector, ordering and index generation they require. The final page must be distinguishable from a budget-limited page. A partial answer must never be reported as complete, and an expired or incompatible cursor must not resume against the latest view.

Mapped byte views must retain mapping and lease ownership for their entire usable lifetime. Closing a parent handle must not leave dangling bytes or allow collection to remove a live backing object. Slices and scalar reads must be bounds-checked. Producing owned bytes or a decoded model must be explicit and accounted separately. General model decoding is not promised to be zero-copy merely because its source is mapped.

The ordinary `get` operation returns an owned `RecordVersion[T]` inside `RecordState[T]`. An explicit projection/view operation may avoid decoding unrequested fields and borrow mapped bytes; its lifetime and supported representations must be visible in its type contract. Neither spelling may conceal a whole-store or whole-graph materialization.

Holding an idle snapshot must not load or retain all of its records, relationships or decoded payloads. Read and traversal budgets must cover visited data and retained cursor/frontier state, not only the number of values returned.

### Typed batches and storage access

The proposed `RecordBatch[P]` represents a bounded, typed field projection `P` from one family under one snapshot. A batch must carry its projection descriptor, row count, stable record/revision associations where requested, and explicit completion or continuation evidence. Field columns must preserve their declared value, optionality and ordering semantics; they must not be untyped dictionaries or a foreign query engine's mandatory public types.

The store must offer a scan path that exposes selected fields in typed batches without constructing an owned full model for every row. A native column-oriented profile must support this for declared scalar and variable-width field types. Custom codecs and other layouts must advertise whether they can supply projected columns directly or require bounded decoding/transposition. That work must be observable, and a caller must be able to reject an unsupported direct-column requirement rather than unknowingly accept a row-by-row fallback. This does not require every family, payload or index to use one physical columnar layout.

Batch buffers and borrowed columns inherit snapshot and mapping lifetimes. Zero-copy is a per-buffer capability, not a universal claim: decompression, schema conversion and layout conversion may allocate and must count those bytes. Row and byte budgets must apply to scan work, decoded buffers and retained continuation state, not only the returned batch size. Releasing a batch must release its temporary buffers and leases when no other consumer retains them.

Storage selectors may use declared indexes, key ranges and supported typed predicates. A scan must report which requested predicates were applied exactly and which remain for the caller; an unsupported predicate must never be silently treated as satisfied. Any pruning must preserve all potentially matching rows under the declared value semantics. A query runtime owns residual expression evaluation, join ordering, aggregation, vectorized operators and execution strategy. A graph runtime owns traversal. Neither is required merely to open a store or read a record.

Point reads, graph access and projected batches must agree on the same snapshot's logical contents. Batch access must contribute the same kind of selection and negative-read evidence as other reads, so an incremental consumer cannot reuse an answer after newly matching records appear. Efficient batches are a storage capability; parity with an established analytical engine requires separate query-engine and workload evidence.

### Read traces, changes and invalidation

A trace must identify its snapshot, schema and index profiles, selectors and the aspects actually read: whole value, declared field projection, existence, ordered range, relationship direction or named configuration. It must include successful reads, absent lookups and selection boundaries. A new record matching a previously empty selector must invalidate an observation of that empty selection.

Read validation returns `Unchanged`, `Changed` or a typed inability to validate; uncertainty must never authorize reuse. Conservative invalidation is permitted. False reuse is not. Producer code/profile identity is a separate required input to reusing a computed result; unchanged data alone is insufficient.

A trace for an incomplete page certifies only the observed prefix and its continuation contract, not a complete predicate result. Lazy readers must record the reads actually consumed. Early termination and limits must remain visible in the result evidence.

Changes must name the before and after snapshots and distinguish inserts, updates and deletions. Unchanged content reached through different physical pages is not a semantic change. Changes may be paged or coalesced by an explicit profile, but a consumer needing every intermediate event must use a separately retained event family rather than infer a delivery log from a snapshot diff.

`std.db` must not invoke callbacks, schedule recomputation or deliver UI events during commit or change enumeration. A reactive runtime owns scheduling, cancellation, reentrancy and back-pressure. A store may expose bounded notifications as wake-up hints only if the retained snapshot/change API remains the recovery authority.

### Retention, collection and compaction

Reachability roots include published views, explicitly retained snapshots, live reader and mapped-view leases, required history, retained sealed sets and protected in-flight publication results. Root registration, publication and collection must share a race-free coordination protocol. Releasing a view must not collect bytes still reachable through another view, exact revision reference or live lease.

Collection must derive eligibility from reachability and declared retention. Size caps, age or memory pressure alone must not authorize deletion of reachable records. A resource limit may suspend work or return a typed resource error; it must not weaken integrity or retention silently.

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

Inspection must expose capabilities, schemas, view and snapshot identities, index coverage, retained roots, outstanding leases, transaction outcomes and storage-format profiles. Per-operation counters must distinguish bytes verified, mapped, copied and decoded; index pages touched; records examined and returned; files and bytes written; and budget termination. Shared physical pages must not be counted as separate physical allocations merely because multiple processes map them.

### Conformance workloads

| Workload                 | Required evidence                                                                                                                                                                                                                               |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Publication and recovery | Faults before and after each durable publication boundary preserve an old or new complete snapshot; ambiguous acknowledgment resolves by transaction identity. Fault injection and filesystem/power-loss qualification are reported separately. |
| Concurrent preparation   | Competing writers serialize or conflict correctly; readers remain snapshot-consistent; retries do not duplicate a transaction.                                                                                                                  |
| Scope and integrity      | Wrong-store handles, incompatible schemas, damaged pages and forced digest collisions fail distinctly without exposing unrelated data.                                                                                                          |
| Fork and retention       | Forks share contents; edits remain isolated; leases protect selected roots during concurrent collection and compaction.                                                                                                                         |
| Incremental publication  | Body, title, include, template, route, rename and deletion changes produce the same outputs as a clean recomputation, including one source rendered under multiple profiles and routes.                                                         |
| Interactive experience   | A render reads one state snapshot; a draft fork cannot mutate the published view; stale cursors and absent-selection changes are detected; no callback or effect executes inside storage operations.                                            |
| Scale and memory         | Point reads, bounded ranges, idle snapshots and streaming compaction are measured at increasing corpus sizes; counters expose unrelated work and hidden whole-store materialization.                                                            |
| Packaging                | A consumer selecting memory graphs alone excludes storage; a storage consumer excludes analytical engines, protocol compilers and renderer runtimes unless explicitly selected.                                                                 |
| Projected batch scans    | A narrow projection over a wide family agrees with point reads; counters prove no full-model construction on the direct-column path, disclose decoded/copied bytes and keep unsupported predicates explicit.                                    |

No elapsed-time improvement is established by a zero-copy counter alone. Cold, warm, single-process and multi-process measurements must report equal workloads, selected profiles, output equivalence, repetitions and observer overhead.

## Design details

### Module and dependency boundaries

`incan_platform.store` owns one generic substrate implementation. `std.db` owns the public typed API and does not call compiler services or a higher-level query library. `std.graph` remains storage-independent; the database supplies a graph adapter implementing RFC 165. Generic mechanisms must not contain consumer names, model families or policies.

The database must be a separately selectable standard-library capability. Importing an unrelated standard module must not select it. The dependency closure must not require DataFusion, Substrait, `protoc`, SQL support or a renderer. Build receipts must make the selected closure verifiable.

A compiler may consume the platform storage contract directly. Opening its own stored facts must not depend on compiling a downstream query library with that compiler. Optional query adapters consume the same records and snapshots; they must not maintain a second authoritative copy of the compiler graph. Syntax records are not checked semantic facts: their producer, schema, revision and cross-references must keep that distinction explicit, including RFC 161's checked SCIR authority.

RFC 145 owns Oven's download, development-state and evidence-retention policy. This RFC supplies roots, leases, reservations and collection mechanisms that can enforce that policy; it does not import Oven's expiration defaults into application databases. Policy may explicitly retire an eligible registration, after which the substrate collects only content no longer reachable from any protected root. Compact historical evidence can use non-retaining observations rather than accidentally pinning every past executable. Adopting the shared substrate must reuse the admitted owner, not copy a closure into a second database.

Incan owns the authored semantics and algorithms. Existing native libraries may supply narrow platform mechanisms through ordinary interop. An implementation must not replace the proposed typed Incan surface with a private Rust API for convenience.

### Targets and application frameworks

Memory-only operation must have an explicit target capability profile independent of durable filesystem support. Browser or sandbox targets must not receive a fabricated local-durable guarantee, silently contact a server, or acquire host filesystem authority. A future target adapter must declare its supported guarantees and pass the matching conformance profile.

Declarative experience frameworks retain ownership of surface meaning, interaction state transitions, authority, accessibility intent, lowering and target effects. They may persist typed state and artifact references, but runtime-resource bindings must remain separately managed. Storage location must not confer semantic authority or turn an unsynchronized local candidate into an accepted domain artifact.

### Compatibility and migration

This RFC adds a standard module. It does not reopen RFC 047 or reinterpret existing graph-local ids as durable record ids. Existing toolchain stores may converge on the shared substrate, but their bytes must not be declared compatible without format verification or explicit migration. Higher-level databases may wrap this API without being renamed or absorbed into the standard library.

## Alternatives considered

- **A separate storage engine per consumer:** rejected because identity, snapshot, durability and collection mechanisms would diverge and require duplicate qualification.
- **Make the platform depend on a query product:** rejected because storage mechanics must be usable without that product's language, planner, backend choices or release process.
- **Only expose an untyped key/value store:** insufficient as the public standard-library contract; every consumer would rebuild schemas, reference validation and revision-safe materialization.
- **Put a complete analytical or graph-query engine in `std.db`:** rejected because storage access must remain independently selectable and domain-neutral. Query libraries can consume typed scans and graph views.
- **Serialize arbitrary native objects:** rejected because process-local layout and resources cannot define portable durable identity or safe recovery.
- **Make persistence mandatory for interactive state:** rejected because transient state and pure in-memory graphs are first-class workloads.

## Drawbacks

The standard library would acquire a long-lived storage-format and recovery commitment. Precise identities, leases, cursors and trace semantics add API weight, and typed schemas require explicit evolution. Retention can consume disk until a caller releases roots; bounded resource errors are less convenient than silent eviction. Supporting both memory and durable profiles requires conformance across different failure models. These costs are justified only if consumers actually share the implementation and its proofs.

## Layers affected

- **Standard library and platform:** the shared storage substrate, typed API, graph adapter, capability profiles and independent package selection.
- **Type checking and derive support:** model/codec compatibility, typed family and endpoint validation, and lifetime-safe materialization through existing language mechanisms.
- **Tooling and inspection:** stable schemas, identities, read evidence, resource counters and durability qualification.
- **Documentation and contracts:** public API reference and independent storage, graph, publishing and interactive-state conformance cases.

## Unresolved questions

- What is the precise author-facing type for a borrowed typed field projection, given the existing borrowing and declaration-metadata contracts? Ordinary `get` is owned; this question does not change that materialization boundary.
- Which existing schema-descriptor facilities are sufficient for `Record`, and which require a separately specified extension before its derive can be accepted? There must be no compiler-private descriptor shortcut.
- Which filesystem and target profiles will claim local-durable support at first adoption, and what independently reproducible power-loss evidence is required for each?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
