# Codegraph importer

This runnable Incan example consumes the common record envelope of schema-v1 through schema-v9 JSONL streams from `incan inspect codegraph`. It is an external consumer: it does not parse `.incn`, resolve names, infer targets, or become a semantic authority.

The importer uses `std.json.parse_jsonl` for the wire boundary and validates every supported envelope explicitly. JSONL itself has no header; the CodeGraph protocol uses its first JSON object as snapshot metadata, identified by `"record": "header"`. Schema v2 adds compiler-checked typed registry facts, schema v3 adds checked C binding and explicit-unsafe C call facts, schema v6 adds compiler-proven public façade-to-private-bridge relations, schema v7 adds structured canonical identity to checked declarations, references, and calls, and schema v8 gives nested stable identities a checked owner and collision-local ordinal. The importer counts records without interpreting their payloads. It preserves unknown future record kinds as opaque records and prints a deterministic JSON summary. A production indexer can persist the original records alongside that summary or use the same boundary checks before mapping facts into another store.

## Try it

From this directory:

```bash
incan inspect codegraph ../../simple/hello.incn --format jsonl > codegraph.jsonl
incan run src/main.incn
```

The output is a compact JSON summary. Remove `codegraph.jsonl` afterwards; it is an input artifact, not checked-in source.

An optional program argument selects another input: `incan run src/main.incn -- /path/to/snapshot.jsonl`. Missing or invalid input exits unsuccessfully. `make examples` generates a snapshot of the hello example automatically in its output directory and passes that path to the importer.

## Deliberate limits

- The compiler-owned JSONL remains the source of truth.
- Unknown record kinds are counted but never interpreted.
- The importer does not write a database, infer missing relationships, or claim that its summary is a complete graph.
- It accepts schema versions 1 through 9, whose `record` and `degraded` envelope fields have the same shape. It reads the header's `schema_version`, `mode` and `degraded`; the remaining metadata and fact payloads stay opaque. A future schema must be reviewed before extending the admitted range.
