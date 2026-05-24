# incan_codegraph

`incan_codegraph` defines the stable, storage-agnostic code-index fact schema used by Incan tooling. It is intentionally separate from RFC 047 `std.graph`, which is a runtime graph data-structure surface for Incan programs.

The compiler owns fact extraction because it has access to parsed, import-aware, and typechecked source. This crate owns only serializable graph records and helpers for JSON/JSONL output.

The CLI exporter enriches source graph records with RFC 048 checked API metadata facts where available. Downstream indexers can join file/module/import topology with the typed public API contract that Incan already emits for library artifacts.

Body-level facts are represented as source-backed graph nodes, starting with:

- `match_dispatch` nodes for match expressions with a syntactic dispatch domain, stable pattern labels, explicit arm counts, and wildcard/default-arm context
- `call_site` nodes for syntactic function, method, constructor, and surface-symbol calls
- `reference` nodes for syntactic identifier, `self`, and field references

These facts are intentionally deterministic and parser-backed. Name resolution and type-directed symbol binding can be layered onto the graph later without requiring agents to re-walk the AST for every architecture rule.
