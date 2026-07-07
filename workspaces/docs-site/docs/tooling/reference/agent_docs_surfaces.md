# Agent and tooling documentation surfaces

Incan documentation should be useful to humans, CI systems, editors, and coding agents, so the public contract should be structured docs, schema-versioned command output, and explicit current/planned labels rather than "read the terminal output and guess."

## Current public surfaces

| Surface                                                            | Status  | Use                                                                                             |
| ------------------------------------------------------------------ | ------- | ----------------------------------------------------------------------------------------------- |
| `llms.txt`                                                         | Current | Short machine-readable entrypoint list for language models and agent tools.                     |
| [Feature inventory](../../language/reference/feature_inventory.md) | Current | Generated index of public features, status, examples, and references.                           |
| `incan check --format json`                                        | Current | Stable diagnostics with codes, severity, phase, spans, notes, hints, and `incan explain` hooks. |
| `incan explain --format json`                                      | Current | Machine-readable diagnostic explanation records.                                                |
| `incan build --report json`                                        | Current | Build and artifact metadata for successful builds.                                              |
| `incan inspect rust --format json`                                 | Current | Current backend output inspection. Generated Rust is inspectable output, not a stable ABI.      |
| `incan inspect codegraph --format jsonl`                           | Current | Source-structure graph facts with provenance and degraded-state records.                        |
| [Checked API metadata](checked_api_metadata.md)                    | Current | Checked public API JSON for docs, package browsers, editors, and tooling.                       |
| [Checked contract metadata](contract_metadata.md)                  | Current | Model bundle metadata and materialization facts.                                                |

## Planned or incomplete surfaces

| Surface                                                                           | Status  | Required next step                                                                                                               |
| --------------------------------------------------------------------------------- | ------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Dedicated JSON Schema files for diagnostics, build reports, and codegraph records | Planned | Publish schema files beside the docs and link them from command references.                                                      |
| Markdown export links on every docs page                                          | Planned | Add a docs-site convention or build hook that exposes source Markdown URLs consistently.                                         |
| Current/planned labels across all feature docs                                    | Partial | Make every feature page state whether it is current, beta, experimental, planned, or deferred.                                   |
| Full semantic inspection database                                                 | Planned | Keep RFC 102/RFC 106 work separate from the narrower 0.4 JSON surfaces.                                                          |
| Rust graph records in codegraph output                                            | Planned | Add first-class Rust records when the compiler owns those facts instead of asking consumers to infer them from generated source. |

## Schema-version rule

Machine-readable outputs should include a schema version and enough compiler/build identity for consumers to decide whether they can trust the record. Consumers should prefer:

- explicit `schema_version` fields;
- compiler version and package identity fields;
- source-file breadcrumbs and spans;
- provenance fields;
- degraded-state markers;
- diagnostic records over partial silent failure.

If a page documents a JSON or JSONL output but no formal JSON Schema file exists yet, the page should say so. That is better than implying a stronger contract than the toolchain currently provides.

## `llms.txt`

The site-level `llms.txt` file is intentionally short. It should point agents at stable entrypoints rather than trying to duplicate the whole documentation site. Keep it focused on:

- what Incan is for;
- install and first-contact docs;
- language and CLI references;
- Python and Rust comparisons;
- feature inventory;
- diagnostics, build reports, and codegraph inspection;
- the roadmap and 1.0 public contracts.

## Do not scrape generated Rust as the semantic API

Generated Rust is useful for debugging the current backend. It is not the durable public compatibility boundary. Agents and tools should prefer Incan source, manifests, checked metadata, CLI report schemas, semantic facts, package metadata, and documented interop contracts.

## Related docs

- [CLI reference](cli_reference.md)
- [Codegraph inspection](codegraph_inspection.md)
- [Checked API metadata](checked_api_metadata.md)
- [Checked contract metadata](contract_metadata.md)
- [1.0 public contracts](../../start_here/public_contracts.md)
