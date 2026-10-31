# Codegraph inspection

This page specifies the JSONL records `incan inspect codegraph` writes. Its options and `PATH` selection are in [Agent context CLI and MCP](agent_context_cli.md#cli-signatures).

```bash
incan inspect codegraph src/main.incn --format jsonl
incan inspect codegraph . --format jsonl --strict
```

## Stream

- Every line is one JSON object with a `record` discriminator. The first record is the `header`. An export carries the requested project's records; dependency records remain in their published graphs, addressed by the header's `dependencies`. The unit's `diagnostic` records follow its other records; with `--risk`, `risk_signal` records come last.
- The record kinds are `header`, `file`, `namespace`, `module`, `declaration`, `import`, `export`, `reference`, `call`, `containment`, `registry`, `capability`, `c_binding`, `c_binding_call`, `c_binding_facade` and `diagnostic`, and with `--risk`, `risk_signal`.
- The records and their order are deterministic for the same compiler version, source and filesystem layout. An `id` is unique within one export. Ids are not stable across file moves, renames or schema versions.

## Modes and degraded state

| `mode` | Selected by | Source with an error |
| --- | --- | --- |
| `strict` | `--strict` | Fails the command. |
| `allow_errors` | The default. | Parseable units still produce records; stderr names unchecked units. |

- An unchecked unit contributes syntax facts without resolved reference or call edges. Checked units in the same selection retain their checked facts.
- The header's `degraded` is `true` when any selected unit failed checking.
- A source that checks with warnings produces no `diagnostic` records in either mode.

## Common fields

| Field | On | Contents |
| --- | --- | --- |
| `record` | Every record | The record kind. |
| `degraded` | Every record | Whether the record belongs to a partial graph recovered from diagnostics. |
| `id` | Every record but the header | Export-local id. |
| `language` | Every record but the header | `"incan"` or `"rust"`. |
| `provenance` | Every record but the header | Where the fact came from: `source` (source text or filesystem shape), `syntax` (parsed syntax), `checked` (checked compiler facts), `diagnostic` (compiler diagnostics) or, on `risk_signal`, `process_observed`. |
| `span` | Source-backed records | A source span, or `null` when none applies. |

`language` identifies the source language of the represented fact. Rust declarations and their checked relationships can appear alongside Incan declarations.

A source span has `file`, an inclusive `start` and an exclusive `end` byte offset, and 1-based `start_line`, `start_column`, `end_line` and `end_column`. Columns count Unicode scalar values.

## Header

| Field | Contents |
| --- | --- |
| `schema_version` | `9`. |
| `compiler_version` | The producing compiler's version. |
| `mode` | `strict` or `allow_errors`. |
| `root_path` | The requested path. |
| `languages` | The languages represented in the selection. |
| `package` | `name`, `version` and `root_path` from the nearest `loaf.toml`, each `null` when absent; `null` without a manifest. |
| `semantic_contexts` | The selected projects and Loaves. |
| `degraded` | See [Modes and degraded state](#modes-and-degraded-state). |
| `identity_schema` | `incan.graph.identity/2`. |
| `declaration_digests` | See [Export identity and snapshots](agent_context_cli.md#export-identity-and-snapshots). |
| `snapshot_id` | See [Export identity and snapshots](agent_context_cli.md#export-identity-and-snapshots). |
| `dependencies` | Array of dependency graph references, each with `id` (`graph:sha256:` followed by the graph's byte digest), `graph` (artifact path), `root_path` (source anchor root), `package` and `version` (empty strings when unavailable). Dependency payloads are absent from this export. |

A semantic context has `project_root` and `loaves`. Each Loaf has `key`, `name`, `version`, `standard` and a `sha256:` artifact `digest`. Consumer identities, snapshots, context packs and optional process records are specified in [Agent context CLI and MCP](agent_context_cli.md).

When a unit is refused before the compiler can export any facts, its presentation header retains request metadata and dependency addresses. Compiler-owned fields, including `schema_version`, `compiler_version` and `package`, are absent in that header.

## Files, namespaces and modules

| Record | Fields |
| --- | --- |
| `file` | `path`, `size_bytes`. Provenance `source`. |
| `namespace` | `namespace_path`, and `name`: the last path segment, or `crate` for the empty path. Provenance `syntax`. |
| `module` | `file_id`, `module_path`, `namespace_path`, `internal`, `name`, `span`. Provenance `syntax`. |

A module path segment that starts with one underscore, and not two, marks a module internal. A module's `namespace_path` is its `module_path` up to its first internal segment, and `internal` is `true` when there is one. One `namespace` record is written per distinct `namespace_path`.

## Declarations, imports and exports

`declaration`: a top-level declaration.

| Field | Contents |
| --- | --- |
| `module_id` | The owning module record. |
| `kind` | `function`, `model`, `class`, `trait`, `enum`, `newtype`, `rusttype`, `type_alias`, `const`, `static`, `alias`, `partial`, `capability` or `test_module`. |
| `name`, `visibility` | The source name, and `public` or `private`. |
| `type_params` | Generic parameter names. |
| `signature` | A rendered signature, or `null`. |
| `canonical_identity`, `stable_identity` | See [Identities](#identities). |
| `semantic_digest` | A `sha256:` digest of checked executable facts; omitted when not proven. It does not guarantee stability across position-only source edits. |
| `doc_digest` | A `sha256:` digest of the declaration's documentation; omitted when there is none. |
| `span` | The declaration's source span. |

Provenance is `checked` when `canonical_identity` is present, else `syntax`.

`import`: one import declaration.

| Field | Contents |
| --- | --- |
| `module_id` | The importing module record. |
| `kind` | `module`, `from`, `pub_library`, `pub_from`, `python`, `rust_crate` or `rust_from`. |
| `path`, `items`, `alias` | The imported path, the imported item names, and the top-level alias or `null`. |
| `bindings` | One entry per local binding in source order: `local_name`, `canonical_identity` (or `null`) and `stable_identity` (when proven). |
| `visibility`, `span` | `public` or `private`, and the import's source span. |

Provenance is `checked` when a binding carries a `canonical_identity`, else `syntax`.

`export`: a public name of a module.

| Field | Contents |
| --- | --- |
| `module_id` | The exporting module record. |
| `name` | The public name. |
| `kind`, `source_id` | `declaration` or `import`, and the record it comes from. |
| `canonical_identity`, `stable_identity` | The exported declaration's identities; an alias or re-export keeps the original declaration's identities. |
| `span` | The export's source span. |

## References and calls

`reference`: a name reference inside a declaration body, or a Rust item a `rust::` import names.

| Field | Contents |
| --- | --- |
| `module_id`, `owner_id` | The module record, and the containing declaration record or `null`. |
| `name` | The referenced spelling. |
| `kind` | `identifier`, `field`, `self`, `surface_path`, `type`, `imported_module` or `imported_item`. |
| `target_id` | The target's declaration ID, in this export or a referenced dependency graph, or `null`. |
| `canonical_identity` | The resolved target's identity, unchanged through imports, aliases and re-exports; `null` when resolution proves none. |
| `canonical_owner` | The closest checked declaring owner of the target; omitted when none is proven. |
| `stable_identity` | See [Identities](#identities). |
| `span` | The reference's source span. |

A `type` reference is a type the compiler proved at an expression's span. An expression whose type names several declarations produces one `type` reference per named declaration, all at that span.

`call`: a call expression inside a declaration body.

| Field | Contents |
| --- | --- |
| `module_id`, `owner_id` | The module record, and the containing declaration record or `null`. |
| `callee` | The callee spelling. |
| `kind` | `function`, `method`, `constructor` or `surface_symbol`. |
| `argument_count`, `type_argument_count` | The value and explicit type arguments at the call site. |
| `target_id`, `canonical_identity`, `canonical_owner`, `stable_identity` | As for `reference`, for the uniquely selected callable. A failed or ambiguous selection has no identity. |
| `span` | The call's source span. |

Provenance of a `reference` or `call` is `checked` when `canonical_identity` is present, else `syntax`.

## Containment

`containment`: `parent_id`, `child_id`, `kind` and `span`.

| `kind` | Parent | Child |
| --- | --- | --- |
| `namespace_contains_module` | `namespace` | `module` |
| `file_contains_module` | `file` | `module` |
| `module_contains_import` | `module` | `import` |
| `module_contains_declaration` | `module` | `declaration` |
| `module_contains_c_binding` | `module` | `c_binding` |
| `declaration_contains_reference` | `declaration` | `reference` |
| `declaration_contains_call` | `declaration` | `call` |
| `import_contains_reference` | `import` | `reference` of a Rust item |

## Registries and capabilities

`registry`: one checked typed-registry entry. Provenance `checked`.

| Field | Contents |
| --- | --- |
| `module_id` | The module that owns the registration. |
| `registry_identity`, `registry_public` | The canonical registry identity, and whether the registry is public to package consumers. |
| `key`, `descriptor` | The checked key and descriptor values. |
| `subject_kind`, `subject_identity` | `function`, `method`, `compilation_unit` or `package`, and the subject's canonical identity. |
| `registration_span`, `subject_span` | The `@describe` or `RegistryEntry` registration, and the subject declaration or expression. |
| `reexport_paths` | Each public import that re-exports the entry: its `path` and `span`. Omitted when empty. |

A registry value is an object with a `kind` and a `value`:

| `kind` | `value` |
| --- | --- |
| `int`, `bool`, `string` | The literal. |
| `float` | The literal's spelling, as a string. |
| `bytes` | The bytes as an array of integers. |
| `none` | `null`. |
| `type` | The type's spelling. |
| `const_ref` | The const's path segments. |
| `option` | A registry value. |
| `list` | An array of registry values. |
| `dict` | An array of entries, each a `key` and a `value` registry value. |
| `newtype` | The newtype's `name` and its wrapped registry `value`. |
| `model` | The model's `name` and its `fields`, each a `name` and a registry `value`. |

A re-export adds a `reexport_paths` entry; it does not add a `registry` record or change the subject. A `registry` record describes the checked declaration, not a loaded runtime registry.

`capability`: one checked capability declaration. Provenance `checked`.

| Field | Contents |
| --- | --- |
| `module_id` | The declaring module record. |
| `capability_identity`, `name`, `public` | The canonical identity, the declared name, and whether it is public to package consumers. |
| `description` | The `description` clause; omitted when absent. |
| `scope` | Each scope dimension's `name` and declared `ty`, in declaration order; omitted when empty. |
| `requires` | The canonical identities of the capabilities it requires, in declaration order; omitted when empty. |
| `span` | The declaration span; omitted when absent. |

## C bindings

`c_binding`: one checked `binding` declaration. Provenance `checked`.

| Field | Contents |
| --- | --- |
| `module_id`, `declaration_id` | The owning module record, and the binding's ordinary `class` declaration record. |
| `name` | The binding name visible to Incan source. |
| `binding_identity` | A digest of the checked binding descriptor. It excludes source spans and file locations, includes the declared header spelling, and changes with every ABI-affecting descriptor field. |
| `header`, `system_library`, `link_capability` | The declared header, the logical native link name, and `system_library` or `framework`. |
| `resources`, `symbols`, `enums`, `structs` | The declared members, with the shapes in the [binding inspection schema](binding_inspection_schema.md#binding-fields) and its [structural types](binding_inspection_schema.md#structural-types). |
| `span` | The binding declaration's source span. |

`c_binding_call`: one direct binding-symbol call inside an `unsafe:` block. Provenance `checked`.

| Field | Contents |
| --- | --- |
| `module_id`, `call_id` | The module record, and the ordinary `call` record of the expression or `null`. |
| `binding_id`, `binding_identity`, `binding`, `symbol` | The `c_binding` record, its identity, the binding name and the binding-local symbol name. |
| `owner_declaration_id`, `owner_visibility` | The named function that makes the call and its visibility; omitted when the call has no named owner. |
| `unsafe_acknowledged` | `true`: the call is inside an `unsafe:` block. It describes this call site only. |
| `span` | The call expression's source span. |

`c_binding_facade`: a public function that directly calls a private function of the same module, where the private function makes checked binding-symbol calls. Provenance `checked`.

| Field | Contents |
| --- | --- |
| `module_id` | The module that declares both functions. |
| `facade_declaration_id`, `bridge_declaration_id` | The public function and the private function. |
| `call_id` | The `call` record of the public function's call, or `null`. |
| `raw_call_ids` | The private function's `c_binding_call` records. |
| `span` | The call's source span. |

No `c_binding_facade` record is written for a transitive, imported, re-exported, method or syntax-only relation.

## Identities

`canonical_identity` is the compiler's identity of a declaration:

| Field | Contents |
| --- | --- |
| `namespace` | `ordinary_lexical`, `member` or `module_path`. |
| `origin` | `kind` `module` with `path`; `package` with `library` and `module_path`; `rust_crate` with `path`; or `builtin`. |
| `declaration_name`, `declaration_kind` | The name at the declaration site and the declaration category. |
| `scope_discriminant` | An index for a binding below module level, or `null`. |
| `declaration_span` | The declaration's `start` and `end` byte offsets. |

A record with a proven identity carries it even when this export holds no `declaration` record for it, such as a declaration of a published package, a Rust crate or a builtin. `target_id` links to a `declaration` record of this export when one exists for that identity; it is not an identity, and a checked identity can come with `target_id: null`. A syntax-only or ambiguous fact has no identity.

`stable_identity` identifies the same declaration without positions. It appears on `declaration`, import `bindings`, `export`, `reference` and `call` records, and is omitted when not proven. It has `namespace`, `origin`, `declaration_name` and `declaration_kind` as above, a `location`, and a canonical `signature` that tells apart declarations sharing the other fields; `signature` is omitted when not proven.

Checked Incan callable signatures use `incan.source-signature/1:sha256:` followed by 64 hexadecimal digits. The signature includes checked source types, generic binder positions, parameter kinds, default presence, mutation markers and asyncness. Nominal references retain their declaring identities, and methods retain their nominal owners. Comments, declaration coordinates and native representations do not enter this signature. An unresolved source type leaves the signature unproved. Frozen dependency graphs retain the signature profile recorded in their publication; different profile values produce different declaration keys.

`location` is `{"kind":"module_level"}` for a declaration unique at module, package, crate or registry level, or a `nested` location that names the nearest named owner's stable identity and a zero-based `binding_ordinal` among same-name, same-kind bindings in that owner:

```json
{"kind":"module_level"}
{"kind":"nested","owner":{"namespace":"ordinary_lexical","origin":{"kind":"module","path":["main"]},"declaration_name":"outer","declaration_kind":"function","location":{"kind":"module_level"},"signature":"incan.source-signature/1:sha256:b86ba7daa65b2985370b641aacbe31c524be2687b9fd8641a905e07fbb7afe0b"},"binding_ordinal":0}
```

Inserting or reordering an indistinguishable binding in the owner changes the ordinals after it. Comments, unrelated names and scope-table indices do not change a stable identity.

## Diagnostic records

`diagnostic` records appear in `allow_errors` exports and carry the diagnostics `incan check --format json` reports for the same source.

| Field | Contents |
| --- | --- |
| `code` | The diagnostic code. |
| `severity` | `error`, `warning` or `hint`. |
| `phase` | The pipeline phase that found the problem. |
| `origin` | The compiler subsystem that produced the diagnostic. |
| `message` | A human-readable summary. |
| `primary_span` | A source span. |
| `notes`, `hints` | Explanations and suggested remedies. |
| `expected`, `actual` | The compared values or types; omitted when the diagnostic compares none. |
| `related_spans` | Each secondary `span` with its `label`; omitted when empty. |
| `related_declarations` | Each related declaration's canonical `identity` with its `label`; omitted when empty. The identity keeps its own declaration span. |
| `explain` | Always present; the string `incan explain CODE`, with the diagnostic's code. |
| `provenance` | `diagnostic`. |
| `degraded` | `true`. |

## See also

- [Retain CI evidence](../how-to/ci_and_automation.md#retain-evidence)
- [Inspect checked C bindings](../how-to/inspect_checked_c_bindings.md)
