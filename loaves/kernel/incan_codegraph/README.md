# incan_codegraph

Ring: **kernel** — the stable codegraph fact schema for tooling and agent context.
`incan_codegraph` defines the fact schema and portable Loaf graph artifact used by `incan inspect codegraph`.

The crate owns record types, schema versioning, language/provenance vocabulary, source span shapes, degraded-state flags, and JSONL serialization helpers. It does not extract facts from source code. The compiler and tooling layers produce records; downstream tools decide how to index, query, rank, visualize, or serve them.

## Scope

Codegraph began as the first v0.5 RFC 106 slice. The current v0.6 schema covers:

- export headers with schema version, compiler version, mode, root, languages, package identity, and degraded state
- source files and modules
- top-level declarations
- imports and public exports
- compiler-checked registry entries, including public facade projections that preserve one canonical subject identity
- compiler-checked C binding declarations, direct C calls admitted through explicit `unsafe:` source, and compiler-proven public façade-to-private-bridge relations
- canonical declaration identities on checked declaration, reference, and call records, with export-local `target_id` linkage when the target declaration is present in the same graph
- containment relationships
- stable diagnostic records in tolerant exports, including canonical identities for related declarations
- source spans, explicit language tags, provenance, and degraded-state flags

This crate deliberately has no dependency on compiler internals, graph databases, embeddings, MCP servers, or storage engines.

## Portable Loaf artifacts

`artifact::encode_graph` projects compiler records into `.incngraph` version 1. The header is a little-endian `u32` version followed by a little-endian `u64` index length. A postcard index holds the shared field-name dictionary and each record's kind, ID, payload offset and length. Independently encoded postcard records follow the index. Declaration IDs derive from compiler-proven span-free identities; physical source roots become Loaf-relative anchors. Readers validate the version and index before decoding selected record groups. JSON remains an output representation, never the persisted graph.

The wire schema and binary layout versions are separate. A reader rejects incompatible versions and malformed payloads; consumers must not silently replace a corrupt published graph with a new source export.

## 0.6 Contract

The v0.6 exporter includes checked Incan and Rust facts:

```json
{"record":"header","schema_version":9,"languages":["incan","rust"]}
```

Every non-header fact record carries:

- `language`
- `provenance`
- `degraded`

Checked declaration, reference, and call records also carry a structured `canonical_identity`. References reached through imports, aliases, or re-exports retain the original declaration identity. `target_id` is only an optional link to a declaration record in this particular export; a checked identity remains present when no such record exists.

Rust graph facts are collected by the installed rustc driver. Incan facts come from the shared checked-unit exporter, including authored Incan dependencies in mixed-language closures.

## Non-goals

`incan_codegraph` is not:

- runtime `std.graph`
- a graph database
- an MCP server
- an embedding or search index
- a generated-Rust ABI contract
- a full resolved reference or call graph
- an architecture recommendation engine
- a process-risk scoring engine

Those capabilities can consume or extend codegraph records, but they should not replace the compiler-owned schema contract.

Checked call and reference records also carry an optional `canonical_owner`, projected from the same semantic relationship query used for package execution requirements. It identifies the closest checked declaration even when the export has no navigation record for a method or field. A missing owner remains unproven; syntax-only containment is not execution authority. This is an additive schema 7 field: older records deserialize with `None`, and readers must tolerate unknown optional fields.
