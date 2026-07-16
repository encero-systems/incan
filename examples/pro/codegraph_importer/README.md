# Codegraph importer

This runnable Incan example consumes the schema-v1 JSONL stream from `incan inspect codegraph`. It is an external consumer: it does not parse `.incn`, resolve names, infer targets, or become a semantic authority.

The importer uses `std.json.parse_jsonl` for the wire boundary, validates the schema-v1 envelope, requires one header record, counts the currently known fact kinds, preserves unknown future kinds as opaque records, and prints a deterministic JSON summary. A production indexer can persist the original records alongside that summary or use the same boundary checks before mapping facts into another store.

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
- It currently accepts schema version 1 only. A future schema adapter must be explicit rather than silently treating a new version as compatible.
