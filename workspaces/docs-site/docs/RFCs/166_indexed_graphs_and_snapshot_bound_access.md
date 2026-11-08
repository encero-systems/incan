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
    - RFC 107 (`Variant[E]` tokens for relationship kinds)
    - RFC 115 (fallible iteration)
    - RFC 122 (construction-safe filters)
    - RFC 142 (cancellation tokens and deadlines)
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

`read_view()` reads the whole graph, and `read_selected(...)` reads part of it. The selection is fixed when the reader opens, as a database projection fixes its families:

```incan
from std.graph import MemoryReadError, RelationGraph, ScopedNodeId

def title_reads(
    graph: RelationGraph[Region, ReadsState],
    editor: ScopedNodeId,
    preview: ScopedNodeId,
) -> Result[int, MemoryReadError]:
    """Count the title dependencies leaving the editor within two regions."""
    view = graph.read_selected(
        node_keys=[editor, preview],
        edge_filter=(edge) => edge.value.field == StateField.Title,
    )?
    mut count = 0
    with view.outgoing(editor).cursor(max_records=64, max_bytes=65_536) as cursor:
        while let Some(batch) = cursor.next_batch()?:
            for edge in batch:
                count += 1
    return Ok(count)
```

Over the graph built by `region_dependencies()`, the count is 1: the body read between the same regions fails the filter. An edge from the editor to a third region would not count either, because its target is outside `node_keys`, even though the edge exists in the graph.

Algorithms accept this protocol, start keys, a direction and a budget. A bounded walk returns a page of visits and a completion outcome. The caller may resume from the page's continuation or stop. It must not treat a budget-limited result as the complete descendant set.

```incan
from std.graph import Direction, ScopedNodeId, TraversalBudget, TraversalOutcome, breadth_first

def affected_regions(
    graph: RelationGraph[Region, ReadsState],
    edited: ScopedNodeId,
) -> None:
    """Report which regions an edit can reach, and whether the answer is whole."""
    budget = TraversalBudget(
        max_nodes=1_000, max_edges=10_000, max_bytes=1_048_576, max_retained=4_096,
    )
    view = graph.read_view()
    page = breadth_first(view, [edited], Direction.Outgoing, budget)
    match page.outcome:
        TraversalOutcome.Complete =>
            print(f"{len(page.visits)} regions affected")
        TraversalOutcome.Limited(reason, _) =>
            print(f"partial: stopped at the {reason} bound after {len(page.visits)} regions")
        TraversalOutcome.Cancelled =>
            print("cancelled")
        TraversalOutcome.Failed(error) =>
            print(f"read failed: {error}")
```

A limited page carries a continuation. `resume(budget)` on it returns the next page of the same walk.

Read-only access does not require a writable graph. A database graph view must not load every node into `DiGraph` before answering a neighbor query.

### One source, several appearances

A publisher should be able to distinguish a source document, its parsed revision, and each route/profile-specific rendered occurrence. An interactive framework should likewise distinguish a shared model record from two component instances observing it.

Graphs must not deduplicate these nodes simply because their payloads or rendered text are equal. Shared computations may refer to one common result node; occurrences remain separately identifiable. Edges distinguish containment, reference, observation and dependency rather than forcing all four into one adjacency with assumed semantics.

## Reference-level explanation

### Existing containers and compatibility

RFC 047 remains the contract for existing constructors and eager methods. This RFC adds indexed implementations and new read surfaces. Existing eager list-returning methods may remain as documented convenience APIs; they must not become the hidden implementation of bounded graph readers.

The source representation of a container is not its semantic identity. Implementations may use adjacency indexes, dense slots, ordinal maps or persistent pages, but arbitrary mutation of internal storage fields must not become a supported way to bypass graph invariants.

Existing graph-local `NodeId` and `EdgeId` handles do not become globally unique or durable. `to_int()` remains a representation for diagnostics or caller-managed interchange, not a credential, persistent record identity or safe cross-graph key. A new read protocol must carry scope separately when adapting such handles.

The new in-memory read surface uses `ScopedNodeId` and `ScopedEdgeId`, combining graph-instance identity with the existing local handle. Standard adapters must validate that identity before lookup. Bare legacy ids remain accepted by legacy container methods only; readers issue scoped keys for their own nodes and edges and must not offer an unchecked integer-to-scoped-key cast. An adapter cannot recover provenance that a legacy bare integer never carried and must not claim otherwise. A legacy container may offer a checked conversion from one of its own bare ids to a scoped key, so that code holding legacy ids can reach the bounded readers. The conversion checks only that the node or edge is live in that container, trusts the caller about where the id came from exactly as the container's other legacy methods do, and must document that it establishes nothing more. A reader never accepts a bare id. Persistent adapters use their own typed node keys, one for each graph membership, not these scoped in-memory handles.

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

### Selecting a read scope

A reader's selection is fixed when the reader opens. An open reader is never narrowed or widened; another selection means another reader.

On an in-memory graph, `graph.read_view()` selects the whole graph, and `graph.read_selected(...)` selects through the arguments below. Each argument is one of the three selector kinds a projection declares:

| Argument | Selector kind | Selects |
| --- | --- | --- |
| `node_keys` | Indexed keys | Nodes, as a collection of the graph's own scoped node keys. |
| `edge_kinds` | Relation families | Relationship kinds, as `Variant[E]` tokens written `E.Name` (RFC 107). The family/variant index serves this selection, so no other payload is read. |
| `node_filter` | Post-read predicate | Nodes for which the filter returns `true`. The filter receives a node item: `key` and `value`. |
| `edge_filter` | Post-read predicate | Edges for which the filter returns `true`. The filter receives the edge item an adjacency batch yields: `key`, `source`, `target` and `value`. |

An omitted argument selects everything of its kind. An empty collection selects nothing. The induced rule combines the arguments. A node is visible when it is in `node_keys` and passes `node_filter`. An edge is visible when both endpoints are visible, its kind is in `edge_kinds`, and it passes `edge_filter`. Rejected candidates, and the payloads the filters read, count in the traversal report.

`edge_kinds` requires `E` to be an enum. Passing it to a graph whose edge payload is a model, or to a container without edge payloads, is a compile-time error. Because a `Variant[E]` token carries its enum, a variant of another type cannot reach the reader.

A filter judges only the candidate it receives, and receives payloads read-only. It must be a construction-safe callable under RFC 122: synchronous, deterministic and free of observable effects. Its captures must carry no borrow. A capture that depends on the graph, on a reader or on any other borrowed source is a compile-time error. Keys are values and carry no borrow, so a filter may capture them. This keeps a filter from hiding a scan of the graph. A reader's only source is its graph.

`read_selected` copies `node_keys` into the reader, at a cost proportional to the selection, never to the graph. It validates the keys and is fallible. A key from another graph instance fails with `ScopeMismatch("node_keys")`. A key of a removed node selects nothing and is not an error.

A database reader takes the same arguments: `snapshot.graph[N, E](name, node_keys=..., edge_kinds=..., node_filter=..., edge_filter=...)`, with the database graph's node keys in place of scoped node keys. A node key names one graph membership, so a record attached twice has two keys. The explicit projection form keeps its family descriptors. An algorithm's relation selection applies within its reader's scope and never widens it.

### Read failures

Every read surface fails with `GraphReadError[F]`, where `F` is the source's own failure type:

| Case | Meaning |
| --- | --- |
| `UnknownKey(argument)` | A key is not visible in the reader's selection. |
| `ScopeMismatch(argument)` | A key belongs to another graph instance or snapshot. |
| `UnsupportedSelection(argument)` | A selection the reader cannot serve, such as a kind outside its selection. |
| `InvalidBudget(argument)` | A budget count that is not positive. |
| `StaleCursor` | A cursor whose reader, selection or ordering no longer matches. |
| `ResourceLimit(reason)` | A cursor's hard limit, as a `LimitReason`. |
| `Source(F)` | A failure of the underlying source, kept inspectable. |

`argument` names the argument at fault, such as `"starts"`, `"target"` or `"node_keys"`. An in-memory reader has no source failures: its `F` is RFC 047's `GraphError` and it never produces `Source`. `std.graph` names that type `MemoryReadError`.

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
| `graph.read_selected(...)` on an in-memory graph | `Result[reader, MemoryReadError]`: a shared reader over the validated selection, with the same borrow of the graph. Its only public source dependency is the graph receiver; it owns its copied keys and its filters. |
| `snapshot.graph[N, E](name)` | A fallible database reader that owns a lease on the selected snapshot, graph definition and indexes. Successful acquisition establishes that retention; the handle may outlive the binding used to obtain it. |
| `reader.incoming(key)` / `reader.outgoing(key)` | A lazy adjacency selection retaining the reader dependency and direction. Constructing it does not materialize neighbors. |
| `selection.cursor(max_records=..., max_bytes=...)` | A scoped cursor borrowing its selection/reader, with checked keys, fixed ordering and bounded state. Entering the scope acquires cursor resources; exit releases them without committing. |
| `cursor.next_batch()` | `Result[Option[Batch], GraphReadError[F]]`, with edge items carrying `key`, `source`, `target` and the requested `value`. A batch examines at most `max_records` candidates, so a batch whose candidates the filters all rejected is empty. An empty batch is not exhaustion: only `None` means the selected adjacency is complete, never the end of a work budget. |
| Traversal functions | They borrow their reader. A result depends on that reader (`from reader`) through its continuation. Under RFC 164, a branch that matched an outcome without a continuation holds nothing borrowed. |
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

### Traversal operations

The bounded algorithms are functions in `std.graph` over any `GraphRead` reader, so memory and database readers share one engine. The reader is the call's scope; no algorithm reads outside it.

| Function | Result |
| --- | --- |
| `breadth_first(reader, starts, direction, budget, edge_kinds=...)` | `TraversalPage[R]`: visits in breadth-first order |
| `depth_first(reader, starts, direction, budget, edge_kinds=...)` | `TraversalPage[R]`: visits in depth-first preorder |
| `find_path(reader, start, target, direction, budget, edge_kinds=...)` | `PathSearch[R]`: a shortest path, a proof of none, or an inconclusive search |
| `topological_order(reader, direction, budget, edge_kinds=...)` | `OrderResult[R]` over the reader's whole selection |
| `strongly_connected_components(reader, direction, budget, edge_kinds=...)` | `ComponentsResult[R]` over the reader's whole selection |

`R` is the reader's type, and the key type `K`, edge identity type `I` and failure type `F` follow from it.

Common arguments:

- `starts` is a collection of node keys; one start is a one-element collection. A repeated start is visited once, at its first position. An empty collection is `Complete` with no visits.
- `direction` is `Direction.Outgoing` or `Direction.Incoming` and has no default. `Incoming` follows the same edges from target to source.
- `edge_kinds` narrows the reader's relation selection for one call, with the same `Variant[E]` tokens `read_selected` takes. A kind outside the reader's selection fails with `UnsupportedSelection("edge_kinds")`.
- An algorithm calls no user code except the reader's filters and construction-safe projections that its own signature declares. There is no visitor callback; a caller acts on a result after it returns.

Inputs are checked before any work. A start or target that is not visible in the reader's selection fails with `UnknownKey`, and a key from another graph or snapshot fails with `ScopeMismatch`; either names its argument. A budget count that is not positive fails with `InvalidBudget`.

`TraversalBudget(max_nodes, max_edges, max_bytes, max_retained, stop=None, deadline=None)` bounds one call:

- `max_nodes` counts visited nodes. An algorithm visits a node when it first reaches it, and `topological_order` and `strongly_connected_components` visit every selected node once.
- `max_edges` counts examined edges: every candidate the call reads, including those that the induced rule or a filter rejects. Edges that `edge_kinds` excludes are never read and never count.
- `max_bytes` counts the RFC 165 encoded size of the payloads the call reads, whichever backend serves it. A structural walk without payload filters reads none.
- `max_retained` counts frontier, visited-set and continuation entries held at once.
- `stop` is an RFC 142 `CancellationToken`. It is checked before the first read and between steps, as RFC 142's consumers check it.
- `deadline` is an RFC 142 `Instant`, checked at the same points. It supplements the four counts and never replaces them.

The four counts have no defaults, so an unbounded traversal cannot be written by leaving a bound out. A step is one node visited or one edge examined. A call stops only when its next step would exceed a count, so a count can be reached exactly. When several conditions hold at the same step, the outcome is the first of `Failed`, `Complete`, `Cancelled` and `Limited`.

A walk returns `TraversalPage[R]` with `visits`, `outcome` and `report`:

- Each `Visit` has `node`, `depth` and `via`. `via` is the identity of the edge that first reached the node, or `None` for a start. Following `via` back reconstructs a path, with parallel edges kept distinct.
- `breadth_first` seeds every start at depth 0, in the order given, and `depth` is the distance from the nearest start. `depth_first` runs from each start in turn and skips a start an earlier one already reached; `depth` and `via` describe its depth-first forest. Neighbors are taken in the reader's ordering profile, so equal inputs give equal pages.
- `outcome` is a `TraversalOutcome`:
    - `Complete`: the frontier is empty, so every node reachable from `starts` within the scope was visited.
    - `Limited(reason, continuation)`: `reason` is a `LimitReason` (`Nodes`, `Edges`, `Bytes`, `Retained` or `Deadline`). `continuation` is `None` when the remaining state does not fit `max_retained`.
    - `Cancelled`: the token fired. There is no continuation.
    - `Failed(error)`: a `GraphReadError[F]`.
- The visits made before a `Limited`, `Cancelled` or `Failed` outcome stay in the page.

`continuation.resume(budget)` returns the next result of the same call, of the same type, and consumes the continuation, so no page is produced twice. A continuation borrows the reader of the call that produced it. It therefore resumes against the same scope, selection, revision and ordering by construction. For a walk, the visits of successive pages, in order, equal the visits of one call whose budget was never reached. The state a continuation carries counts against the resumed budget's `max_retained`. A resume that cannot hold that state returns `Limited(Retained, None)`.

`find_path` returns `PathSearch[R]` with `outcome` and `report`. Its `PathOutcome` is one of:

- `Found(path)`: a shortest path by edge count, the first in the reader's ordering among equals. A `Path` has `nodes`, from `start` to `target`, and `edges`, where `edges[i]` joins `nodes[i]` and `nodes[i + 1]`. With `Incoming`, each edge is followed against its direction. `start == target` is found with one node and no edges.
- `NoPath`: the search exhausted the scope. This proves the absence of a path within the scope only.
- `Inconclusive(reason, continuation)`: a bound was reached first.
- `Cancelled` or `Failed(error)`.

`topological_order` returns `OrderResult[R]` with `outcome` and `report`. Its `OrderOutcome` is one of:

- `Ordered(nodes)`: every node of the selection, each before the nodes its selected edges lead to in `direction`. With `Outgoing` a caller comes before its callees, and with `Incoming` it comes after them. The order is Kahn's, with a first-in, first-out queue. The queue is seeded with the nodes no selected edge leads to in `direction`, in the reader's ordering profile. The nodes each placed node leads to join the queue in adjacency order once nothing else leads to them.
- `Cyclic(cycle)`: a witnessed cycle, as a `Path` whose last edge returns to its first node. The cycle is the one closed by the first back edge of a depth-first search over the nodes Kahn's order could not place. That search starts from those nodes in profile order and takes adjacency in profile order. A self-loop is a one-edge cycle. No partial order is returned in its place.
- `Limited(reason, continuation)`, `Cancelled` or `Failed(error)`.

`strongly_connected_components` returns `ComponentsResult[R]` with `outcome` and `report`. Its `ComponentsOutcome` is one of:

- `Components(components)`: every node of the selection in exactly one `Component`. A component has `nodes` and `cyclic`, which is true when it has more than one node or a self-loop. Components and their nodes come in Tarjan's emission order, which is a reverse topological order of the condensation in `direction`. Roots are tried in the reader's ordering profile, and adjacency is taken in profile order.
- `Limited(reason, continuation)`, `Cancelled` or `Failed(error)`.

A limited, cancelled or failed `topological_order` or `strongly_connected_components` result carries no partial answer; only a final result holds the order, cycle or components. These algorithms need `max_retained` at least the size of the selection to finish.

Every `report` is a `TraversalReport`. It names the scope, the effective selection and the ordering profile, so a topological or component result identifies the subgraph it describes. The effective selection is the reader's selection narrowed by the call's `edge_kinds`. The report also gives the budget and these counters:
- nodes visited;
- edges examined;
- candidates rejected;
- payloads read and bytes counted;
- the retained high-water mark;
- index seeks and pages, when the backend supplies them;
- the reason the call stopped.

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
- **Selection at opening:** a reader's selection is fixed when it opens, through the same arguments in memory and in a database. Filters judge only the candidate they receive, so cost accounting stays exact and a reader carries no hidden second view of its graph.
- **Reader lifetime:** memory readers and ordinary cursors borrow their actual source. Database snapshot graph readers own acquired leases; explicitly retained continuations may do the same. Borrowed buffers cannot outlive or be invalidated by their owners, and ordinary users rely on duckborrowing.
- **Traversal operations:** the algorithms are `std.graph` functions over any reader, so every backend shares one engine. A budget has no default bounds, counting is exact at a bound, and a result names complete, limited, cancelled or failed work. Orders and components are defined to the node, so equal inputs give equal answers on every backend. A continuation resumes once, against the reader it borrows.
- **Traversal surface:** explicit pages and scoped batch cursors are required. Fallible iterator convenience uses the same engine and preserves limited outcomes; it is not a separate prerequisite. Limits must never look like successful exhaustion.
- **Record-set boundary:** RFC 169 owns general projections over records of several kinds, record streams and change streams. Supporting multiple graph payload kinds and implementing the graph reader do not depend on that surface.
