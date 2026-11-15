# std.graph reference

This page is the API reference for `std.graph`. For task-oriented examples, see [Working with graphs](../../how-to/working_with_graphs.md). For the design model and boundaries, see [Why `std.graph` exists](../../explanation/graph_model.md).

## Importing the graph API

Import graph containers, identifier handles, read pages, and graph errors from `std.graph`:

```incan
from std.graph import Dag, DiGraph, GraphPageBudget, GraphRead, MultiDiGraph, RelationGraph
```

## Types

| Type | Meaning |
| --- | --- |
| `DiGraph[T]` | Mutable directed graph with at most one active edge for each ordered node pair. |
| `Dag[T]` | Directed acyclic graph wrapper that rejects any inserted edge that would create a cycle. |
| `MultiDiGraph[T]` | Directed graph that allows parallel edges and returns stable `EdgeId` handles for each edge. |
| `RelationGraph[N, E]` | Mutable directed graph with typed node and edge payloads, parallel edges, and scoped identities. |
| `NodeId` | Opaque node handle scoped to the graph instance that created it. |
| `EdgeId` | Opaque edge handle scoped to the multigraph instance that created it. |
| `ScopedNodeId` | Unforgeable node identity carrying the graph instance that issued it. |
| `ScopedEdgeId` | Unforgeable edge identity carrying the graph instance that issued it. |
| `GraphError` | Error value returned when graph operations encounter unknown ids, duplicate edges, cycles, or missing edges. |
| `GraphRead[K, I, F]` | Storage-independent protocol for scoped, bounded structural graph reads. |
| `GraphReadErrorKind` | Inspectable category of a graph read failure. |
| `GraphReadError[F]` | Typed read failure preserving the backend failure type `F`. |
| `MemoryGraphFailure` | Unconstructible source-failure type used by in-memory readers. |
| `GraphScope` | Provider, selection, revision, and ordering fixed for one read scope. |
| `GraphProvider` | Backend family serving a read scope; the memory implementation reports `Memory`. |
| `GraphSelection` | Finite selection fixed for a read scope; relation graphs initially report `Whole`. |
| `GraphOrdering` | Stable ordering profile; memory pages report `Insertion`. |
| `GraphPageBudget` | Maximum structural records requested from one page, or an unbounded request. |
| `GraphPageToken` | Opaque owned continuation for one compatible page stream. |
| `NodePage[K]` | Node keys, continuation, and counters from one bounded page. |
| `EdgePage[K, I]` | Structural edges, continuation, and counters from one bounded page. |
| `GraphEdgeItem[K, I]` | One edge identity and its directed source and target keys. |
| `PageCounters` | Physical work and materialization counters for one structural page. |

## `NodeId` and `EdgeId`

| Member | Returns | Meaning |
| --- | --- | --- |
| `id.to_int()` | `int` | Runtime numeric representation for diagnostics, storage, or display. |

`NodeId` and `EdgeId` values are graph-local handles. Do not reuse ids across graph instances unless the surrounding data model explicitly tracks the graph they came from.

## Scoped identities

`ScopedNodeId` and `ScopedEdgeId` combine a local numeric identity with the graph instance that issued it. Their fields are private and there is no integer-to-scoped-key conversion. A key from another graph is rejected with a scope mismatch. `to_int()` returns only the local diagnostic representation; it does not remove or reproduce the scope.

Cloning a `RelationGraph[N, E]` creates a fresh graph scope. The clone rejects keys issued by the original graph.

## `RelationGraph[N, E]`

Construct a typed directed multigraph with `RelationGraph[N, E]()`.

| Member | Returns | Meaning |
| --- | --- | --- |
| `g.add_node(value)` | `ScopedNodeId` | Add one node carrying `value`. |
| `g.add_edge(source, target, value)` | `Result[ScopedEdgeId, GraphError]` | Add one directed edge after validating both scoped endpoints. |
| `g.remove_node(node)` | `Result[None, GraphError]` | Remove one node and every incident edge. |
| `g.remove_edge(edge)` | `Result[None, GraphError]` | Remove one edge without affecting parallel edges. |
| `g.node_value(node)` | `Result[N, GraphError]` | Return the typed node payload. |
| `g.edge_value(edge)` | `Result[E, GraphError]` | Return the typed edge payload. |
| `g.replace_node(node, value)` | `Result[None, GraphError]` | Replace one node payload by identity. |
| `g.replace_edge(edge, value)` | `Result[None, GraphError]` | Replace one edge payload by identity. |
| `g.contains_node(node)` | `Result[bool, GraphReadError[MemoryGraphFailure]]` | Return scoped node membership. |
| `g.contains_edge(edge)` | `bool` | Return whether one scoped edge is live in this graph. |

Node and edge identities are never reused within one graph instance. Equal payloads remain separate nodes or edges. Parallel edges retain distinct identities, and replacement addresses exactly one identity. A wrong-scope or unknown endpoint is rejected before an edge becomes visible.

## `GraphRead[K, I, F]`

`RelationGraph[N, E]` implements `GraphRead[ScopedNodeId, ScopedEdgeId, MemoryGraphFailure]`. `MemoryGraphFailure` has no public constructor, and memory readers do not produce `GraphReadError.Source`.

| Member | Returns | Meaning |
| --- | --- | --- |
| `reader.scope()` | `GraphScope` | Return the fixed provider, selection, revision, and ordering profile. |
| `reader.contains_node(key)` | `Result[bool, GraphReadError[F]]` | Return scoped node membership. |
| `reader.nodes(budget, cursor)` | `Result[NodePage[K], GraphReadError[F]]` | Return a bounded insertion-order page of node keys. |
| `reader.out_edges(key, budget, cursor)` | `Result[EdgePage[K, I], GraphReadError[F]]` | Return a bounded insertion-order page of outgoing edges. |
| `reader.in_edges(key, budget, cursor)` | `Result[EdgePage[K, I], GraphReadError[F]]` | Return a bounded insertion-order page of incoming edges. |
| `reader.edge(key)` | `Result[GraphEdgeItem[K, I], GraphReadError[F]]` | Return one edge identity with its source and target keys. |

`GraphScope` exposes `provider: GraphProvider`, `selection: GraphSelection`, `revision: int`, and `ordering: GraphOrdering`. The memory reader reports `GraphProvider.Memory`, `GraphSelection.Whole`, and `GraphOrdering.Insertion`.

### Page request and result types

| Member | Returns | Meaning |
| --- | --- | --- |
| `GraphPageBudget.records(max_records)` | `GraphPageBudget` | Request at most `max_records` structural records. Values less than one produce a resource error when read. |
| `GraphPageBudget.unbounded()` | `GraphPageBudget` | Request every remaining structural record. |
| `budget.is_full(count)` | `bool` | Return whether `count` has reached the record bound. |
| `budget.is_empty_request()` | `bool` | Return whether the budget cannot admit one record. |

| Type | Public fields |
| --- | --- |
| `NodePage[K]` | `values: list[K]`, `next: Option[GraphPageToken]`, `counters: PageCounters` |
| `EdgePage[K, I]` | `values: list[GraphEdgeItem[K, I]]`, `next: Option[GraphPageToken]`, `counters: PageCounters` |
| `GraphEdgeItem[K, I]` | `id: I`, `source: K`, `target: K` |
| `PageCounters` | `index_seeks: int`, `node_slots_examined: int`, `adjacency_entries_examined: int`, `live_entries: int`, `tombstones_examined: int`, `payloads_materialized: int`, `bytes_copied: int` |

`NodePage.values` holds node keys. `EdgePage.values` holds directed structural edges without payloads.

`GraphPageToken` is opaque and owned. It records the graph scope, revision, selection, ordering profile, stream, subject, and physical position. Reusing a token with another graph reports `ScopeMismatch`; using it after mutation reports `StaleCursor`; changing the selection, ordering, direction, or adjacency subject reports `IncompatibleCursor`.

| Member | Returns | Meaning |
| --- | --- | --- |
| `token.scope_id()` | `int` | Issuing graph scope identity. |
| `token.revision()` | `int` | Issuing graph revision. |
| `token.selection_id()` | `int` | Issuing selection identity. |
| `token.ordering_id()` | `int` | Issuing ordering profile identity. |
| `token.stream_id()` | `int` | Issuing page-stream identity. |
| `token.subject_id()` | `int` | Issuing adjacency subject, or the node-stream sentinel. |
| `token.position()` | `int` | Next physical position in the page stream. |

`PageCounters` exposes `index_seeks`, `node_slots_examined`, `adjacency_entries_examined`, `live_entries`, `tombstones_examined`, `payloads_materialized`, and `bytes_copied`. Structural pages do not materialize payloads, so `payloads_materialized` is zero. `bytes_copied` counts 16 bytes per scoped node key and 48 bytes per structural edge item: each scoped identity contains two 64-bit integers, and an edge item contains its edge, source, and target identities.

### Read errors

`GraphReadError[F]` exposes `kind: GraphReadErrorKind`, `unknown_id: Option[int]`, `resource_detail: Option[str]`, and `source: Option[F]`.

| Constructor | Meaning |
| --- | --- |
| `GraphReadError.UnknownNode(node)` | The scoped local node identity is not live. |
| `GraphReadError.UnknownEdge(edge)` | The scoped local edge identity is not live. |
| `GraphReadError.ScopeMismatch()` | A key or cursor belongs to another graph scope. |
| `GraphReadError.StaleCursor()` | A cursor predates the current graph revision. |
| `GraphReadError.IncompatibleCursor()` | A cursor belongs to another selection, ordering, stream, or adjacency subject. |
| `GraphReadError.UnsupportedSelection()` | The reader cannot serve the requested selection. |
| `GraphReadError.Resource(detail)` | A resource request is invalid or exhausted; `resource_detail` contains `detail`. |
| `GraphReadError.Cancelled()` | The caller cancelled the read. |
| `GraphReadError.Source(failure)` | The backend produced the typed source failure `failure`. |

`GraphReadErrorKind` has the corresponding `UnknownNode`, `UnknownEdge`, `ScopeMismatch`, `StaleCursor`, `IncompatibleCursor`, `UnsupportedSelection`, `Resource`, `Cancelled`, and `Source` variants. Only `unknown_id`, `resource_detail`, or `source` relevant to the selected kind is present.

## `DiGraph[T]`

Construct a directed graph with `DiGraph[T]()`.

| Member | Returns | Meaning |
| --- | --- | --- |
| `g.add_node(payload)` | `NodeId` | Add one active node carrying `payload`. |
| `g.remove_node(node)` | `Result[None, GraphError]` | Remove one active node and all incident edges. |
| `g.contains_node(node)` | `bool` | Return whether `node` names a live node in `g`. |
| `g.node_payload(node)` | `Result[T, GraphError]` | Return the payload stored for `node`. |
| `g.add_edge(from_, to)` | `Result[None, GraphError]` | Add the directed edge `from_ -> to`. |
| `g.remove_edge(from_, to)` | `Result[None, GraphError]` | Remove the directed edge `from_ -> to`. |
| `g.contains_edge(from_, to)` | `bool` | Return whether the directed edge `from_ -> to` exists. |
| `g.successors(node)` | `Result[list[NodeId], GraphError]` | Return outgoing neighbors for `node`. |
| `g.predecessors(node)` | `Result[list[NodeId], GraphError]` | Return incoming neighbors for `node`. |
| `g.active_node_ids()` | `list[NodeId]` | Return live node ids in allocation order. |
| `g.active_edge_ids()` | `list[EdgeId]` | Return live edge ids in allocation order. |
| `g.adjacency_physical_count(node)` | `Result[int, GraphError]` | Return the number of physical outgoing adjacency entries for `node`. |
| `g.adjacency_live_count(node)` | `Result[int, GraphError]` | Return the number of live outgoing adjacency entries for `node`. |
| `g.adjacency_tombstone_count(node)` | `Result[int, GraphError]` | Return the number of deleted outgoing adjacency entries retained for `node`. |
| `g.roots()` | `list[NodeId]` | Return active nodes with no incoming edges. |
| `g.sinks()` | `list[NodeId]` | Return active nodes with no outgoing edges. |
| `g.bfs_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in breadth-first order. |
| `g.dfs_preorder_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in depth-first preorder. |
| `g.topological_order()` | `Result[list[NodeId], GraphError]` | Return a topological order, or `Err(GraphError)` when the graph contains a cycle. |

`add_edge` rejects unknown endpoints and duplicate active edges. `remove_node` marks the node inactive and removes every incoming and outgoing edge attached to that node. Node and edge ids are never reused.

`successors` and `predecessors` return distinct neighbors in ascending `NodeId` order. Breadth-first and depth-first traversal use that neighbor order. `topological_order` uses repeated passes over active nodes in allocation order, so a node that becomes ready during a pass is emitted when that same pass reaches it. These legacy orders remain distinct from the edge-insertion order used by the indexed reader API.

The three adjacency count methods expose deterministic storage metrics. Physical entries equal live plus retained tombstones. An adjacency list compacts after tombstones outnumber live entries, after which the retained tombstone count is zero.

## `Dag[T]`

Construct a directed acyclic graph with `Dag[T]()`.

| Member | Returns | Meaning |
| --- | --- | --- |
| `g.add_node(payload)` | `NodeId` | Add one active node carrying `payload`. |
| `g.remove_node(node)` | `Result[None, GraphError]` | Remove one active node and all incident edges. |
| `g.contains_node(node)` | `bool` | Return whether `node` names a live node in `g`. |
| `g.node_payload(node)` | `Result[T, GraphError]` | Return the payload stored for `node`. |
| `g.add_edge(from_, to)` | `Result[None, GraphError]` | Add `from_ -> to`, rejecting duplicates and cycle-creating edges. |
| `g.remove_edge(from_, to)` | `Result[None, GraphError]` | Remove the directed edge `from_ -> to`. |
| `g.contains_edge(from_, to)` | `bool` | Return whether the directed edge `from_ -> to` exists. |
| `g.successors(node)` | `Result[list[NodeId], GraphError]` | Return outgoing neighbors for `node`. |
| `g.predecessors(node)` | `Result[list[NodeId], GraphError]` | Return incoming neighbors for `node`. |
| `g.active_node_ids()` | `list[NodeId]` | Return live node ids in allocation order. |
| `g.active_edge_ids()` | `list[EdgeId]` | Return live edge ids in allocation order. |
| `g.roots()` | `list[NodeId]` | Return active nodes with no incoming edges. |
| `g.sinks()` | `list[NodeId]` | Return active nodes with no outgoing edges. |
| `g.bfs_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in breadth-first order. |
| `g.dfs_preorder_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in depth-first preorder. |
| `g.topological_order()` | `list[NodeId]` | Return the DAG's topological order. |
| `g.can_reach(start, target)` | `bool` | Return whether `target` is reachable from `start`. |

`Dag[T]` rejects cycle-creating edge insertion. Its `topological_order()` returns a list directly. Neighbor and topological orders match the corresponding legacy `DiGraph[T]` orders.

## `MultiDiGraph[T]`

Construct a directed multigraph with `MultiDiGraph[T]()`.

| Member | Returns | Meaning |
| --- | --- | --- |
| `g.add_node(payload)` | `NodeId` | Add one active node carrying `payload`. |
| `g.remove_node(node)` | `Result[None, GraphError]` | Remove one active node and all incident edges. |
| `g.contains_node(node)` | `bool` | Return whether `node` names a live node in `g`. |
| `g.node_payload(node)` | `Result[T, GraphError]` | Return the payload stored for `node`. |
| `g.add_edge(from_, to)` | `Result[EdgeId, GraphError]` | Add a directed edge and return its edge id. |
| `g.remove_edge(edge)` | `Result[None, GraphError]` | Remove the active edge identified by `edge`. |
| `g.contains_edge(edge)` | `bool` | Return whether `edge` names a live edge in `g`. |
| `g.edges_between(from_, to)` | `Result[list[EdgeId], GraphError]` | Return all active edge ids connecting `from_ -> to`. |
| `g.edge_endpoints(edge)` | `Result[Tuple[NodeId, NodeId], GraphError]` | Return the source and target node ids for `edge`. |
| `g.successors(node)` | `Result[list[NodeId], GraphError]` | Return outgoing neighbors for `node`. |
| `g.predecessors(node)` | `Result[list[NodeId], GraphError]` | Return incoming neighbors for `node`. |
| `g.active_node_ids()` | `list[NodeId]` | Return live node ids in allocation order. |
| `g.active_edge_ids()` | `list[EdgeId]` | Return live edge ids in allocation order. |
| `g.roots()` | `list[NodeId]` | Return active nodes with no incoming edges. |
| `g.sinks()` | `list[NodeId]` | Return active nodes with no outgoing edges. |
| `g.bfs_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in breadth-first order. |
| `g.dfs_preorder_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in depth-first preorder. |
| `g.topological_order()` | `Result[list[NodeId], GraphError]` | Return a topological order, or `Err(GraphError)` when the graph contains a cycle. |

Parallel edges are distinct when their `EdgeId` values differ, even when they connect the same source and target node ids. `edges_between` returns live edge ids in edge-insertion order. `successors` and `predecessors` deduplicate parallel relationships and return distinct neighbors in ascending `NodeId` order.

## Sealed storage migration

Graph containers own indexed storage whose physical representation is private. Code that previously inspected that representation must use the supported operations below.

| Private surface | Replacement |
| --- | --- |
| `DiGraph.next_node_id`, `DiGraph.nodes` | `active_node_ids()` |
| `DiGraph.next_edge_id`, `DiGraph.edges` | `active_edge_ids()` |
| `Dag.graph` | `Dag` methods, including `contains_edge()` and `active_node_ids()` |
| `MultiDiGraph.graph` | `MultiDiGraph` methods, including `active_edge_ids()` |
| `GraphNode`, `GraphEdge` | `NodeId`, `EdgeId`, and graph lookup methods |
| `DiGraph.insert_edge()` | `add_edge()` or `MultiDiGraph.add_edge()` |
| `DiGraph.node_index()` | `contains_node()` and `node_payload()` |
| `DiGraph.edge_index()` | `active_edge_ids()` |

The migration diagnostics name these replacements. They are refusals, not deprecation warnings: direct access could bypass acyclicity, parallel-edge, lifetime, or compaction invariants.

## Error cases

| Operation | Error condition |
| --- | --- |
| Node lookup, removal, traversal, or adjacency query | The supplied `NodeId` does not name a live node in that graph. |
| `DiGraph[T].add_edge` | Either endpoint is unknown, or the active edge already exists. |
| `Dag[T].add_edge` | Either endpoint is unknown, the active edge already exists, or the edge would create a cycle. |
| `DiGraph[T].remove_edge` | Either endpoint is unknown, or the active edge does not exist. |
| `MultiDiGraph[T].remove_edge` or `edge_endpoints` | The supplied `EdgeId` does not name a live edge in that multigraph. |
| `DiGraph[T].topological_order` or `MultiDiGraph[T].topological_order` | The graph contains a cycle. |

## Boundaries

`std.graph` is an in-memory structural graph module. Persistence, graph databases, network synchronization, weighted edges, property-graph queries, and alternate storage backends belong in higher-level libraries or future standard-library extensions.
