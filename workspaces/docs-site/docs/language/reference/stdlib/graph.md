# std.graph reference

This page documents the graph API exposed by `std.graph`. Use this module when a program needs explicit in-memory dependency, plan, workflow, or relationship graphs instead of ad hoc `dict` and `list` structures.

`std.graph` values are ordinary data. Create them, pass them as arguments, store them on models, and keep separate graph instances for separate pipelines or requests. The standard library does not define one ambient process-wide graph.

## Importing the graph API

Import the graph types and identifier types with:

```incan
from std.graph import Dag, DiGraph, EdgeId, MultiDiGraph, NodeId
```

## Types

### `DiGraph[T]`

`DiGraph[T]` is a mutable directed graph whose nodes carry payloads of type `T`. It is the default graph type for ordinary directed relationships where one edge from `a` to `b` is enough.

Edges connect `NodeId` values, not raw payload objects. Use `DiGraph[T]` when the graph structure itself matters: scheduling dependencies, plan stages, validation pipelines, state machines, or other data where traversal and adjacency are part of the program contract.

### `Dag[T]`

`Dag[T]` is a directed acyclic graph. It has the same node payload model as `DiGraph[T]`, but edge insertion rejects any edge that would create a cycle.

Use `Dag[T]` when acyclicity is a data invariant rather than just a query-time check. Dependency plans, build steps, and staged pipelines usually want `Dag[T]` because accepting a cycle would make the value invalid.

### `MultiDiGraph[T]`

`MultiDiGraph[T]` is a directed graph that allows multiple distinct edges between the same source and destination nodes. Each edge has an `EdgeId`, so callers can add, inspect, and remove one relationship without collapsing every edge between the same node pair.

Use `MultiDiGraph[T]` when parallel relationships are meaningful, such as a dataflow edge and a control edge between the same two nodes, or multiple named fallback paths.

### `NodeId`

`NodeId` identifies a node inside one graph instance. Node ids are stable for the lifetime of the node in that graph, but they are not portable across different graph instances. Do not compare or reuse a `NodeId` from one graph with another graph.

### `EdgeId`

`EdgeId` identifies one edge inside a `MultiDiGraph[T]`. Edge ids are stable for the lifetime of the edge in that multigraph, but they are not portable across different graph instances. Use `EdgeId` when parallel edges need separate identity for lookup, metadata, or removal.

## Constructing graphs

Construct graph values directly:

```incan
from std.graph import Dag, DiGraph, MultiDiGraph

mut graph = DiGraph[str]()
mut dag = Dag[str]()
mut multigraph = MultiDiGraph[str]()
```

Add nodes with typed payloads and connect them with directed edges:

```incan
from std.graph import DiGraph, NodeId

mut g = DiGraph[str]()

scan: NodeId = g.add_node(payload="scan_users")
filter: NodeId = g.add_node(payload="filter_active")
join: NodeId = g.add_node(payload="join_orders")

g.add_edge(from_=scan, to=filter)
g.add_edge(from_=filter, to=join)
```

`add_node(payload=...)` returns the new node id. `add_edge(from_=..., to=...)` adds a directed edge from the first node to the second node. Both endpoints must belong to the graph.

## Payloads

Payloads are not limited to primitive values. A graph can carry model, class, enum, newtype, or other ordinary Incan values as its node payload type:

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

The node id is the graph identity. If two nodes should represent the same logical object, choose that explicitly in your domain model: either reuse one node id or add separate nodes with separate payload values.

## Querying nodes and edges

Use the graph query methods to move between ids, payloads, and adjacency:

| Method                     | Returns                          | Meaning                                              |
| -------------------------- | -------------------------------- | ---------------------------------------------------- |
| `g.contains_node(id)`       | `bool`                           | Whether `id` names a live node in `g`                |
| `g.node_payload(id)`        | `Result[T, GraphError]`          | Payload stored for `id`                              |
| `g.successors(id)`          | `Result[list[NodeId], GraphError]` | Nodes reached by outgoing edges from `id`          |
| `g.predecessors(id)`        | `Result[list[NodeId], GraphError]` | Nodes with outgoing edges into `id`                |
| `g.roots()`                 | `list[NodeId]`                   | Nodes with no incoming edges                         |
| `g.sinks()`                 | `list[NodeId]`                   | Nodes with no outgoing edges                         |
| `g.contains_edge(src, dst)` | `bool`                           | Whether the directed edge `src -> dst` exists in `g` |

Roots and sinks are structural properties of the current graph, not payload properties. In a disconnected graph, each independent component can have its own roots and sinks.

## Removal

Removing a node also removes every incoming and outgoing edge attached to that node. After removal, the old `NodeId` no longer names a live node in that graph. Implementations must not leave dangling edges behind.

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

`DiGraph[T]` and `Dag[T]` remove a simple edge by endpoint pair:

```incan
removed_edge: Result[None, GraphError] = g.remove_edge(first, second)
```

`MultiDiGraph[T]` removes one edge by `EdgeId`, so parallel edges can be managed independently:

```incan
from std.graph import EdgeId, GraphError, MultiDiGraph

mut g = MultiDiGraph[str]()
source = g.add_node(payload="read")
target = g.add_node(payload="write")

primary_result: Result[EdgeId, GraphError] = g.add_edge(source, target)
backup_result: Result[EdgeId, GraphError] = g.add_edge(source, target)

match primary_result:
    case Ok(primary):
        g.remove_edge(primary)
    case Err(_):
        pass

match backup_result:
    case Ok(backup):
        assert g.contains_edge(backup)
    case Err(_):
        pass
```

## Traversal

Traversal starts from one node id and returns node ids in a documented order. The traversal only visits nodes reachable from the start node.

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

`bfs_nodes(start)` visits in breadth-first order: the start node first, then reachable successors by increasing edge distance from the start. `dfs_preorder_nodes(start)` visits in depth-first preorder: a node appears before the nodes reached by continuing down that path.

Traversal methods visit each reachable node at most once, even when cycles exist. `Dag[T]` traversal follows the same order contracts, with the additional guarantee that the graph itself is acyclic.

## Topological order

Use topological order when edges represent dependency direction and every edge should point from an earlier requirement to a later dependent:

```incan
from std.graph import DiGraph, NodeId

def ordered_or_empty(g: DiGraph[str]) -> list[NodeId]:
    match g.topological_order():
        case Ok(order):
            return order
        case Err(_):
            return []
```

`topological_order()` returns `Ok(list[NodeId])` when every edge can be ordered from an earlier node to a later node. If a `DiGraph[T]` or `MultiDiGraph[T]` contains a cycle, it returns an error instead of silently dropping edges or returning a partial order.

For `Dag[T]`, edge insertion enforces acyclicity, so topological order is the normal scheduling view:

```incan
from std.graph import Dag, NodeId

mut plan = Dag[str]()
fetch = plan.add_node(payload="fetch")
parse = plan.add_node(payload="parse")
plan.add_edge(from_=fetch, to=parse)

order: list[NodeId] = plan.topological_order()
```

## Multigraph edge ids

Use `MultiDiGraph[T]` when parallel edges have separate identity:

```incan
from std.graph import EdgeId, GraphError, MultiDiGraph, NodeId

mut g = MultiDiGraph[str]()
read: NodeId = g.add_node(payload="read")
write: NodeId = g.add_node(payload="write")

data: Result[EdgeId, GraphError] = g.add_edge(read, write)
control: Result[EdgeId, GraphError] = g.add_edge(read, write)

edges: Result[list[EdgeId], GraphError] = g.edges_between(read, write)
```

`edges_between(from_=..., to=...)` returns every edge id from the source node to the destination node. `edge_endpoints(edge)` returns the source and destination node ids for one edge. If relationships need labels or attributes, store that data in your own payload model and keep the graph as the structural index.

## Storing graphs as data

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

## Boundaries

`std.graph` is intentionally small:

- It is in-memory; persistence and network synchronization belong in higher-level libraries.
- It covers directed graphs, directed acyclic graphs, and directed multigraphs.
- It is not a graph database or query language.
- Future graph expansion, such as weighted edges, property graph queries, or alternate storage backends, belongs in a documented standard-library extension.
