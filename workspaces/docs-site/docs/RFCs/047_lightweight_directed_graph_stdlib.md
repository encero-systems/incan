# RFC 047: Lightweight directed graph types (stdlib)

- **Status:** Draft
- **Created:** 2026-03-31
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 005 (Rust interop)
    - RFC 023 (stdlib namespacing and compiler handoff)
    - RFC 030 (std collections)
    - RFC 033 (`ctx` — contrast only; graphs are not ambient singletons)
- **Issue:** https://github.com/dannys-code-corner/incan/issues/204
- **RFC PR:** —
- **Written against:** v0.2
- **Shipped in:** —

## Summary

This RFC proposes a small, opinionated directed-graph abstraction in the Incan standard library: stable node identifiers, node payloads, directed edges with optional kind metadata, and a minimal interrogation API covering adjacency, roots and sinks, traversal, and optional topological order. It is not a graph database, query language, or persistence layer. New graph-related stdlib surfaces should continue to go through RFCs rather than growing ad hoc.

## Motivation

Several Incan-backed domains benefit from a shared in-memory graph model: relational plan DAGs, task or dependency graphs, configuration pipelines, and other tools that need nodes, edges, and programmatic traversal rather than ad hoc maps and lists per project.

Without a stdlib shape, each library reinvents id allocation, edge storage, and walk order. That fragments tooling, tests, and interop expectations. A lightweight, RFC-governed type keeps the surface small while still allowing domain-specific payloads and future extension, such as weights, attributes, or alternate backends, in follow-on RFCs.

## Goals

- Define normative semantics for a directed-graph type, and optionally for a
DAG variant or mode, exposed through the stdlib.
- Specify stable `NodeId` semantics: what it means to add or remove nodes and
edges, and when ids must not be reused in ways that confuse callers.
- Specify a minimal interrogation API sufficient for common compiler and
orchestration patterns: successors, predecessors, roots, sinks, reachability traversal from a start node in at least one documented order, and topological order when acyclic.
- Keep future stdlib graph expansion on the RFC track rather than letting it
grow ad hoc.

## Non-Goals

- A **graph query language**, **index structures** for scale-out property search, or **disk-backed** storage.
- **Undirected** graphs as a first-class mandatory type (may be added later via RFC).
- **Distributed** graphs, **transactions**, or **Cypher**-style interfaces.
- Mandating one specific backend implementation; the contract is behavioral,
not implementation-specific.
- A process-wide or language-global singleton graph. Named registries or
application-level default graphs may exist in higher-level systems, but this stdlib contract must not require them.

## Guide-level explanation

Authors obtain a graph handle from the stdlib, add nodes with optional payloads, and add directed edges from one node to another. They can ask for outgoing or incoming neighbors, enumerate roots or sinks, and walk from a start node in a documented order. Common walk orders are breadth-first search and depth-first search; the stdlib must document which order each traversal API uses. When the graph is intended to be acyclic, authors may use a DAG mode or type that rejects edge insertions that would create a cycle, or they may call topological-order APIs and handle the error if a cycle exists.

The type is in-memory and library-scoped. Persistence, network sync, and engine-specific optimization remain outside this RFC.

Authors construct and pass graph values explicitly rather than reaching for one implicit global graph.

## Illustrative API sketches (non-normative)

The following examples show one possible Incan-facing shape. Names, return types, and error modeling are illustrative until the RFC moves beyond Draft; they are not commitments to exact signatures.

**Module path** is assumed to be `std.graph` (see RFC 022 stdlib namespacing); final names **may** differ.

### Constructing a directed graph and querying neighbors

```incan
from std.graph import DiGraph, NodeId

# Empty graph; nodes and edges added explicitly.
mut g = DiGraph[str].new()

a: NodeId = g.add_node(payload="scan_users")
b: NodeId = g.add_node(payload="filter_active")
c: NodeId = g.add_node(payload="join_orders")

g.add_edge(from_=a, to=b)
g.add_edge(from_=b, to=c)

out_from_b: list[NodeId] = g.successors(b)
roots: list[NodeId] = g.roots()
```

### Node payloads as **object references** (e.g. `Step`)

Graphs are not limited to string or primitive payloads. A common pattern is
`DiGraph[Step]`, or another user type, where each `NodeId` maps to a `Step`
value the author already holds. Edges still connect `NodeId`s, so one `Step` instance can be the payload of node `a` and another of node `b`, with a directed edge `a -> b` meaning "this step runs before that step," or any other domain-specific relationship.

```incan
from std.graph import DiGraph, NodeId

pub model Step:
    pub name: str
    pub timeout_ms: int

def build_step_graph() -> DiGraph[Step]:
    mut g = DiGraph[Step].new()
    fetch = Step(name="fetch", timeout_ms=5000)
    parse = Step(name="parse", timeout_ms=2000)

    n_fetch: NodeId = g.add_node(payload=fetch)
    n_parse: NodeId = g.add_node(payload=parse)

    g.add_edge(from_=n_fetch, to=n_parse)
    return g

def step_name_at(g: DiGraph[Step], nid: NodeId) -> str:
    return g.node_payload(nid).name
```

**Semantics:** `add_node(payload=...)` ties a `NodeId` to that payload object.
If two nodes should represent the same `Step` instance, that is a domain choice: either one node is shared or two nodes hold cloned payloads. The stdlib must document how clone versus shared-reference behavior works for mutable payloads.

**Bounds:** generic graphs such as `DiGraph[T]` will likely require `T` to
satisfy whatever the language requires for stored or shared data. Exact bounds are implementation detail, but they must appear in user-facing docs.

### Traversal (BFS / DFS) from a start node

```incan
from std.graph import DiGraph, NodeId

def collect_bfs(g: DiGraph[str], start: NodeId) -> list[NodeId]:
    return g.bfs_nodes(start).to_list()

def collect_dfs_preorder(g: DiGraph[str], start: NodeId) -> list[NodeId]:
    return g.dfs_preorder_nodes(start).to_list()
```

Implementations **must** document iteration order; the method names above are **placeholders**.

### Topological order on a DAG-shaped graph

```incan
from std.graph import DiGraph, NodeId

def topo_or_empty(g: DiGraph[str]) -> list[NodeId]:
    match g.topological_order():
        case Ok(order):
            return order
        case Err(_):
            return []  # cycle present — real API might use Result throughout
```

### DAG mode: reject edges that would create a cycle

```incan
from std.graph import Dag, NodeId

mut d = Dag[int].new()
x = d.add_node(payload=1)
y = d.add_node(payload=2)
z = d.add_node(payload=3)

d.add_edge(from_=x, to=y)
d.add_edge(from_=y, to=z)

match d.try_add_edge(from_=z, to=x):
    case Ok(_):
        pass  # unexpected if cycle detection works
    case Err(CycleWouldBeCreated):
        pass  # expected
```

Alternatively, a single `DiGraph` type could use
`DiGraph.new(enforce_dag=True)` instead of a separate `Dag` name. See
Unresolved questions.

### Parallel edges (multigraph profile), if supported

```incan
from std.graph import MultiDiGraph, NodeId, EdgeId

mut m = MultiDiGraph[str].new()
u = m.add_node("a")
v = m.add_node("b")

e1: EdgeId = m.add_edge(from_=u, to=v, kind="primary")
e2: EdgeId = m.add_edge(from_=u, to=v, kind="backup")

edges_uv: list[EdgeId] = m.edges_between(u, v)
```

If v1 is **simple-only**, this profile is omitted or deferred; authors use **`DiGraph`** without **`MultiDiGraph`**.

### Edge **kind** on a simple directed graph

```incan
from std.graph import DiGraph, NodeId

mut g = DiGraph[str].new()
read = g.add_node("read")
write = g.add_node("write")

g.add_edge(from_=read, to=write, kind="dataflow")
```

### Storing a graph as **data** (not a singleton)

```incan
pub model PipelinePlan:
    pub deps: DiGraph[str]
    pub tip: NodeId

def describe_roots(plan: PipelinePlan) -> str:
    return ", ".join(plan.deps.node_payload(nid) for nid in plan.deps.roots())
```

## Reference-level explanation

### Directed graph

A **directed graph** value **must** conceptually consist of:

- A set of nodes, each with a distinct `NodeId` while the node exists. Each
node may carry an optional payload: any Incan value the API allows, including primitives, `model` or `class` instances, newtypes, and similar values. Edges always connect `NodeId`s, not raw object references.
- A multiset of directed edges from one `NodeId` to another `NodeId`. Parallel
edges may be permitted unless a future profile forbids them. If they are permitted, edges must be distinguishable by an `EdgeId` or by iteration order, as specified by the stdlib API.

Removing a node must either remove all incident edges or define explicit behavior, such as an error, in the stdlib contract. Implementations must not leave the graph in an inconsistent state silently.

### NodeId stability

The stdlib must document whether `NodeId` values are reused after node removal. The recommended default is not to reuse ids in the same graph instance without an explicit compact or reset operation documented for advanced users.

### DAG profile

If a DAG type or acyclicity-enforcing mode is provided, adding an edge that would introduce a cycle must fail with a defined error or return type. The graph must not accept the edge.

### Topological order

When `topological_order`, or an equivalent API, is provided, it must return a linear order consistent with all edges, meaning every edge goes from an earlier to a later index. If the graph contains a cycle, the operation must report failure in a defined way.

### Interrogation API (minimum bar)

The stdlib **must** expose operations equivalent to the following capabilities (names may differ):

- **Lookup** node existence by `NodeId`.
- **Successors** and **predecessors** (or outgoing/incoming edge iteration).
- **Roots** and **sinks** according to the definitions above.
- **Reachability traversal** from a start `NodeId` in at least one documented
order, for example breadth-first search or depth-first search.

Implementations **may** add further helpers (e.g. strongly connected components) in later RFCs.

## Design details

- **Payload model.**
Nodes should carry a payload slot. Payloads may be ordinary Incan object references such as `model` or `class` values, so that `DiGraph[Step]` means each node's payload is a `Step`. Payloads may also use newtypes or opaque handles where that fits better. The RFC does not mandate one `payload: any` model; the stdlib must document how payloads are represented and how cloning or shared mutation interact with graph operations when payloads are mutable.

- **Edge metadata.**
Edges should support an optional kind or label, whether enumerated or string, so multiple domains can filter edges without separate side maps. This must be specified in the final API review before Planned status.

- **Immutability vs mutation.**
The stdlib may offer mutable graphs, immutable persistent updates, or both as separate types. The chosen model must be documented so carriers such as lazy plans can pick cheap clone semantics where needed.

- **Relationship to other stdlib types.**
Graph types should compose with existing collections, such as RFC 030 types, where natural, for example returning lists of `NodeId`. They must not silently duplicate dictionary semantics without documenting key conflicts.

- **Graph values vs global singletons (design decision).**
    - **Normative:** The stdlib graph abstraction must be usable as ordinary
      first-class data. Many independent instances may exist at once, and
      callers pass or store them explicitly.
    - **Normative:** The stdlib must not define or require one ambient graph
      analogous to RFC 033 `ctx`. Graphs represent structured relational or
      dependency data that is naturally per-pipeline, per-test, or per-request.
      Conflating the two would break concurrency, test isolation, and
      composition.
    - **Non-normative:** A session, runtime, or application layer may still
      offer a default or named graph registry for ergonomics, but that remains
      outside this RFC and must layer on top of value-based graph types.

## Alternatives considered

1. **Only document a pattern** such as a hand-rolled `dict` plus lists:
rejected because it avoids stdlib surface at the cost of fragmentation and weak interoperability between libraries.

2. **Expose a full-featured property-graph stdlib immediately**: rejected
because it is too large and blurs boundaries with databases and query engines.

3. **Standardize on one external implementation’s user-visible API**: rejected
because it couples the language too tightly to one dependency layout.

## Drawbacks

- **Surface area**: even a minimal graph API is a long-lived commitment; changes require RFC discipline.
- **Performance**: a single generic graph type may not fit every workload.
Domains may still need specialized structures behind thin wrappers.
- **Teaching cost**: users must learn **when** to use graph stdlib vs plain collections.

## Layers affected

- **Stdlib / runtime bindings**: new graph types and methods, potentially
backed by runtime-native structures exposed through the normal stdlib binding path.
- **Typechecker** — generic methods on graph types, error types for cycle detection.
- **Formatter / LSP** — completions and formatting for new stdlib paths.
- **Docs-site** — user-facing reference for graph types and examples.
- **RFC process** — this RFC establishes that **further** graph stdlib growth is **RFC-gated**.

## Prior art survey (informative)

This section is not normative. It summarizes how popular libraries split directed versus multigraph concerns and DAG policy to inform unresolved questions 1 and 2. It is not a mandate to copy any API.

- **Python (NetworkX).** `DiGraph` is simple by default, with at most one edge
per ordered node pair. `MultiDiGraph` is the parallel-edge variant. Acyclicity is usually an algorithm-level concern rather than a separate base type.

- **Java (JGraphT).** Separate concrete types are common. Authors choose a
simple directed graph or a multigraph at construction time.

- **JavaScript / TypeScript (Graphology).** One graph family with options such
as `type: 'directed'` and `multi: true | false`, plus specialized constructors. Parallel edges are opt-in.

- **Ruby (RGL).** Directed adjacency is the common teaching model, and
multigraph-oriented APIs are less central.

- **Rust (`petgraph`, illustrative only).** `Graph` can represent multiple
edges between the same node pair, while `GraphMap` does not. Acyclicity is not enforced by default; DAG guarantees are algorithm or wrapper concerns.

**Loose takeaway for question 1 (DAG shape).** Ecosystems rarely expose a
third ubiquitous type name `Dag` alongside `DiGraph`; DAG is often policy on a directed container. A separate `Dag` type in Incan is still allowed if it improves ergonomics or static guarantees, but it is less common in the surveyed libraries.

**Loose takeaway for question 2 (parallel edges).** Simple directed graphs are
the default in several major libraries, while parallel edges are opt-in through multigraph types or configuration. That suggests defaulting to no parallel edges in v1 unless a domain clearly needs a multigraph profile, but the RFC does not have to settle that yet.

## Unresolved questions

1. **Single `DiGraph` type vs separate `Dag` type vs `DiGraph` with
   `enforce_dag: bool` at construction**: which default best matches Incan
authors? Prior art suggests DAG-as-policy is more common than a distinct
   `Dag` type.
2. **Parallel edges**: allowed by default, or opt-in multigraph profile only?
Prior art suggests opt-in multigraph support is the recurring pattern.
3. **Node removal**: required for v1, or **append-only** graphs only until a follow-on RFC?
4. **Payload typing** on the Incan side: opaque handle only for v1, or a
generic `DiGraph[T]` / `Graph[NodePayload, EdgeKind]` style surface?
5. **Serialization**: stable JSON, or another snapshot format, in v1, or
explicitly out of scope until a dedicated RFC?

<!-- Rename this section to "Design Decisions" once all questions have been resolved. An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
