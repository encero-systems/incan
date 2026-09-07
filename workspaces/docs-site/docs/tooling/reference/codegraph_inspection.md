# Codegraph inspection

`incan inspect codegraph` exports deterministic JSONL records for the source structure the compiler can see without asking a downstream tool to scrape `.incn` text. The surface emits Incan-language files, modules, top-level declarations, imports, public exports, checked registry entries, checked C binding declarations and explicit-unsafe C calls, compiler-proven public façade-to-private-bridge relations, containment edges, body-level reference and call syntax, canonical resolved identities, source spans, provenance, degraded state, and diagnostics. This is the durable compiler-owned codegraph surface.

Use it when an editor, CI job, architecture review tool, or agent needs basic Incan structure with compiler-owned provenance. Do not treat it as a graph database, full reference index, whole-program call graph, or stable generated-Rust ABI. The command reports source and syntax facts, compiler-proven registry entries, and diagnostics in tolerant mode. Checked declaration, reference, and call facts carry structured `canonical_identity`; `target_id` is an additional export-local link when the target declaration record is present.

Codegraph inspection is one piece of the v0.5 semantic-inspection baseline. Pair it with `incan check --format json` for stable diagnostics, `incan build --report json` for build and artifact metadata, `incan inspect registry` for one selected complete checked catalogue, and `incan inspect rust --format json` for current generated Rust output. The surfaces should agree on compiler version, project identity, source breadcrumbs, and explicit degraded-state or diagnostic reporting where their scopes overlap.

```bash
incan inspect codegraph src/main.incn --format jsonl
incan inspect codegraph src --format jsonl --allow-errors
```

The first record is always a `header` record. It includes schema version 7, compiler version, strict or tolerant mode, requested root path, languages represented by the export, optional package identity from `incan.toml`, typed SDK/package/provider semantic contexts, and whether the export is degraded. Subsequent records describe source files, modules, declarations, imports, exports, checked registry entries, checked C binding declarations, checked direct C calls, compiler-proven C façade relations, body references, body calls, containment relationships, and diagnostics. Every non-header record carries `language`, `provenance`, and `degraded` fields. Registry records always use `provenance: "checked"`: they contain the structural key and descriptor values, canonical subject kind and identity, visibility, registration and subject anchors, and any public facade projections from the same typechecker facts used by `incan inspect registry`. A facade projection gives another import path; it never creates a duplicate registry record or changes its canonical subject. Registry records do not describe loaded runtime registry state. A declaration, reference, or call with a compiler-proven identity carries `canonical_identity` and uses `provenance: "checked"`, even when no matching declaration record exists in this export. `target_id` points only at a declaration record emitted in the same JSONL export and is never used to reconstruct identity. Public-package declarations remain external to the graph, but their checked package identity can still be carried by reference and call records. Syntax-only body facts keep `provenance: "syntax"`. Consumers should treat unknown future record kinds as opaque records rather than failing closed.

A `c_binding` record is a structural projection of one successfully checked `binding` declaration. It links to the ordinary class declaration through `declaration_id` and contains a `binding_identity`, the header and logical system-library capability, opaque-resource release association, symbol parameter and return contracts, ownership modes, `out` or `in_out` transitions, enum carriers and native constant spellings, plain C structures, and a binding-declaration span. `binding_identity` excludes source spans and source-file locations but retains the declared header spelling, so a portable header spelling is required for relocation-stable identities. It changes with any ABI-affecting checked descriptor field. A `c_binding_call` record carries the same `binding_identity` and marks a direct symbol call the typechecker admitted under an explicit `unsafe:` block. When the typechecker recorded an owning named function, `owner_declaration_id` and `owner_visibility` link the raw call to that callable. A `c_binding_facade` record is emitted only when the typechecker proves that a public function directly calls a private function in the same module and that bridge owns one or more checked raw calls. It links the façade declaration, bridge declaration, ordinary checked call, and raw-call records without treating a name or generated Rust as an authority. It does not represent transitive, imported, re-exported, method, or syntax-only relationships. `unsafe_acknowledged: true` means that particular raw call was explicitly acknowledged; it is not a general claim that the binding, library, or package is safe.

These C records are language-contract facts only. They do not resolve an Oven requirement to a local compiler or SDK, read a lock receipt, fetch or stage an artifact, build a shim, prove that a library is present at runtime, or expose an editor navigation result. Use `incan inspect bindings` for the focused declaration review and `[oven.interop]` with `incan lock` for declared and locked package inputs.

Strict mode is the default. If parsing, import resolution, or type checking produces diagnostics for a checked entrypoint, the command fails instead of emitting a partial graph. `--allow-errors` changes that contract: parseable files still produce facts, diagnostics become graph records, and the snapshot metadata marks the export as degraded. That mode is meant for WIP packages and agent context, not for release gates that require a fully checked graph.

`std.graph` and `incan inspect codegraph` solve different problems. `std.graph` is a runtime library for graph values inside Incan programs. `incan inspect codegraph` is tooling output about Incan source and project structure. Sharing the word "graph" does not make the tooling export part of the runtime standard library, and runtime graph APIs should not depend on this command.

The v0.6 exporter emits `language: "incan"` facts only. First-class Rust graph records, MCP tools, task-ranked context packing, process-risk signals, and architecture findings remain future work. Generated Rust remains inspectable through `incan inspect rust`, but that command is not a substitute for Rust codegraph facts.

## External importer example

`examples/pro/codegraph_importer` is a runnable Incan-authored consumer of this JSONL contract. It accepts the v1 through v7 snapshot envelopes, counts the currently known record kinds, preserves unknown future kinds as opaque, and prints a deterministic JSON summary.

```bash
incan inspect codegraph src/main.incn --format jsonl > codegraph.jsonl
cd examples/pro/codegraph_importer
incan run src/main.incn
```

The example does not parse `.incn`, resolve names, infer missing edges, or store graph data. It demonstrates the intended boundary: an importer may validate, persist, compare, or visualize compiler-owned facts, but must not become their semantic authority. Schema versions 1 through 7 are explicit; adapters for later versions must be deliberate rather than silently accepting a changed contract.

## JSONL records

Every line is a standalone JSON object with a `record` discriminator. Current record kinds are:

- `header` (snapshot metadata): export schema, compiler version, mode, root, languages, package identity, and degraded flag.
- `file`: source language, source file path, byte size, provenance, and degraded flag.
- `module`: source language, module path, parent file id, source span, provenance, and degraded flag.
- `declaration`: source language, top-level declaration kind, name, visibility, type parameters, optional signature, optional compiler-owned `canonical_identity`, source span, provenance, and degraded flag.
- `import`: source language, import kind, path, imported items, alias, visibility, and per-binding local spelling plus canonical target identity where checked, source span, provenance, and degraded flag.
- `export`: public symbol exported by a declaration or public import, carrying the exported declaration identity when checked.
- `registry`: complete compiler-checked typed registry entry with canonical registry and subject identities, key and descriptor structure, visibility, registration and subject anchors, optional `reexport_paths` (each with a facade `path` and source `span`), and checked provenance. A reexport does not create a duplicate registry fact.
- `c_binding`: compiler-checked C binding declaration with a `declaration_id` link to its ordinary class record, a relocation-stable `binding_identity`, header, logical system-library capability, resources and release associations, structural symbol contracts, output outcomes, enums, structures, declaration span, and checked provenance. It is a language contract, not a resolved artifact or runtime-library receipt.
- `c_binding_call`: direct binding-symbol call the compiler admitted inside explicit `unsafe:` source. It carries the owning `binding_identity`, and links to the `c_binding` record and, where the checked raw call has a named function owner, that owner through `owner_declaration_id` and `owner_visibility`; it also links to the ordinary `call` record where exported. `unsafe_acknowledged` only describes this call-site acknowledgement.
- `c_binding_facade`: compiler-proven public-function-to-private-bridge relation. It links `facade_declaration_id`, `bridge_declaration_id`, its ordinary checked `call_id`, and the bridge's `raw_call_ids`. It is emitted only for a direct same-module function call whose private target owns checked raw calls; it does not infer a broader safe API.
- `reference`: source-level name references inside declaration bodies, including identifier, field, `self`, and surface-path forms. A successfully resolved reference carries its structured `canonical_identity`, unchanged through imports, aliases, and re-exports. `target_id` additionally points at an emitted declaration record when that exact identity is present in the same export; otherwise it remains `null` without erasing the identity.
- `call`: source-level call expressions inside declaration bodies, including function, method, constructor, and surface-symbol calls. A uniquely selected callable carries its structured `canonical_identity`; failed or ambiguous selection remains identityless. `target_id` is the optional same-export declaration link, not semantic authority.
- `containment`: parent-child relationship between file, module, declaration, import, C binding, reference, or call records.
- `diagnostic`: stable diagnostic code, severity, phase, compiler origin, message, primary span, notes, hints, optional structured expected/actual values, labelled related spans, canonical related declarations, and explain command.

Paths and ids are deterministic for the same compiler version and filesystem layout. The schema does not promise that ids are stable across file moves, symbol renames, or future schema versions; consumers that persist the graph should store the schema version and compiler version with their index.

### Diagnostic records

Diagnostic records appear in tolerant exports produced with `--allow-errors`. They project the same compiler-owned diagnostic facts used by `incan check --format json` and the LSP; consumers should read the structured fields instead of reverse-engineering the human message.

| Field                   | Contract                                                                                                              |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------- |
| `code`                  | Stable public diagnostic identifier used by `incan explain`.                                                          |
| `severity`              | Diagnostic level such as `error`, `warning`, or `hint`.                                                               |
| `phase`                 | Compiler phase that detected the problem, such as parsing or typechecking.                                            |
| `origin`                | Compiler subsystem that produced the fact. Legacy schema-v1 records without this field deserialize as `unknown`.      |
| `message`               | Human-readable summary; not a machine-readable replacement for the other fields.                                      |
| `primary_span`          | Primary source location with inclusive start and exclusive end byte offsets plus 1-based line and column positions.   |
| `notes` and `hints`     | Additional explanation and suggested remedies.                                                                        |
| `expected` and `actual` | Optional structured values or types for comparisons known to the compiler. The fields are omitted when unavailable.   |
| `related_spans`         | Zero or more objects containing a secondary `span` and compiler-owned relationship `label`. Empty arrays are omitted. |
| `related_declarations`  | Zero or more canonical declaration identities plus relationship labels. Provider-owned offsets remain inside the identity and are not projected into the primary file. |
| `explain`               | Exact command that opens the diagnostic's longer explanation.                                                         |
| `provenance`            | `diagnostic` for compiler-owned diagnostic records.                                                                   |
| `degraded`              | Always `true`; the export contains facts recovered in the presence of diagnostics.                                    |

Codegraph spans include byte offsets because graph consumers often anchor directly into source buffers. Preserve the accompanying line and column values for display rather than trying to reconstruct them from bytes under an assumed encoding.
