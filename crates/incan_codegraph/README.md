# incan_codegraph

`incan_codegraph` defines the storage-agnostic JSONL record schema used by `incan inspect codegraph`.

The crate owns record types, schema versioning, language/provenance vocabulary, source span shapes, degraded-state flags, and JSONL serialization helpers. It does not extract facts from source code. The compiler and tooling layers produce records; downstream tools decide how to index, query, rank, visualize, or serve them.

## Scope

Codegraph began as the first v0.5 RFC 106 slice. The current v0.6 schema covers:

- export headers with schema version, compiler version, mode, root, languages, package identity, and degraded state
- source files and modules
- top-level declarations
- imports and public exports
- compiler-checked registry entries, including public facade projections that preserve one canonical subject identity
- compiler-checked C binding declarations, direct C calls admitted through explicit `unsafe:` source, and compiler-proven public façade-to-private-bridge relations
- canonical declaration identities on checked declaration, reference, and call records, with export-local `target_id`
  linkage when the target declaration is present in the same graph
- containment relationships
- stable diagnostic records in tolerant exports, including canonical identities for related declarations
- source spans, explicit language tags, provenance, and degraded-state flags

This crate deliberately has no dependency on compiler internals, graph databases, embeddings, MCP servers, or storage engines.

## 0.6 Contract

The v0.6 exporter emits Incan-language facts only:

```json
{"record":"header","schema_version":7,"languages":["incan"]}
```

Every non-header fact record carries:

- `language`
- `provenance`
- `degraded`

Checked declaration, reference, and call records also carry a structured `canonical_identity`. References reached
through imports, aliases, or re-exports retain the original declaration identity. `target_id` is only an optional link
to a declaration record in this particular export; a checked identity remains present when no such record exists.

The schema already has a `rust` language value because Rust is Incan's host, generated-code target, and interop substrate. That is reserved for follow-up work; the v0.6 CLI must not emit Rust graph facts until first-class Rust support lands.

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
