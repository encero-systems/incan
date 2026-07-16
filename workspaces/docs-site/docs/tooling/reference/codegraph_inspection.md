# Codegraph inspection

`incan inspect codegraph` exports deterministic JSONL records for the source structure the compiler can see without asking a downstream tool to scrape `.incn` text. The 0.4 surface is deliberately small: it emits Incan-language files, modules, top-level declarations, imports, public exports, containment edges, body-level reference and call syntax, conservative resolved reference and call targets, source spans, provenance, degraded state, and diagnostics. This is the first durable RFC 106 codegraph slice under the broader RFC 102 semantic inspection surface.

Use it when an editor, CI job, architecture review tool, or agent needs basic Incan structure with compiler-owned provenance. Do not treat it as a graph database, full reference index, whole-program call graph, or stable generated-Rust ABI. The command reports source and syntax facts today, with diagnostics in tolerant mode; checked typechecking facts can populate `reference.target_id` and `call.target_id` when the compiler has a conservative declaration identity for the source target.

Codegraph inspection is one piece of the 0.4 semantic-inspection baseline. Pair it with `incan check --format json` for stable diagnostics, `incan build --report json` for build and artifact metadata, and `incan inspect rust --format json` for current generated Rust output. The four surfaces should agree on schema version, compiler version, project identity, source breadcrumbs, and explicit degraded-state or diagnostic reporting where their scopes overlap.

```bash
incan inspect codegraph src/main.incn --format jsonl
incan inspect codegraph src --format jsonl --allow-errors
```

The first record is always a `header` record. It includes the schema version, compiler version, strict or tolerant mode, requested root path, languages represented by the export, optional package identity from `incan.toml`, and whether the export is degraded. Subsequent records describe source files, modules, declarations, imports, exports, body references, body calls, containment relationships, and diagnostics. Every non-header record carries `language`, `provenance`, and `degraded` fields. Body facts with a compiler-proven `target_id` use `provenance: "checked"`; syntax-only body facts keep `provenance: "syntax"`. In 0.4, `target_id` points only at declaration records emitted in the same JSONL export. Public-package manifest identity is still checked during typechecking and remains available in library manifests, but external package declarations are not emitted as codegraph declaration records until the schema grows a first-class external-target shape. Consumers should treat unknown future record kinds as opaque records rather than failing closed.

Strict mode is the default. If parsing, import resolution, or type checking produces diagnostics for a checked entrypoint, the command fails instead of emitting a partial graph. `--allow-errors` changes that contract: parseable files still produce facts, diagnostics become graph records, and the header marks the export as degraded. That mode is meant for WIP packages and agent context, not for release gates that require a fully checked graph.

`std.graph` and `incan inspect codegraph` solve different problems. `std.graph` is a runtime library for graph values inside Incan programs. `incan inspect codegraph` is tooling output about Incan source and project structure. Sharing the word "graph" does not make the tooling export part of the runtime standard library, and runtime graph APIs should not depend on this command.

The 0.4 exporter emits `language: "incan"` facts only. First-class Rust graph records, MCP tools, task-ranked context packing, process-risk signals, and architecture findings are RFC 106 follow-up work. Generated Rust remains inspectable through `incan inspect rust`, but that command is not a substitute for Rust codegraph facts.

## External importer example

`examples/pro/codegraph_importer` is a runnable Incan-authored consumer of this JSONL contract. It takes an exported `codegraph.jsonl` file, requires the schema-v1 header and fact envelope, counts the current record kinds, preserves unknown future kinds as opaque, and prints a deterministic JSON summary.

```bash
incan inspect codegraph src/main.incn --format jsonl > codegraph.jsonl
cd examples/pro/codegraph_importer
incan run src/main.incn
```

The example does not parse `.incn`, resolve names, infer missing edges, or store graph data. It demonstrates the intended boundary: an importer may validate, persist, compare, or visualize compiler-owned facts, but must not become their semantic authority. Schema version 1 is explicit; adapters for later versions must be deliberate rather than silently accepting a changed contract.

## JSONL records

Every line is a standalone JSON object with a `record` discriminator. Current record kinds are:

- `header`: export schema, compiler version, mode, root, languages, package identity, and degraded flag.
- `file`: source language, source file path, byte size, provenance, and degraded flag.
- `module`: source language, module path, parent file id, source span, provenance, and degraded flag.
- `declaration`: source language, top-level declaration kind, name, visibility, type parameters, optional signature, source span, provenance, and degraded flag.
- `import`: source language, import kind, path, imported items, alias, visibility, source span, provenance, and degraded flag.
- `export`: public symbol exported by a declaration or public import.
- `reference`: source-level name references inside declaration bodies, including identifier, field, `self`, and surface-path forms. `target_id` points at an emitted declaration record for compiler-proven source identifiers, including local declarations and supported source imports within the exported graph. Unsupported, ambiguous, degraded, field, `self`, surface-path, external-package, and syntax-only cases keep `target_id: null`.
- `call`: source-level call expressions inside declaration bodies, including function, method, constructor, and surface-symbol calls. `target_id` points at an emitted declaration record for compiler-proven source function and constructor calls, including local declarations, supported source imports, and facade reexports where the original declaration identity is known within the exported graph. Unsupported, ambiguous, degraded, method, surface-symbol, external-package, and syntax-only cases keep `target_id: null`.
- `containment`: parent-child relationship between file, module, declaration, import, reference, or call records.
- `diagnostic`: stable diagnostic code, phase, message, primary span, notes, hints, and explain command.

Paths and ids are deterministic for the same compiler version and filesystem layout. The schema does not promise that ids are stable across file moves, symbol renames, or future schema versions; consumers that persist the graph should store the schema version and compiler version with their index.
