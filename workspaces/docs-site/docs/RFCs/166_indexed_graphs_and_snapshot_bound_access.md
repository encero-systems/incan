# RFC 166: Indexed graphs and snapshot-bound graph access

- **Status:** Draft
- **Created:** 2026-10-30
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 164 (explicit ownership and borrowing contracts)
    - RFC 030 (extended collections)
    - RFC 047 (lightweight directed graph types)
    - RFC 101 (ordinal maps)
    - RFC 106 (compiler-backed context graphs)
    - RFC 115 (fallible iteration)
    - RFC 161 (semantic core IR)
    - RFC 165 (typed embedded storage)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.6.0-dev.9
- **Shipped in:** —

## Summary

Evolve `std.graph` into an indexed, bounded and storage-independent graph facility while preserving RFC 047's in-memory containers. Add typed relationship payloads, snapshot-bound graph readers, paged traversal and explicit completeness, allowing the same algorithms to work over ordinary graph values or `std.db` records without copying an entire graph. Native performance becomes an inspectable access contract, not an inference from the compilation target. This is a follow-on to implemented RFC 047, not a replacement of its historical specification or a requirement that all graphs become databases.

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

The new in-memory read surface uses `ScopedNodeId` and `ScopedEdgeId`, combining graph-instance identity with the existing local handle. Standard adapters must validate that identity before lookup. Bare legacy ids remain accepted by legacy container methods only; readers issue scoped keys for their own nodes and edges and must not offer an unchecked integer-to-scoped-key cast. An adapter cannot recover provenance that a legacy bare integer never carried and must not claim otherwise. Persistent adapters use typed record keys instead, not these scoped in-memory handles.

Compatibility work must explicitly account for the current public model fields. Removing or privatizing a publicly exposed field is a breaking change even when direct mutation was undesirable; migration must identify and deprecate that access before removal. No implementation may silently preserve writable raw lists that can make an adjacency index stale.

### Typed relationship graph

`RelationGraph[N, E]` must store typed node payloads `N` and edge payloads `E`, with stable node and edge identities scoped to its instance. `add_node(value)` returns a `ScopedNodeId`; `add_edge(source, target, payload)` returns `Result[ScopedEdgeId, GraphError]`. It must support parallel edges, payload lookup and replacement, node deletion with incident-edge removal, and forward/reverse adjacency. An edge update must not accidentally replace another parallel edge with the same endpoints. Removed identities must not be reused within that graph instance.

This container does not infer legal domain endpoint kinds from an arbitrary enum payload. A domain adapter or a typed family relation must declare and validate those rules. Invalid endpoint references must fail before an edge becomes visible.

Database relationship indexes must describe the same authoritative records as direct record reads. Type erasure inside an implementation must not permit incompatible model or relation families at the public API boundary.

### Indexed access and performance obligations

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

RFC 164 owns explicit borrowing and returned reference contracts. Read-only projections use `Borrowed[T]`; `Reference[T]` is exclusive access and must not be used as a generic name for every retained view. A borrowed in-memory reader and its cursor must preserve their source dependencies. A database reader may instead own a lease retaining one immutable selection and may outlive the local snapshot binding when its complete resource contract permits that escape. Neither `Owned` nor a cursor type name alone proves independent retention. Views into reusable buffers remain borrowed until their last dependent use; advancing must not invalidate them. Concrete reader and cursor declarations must be reviewed against RFC 164, whose single-source reference contract targets 0.6.

A traversal must observe one consistent read revision. The default in-memory `read_view()` holds a scoped read borrow preventing conflicting mutation and cannot outlive its source. A separate explicit snapshot operation may retain an immutable version with a documented copy or copy-on-write cost. Neither may quietly mix pre-edit and post-edit adjacency, and opening a reader must not conceal a whole-graph copy.

Database readers inherit the snapshot lease and retention guarantees of RFC 165. A cursor must retain the read scope and any frontier state needed to resume safely. Cursor creation, continuation and disposal must account for that state; cursors must not hide unbounded copies of the graph.

Resuming against another snapshot, graph selection, ordering or traversal profile must fail. A caller wanting a newer graph must start a new traversal or explicitly compute a diff. Releasing the last reader/cursor releases its retained state and lease; detached read scopes require explicit ownership transfer rather than a dangling borrow.

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

## Unresolved questions

- Which migration window and diagnostics should cover the existing public container fields before their storage representation can become opaque?
- Which concrete reader and resumable-cursor declarations instantiate RFC 164's source-dependency contract for memory graphs and owned lease retention for database graphs? RFC 164 owns the general borrowing syntax. The default memory reader remains a scoped read borrow, and neither implementation may substitute a hidden graph copy.
- Should bounded algorithms expose a fallible iterator in addition to pages at initial adoption? Either surface must preserve identical completeness, resource and scope rules; an iterator must not hide an unbounded frontier.

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
