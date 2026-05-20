# Why `std.graph` exists

`std.graph` exists for programs where relationships are not just a storage detail. Dependency plans, workflow stages, validation pipelines, state machines, and dataflow structures often need stable identity, adjacency queries, traversal, and ordering as part of the public contract.

## Graphs are explicit values

`std.graph` does not define an ambient process-wide graph. A graph is an ordinary value that can be created, passed as an argument, stored on a model, returned from a function, and constructed independently in each test.

That shape keeps ownership visible. A function can accept `DiGraph[Step]`, `Dag[Task]`, or `MultiDiGraph[Route]` without relying on hidden module state, and callers can keep separate graph instances for separate pipelines or requests.

## Node ids are the graph identity

Node payloads are user data. `NodeId` is the graph identity. Two nodes may carry equal payloads while still representing different places in a plan, and one payload value may be reused by choosing to reuse one node id.

This is different from using a dictionary keyed by payload. A dictionary makes the key the identity, which is right for many maps but awkward when the same logical value can appear in multiple positions or when the graph should control node lifetime.

## DAGs make acyclicity a data invariant

`DiGraph[T].topological_order()` can report cycles because a directed graph is allowed to contain them. `Dag[T]` rejects cycle-creating edges when they are inserted, so acyclicity becomes an invariant of the value rather than a check every caller must remember to perform.

Use `Dag[T]` for dependency plans, build stages, and ordered pipelines where accepting a cycle would make the value invalid. Use `DiGraph[T]` when cycles are valid domain data or when cycle detection is only one query among others.

## Multigraphs keep relationship identity separate

Some domains need more than one edge between the same two nodes. A dataflow edge and a control edge may connect the same stages, or several fallback paths may share source and target nodes. `MultiDiGraph[T]` keeps those relationships distinct by assigning each edge an `EdgeId`.

Labels and attributes still belong in user-owned data. `std.graph` tracks structure and ids; richer relationship metadata can live in payload models, side tables keyed by `EdgeId`, or a higher-level library.

## Boundaries are intentional

`std.graph` is a small in-memory graph module. It is not a graph database, query language, persistence format, or distributed synchronization layer. Weighted edges, property graph queries, alternate storage backends, and serialization-specific contracts should be designed as explicit extensions rather than folded into the core reference surface by accident.

Keeping the standard module narrow makes it useful as a dependable structural primitive without making every graph-shaped workload inherit database-scale concepts.

## See also

- [Working with graphs](../how-to/working_with_graphs.md)
- [`std.graph` reference](../reference/stdlib/graph.md)
- [RFC 047: Lightweight directed graph types](../../RFCs/closed/implemented/047_lightweight_directed_graph_stdlib.md)
