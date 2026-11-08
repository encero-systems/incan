# std.graph reference

This page is the API reference for `std.graph`. For task-oriented examples, see [Working with graphs](../../how-to/working_with_graphs.md). For the design model and boundaries, see [Why `std.graph` exists](../../explanation/graph_model.md).

## Importing the graph API

Import graph containers, identifier handles, and graph errors from `std.graph`:

```incan
from std.graph import Dag, DiGraph, EdgeId, GraphError, MultiDiGraph, NodeId
```

## Types

| Type | Meaning |
| --- | --- |
| `DiGraph[T]` | Mutable directed graph with at most one active edge for each ordered node pair. |
| `Dag[T]` | Directed acyclic graph wrapper that rejects any inserted edge that would create a cycle. |
| `MultiDiGraph[T]` | Directed graph that allows parallel edges and returns stable `EdgeId` handles for each edge. |
| `NodeId` | Opaque node handle scoped to the graph instance that created it. |
| `EdgeId` | Opaque edge handle scoped to the multigraph instance that created it. |
| `GraphError` | Error value returned when graph operations encounter unknown ids, duplicate edges, cycles, or missing edges. |

## `NodeId` and `EdgeId`

| Member | Returns | Meaning |
| --- | --- | --- |
| `id.to_int()` | `int` | Runtime numeric representation for diagnostics, storage, or display. |

`NodeId` and `EdgeId` values are graph-local handles. Do not reuse ids across graph instances unless the surrounding data model explicitly tracks the graph they came from.

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

`Dag[T]` keeps acyclicity as an insertion-time invariant. Its `topological_order()` returns a list directly because successful edge insertion maintains the ordering precondition. Neighbor and topological orders match the corresponding legacy `DiGraph[T]` orders.

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
