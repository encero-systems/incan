# Codegraph importer

This runnable Incan example consumes the schema-v1 and schema-v2 JSONL streams from `incan inspect codegraph`. It is an external consumer: it does not parse `.incn`, resolve names, infer targets, or become a semantic authority.

The importer uses `std.json.parse_jsonl` for the wire boundary and validates both supported envelopes explicitly. JSONL itself has no header; the CodeGraph protocol uses its first JSON object as snapshot metadata, identified by `"record": "header"`. Schema v2 adds compiler-checked typed registry facts, which the importer counts without evaluating their descriptors. It preserves unknown future record kinds as opaque records and prints a deterministic JSON summary. A production indexer can persist the original records alongside that summary or use the same boundary checks before mapping facts into another store.

## Try it

From this directory:

```bash
incan inspect codegraph ../../simple/hello.incn --format jsonl > codegraph.jsonl
incan run src/main.incn
```

The output is a compact JSON summary. Remove `codegraph.jsonl` afterwards; it is an input artifact, not checked-in source.

## Deliberate limits

- The compiler-owned JSONL remains the source of truth.
- Unknown record kinds are counted but never interpreted.
- The importer does not write a database, infer missing relationships, or claim that its summary is a complete graph.
- It accepts schema versions 1 and 2 through explicit branches. A future schema adapter must be added deliberately rather than silently treating a new version as compatible.
