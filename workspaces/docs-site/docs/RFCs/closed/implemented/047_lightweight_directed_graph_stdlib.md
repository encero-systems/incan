# RFC 047: Lightweight directed graph types (stdlib)

- **Status:** Implemented
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
- **Shipped in:** v0.3.0-dev.37

## Summary

This RFC proposes a small, opinionated directed-graph abstraction in the Incan standard library: stable node identifiers, node payloads, directed edges, multigraph edge identity, and a minimal interrogation API covering adjacency, roots and sinks, traversal, and optional topological order. It is not a graph database, query language, or persistence layer. New graph-related stdlib surfaces should continue to go through RFCs rather than growing ad hoc.

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

Authors obtain a graph handle from the stdlib, add nodes with payloads, and add directed edges from one node to another. They can ask for outgoing or incoming neighbors, enumerate roots or sinks, and walk from a start node in a documented order. Common walk orders are breadth-first search and depth-first search; the stdlib must document which order each traversal API uses. When the graph is intended to be acyclic, authors may use checked edge insertion that rejects a cycle, or they may call topological-order APIs and handle the error if a cycle exists.

The type is in-memory and library-scoped. Persistence, network sync, and engine-specific optimization remain outside this RFC.

Authors construct and pass graph values explicitly rather than reaching for one implicit global graph.

## Illustrative API sketches (non-normative)

The following examples show the intended Incan-facing shape. Names, return types, and error modeling may still be refined during implementation, but the design decisions below define the stable contract.

**Module path** is assumed to be `std.graph` (see RFC 022 stdlib namespacing); final names **may** differ.

### Constructing a directed graph and querying neighbors

```incan
from std.graph import DiGraph, NodeId

# Empty graph; nodes and edges added explicitly.
mut g = DiGraph[str]()

a: NodeId = g.add_node(payload="scan_users")
b: NodeId = g.add_node(payload="filter_active")
c: NodeId = g.add_node(payload="join_orders")

g.add_edge(from_=a, to=b)
g.add_edge(from_=b, to=c)

out_from_b: list[NodeId] = g.successors(b)
roots: list[NodeId] = g.roots()
```

### Node payloads as **object references** (e.g. `Step`)

Graphs are not limited to string or primitive payloads. A common pattern is `DiGraph[Step]`, or another user type, where each `NodeId` maps to a `Step` value the author already holds. Edges still connect `NodeId`s, so one `Step` instance can be the payload of node `a` and another of node `b`, with a directed edge `a -> b` meaning "this step runs before that step," or any other domain-specific relationship.

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

**Semantics:** `add_node(payload=...)` ties a `NodeId` to that payload object. If two nodes should represent the same `Step` instance, that is a domain choice: either one node is shared or two nodes hold cloned payloads. The stdlib must document how clone versus shared-reference behavior works for mutable payloads.

**Bounds:** generic graphs such as `DiGraph[T]` will likely require `T` to satisfy whatever the language requires for stored or shared data. Exact bounds are implementation detail, but they must appear in user-facing docs.

### Traversal (BFS / DFS) from a start node

```incan
from std.graph import DiGraph, NodeId

def collect_bfs(g: DiGraph[str], start: NodeId) -> list[NodeId]:
    match g.bfs_nodes(start):
        case Ok(nodes):
            return nodes
        case Err(_):
            return []

def collect_dfs_preorder(g: DiGraph[str], start: NodeId) -> list[NodeId]:
    match g.dfs_preorder_nodes(start):
        case Ok(nodes):
            return nodes
        case Err(_):
            return []
```

Implementations **must** document iteration order; `bfs_nodes` and `dfs_preorder_nodes` are the shipped traversal methods.

### Topological order on a DAG-shaped graph

```incan
from std.graph import DiGraph, NodeId

def topo_or_empty(g: DiGraph[str]) -> list[NodeId]:
    match g.topological_order():
        case Ok(order):
            return order
        case Err(_):
            return []  # cycle present
```

### DAG type: reject edges that would create a cycle

```incan
from std.graph import Dag, GraphError, NodeId

mut d = Dag[int]()
x = d.add_node(payload=1)
y = d.add_node(payload=2)
z = d.add_node(payload=3)

d.add_edge(from_=x, to=y)
d.add_edge(from_=y, to=z)

cycle_attempt: Result[None, GraphError] = d.add_edge(z, x)
match cycle_attempt:
    case Ok(_):
        pass  # unexpected if cycle detection works
    case Err(_):
        pass  # expected
```

The stable contract includes a first-class `Dag[T]` surface for domains that need acyclicity as part of the type they pass around.

### Parallel edges with `MultiDiGraph`

```incan
from std.graph import EdgeId, GraphError, MultiDiGraph, NodeId

mut m = MultiDiGraph[str]()
u = m.add_node("a")
v = m.add_node("b")

e1: Result[EdgeId, GraphError] = m.add_edge(u, v)
e2: Result[EdgeId, GraphError] = m.add_edge(u, v)

edges_uv: Result[list[EdgeId], GraphError] = m.edges_between(u, v)
```

`MultiDiGraph[T]` is part of this RFC's stable graph family for domains that need multiple separately identified edges between the same ordered node pair.

### Storing a graph as **data** (not a singleton)

```incan
pub model PipelinePlan:
    pub deps: DiGraph[str]
    pub tip: NodeId

def describe_roots(plan: PipelinePlan) -> str:
    mut roots: list[str] = []
    for nid in plan.deps.roots():
        match plan.deps.node_payload(nid):
            case Ok(name):
                roots.append(name)
            case Err(_):
                pass
    return ", ".join(roots)
```

## Reference-level explanation

### Directed graph

A **directed graph** value **must** conceptually consist of:

- A set of nodes, each with a distinct `NodeId` while the graph instance exists. Each node carries a payload: any Incan value the API allows, including primitives, `model` or `class` instances, newtypes, and similar values. Edges always connect `NodeId`s, not raw object references.
- A set of directed edges from one `NodeId` to another `NodeId`. `DiGraph[T]` is simple and does not permit parallel edges between the same ordered node pair; `MultiDiGraph[T]` permits parallel edges and distinguishes them with `EdgeId`.

Removing an edge must remove that edge from outgoing and incoming adjacency. Removing a node must remove all incident edges. Implementations must not leave the graph in an inconsistent state silently.

### NodeId stability

`NodeId` values must not be reused by ordinary graph operations within the same graph instance, including after node removal.

### DAG profile

`Dag[T]` is a first-class graph type. Adding an edge that would introduce a cycle must fail with a defined error or return type. The graph must not accept the edge.

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
Edges should support an optional kind or label so multiple domains can filter edges without separate side maps. `DiGraph[T]` stores at most one edge per ordered node pair; `MultiDiGraph[T]` stores multiple edge ids per ordered node pair.

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

## Implementation Plan

### Phase 1: Stdlib Runtime Surface

- Add the source-defined `std.graph` module surface for `DiGraph[T]`, `Dag[T]`, `MultiDiGraph[T]`, `NodeId`, `EdgeId`, and graph errors.
- Implement directed edges, payload storage, stable non-reused node ids, edge/node removal semantics, roots/sinks, successors/predecessors, BFS, DFS preorder, topological order, and checked acyclic insertion.
- Keep serialization out of the stable surface.

### Phase 2: Compiler and Stdlib Wiring

- Register `std.graph` in the stdlib namespace registry and expose IDE/typechecker stubs through the stdlib source tree.
- Ensure the pure Incan `std.graph` implementation lowers and emits correctly with generic graph types.
- Add typechecker coverage for valid graph construction, method calls, and invalid edge/node usage where diagnostics can be resolved at compile time.

### Phase 3: Documentation and Release Surface

- Add authored user-facing `std.graph` reference documentation with examples for construction, adjacency, traversal, topological order, `Dag[T]`, and `MultiDiGraph[T]`.
- Update stdlib reference navigation and release notes for the active dev line.
- Bump the active dev version after implementation work lands.

### Phase 4: Verification and Closeout

- Add unit and integration tests for runtime behavior, stdlib importability, code generation, and user-facing examples.
- Run targeted slice verification, integrated review/fix, and the repository pre-commit gate.
- Update this checklist as implementation phases land.

## Implementation log

### RFC / lifecycle

- [x] Move RFC 047 from Draft to Planned with settled design decisions.
- [x] Move RFC 047 from Planned to In Progress when the Ralph implementation loop starts.
- [x] Move RFC 047 from In Progress to Implemented when the implementation is PR-ready.
- [x] Keep this checklist current as slices land.

### Stdlib implementation

- [x] Add pure Incan `std.graph` implementation in the stdlib source tree.
- [x] Add `NodeId`, `EdgeId`, `DiGraph[T]`, `Dag[T]`, `MultiDiGraph[T]`, and graph error types.
- [x] Implement node insertion and stable non-reused node id semantics.
- [x] Implement edge and node removal semantics.
- [x] Implement simple directed edge insertion with optional kind metadata.
- [x] Implement multigraph edge insertion with distinct `EdgeId` values.
- [x] Reject duplicate simple edges between the same ordered node pair.
- [x] Implement successors, predecessors, roots, and sinks.
- [x] Implement BFS and DFS preorder traversal with deterministic documented order.
- [x] Implement topological order with cycle reporting.
- [x] Implement `Dag[T]` edge insertion that rejects cycles without mutating the graph.

### Compiler / stdlib wiring

- [x] Register `std.graph` in the stdlib namespace registry.
- [x] Add stdlib `.incn` declarations for graph types and methods.
- [x] Ensure graph imports activate through the normal stdlib loader path.
- [x] Ensure generic `DiGraph[T]`, `Dag[T]`, and `MultiDiGraph[T]` types resolve and emit to the runtime Rust types.
- [x] Add focused typechecker/codegen coverage for graph construction and method use.

### Docs / release

- [x] Add `std.graph` reference documentation.
- [x] Update stdlib reference index and navigation.
- [x] Add release notes for `0.3`.
- [x] Bump `0.3.0-dev.33` to the next dev version.

### Verification

- [x] Add Incan source, generated-build, and integration coverage for graph behavior.
- [x] Add Incan integration/codegen tests for `std.graph`.
- [x] Run targeted stdlib/compiler tests.
- [x] Run integrated review/fix.
- [x] Run `make pre-commit`.

## Prior art survey (informative)

This section is not normative. It summarizes how popular libraries split directed versus multigraph concerns and DAG policy. It is not a mandate to copy any API.

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

**Loose takeaway for question 1 (DAG shape).** Ecosystems rarely expose a third ubiquitous type name `Dag` alongside `DiGraph`; DAG is often policy on a directed container. A separate `Dag` type in Incan is still allowed if it improves ergonomics or static guarantees, but it is less common in the surveyed libraries.

**Loose takeaway for question 2 (parallel edges).** Simple directed graphs are the default in several major libraries, while parallel edges are opt-in through multigraph types or configuration. That supports keeping `DiGraph[T]` simple while also including `MultiDiGraph[T]` for domains that need a multigraph profile.

## Design Decisions

1. **The stable graph family includes `DiGraph[T]`, `Dag[T]`, and `MultiDiGraph[T]`.** `DiGraph[T]` is the ordinary directed graph, `Dag[T]` is the acyclicity-preserving directed graph for dependency and workflow domains, and `MultiDiGraph[T]` is the parallel-edge variant.
2. **`Dag[T]` is not a follow-up.** Domains such as workflow and dependency modeling need acyclicity in the type they pass around, not just an optional helper call on a general graph.
3. **Parallel edges are first-class through `MultiDiGraph[T]`.** `DiGraph[T]` stays simple, but the RFC includes a multigraph type for domains that need multiple labeled edges between the same ordered node pair.
4. **Graph construction should use ordinary Incan call syntax.** Examples use `DiGraph[T]()`, `Dag[T]()`, and `MultiDiGraph[T]()` rather than Rust-style `.new()` calls.
5. **Node ids are stable and not reused.** Removing a node removes incident edges, but `NodeId` values remain non-reused within the same graph instance.
6. **Payloads are generic.** The stdlib surface uses graph types parameterized by node payload type. Richer typed edge metadata, such as `Graph[NodePayload, EdgeKind]`, may be proposed later if string or enum-like edge kinds are not enough.
7. **Serialization is out of scope.** This RFC does not define stable JSON or another graph snapshot format. Persistence and interchange formats require a dedicated RFC because they imply compatibility, id-stability, and payload-encoding guarantees beyond the in-memory graph contract.
