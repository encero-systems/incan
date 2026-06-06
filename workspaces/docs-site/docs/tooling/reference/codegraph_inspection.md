# Codegraph inspection

`incan inspect codegraph` exports deterministic JSONL records for the source structure the compiler can see without asking a downstream tool to scrape `.incn` text. The 0.4 surface is deliberately small: it emits Incan-language files, modules, top-level declarations, imports, public exports, containment edges, body-level reference and call syntax, source spans, provenance, degraded state, and diagnostics. This is the first durable RFC 106 codegraph slice under the broader RFC 102 semantic inspection surface.

Use it when an editor, CI job, architecture review tool, or agent needs basic Incan structure with compiler-owned provenance. Do not treat it as a graph database, full reference index, resolved call graph, or stable generated-Rust ABI. The command reports source and syntax facts today, with diagnostics in tolerant mode; later releases can add richer checked relationships and target ids on the same record stream.

```bash
incan inspect codegraph src/main.incn --format jsonl
incan inspect codegraph src --format jsonl --allow-errors
```

The first record is always a `header` record. It includes the schema version, compiler version, strict or tolerant mode, requested root path, languages represented by the export, optional package identity from `incan.toml`, and whether the export is degraded. Subsequent records describe source files, modules, declarations, imports, exports, body references, body calls, containment relationships, and diagnostics. Every non-header record carries `language`, `provenance`, and `degraded` fields. Consumers should treat unknown future record kinds as opaque records rather than failing closed.

Strict mode is the default. If parsing, import resolution, or type checking produces diagnostics for a checked entrypoint, the command fails instead of emitting a partial graph. `--allow-errors` changes that contract: parseable files still produce facts, diagnostics become graph records, and the header marks the export as degraded. That mode is meant for WIP packages and agent context, not for release gates that require a fully checked graph.

`std.graph` and `incan inspect codegraph` solve different problems. `std.graph` is a runtime library for graph values inside Incan programs. `incan inspect codegraph` is tooling output about Incan source and project structure. Sharing the word "graph" does not make the tooling export part of the runtime standard library, and runtime graph APIs should not depend on this command.

The 0.4 exporter emits `language: "incan"` facts only. First-class Rust graph records, MCP tools, task-ranked context packing, process-risk signals, and architecture findings are RFC 106 follow-up work. Generated Rust remains inspectable through `incan inspect rust`, but that command is not a substitute for Rust codegraph facts.

## JSONL records

Every line is a standalone JSON object with a `record` discriminator. Current record kinds are:

- `header`: export schema, compiler version, mode, root, languages, package identity, and degraded flag.
- `file`: source language, source file path, byte size, provenance, and degraded flag.
- `module`: source language, module path, parent file id, source span, provenance, and degraded flag.
- `declaration`: source language, top-level declaration kind, name, visibility, type parameters, optional signature, source span, provenance, and degraded flag.
- `import`: source language, import kind, path, imported items, alias, visibility, source span, provenance, and degraded flag.
- `export`: public symbol exported by a declaration or public import.
- `reference`: source-level name references inside declaration bodies, including identifier, field, `self`, and surface-path forms; `target_id` is currently `null` because these records are syntax-provenance facts.
- `call`: source-level call expressions inside declaration bodies, including function, method, constructor, and surface-symbol calls; `target_id` is currently `null` because resolved call targets belong to the later semantic graph layer.
- `containment`: parent-child relationship between file, module, declaration, import, reference, or call records.
- `diagnostic`: stable diagnostic code, phase, message, primary span, notes, hints, and explain command.

Paths and ids are deterministic for the same compiler version and filesystem layout. The schema does not promise that ids are stable across file moves, symbol renames, or future schema versions; consumers that persist the graph should store the schema version and compiler version with their index.
