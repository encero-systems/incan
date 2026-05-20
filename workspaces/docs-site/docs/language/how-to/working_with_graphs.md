# Working with graphs

Use `std.graph` when relationships are part of the program model. A graph is a better fit than ad hoc `dict` and `list` structures when callers need stable node ids, edge queries, traversal, topological ordering, or DAG cycle rejection.

## Choose a graph type

| Need                                                                           | Use               |
| ------------------------------------------------------------------------------ | ----------------- |
| Ordinary directed relationships with at most one edge from one node to another | `DiGraph[T]`      |
| Dependency plans, build steps, or staged pipelines that must stay acyclic      | `Dag[T]`          |
| Parallel relationships between the same source and target nodes                | `MultiDiGraph[T]` |

Use a builtin `dict` or `list` when the relationships are temporary implementation details. Use `std.graph` when the relationship structure should be visible to callers or tests.

## Build a directed graph

Add payload-carrying nodes first, then connect node ids with directed edges:

```incan
from std.graph import DiGraph, NodeId

mut g = DiGraph[str]()

scan: NodeId = g.add_node(payload="scan_users")
filter: NodeId = g.add_node(payload="filter_active")
join: NodeId = g.add_node(payload="join_orders")

g.add_edge(from_=scan, to=filter)
g.add_edge(from_=filter, to=join)
```

`add_node(payload=...)` returns the node id. `add_edge(from_=..., to=...)` connects existing node ids, and both endpoints must belong to the same graph.

## Carry structured payloads

Payloads can be model, class, enum, newtype, or other ordinary Incan values:

```incan
from std.graph import DiGraph, NodeId

pub model Step:
    pub name: str
    pub timeout_ms: int

def build_step_graph() -> DiGraph[Step]:
    mut g = DiGraph[Step]()

    fetch = Step(name="fetch", timeout_ms=5000)
    parse = Step(name="parse", timeout_ms=2000)

    n_fetch: NodeId = g.add_node(payload=fetch)
    n_parse: NodeId = g.add_node(payload=parse)

    g.add_edge(from_=n_fetch, to=n_parse)
    return g

def step_name_at(g: DiGraph[Step], nid: NodeId) -> str:
    match g.node_payload(nid):
        case Ok(step):
            return step.name
        case Err(_):
            return ""
```

The node id is the graph identity. If two nodes should represent the same logical object, decide that in the domain model by reusing one node id or adding separate nodes with separate payload values.

## Remove nodes and edges

Removing a node also removes every incoming and outgoing edge attached to that node:

```incan
from std.graph import DiGraph, GraphError, NodeId

mut g = DiGraph[str]()
first: NodeId = g.add_node(payload="first")
second: NodeId = g.add_node(payload="second")
g.add_edge(from_=first, to=second)

removed: Result[None, GraphError] = g.remove_node(first)
assert not g.contains_node(first)
assert not g.contains_edge(first, second)
```

`DiGraph[T]` and `Dag[T]` remove simple edges by endpoint pair:

```incan
removed_edge: Result[None, GraphError] = g.remove_edge(first, second)
```

## Traverse reachable nodes

Use breadth-first traversal when edge distance from the start node matters. Use depth-first preorder when each node should appear before the nodes reached by continuing down that path.

```incan
from std.graph import DiGraph, NodeId

def collect_bfs(g: DiGraph[str], start: NodeId) -> list[NodeId]:
    match g.bfs_nodes(start):
        case Ok(nodes):
            return nodes
        case Err(_):
            return []

def collect_dfs(g: DiGraph[str], start: NodeId) -> list[NodeId]:
    match g.dfs_preorder_nodes(start):
        case Ok(nodes):
            return nodes
        case Err(_):
            return []
```

Traversal methods visit each reachable node at most once, even when cycles exist. `Dag[T]` traversal follows the same order contracts with the extra guarantee that the graph value itself is acyclic.

## Schedule dependency work with `Dag`

Use topological order when every edge points from an earlier requirement to a later dependent:

```incan
from std.graph import Dag, NodeId

mut plan = Dag[str]()
fetch: NodeId = plan.add_node(payload="fetch")
parse: NodeId = plan.add_node(payload="parse")
plan.add_edge(from_=fetch, to=parse)

order: list[NodeId] = plan.topological_order()
```

`Dag[T]` rejects cycle-creating edges when they are inserted, so topological order is the normal scheduling view. `DiGraph[T]` and `MultiDiGraph[T]` can contain cycles, so their `topological_order()` methods return `Result[list[NodeId], GraphError]`.

## Keep parallel edges separate

Use `MultiDiGraph[T]` when multiple relationships can connect the same node pair and each one needs separate identity:

```incan
from std.graph import EdgeId, GraphError, MultiDiGraph, NodeId

mut g = MultiDiGraph[str]()
read: NodeId = g.add_node(payload="read")
write: NodeId = g.add_node(payload="write")

data: Result[EdgeId, GraphError] = g.add_edge(read, write)
control: Result[EdgeId, GraphError] = g.add_edge(read, write)

edges: Result[list[EdgeId], GraphError] = g.edges_between(read, write)
```

`edges_between(from_=..., to=...)` returns every active edge id from the source node to the destination node. `edge_endpoints(edge)` returns the source and destination node ids for one edge. If relationships need labels or attributes, store that data in your own payload model and keep the graph as the structural index.

## Store graphs as data

Graphs compose with ordinary Incan models:

```incan
from std.graph import Dag, NodeId

pub model PipelinePlan:
    pub deps: Dag[str]
    pub tip: NodeId

def root_names(plan: PipelinePlan) -> list[str]:
    mut names: list[str] = []
    for nid in plan.deps.roots():
        match plan.deps.node_payload(nid):
            case Ok(name):
                names.append(name)
            case Err(_):
                pass
    return names
```

This keeps graph ownership explicit. It also keeps tests isolated because each test can construct only the graph it needs.

## See also

- [`std.graph` reference](../reference/stdlib/graph.md)
- [Why `std.graph` exists](../explanation/graph_model.md)
- [RFC 047: Lightweight directed graph types](../../RFCs/closed/implemented/047_lightweight_directed_graph_stdlib.md)
