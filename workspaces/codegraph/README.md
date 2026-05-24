# Incan CodeGraph Bridge

This workspace is the handoff point for integrating Incan's compiler-backed code-index facts with external CodeGraph-style tools.

Incan exports authoritative facts with:

```bash
incan tools codegraph export path/to/source-or-directory --format jsonl
```

For work-in-progress repositories that do not currently type-check, use:

```bash
incan tools codegraph export path/to/source-or-directory --format jsonl --allow-errors
```

The exporter is intentionally Incan-first:

- Incan parses and type-checks source before exporting facts by default.
- `--allow-errors` still emits the source graph after type-check errors, but omits checked API facts for failing modules.
- Directory inputs recursively export every `.incn` file under that root.
- Public declaration nodes link to RFC 048 checked API metadata through stable `checked_api_*` facts.
- Checked API members such as fields, methods, and enum variants are exported as graph children for package exploration.
- Body facts such as `match_dispatch`, `call_site`, and `reference` nodes are exported from parsed source bodies.
- `match_dispatch` facts include explicit pattern counts and wildcard/default-arm context for overlap-ratio filtering.
- `incan architect` consumes `match_dispatch` and `call_site` graph facts for deterministic architecture signals.
- `incan_codegraph` owns only the stable wire schema.
- External indexers should ingest the exported JSON/JSONL instead of reparsing `.incn` as syntax-only text.
- CodeGraph storage, embeddings, MCP tools, and tree-sitter support remain outside the compiler.

This is separate from RFC 047 `std.graph`, which is the runtime graph data-structure surface available to Incan programs.
