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
| `g.roots()` | `list[NodeId]` | Return active nodes with no incoming edges. |
| `g.sinks()` | `list[NodeId]` | Return active nodes with no outgoing edges. |
| `g.bfs_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in breadth-first order. |
| `g.dfs_preorder_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in depth-first preorder. |
| `g.topological_order()` | `Result[list[NodeId], GraphError]` | Return a topological order, or `Err(GraphError)` when the graph contains a cycle. |

`add_edge` rejects unknown endpoints and duplicate active edges. `remove_node` marks the node inactive and removes every incoming and outgoing edge attached to that node.

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
| `g.successors(node)` | `Result[list[NodeId], GraphError]` | Return outgoing neighbors for `node`. |
| `g.predecessors(node)` | `Result[list[NodeId], GraphError]` | Return incoming neighbors for `node`. |
| `g.roots()` | `list[NodeId]` | Return active nodes with no incoming edges. |
| `g.sinks()` | `list[NodeId]` | Return active nodes with no outgoing edges. |
| `g.bfs_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in breadth-first order. |
| `g.dfs_preorder_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in depth-first preorder. |
| `g.topological_order()` | `list[NodeId]` | Return the DAG's topological order. |
| `g.can_reach(start, target)` | `bool` | Return whether `target` is reachable from `start`. |

`Dag[T]` keeps acyclicity as an insertion-time invariant. Its `topological_order()` returns a list directly because successful edge insertion maintains the ordering precondition.

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
| `g.roots()` | `list[NodeId]` | Return active nodes with no incoming edges. |
| `g.sinks()` | `list[NodeId]` | Return active nodes with no outgoing edges. |
| `g.bfs_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in breadth-first order. |
| `g.dfs_preorder_nodes(start)` | `Result[list[NodeId], GraphError]` | Return reachable nodes in depth-first preorder. |
| `g.topological_order()` | `Result[list[NodeId], GraphError]` | Return a topological order, or `Err(GraphError)` when the graph contains a cycle. |

Parallel edges are distinct when their `EdgeId` values differ, even when they connect the same source and target node ids.

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
