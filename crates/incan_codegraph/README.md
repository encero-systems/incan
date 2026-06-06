# incan_codegraph

`incan_codegraph` defines the storage-agnostic JSONL record schema used by
`incan inspect codegraph`.

The crate owns record types, schema versioning, language/provenance vocabulary,
source span shapes, degraded-state flags, and JSONL serialization helpers. It
does not extract facts from source code. The compiler and tooling layers produce
records; downstream tools decide how to index, query, rank, visualize, or serve
them.

## Scope

The 0.4 schema is the first RFC 106 codegraph slice. It covers:

- export headers with schema version, compiler version, mode, root, languages,
  package identity, and degraded state
- source files and modules
- top-level declarations
- imports and public exports
- body-level reference and call syntax
- containment relationships
- stable diagnostic records in tolerant exports
- source spans, explicit language tags, provenance, and degraded-state flags

This crate deliberately has no dependency on compiler internals, graph
databases, embeddings, MCP servers, or storage engines.

## 0.4 Contract

The 0.4 exporter emits Incan-language facts only:

```json
{"record":"header","schema_version":1,"languages":["incan"]}
```

Every non-header fact record carries:

- `language`
- `provenance`
- `degraded`

The schema already has a `rust` language value because Rust is Incan's host,
generated-code target, and interop substrate. That is reserved for follow-up
work; the 0.4 CLI must not emit Rust graph facts until first-class Rust support
lands.

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

Those capabilities can consume or extend codegraph records, but they should not
replace the compiler-owned schema contract.
