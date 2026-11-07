# RFC 166: Indexed graphs and snapshot-bound graph access

- **Status:** Planned
- **Created:** 2026-10-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 164 (explicit ownership and borrowing contracts)
    - RFC 030 (extended collections)
    - RFC 047 (lightweight directed graph types)
    - RFC 101 (ordinal maps)
    - RFC 106 (compiler-backed context graphs)
    - RFC 115 (fallible iteration)
    - RFC 150 (deprecation diagnostics; `made_private` for the sealed fields)
    - RFC 161 (semantic core IR)
    - RFC 165 (typed embedded storage)
    - RFC 169 (typed record sets and record streams)
- **Issue:** [#2524](https://github.com/encero-systems/incan/issues/2524)
- **RFC PR:** —
- **Written against:** v0.6
- **Shipped in:** —

## Summary

Evolve `std.graph` into an indexed, bounded and storage-independent graph facility while preserving RFC 047's in-memory containers. Add typed relationship payloads, snapshot-bound graph readers, paged traversal and explicit completeness, allowing the same algorithms to work over ordinary graph values or `std.db` records without copying an entire graph. The new reading surface makes scope, ownership, resource bounds and completeness explicit. This is a follow-on to implemented RFC 047, not a replacement of its historical specification or a requirement that all graphs become databases.

## Core model

1. **Graph ownership** defines the identity scope of nodes and edges. Equal payloads do not imply equal identities.
2. **Graph structure** consists of explicit directed edges. Parallel edges retain distinct identities and typed payloads.
3. **A read scope** fixes one graph selection and revision for a traversal. It can come from an in-memory graph or a database snapshot.
4. **Indexed access** visits selected adjacency and values, rather than repeatedly scanning unrelated nodes and edges.
5. **A traversal** has a declared direction, edge selection, ordering and resource budget. Its outcome distinguishes complete, limited, canceled and failed work.
6. **A projection** adapts existing authoritative records into graph access. It does not create a second truth or reinterpret every edge as a dependency.

## Motivation

Compiler facts, query plans, document dependencies and interactive experiences all need graph-shaped access. The lightweight containers introduced by RFC 047 establish useful identity and traversal semantics, but an eager list-returning interface alone cannot serve large persistent graphs, bounded inspection or low-allocation interactive updates.

One source document may appear at several routes. One declaration may be referenced by many bodies. One state record may affect several UI regions while an unrelated edit must affect none of them. A graph layer must preserve these distinct occurrences and relationship kinds while making selected access cheap.

A performant native graph is not merely a native executable holding two lists. Finding one node's neighbors must not inspect every unrelated edge. Conversely, replacing every small graph with a database would penalize legitimate in-memory workloads. The common abstraction must be a graph reader, not a mandatory storage backend.

## Goals

- Preserve `DiGraph[T]`, `Dag[T]` and `MultiDiGraph[T]` as independently usable graph values.
- Make forward and reverse adjacency indexed and traversal proportional to the visited graph rather than repeated whole-graph scans.
- Support typed node and relationship payloads, including heterogeneous domains through explicit models and endpoint-family adapters.
- Share algorithms between in-memory containers and immutable database-backed views without importing database machinery into `std.graph`.
- Make snapshot consistency, deterministic ordering, limits, failures and partial answers explicit.
- Support compiler, publishing and interactive-framework workloads without adopting their scheduling or semantic policy.

## Non-Goals

- A graph query language, a relational optimizer or a domain relationship vocabulary.
- A persistence engine inside `std.graph`; RFC 165 owns the standard storage surface.
- Inferring ownership, approval, authority, causality or invalidation from an unlabeled structural edge.
- Requiring every graph to be acyclic, connected, durable or loaded entirely in memory.
- A UI tree, renderer, event dispatcher, effect runner or automatic distributed synchronization mechanism.

## Guide-level explanation

All new API spellings below describe the proposed library contract, not implemented functionality. Existing RFC 047 calls are identified separately. Ordinary callers use duckborrowing; implementation signatures must preserve RFC 164 ownership and source dependencies.

### Keep ordinary graph values ordinary

Existing construction remains valid:

```incan
from std.graph import Dag, GraphError

def document_dependencies() -> Result[Dag[str], GraphError]:
    """Construct a small dependency graph and propagate insertion errors."""
    mut graph = Dag[str]()
    source = graph.add_node("source")
    parsed = graph.add_node("parsed-document")
    rendered = graph.add_node("rendered-page")
    graph.add_edge(source, parsed)?
    graph.add_edge(parsed, rendered)?
    return Ok(graph)
```

The implementation may change its storage representation to provide indexed access. It must preserve existing documented insertion, deletion, parallel-edge and traversal behavior. It must not require a store directory or a database package for this example.

### Relationships carry types and values

The proposed `RelationGraph[N, E]` extends the in-memory family with typed node and edge payloads. It permits parallel edges with separate identities. `N` may be a domain enum when several node kinds inhabit one graph; it need not be a bag of string properties.

```incan
from std.graph import RelationGraph, GraphError

model Region:
    name: str

enum StateField:
    Title
    Body

model ReadsState:
    field: StateField

def region_dependencies() -> Result[RelationGraph[Region, ReadsState], GraphError]:
    """Distinguish two state dependencies between the same regions."""
    mut graph = RelationGraph[Region, ReadsState]()
    editor = graph.add_node(Region(name="editor"))
    preview = graph.add_node(Region(name="preview"))
    title_read = graph.add_edge(editor, preview, ReadsState(field=StateField.Title))?
    body_read = graph.add_edge(editor, preview, ReadsState(field=StateField.Body))?
    return Ok(graph)
```

`ReadsState` is one application-declared relationship type; `StateField` is its finite domain vocabulary, not a universal graph predicate. Generic graph storage does not know what editing a title means. A framework may instead use a checked field descriptor when it needs to follow model declarations directly.

For relationships between distinct model families, RFC 165's `Relation[S, T, P]` validates source, target and payload types. Its graph adapter preserves those family and relation identities; it must not accept reversed endpoint types merely because both are encoded as integers.

### Store a compiler graph directly

A graph can be database-backed from its first write. The compiler need not assemble an in-memory graph and export it afterward. These application models deliberately simplify compiler identities; production compiler records retain their checked identities rather than deriving identity from a display name.

```incan
from std import db
from std.db import Record
from std.fs import Path

@derive(Record)
model Function:
    name: str
    return_type: str

model SourceLocation:
    file: str
    line: int

model Calls:
    location: SourceLocation

model Inlines:
    reason: str

enum CompilerEdge:
    Calls(Calls)
    Inlines(Inlines)

def store_and_read_calls(path: Path) -> Result[None, db.OperationError]:
    # This demonstration adds new node identities on each invocation.
    with db.open(path) as store:
        with store.transaction() as transaction:
            mut graph = transaction.graph[Function, CompilerEdge]("compiler.calls")?
            # Validate or stage the graph definition and its owned families.
            transaction.declare(graph)?

            process = graph.add_node(
                Function(name="process", return_type="str")
            )?
            normalize = graph.add_node(
                Function(name="normalize", return_type="str")
            )?

            graph.add_edge(process, normalize, CompilerEdge.Calls(Calls(
                location=SourceLocation(file="app.incn", line=12),
            )))?
            graph.add_edge(process, normalize, CompilerEdge.Calls(Calls(
                location=SourceLocation(file="app.incn", line=18),
            )))?
            graph.add_edge(process, normalize, CompilerEdge.Inlines(Inlines(
                reason="Small checked body",
            )))?
            receipt = transaction.commit()?

        # Read exactly the committed selection, even if a newer commit exists.
        snapshot = receipt.snapshot()?
        graph = snapshot.graph[Function, CompilerEdge]("compiler.calls")?

        with graph.incoming(normalize).cursor(
            max_records=256, max_bytes=1_048_576,
        ) as cursor:
            while let Some(batch) = cursor.next_batch()?:
                for edge in batch:
                    match edge.value:
                        CompilerEdge.Calls(call) =>
                            print(f"Call at {call.location.file}:{call.location.line}")
                        CompilerEdge.Inlines(inline) =>
                            print(f"Inlined: {inline.reason}")
    return Ok(None)
```

The two calls have separate edge identities despite sharing endpoints. The inline relationship carries a different model. All three edges point from caller to callee; reading incoming edges inspects that direction from the target and does not create reverse edges. A node payload can likewise be a domain enum containing several model kinds.

The graph selector belongs to the database adapter. It returns a typed graph handle, not a materialized container. `transaction.declare(graph)` explicitly validates or stages its definition; reopening it with incompatible model or relation descriptors must fail rather than migrate implicitly. `add_node` inserts a model and node membership; `add_edge` validates endpoints, stages its payload and updates adjacency in the same transaction. Database mutations are fallible even where the equivalent in-memory convenience operation has an infallible signature. `db.OperationError` retains the graph and storage causes involved in the operation.

No explicit `.crumble()` is required. RFC 165 owns model encoding, typed reconstruction, sharing, retention and incremental staging. It may retain checked typed data in memory or stage encoded data on disk without holding every decoded model until commit. Reading these call-site payloads does not require decoding either endpoint's `Function` model.

The incoming cursor is a convenience over indexed adjacency pages. `next_batch()` returns a fallible optional batch; `None` means the selected adjacency is exhausted. A hard resource limit, invalid cursor or storage failure returns a typed error, not apparent completion. A batch limit is separate from a total traversal budget. Reading a large adjacency may produce many bounded batches, and retained consumer batches remain accounted resources.

### Share existing model records between graphs

The graph-first surface owns records when `add_node(value)` is used. It must also support attaching an existing typed database record as a node without copying its model data. The adapter operation `attach_node(record)` takes a validated typed record reference and returns a distinct graph node identity; it does not accept an unscoped integer or infer identity from a payload hash. Repeated attachment may intentionally create separate occurrences, so idempotent domain lookup must be explicit rather than guessed from value equality.

For an application that already has authoritative node families and relationship records, `snapshot.graph(nodes=nodes, edges=relations)` is the explicit projection form. The descriptors identify endpoint families and their checked mapping; multiple selected families may be represented by typed domain enums. This form reads those existing relationships without duplicating their authority. The named graph form and explicit projection form implement the same reading protocol.

An existing function record can participate in a call graph and a type-use graph. Each graph has its own membership and relationships; the underlying model revision can be shared. Graph membership does not grant permission to delete that shared record, nor does removal of a node imply that an independently retained model record disappears.

### Read only the selected graph

The proposed `read_view()` on an in-memory graph produces a consistent, scoped read borrow. A database snapshot produces a reader over explicitly selected node and relation families. Both implement the reading protocol below.

Algorithms accept this protocol, a start key, direction and a budget. A bounded walk returns a page with visited keys, observed edges and a completion outcome. The caller may continue with the returned cursor or stop. It must not treat a budget-limited result as the complete descendant set.

Read-only access does not require a writable graph. A database graph view must not load every node into `DiGraph` before answering a neighbor query.

### One source, several appearances

A publisher should be able to distinguish a source document, its parsed revision, and each route/profile-specific rendered occurrence. An interactive framework should likewise distinguish a shared model record from two component instances observing it.

Graphs must not deduplicate these nodes simply because their payloads or rendered text are equal. Shared computations may refer to one common result node; occurrences remain separately identifiable. Edges distinguish containment, reference, observation and dependency rather than forcing all four into one adjacency with assumed semantics.

## Reference-level explanation

### Existing containers and compatibility

RFC 047 remains the contract for existing constructors and eager methods. This RFC adds indexed implementations and new read surfaces. Existing eager list-returning methods may remain as documented convenience APIs; they must not become the hidden implementation of bounded graph readers.

The source representation of a container is not its semantic identity. Implementations may use adjacency indexes, dense slots, ordinal maps or persistent pages, but arbitrary mutation of internal storage fields must not become a supported way to bypass graph invariants.

Existing graph-local `NodeId` and `EdgeId` handles do not become globally unique or durable. `to_int()` remains a representation for diagnostics or caller-managed interchange, not a credential, persistent record identity or safe cross-graph key. A new read protocol must carry scope separately when adapting such handles.

The new in-memory read surface uses `ScopedNodeId` and `ScopedEdgeId`, combining graph-instance identity with the existing local handle. Standard adapters must validate that identity before lookup. Bare legacy ids remain accepted by legacy container methods only; readers issue scoped keys for their own nodes and edges and must not offer an unchecked integer-to-scoped-key cast. An adapter cannot recover provenance that a legacy bare integer never carried and must not claim otherwise. A legacy container may offer a checked conversion from one of its own bare ids to a scoped key, so that code holding legacy ids can reach the bounded readers. The conversion checks only that the node or edge is live in that container, trusts the caller about where the id came from exactly as the container's other legacy methods do, and must document that it establishes nothing more. A reader never accepts a bare id. Persistent adapters use typed record keys instead, not these scoped in-memory handles.

With indexed containers, raw structural fields (`nodes`, `edges`, and identity counters) become private. This is an explicit source compatibility change; the implementation must ship migration documentation and a diagnostic at each former public-field access, explaining the supported operation or reader that replaces it. Each member made private carries RFC 150's `made_private` facts, so the visibility refusal at a former public access is that diagnostic and names the replacement. Existing constructor and method behavior remains governed by RFC 047. The migration must not return a copied list under the old field name or silently ignore direct edits.

Graph-owned mutations must pass through supported operations, including payload replacement when indexes depend on the changed fields. This does not require all application models to be frozen. An independently reconstructed or copied model can be edited normally; editing it alone does not publish a change to the graph. No exposed alias may change structural or indexed state behind the maintained indexes.

### Typed relationship graph

RFC 169 defines the common record-set and record-stream interface for records of several model kinds. This graph RFC retains ownership of node/edge identity, adjacency, and traversal. Multiple typed node and edge payloads do not depend on that interface.

`RelationGraph[N, E]` must store typed node payloads `N` and edge payloads `E`, with stable node and edge identities scoped to its instance. `add_node(value)` returns a `ScopedNodeId`; `add_edge(source, target, payload)` returns `Result[ScopedEdgeId, GraphError]`. It must support parallel edges, payload lookup and replacement, node deletion with incident-edge removal, and forward/reverse adjacency. An edge update must not accidentally replace another parallel edge with the same endpoints. Removed identities must not be reused within that graph instance.

`E` may be a complete model or an explicit payload enum admitting several edge models; `N` has the same freedom for node models. Stored relationships must preserve the checked variant and payload identities. Selecting one relationship kind must be possible through a declared family/variant index without decoding unrelated edge payloads. The graph adapter must validate compatible descriptors and must not collapse different variants because field names happen to match. RFC 169 owns their general record-set and record-stream projection surface.

The required relation surface is directed: `source -> target` is authoritative, and incoming adjacency is reverse lookup over the same edge records. An undirected extension must be explicitly selected and must define its identity and traversal rules; this RFC does not imply an undirected constructor or silently add a second edge.

This container does not infer legal domain endpoint kinds from an arbitrary enum payload. A domain adapter or a typed family relation must declare and validate those rules. Invalid endpoint references must fail before an edge becomes visible.

Database relationship indexes must describe the same authoritative records as direct record reads. Type erasure inside an implementation must not permit incompatible model or relation families at the public API boundary.

### Indexed access and performance obligations

Optimizing existing graph operations is ordinary implementation work and does not require this RFC or another language-design approval. Adjacency indexes, faster membership checks and reduced allocations may be delivered independently while preserving existing public contracts. This RFC is needed for typed relationship payloads and the shared, bounded graph-reading surface. Its access requirements constrain those new public contracts; they do not turn every performance improvement into a new design decision. Changing public fields or observable behavior remains a compatibility decision.

Let `V` and `E` denote live nodes and edges, and let `d(v)` be the selected incident edge count. Implementations must report physical retained/deleted entries separately so these variables do not hide tombstone scans.

| Operation                                           | Required access behavior                                                                                                                                                   |
| --------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Node or edge lookup                                 | Indexed lookup; no scan of all payloads. The implementation documents expected or worst-case lookup complexity.                                                            |
| Outgoing or incoming adjacency                      | An index seek plus work proportional to examined selected adjacency, not `V * E` or a whole edge-table scan per node.                                                      |
| Bounded adjacency page                              | Work and retained state bounded by the requested page and declared filtering work; unrelated components do not increase examined adjacency.                                |
| BFS or DFS over indexed memory graphs               | Structural work `O(Vr + Er)` over the reached subgraph, excluding explicitly measured user projection and ordering costs. No repeated linear visited-set membership scans. |
| Topological order and strongly connected components | Structural work `O(Vs + Es)` over the explicitly selected finite graph; cyclic and partial results are reported distinctly.                                                |
| Removal                                             | Work proportional to locating the node and its incident edges, plus index maintenance; no whole payload materialization.                                                   |
| Snapshot-backed graph open                          | A bounded descriptor and lease, not reconstruction of all vertices and links.                                                                                              |

These are access and complexity requirements, not universal latency promises. Persistent index seeks, integrity checks, output decoding and edge filtering must be included in measured counters. Implementations must not hide a full scan inside a predicate callback or call a bounded result efficient merely because it returns few values.

High-degree nodes must be pageable without allocating a complete neighbor list. Algorithm state must use efficient membership structures. Recursion must not be the only traversal mechanism for adversarial depth.

### Storage-independent graph reading protocol

`GraphRead[K, I, F]` is the proposed protocol: `K` is the node-key type, `I` the edge-key type and `F` the source-specific failure type. Keys must support stable equality and the documented ordering within their scope. Algorithms must preserve `F` through `GraphReadError[F]` instead of converting database errors to missing nodes or unstructured strings. This avoids a dependency from `std.graph` to a particular storage module.

| Operation                        | Result contract                                                                     |
| -------------------------------- | ----------------------------------------------------------------------------------- |
| `scope()`                        | Stable `GraphScope`: provider, graph selection, read revision and ordering profile. |
| `contains_node(key)`             | A scoped existence result or typed read failure.                                    |
| `nodes(budget, cursor)`          | A bounded page of node keys from the explicit selection.                            |
| `out_edges(key, budget, cursor)` | A bounded page of edge identities and source/target keys.                           |
| `in_edges(key, budget, cursor)`  | The same contract for incoming edges.                                               |
| `edge(key)`                      | One scoped edge identity and its endpoints, or explicit absence/failure.            |

The protocol separates structural reads from node/edge payload materialization. Typed container access or family adapters retrieve only the payloads a consumer requests. A graph projection must declare whether a selector restricts stored relation families, indexed keys or a post-read predicate. Cost accounting must include rejected candidates for the last case.

The default selection is an induced graph: an edge is visible only when both endpoints and its relation are in the selected scope. Completeness and no-path results apply only to that scope, never to an unexamined containing graph. Reporting boundary references requires an explicit projection profile; an adapter must not silently leave the selected scope to fetch additional nodes.

An adapter may normalize backend-specific keys into a tagged key type, but the mapping must retain origin and family scope. It must not require an eager whole-graph key map. Database-local row offsets must not escape as durable keys.

### Read scopes, ownership and cursor lifetime

RFC 164 owns explicit borrowing and returned reference contracts. Read-only projections use `Borrowed[T]`; `Reference[T]` is exclusive access and must not be used as a generic name for every retained view. A borrowed in-memory reader and its cursor must preserve their source dependencies. A database reader may instead own a lease retaining one immutable selection and may outlive the local snapshot binding when its complete resource contract permits that escape. Neither `Owned` nor a cursor type name alone proves independent retention. Views into reusable buffers remain borrowed until their last dependent use; advancing must not invalidate them. Implementation declarations must carry RFC 164 source dependencies in public metadata; neither a reader wrapper nor a cursor may erase them.

A traversal must observe one consistent read revision. The default in-memory `read_view()` holds a scoped read borrow preventing conflicting mutation and cannot outlive its source. A separate explicit snapshot operation may retain an immutable version with a documented copy or copy-on-write cost. Neither may quietly mix pre-edit and post-edit adjacency, and opening a reader must not conceal a whole-graph copy.

Database readers inherit the snapshot lease and retention guarantees of RFC 165. A cursor must retain the read scope and any frontier state needed to resume safely. Cursor creation, continuation and disposal must account for that state; cursors must not hide unbounded copies of the graph.

Resuming against another snapshot, graph selection, ordering or traversal profile must fail. A caller wanting a newer graph must start a new traversal or explicitly compute a diff. Releasing the last reader/cursor releases its retained state and lease; detached read scopes require explicit ownership transfer rather than a dangling borrow.

### Reader and cursor declarations

The required public operation shapes are below. Concrete generic parameter names and private handle layout are implementation details; the declared ownership and observable behavior are mandatory.

| Operation | Result and retained dependency |
| --- | --- |
| `graph.read_view()` on an in-memory graph | A shared reader that borrows the graph. Its public source dependency is the graph receiver; it cannot escape that owner or permit conflicting graph mutation. |
| `snapshot.graph[N, E](name)` | A fallible database reader that owns a lease on the selected snapshot, graph definition and indexes. Successful acquisition establishes that retention; the handle may outlive the binding used to obtain it. |
| `reader.incoming(key)` / `reader.outgoing(key)` | A lazy adjacency selection retaining the reader dependency and direction. Constructing it does not materialize neighbors. |
| `selection.cursor(max_records=..., max_bytes=...)` | A scoped cursor borrowing its selection/reader, with checked keys, fixed ordering and bounded state. Entering the scope acquires cursor resources; exit releases them without committing. |
| `cursor.next_batch()` | `Result[Option[Batch], F]`, with typed edge identity/endpoints and requested payloads. Absence means completed adjacency, never exhaustion of a work budget. |
| Explicit bounded traversal page | Values plus `Complete`, `Limited`, `Cancelled` or `Failed`, counters and an optional compatible continuation. Continuation retains its read scope and required state. |

Ordinary adjacency cursors borrow their reader; owning the cursor variable does not make it independently retained. An owned traversal continuation over a database may acquire or transfer a lease explicitly, and that operation must expose its ownership contract. No hidden lease or whole-graph clone repairs an escaping borrowed cursor. An in-memory continuation continues to borrow the graph unless an explicit snapshot operation created independently owned immutable storage.

Returned batch columns or payload views retain their actual buffer source. Advancing, closing, dropping or replacing an owner while a dependent borrow remains usable must be rejected, unless an explicitly retained immutable buffer makes that operation safe under RFC 164. Moving an owned database reader or continuation transfers release obligations; closing the original database handle does not revoke an already acquired independent lease.

### Pages and ergonomic iteration

Explicit pages and scoped batch cursors are the required surface. RFC 115 fallible iteration is an adapter over the same page engine, not a separate traversal implementation. It must yield the same selected order and propagate source failures. If a traversal reaches a total budget, the adapter must expose the limited outcome and continuation through a typed result/error rather than end normally. Such iterator convenience may be added without reopening this RFC; it is not a prerequisite to deliver the required page and batch-cursor API.

### Traversal, limits and completeness

The standard algorithms must include bounded breadth-first and depth-first traversal, reachability, topological ordering and strongly connected components. They must take explicit scope, direction, relation selection and resource budgets. General graphs may contain cycles and self-loops; only DAG-specific operations enforce acyclicity.

Budgets must cover visited nodes, examined edges, returned bytes, retained frontier/membership state and cancellation. A time budget may supplement deterministic count/byte bounds, but must not be the only protection against unbounded materialization. Exhaustion must return a limited result or a typed resource failure, never an apparently complete result.

Results must distinguish `Complete`, `Limited` with reason and optional continuation, `Cancelled` and `Failed`. If a traversal cannot retain a safe continuation within the budget, it must say that continuation is unavailable. It must not silently restart and duplicate effects. Traversals themselves must be effect-free.

Reachability must distinguish a witnessed path, a completed proof of no path, and an inconclusive bounded search. A partially scanned graph is not evidence of acyclicity or a complete topological order. Topological and component results must identify the selected subgraph they describe. Paths and component membership must preserve parallel edge identities where those identities affect the question.

### Ordering and determinism

RFC 047's documented legacy ordering must remain observable through legacy methods. New readers must advertise a stable ordering profile. The default in-memory profile enumerates nodes by node insertion order and adjacency by edge insertion order among retained live identities; the default database profile is the declared stable key/index order. Equality of payloads must not be used as a tie-breaker for identity.

Given the same graph revision, selection, ordering and deterministic budgets, paginated and unpaginated execution must yield the same logical order and membership. Changing only page size must not change the answer. Cross-backend byte-identical results require an explicitly shared ordering and encoding profile; they must not be claimed solely because both backends implement `GraphRead`.

### Observation, dependency evidence and graph changes

Storage-backed observations must be able to contribute to RFC 165 read traces. The trace must include negative node/edge lookups, empty adjacency, selected relation/index profiles and the ranges actually examined. Inserting a new relevant edge must invalidate an observation that previously found no edge.

A graph edge is not automatically an invalidation edge. Consumers must specify the dependency relation and the value projection they subscribed to. A compiler may depend on a declaration's checked signature; a renderer may depend on a title but not the body; a UI region may observe one state field. The graph library must not infer these semantic dependencies from containment or references.

Graph diffs must distinguish node changes, edge changes and payload changes within identified before/after scopes. Physical compaction or a changed in-memory slot must not appear as a logical graph change. A consumer may filter changes by its declared subscription, but the graph layer must not run that consumer's scheduler, event handler or renderer.

Dependency cycles require an explicit consumer policy. Strongly connected components are a structural result, not permission to run a fixpoint computation, arbitrarily choose an execution order or suppress an error.

### Errors and inspectability

New read surfaces must distinguish unknown keys, scope mismatch, stale/incompatible cursor, unsupported selection, resource exhaustion, cancellation and source failures. The source failure type must remain inspectable so corruption, unavailable snapshots and access refusals do not become ordinary graph absence.

A traversal report must expose its scope, selection, ordering, bounds, completeness and counters: nodes visited, edges examined, payloads materialized, bytes copied, frontier high-water mark, index seeks/pages when supplied by the backend, and early termination reason. It must not imply whole-graph coverage from a successful bounded request.

### Conformance across consumers

| Scenario                             | Required behavior                                                                                                                                                                           |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Sparse and disconnected graphs       | Adding an unrelated component does not add adjacency work to a fixed local query.                                                                                                           |
| Deep chains, cycles and high fan-out | No stack exhaustion, silent truncation or mandatory full-neighbor allocation; budgets and continuation remain explicit.                                                                     |
| Parallel typed relationships         | Payload and edge identities remain distinct; invalid endpoint families are rejected by typed adapters.                                                                                      |
| Compiler dependency graph            | Signature, body and source-position observations remain distinct; negative lookups and newly matching edges invalidate the appropriate read evidence.                                       |
| Publication graph                    | One parse result may feed multiple route/profile outputs; includes, navigation, templates, renames and deletions preserve clean-versus-incremental equivalence.                             |
| Declarative experience graph         | Shared model identity differs from each surface occurrence; containment, observation, reference and evidence relations remain distinguishable; graph updates do not execute effects.        |
| Ordered interactive occurrences      | Reordering children preserves occurrence identity and local state; two appearances of one model remain distinct; a field-specific observation does not imply a subscription to every field. |
| Draft and published views            | A preview fork cannot mutate the published selection; traversal is snapshot-consistent during commits, collection and compaction.                                                           |
| Backend parity                       | Memory and database readers agree under the same declared graph and ordering profile, including limited and failed results.                                                                 |

Performance evidence must include narrow and high-degree reads, cold and warm runs, retained memory and allocation/copy counters at increasing graph sizes. Whole-workload comparisons must hold semantics and outputs fixed. A faster neighbor microbenchmark is not proof of faster publication, compilation or rendering.

## Design details

### Relationship to `std.db`

`std.graph` owns graph structure and algorithms, not storage publication or durable identity. RFC 165's adapter implements the read protocol over selected typed families and indexes. The adapter owns conversion between persistent record references and graph keys; it must preserve snapshot and family scope without allocating a duplicate graph.

An application can choose memory graphs, a memory database or a durable database according to its needs. A pure graph dependency must not pull in the database, an analytical backend, a protocol compiler or a UI runtime. The platform substrate must not depend on graph algorithms or a downstream query library merely to store records.

### Logical storage and Crumb

A named stored graph has three logical components: a graph definition, node memberships referencing typed model records, and edge records containing identity, endpoints and typed payload references. An edge is a relationship; a fourth generic relationship entity is not required. A projection over existing families may derive memberships instead of materializing a second membership table. This is a logical model, not a requirement for SQL tables, one physical file per component or one storage partition for every edge kind.

Outgoing and incoming indexes reference the same authoritative edges and retain graph, endpoint and relationship-kind scope. A graph query can request structure alone without decoding node or edge payloads. Indexes may be partitioned by family or variant; source selection must disclose which access path is used. Shared Crumb contents never merge node, edge or record identities.

RFC 165 owns Crumb representation, database storage placement and revisions. Disk, spooled-memory and memory stores must expose the same graph meaning. Record revisions and all affected indexes publish atomically; an old snapshot continues to see the old complete graph. Updating a local model or calling `.crumble()` does not itself update a stored edge. Node deletion removes its incident graph relationships or fails atomically according to the graph operation; shared model records retain their separate database identity and retention rules.

A large compiler graph must be usable directly through this path. Opening a graph, narrowing edge kinds, reading a small adjacency and obtaining a continuation must not require decoding or copying the complete graph. Qualification must include large, mixed-model compiler graphs, distinct source locations and long-running revisions, with working memory, decoding, copying and retained leases measured. Small in-memory lookup benchmarks alone do not establish this property.

### Declarative frameworks and targets

An experience framework may use graphs for authored structure, dependencies, artifact bindings and inspection. It retains ownership of accessibility, appearance, event meaning, state-transition policy, reconciliation and target lowering. Graph location does not determine whether a model or decision is authoritative.

Persistent graph payloads are typed data and references, not event callbacks or host resources. Browser and native targets may bind those records to different mechanisms. The structural in-memory surface must remain usable on supported targets without filesystem access; database-backed readers must report unavailable storage capabilities rather than silently change execution placement.

Rendering order and graph traversal order are distinct. A framework needing ordered children must declare an order field or an ordered relation profile, not rely on a database's incidental row order. It may use a stable occurrence identity to preserve local state while a shared content revision changes.

### Compatibility and declaration boundaries

No new parser syntax or graph-query block is introduced. The proposed library types and protocols use ordinary Incan declarations. Any missing generic, borrowing or trait capability must be tracked as a language requirement rather than replaced with a private foreign-language graph API.

Existing eager APIs remain available. New bounded APIs use explicit read scopes and completeness outcomes. Existing ids cannot be upgraded into durable references by casting their integer representation; migration must preserve the graph/family mapping explicitly.

## Alternatives considered

- **Only optimize the existing lists:** useful for the old containers, but insufficient for snapshot-backed readers, typed relationships and bounded persistent traversal.
- **Always materialize a database graph into `DiGraph`:** rejected because it duplicates authoritative data and makes a small read pay for the full graph.
- **Make `std.graph` a database facade:** rejected because small transient graphs must remain independent of persistence and transaction machinery.
- **Expose arbitrary string-labeled property edges as the semantic API:** rejected because typed domains require declared payloads and valid endpoint kinds; a generic internal representation is not an authoring contract.
- **One universal dependency relation and scheduler:** rejected because syntax, containment, observation, evidence and computational dependence have different meanings, and valid graph cycles do not prescribe execution semantics.
- **Silently truncate eager results:** rejected because incomplete reachability, ordering and lineage answers must not masquerade as complete ones.

## Drawbacks

Read scopes, typed failures, cursors and completeness add concepts beyond a small mutable graph. Indexes trade memory and update work for faster selected reads. Supporting legacy eager methods and new bounded readers requires careful compatibility tests. Stable ordering can add sorting or index costs and must be an explicit profile rather than an unmeasured promise. These costs are warranted by consumers needing bounded access and a shared memory/database graph contract.

## Layers affected

- **Standard library:** indexed graph containers, typed edge payloads, the read protocol, bounded algorithms and resource reporting.
- **Storage adapters:** snapshot-bound reading, typed endpoint preservation, index access and dependency evidence through RFC 165.
- **Type checking and ownership:** ordinary generic protocol dispatch and safe reader lifetimes; unsupported language requirements must be explicit.
- **Tooling and contracts:** observable traversal reports, compatibility checks and cross-backend conformance, without claiming a new query language.

## Design Decisions

- **Scope:** typed relationships, storage-independent readers, ownership and bounded traversal are the new library surface. Performance upgrades to existing graph operations remain ordinary implementation work.
- **Typed payloads and direction:** nodes and edges accept full models, with explicit payload enums for multiple kinds. Directed edges preserve separate identities and payloads; incoming lookup does not create reverse edges. Endpoint legality follows declared adapters, not naming conventions.
- **Graph-first storage:** named typed graphs can be authored directly through a database transaction. Existing record attachment and explicit projections avoid model duplication. RFC 165 handles Crumb, snapshots, storage placement and publication; large graphs never require whole-graph materialization.
- **Storage ownership:** graph definitions, node memberships and edge records are logical structures. Incoming/outgoing indexes are derived access paths. Raw structural fields become private in indexed containers, with migration diagnostics and supported replacements; constructor and method compatibility remains governed by RFC 047.
- **Reader lifetime:** memory readers and ordinary cursors borrow their actual source. Database snapshot graph readers own acquired leases; explicitly retained continuations may do the same. Borrowed buffers cannot outlive or be invalidated by their owners, and ordinary users rely on duckborrowing.
- **Traversal surface:** explicit pages and scoped batch cursors are required. Fallible iterator convenience uses the same engine and preserves limited outcomes; it is not a separate prerequisite. Limits must never look like successful exhaustion.
- **Record-set boundary:** RFC 169 owns general projections over records of several kinds, record streams and change streams. Supporting multiple graph payload kinds and implementing the graph reader do not depend on that surface.
