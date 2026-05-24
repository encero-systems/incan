# RFC 102: Incan Semantic Layer Inspection Surface

- **Status:** Draft
- **Created:** 2026-05-23
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (project lifecycle CLI)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 074 (template rendering and boilerplate provenance)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 077 (workspace and multi-package projects)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 079 (`incan.pub` artifact graph)
    - RFC 080 (AI assets, models, prompts, evals, and agent metadata)
    - RFC 082 (checked API documentation generation)
    - RFC 085 (field metadata and type-shaped constraints)
    - RFC 086 (schema descriptors and adapters)
    - RFC 087 (reusable field contracts and model composition)
    - RFC 092 (interactive runtime stdlib contracts)
    - RFC 096 (declaration metadata blocks)
    - RFC 097 (Rust-hosted Incan caller)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines the Incan Semantic Layer Inspection Surface: a local, versioned, machine-readable project model that joins checked source facts, project lifecycle facts, actions, capabilities, policy outcomes, provenance, artifacts, schema descriptors, AI assets, evals, and agent guidance into one inspectable contract for CLI, LSP, CI, docs tooling, registries, and agents. The goal is not to replace the subsystem RFCs that own those facts; the goal is to make their outputs converge into one semantic layer so tools do not scrape source files, generated Rust, manifests, README conventions, or unrelated command output to understand an Incan project.

## Core model

Read this RFC as nine foundations:

1. **The semantic layer is local first:** the source of truth for project inspection is the local project or workspace, not a remote registry.
2. **Checked source facts and lifecycle facts meet in one model:** compiler-owned facts from RFC 048 and lifecycle-owned facts from RFC 074 through RFC 080 must be joinable through stable identities.
3. **Inspection is a product surface:** `incan inspect` or an equivalent command is a stable interface, not debug output.
4. **LSP is the proving consumer:** editor features should consume the same semantic layer as the CLI, CI, docs tooling, and agents.
5. **Human output is a view:** terminal prose may summarize inspection results, but machine-readable output is the canonical integration contract.
6. **Degraded states are explicit:** incomplete, stale, unsupported, unresolved, blocked, or policy-redacted facts must be represented directly instead of disappearing or being silently guessed.
7. **Agents are not privileged:** agent-facing data is the same data available to IDEs and CI, and agents may propose work but must not approve their own mutations.
8. **Graph explanation is required:** users and tools should be able to ask why a fact, action, artifact, policy outcome, or provenance edge exists.
9. **Subsystem RFCs keep ownership:** this RFC defines the aggregation and inspection contract, not the detailed semantics of templates, capabilities, actions, policy, AI assets, schemas, or registries.

## Motivation

Incan already has many of the ingredients of an intent and semantic layer. RFC 048 defines checked API and model metadata. RFC 074 defines template provenance. RFC 075 defines starters, capabilities, mutation plans, file roles, and agent guidance. RFC 076 defines policy outcomes. RFC 077 defines workspace inspection. RFC 078 defines typed actions. RFC 079 defines registry artifact relationships. RFC 080 defines AI assets and eval metadata. RFC 085, RFC 086, RFC 087, and RFC 096 deepen the model and schema contract. Each of those RFCs is useful on its own, but a tool that wants to understand a real project should not have to compose them through ad hoc command calls and local interpretation.

The strategic risk is fragmentation. Incan can land every subsystem RFC and still fail to expose a coherent semantic layer if the facts remain scattered across separate commands, separate JSON shapes, separate sidecar files, and editor-specific glue. That would weaken the strongest product claim: Incan should be a language and toolchain where humans, compilers, IDEs, CI, documentation generators, registries, and agents can reason from the same project model.

The practical problem appears first in the editor. A useful LSP should be able to show a checked declaration, the schema descriptor behind a model, the capability that created a file, the action that validates it, the policy that blocks a mutation, the generated artifact that depends on it, and the agent guidance that applies. If each of those answers comes from a different subsystem with different identity rules, editor tooling becomes a pile of partial integrations. The same is true for CI checks, documentation tooling, package browsers, and agent workflows.

This RFC therefore makes the integration surface explicit. Incan should provide a local semantic inspection model that lets tools ask: what exists, what does it mean, what can run, what can mutate, what verifies it, what generated it, what depends on it, what policy applies, and what should an agent know before touching it?

## Goals

- Define a canonical local semantic inspection surface for Incan projects and workspaces.
- Define a versioned machine-readable semantic package format that can join compiler facts, project facts, lifecycle facts, and artifact facts.
- Define required stable identity classes for declarations, fields, modules, files, actions, capabilities, policies, generated artifacts, AI assets, evals, and graph edges.
- Define high-level command surfaces such as `incan inspect`, `incan graph explain`, and machine-readable LSP-facing equivalents without requiring exact final flag spelling.
- Define the relationship between RFC 048 checked metadata, RFC 074 template provenance, RFC 075 capabilities, RFC 076 policy, RFC 077 workspaces, RFC 078 actions, RFC 079 artifact graph data, and RFC 080 AI assets.
- Define how degraded, incomplete, unsupported, stale, blocked, and redacted facts are represented.
- Require CLI, LSP, CI, docs tooling, registry tooling, and agents to consume the same semantic facts where their needs overlap.
- Make agent-facing inspection an explicit stable integration target while preserving receiver-owned policy and approval boundaries.

## Non-Goals

- This RFC does not define a new source syntax.
- This RFC does not replace RFC 048 checked metadata, RFC 074 templates, RFC 075 capabilities, RFC 076 policy, RFC 077 workspaces, RFC 078 actions, RFC 079 registry graph semantics, or RFC 080 AI asset semantics.
- This RFC does not require a public `incan.pub` registry to exist before local inspection works.
- This RFC does not require every current or future artifact kind to be implemented before the first inspection surface ships.
- This RFC does not define the full LSP protocol mapping for every editor feature.
- This RFC does not allow agents to bypass policy, approval, sandboxing, or user review.
- This RFC does not require inspection commands to execute project code, run tools, fetch remote schemas, download models, or contact external services.
- This RFC does not make generated artifacts authoritative over checked source or checked metadata.
- This RFC does not standardize an on-disk semantic database format for compiler internals.

## Guide-level explanation

Users should be able to inspect an Incan project as a semantic object, not only as a folder of source files and manifests.

```text
incan inspect --format json
```

The human-readable view might summarize the same model:

```text
Project: checkout-console
Members: 3
Capabilities: cli, testing.basic, schema.adapters
Actions: run, test, validate-schema, docs
Policy: source changes require review; remote AI execution blocked
Generated files: 4 tracked, 1 edited
AI assets: 1 prompt template, 2 eval suites
Warnings: schema adapter output is stale for model OrderSummary
```

The JSON output is the integration contract. A CI check, editor plugin, docs generator, or agent can consume the same data without scraping the terminal text.

An editor can use the same model to power richer project affordances. Hovering a model field may show its checked type, field metadata, reusable field contract provenance, schema overlay facts, generated-doc status, and downstream adapter projections. Selecting a generated file may show which template or capability created it, whether it is bootstrap-owned or managed, and which update policy applies. Opening the command palette may show typed actions with risk and policy labels instead of generic shell scripts.

Users and tools should also be able to ask why a relationship exists:

```text
incan graph explain model:OrderSummary.status
incan graph explain action:validate-schema
incan graph explain artifact:target/schema/order_summary.json
```

Example human-readable explanation:

```text
model:OrderSummary.status
  declared by source model OrderSummary
  imports reusable field contract order_status
  appears in schema overlay WarehouseOrder
  validates generated artifact target/schema/order_summary.json
  affected actions: validate-schema, docs
  policy: source metadata changes require review
```

The same explanation should be available as structured data so LSP, CI, docs tooling, and agents can present it in their own UI.

For agents, the model is a bounded context source. An agent can discover relevant files, capabilities, actions, tests, evals, policy restrictions, and generated artifact provenance before proposing a patch. The agent still cannot approve its own mutation, execute hidden lifecycle hooks, or infer permissions from guidance text.

## Reference-level explanation

### Semantic package

The semantic inspection surface must expose a versioned semantic package. The exact JSON field names are not normative in this Draft, but the package must identify:

- semantic package schema version;
- Incan toolchain version;
- project or workspace root identity;
- selected workspace scope when applicable;
- source snapshot identity when available;
- project manifest facts;
- lockfile and dependency facts when available;
- checked source declarations from RFC 048;
- contract-backed model facts from RFC 048;
- field metadata, reusable field provenance, and schema descriptor facts from RFC 085, RFC 086, RFC 087, and RFC 096 where available;
- file roles, capability status, capability provenance, template provenance, and generated-file ownership from RFC 074 and RFC 075;
- typed actions from RFC 078;
- policy outcomes from RFC 076;
- workspace topology from RFC 077;
- artifact graph and registry relationship facts from RFC 079 when available locally;
- AI asset, prompt, eval, and agent guidance facts from RFC 080 when available;
- diagnostics, warnings, degraded states, and redactions.

The semantic package must not require remote registry access for basic local inspection. Remote or registry-backed facts may appear when they are already available in project state, package artifacts, lockfiles, cached descriptors, or explicitly requested registry queries.

### Command surface

The CLI must provide a project inspection command. The recommended spelling is:

```text
incan inspect --format json
```

The exact final spelling may change, but the command must expose the semantic package in a documented machine-readable format.

The CLI should provide a graph explanation command. The recommended spelling is:

```text
incan graph explain <selector> --format json
```

Selectors should support at least declarations, model fields, files, actions, capabilities, generated artifacts, policy decisions, and AI assets when those objects are present in the semantic package.

Existing subsystem commands such as action listing, capability status, policy checks, workspace inspection, metadata extraction, and template status may continue to exist. Their machine-readable output should either embed compatible semantic package fragments or reference the same stable identities used by the semantic package.

### Stable identities

The semantic package must represent stable identities for objects that other tools need to join. This RFC requires stable identities for at least:

- project and workspace members;
- modules and public declarations;
- model fields and reusable field contracts;
- schema descriptors and overlays;
- source files and generated files;
- templates and template provenance records;
- capabilities and applied capability records;
- actions and action providers;
- policy decisions and risk categories;
- package artifacts and generated artifacts;
- AI assets, prompt templates, evals, datasets, and agent guidance records.

Stable identities must be deterministic for a given source and project state. They must not depend on process memory addresses, nondeterministic traversal order, or human-formatted output.

When an identity cannot be made stable, the semantic package must mark it as unstable or local-only. Tools must not treat unstable identities as durable cross-run anchors.

### Edges

The semantic package must represent relationships as first-class edges where possible. This RFC requires support for these relationship kinds:

- `declares`: source or artifact declares a semantic object;
- `materializes`: contract metadata materializes a model or declaration;
- `generates`: template, capability, action, or adapter generates a file or artifact;
- `validates`: action, test, eval, or policy validates an object;
- `depends-on`: object depends on another object;
- `provided-by`: package, capability, or artifact provides an object;
- `applies-policy`: policy decision applies to an action, mutation, artifact, or source;
- `created-by-capability`: file, action, or metadata originated from a capability;
- `projects-from`: generated schema, docs, or adapter output projects from checked descriptors;
- `guided-by`: agent guidance applies to a file role, capability, action, or project shape.

Implementations may add extension edge kinds. Unknown edge kinds must remain visible in machine-readable output and must not be silently dropped by generic consumers.

### Degraded and partial facts

The semantic package must represent degraded states explicitly. Useful states include:

- `complete`: the fact is fully checked and current;
- `partial`: the fact is present but incomplete;
- `unsupported`: the toolchain knows the object exists but cannot inspect it fully;
- `stale`: the fact was derived from an older source state;
- `blocked`: policy or configuration prevents resolving the fact;
- `redacted`: the fact exists but sensitive content is intentionally hidden;
- `unknown`: the toolchain cannot determine whether the fact exists.

For degraded facts, the package should include a reason code and a human-readable diagnostic where possible. Consumers must not infer absence from a missing optional field when a degraded state is available.

### Policy and approval

Policy outcomes from RFC 076 must be represented in the semantic package when policy is evaluated. Inspection may report policy status without applying mutations or running actions.

Agent guidance, AI assets, action descriptors, template provenance, and capability metadata must not grant approval. The semantic package may help an agent propose a patch or select a workflow, but approval remains governed by RFC 076 and the receiving project.

Sensitive values must follow the redaction rules of the owning subsystem. For example, template parameters marked sensitive must not appear as raw values in inspection output, and remote AI configuration must not expose secrets.

### LSP consumption

The LSP should treat the semantic package as the editor-facing project model where practical. It may cache or request focused views, but it should not reimplement independent logic for capability status, action discovery, policy outcomes, generated-file provenance, schema descriptors, or agent guidance.

Editor features that should consume this surface include:

- project tree grouping by file role and generated-file ownership;
- hover and go-to-definition for checked declarations, aliases, partials, fields, reusable field contracts, schema overlays, and generated artifacts;
- action buttons for typed actions with risk and policy labels;
- diagnostics for stale generated files, blocked policy, unsupported actions, invalid capability state, and stale schema projections;
- code actions for reviewable capability, template, or generated artifact updates;
- agent guidance discovery without executing agents or hidden prompts.

The LSP may expose focused protocol-specific requests rather than returning the full semantic package on every editor operation. Those focused responses must preserve the same identities and degraded-state semantics as the CLI inspection surface.

### CI, docs, registry, and agent consumption

CI tools should be able to consume the semantic package to select typed actions, enforce policy checks, verify generated artifact freshness, run relevant evals, and fail on stale or unsupported project states.

Documentation tooling should be able to consume checked declarations, schema descriptors, contract metadata, capability docs links, generated-file provenance, and artifact relationships from the semantic package instead of parsing source or generated Rust.

Registry and package tooling may consume exported semantic package fragments when publishing packages or building artifact cards, but remote registries must not become the local authority for project mutation.

Agentic tooling may consume the semantic package to identify relevant files, tests, evals, actions, capabilities, and constraints. It must treat policy outcomes, risk categories, and degraded states as binding context for proposal generation.

## Design details

### Relationship to RFC 048

RFC 048 remains the owner of checked API metadata and contract-backed model metadata. This RFC treats RFC 048 facts as compiler-owned source facts inside the larger semantic package.

The semantic package must not weaken RFC 048 by falling back to source-text scraping or generated Rust inspection when checked metadata is available. If checked metadata cannot be produced because the source has parse or type errors, the semantic package must report degraded source facts and diagnostics.

### Relationship to RFC 074 and RFC 075

RFC 074 owns template rendering and provenance. RFC 075 owns starter and capability descriptors, application, mutation planning, file roles, tooling metadata, and agent guidance metadata. This RFC joins their records into the local semantic graph.

Capability and template state must remain explicit project tooling state. The semantic package must not infer that a file is generated merely because it resembles a known template.

### Relationship to RFC 076

RFC 076 owns policy evaluation and approval semantics. This RFC requires policy results to be surfaced through the semantic package, but does not define policy rules.

When policy has not been evaluated for an object, the semantic package must distinguish `not-evaluated` from `allow`. Lack of a policy result must not be treated as permission.

### Relationship to RFC 077

RFC 077 owns workspace topology and scoped mutation planning. This RFC requires semantic inspection to include selected workspace scope and member identity so tools do not accidentally treat whole-workspace facts as single-member facts.

### Relationship to RFC 078

RFC 078 owns typed action semantics, source resolution, execution modes, risk labels, dry-run behavior, and invocation. This RFC requires actions to appear as semantic objects with stable identities and graph edges to inputs, outputs, providers, policy outcomes, evals, and generated artifacts where available.

### Relationship to RFC 079

RFC 079 owns the registry artifact graph. This RFC owns the local project semantic graph. The two graphs should share compatible artifact kinds, relationship vocabulary, and identity references where practical, but the local semantic graph must work without a public registry.

Registry metadata may enrich local inspection, but it must not replace receiver-owned planning, policy, or mutation authority.

### Relationship to RFC 080

RFC 080 owns AI asset metadata, prompt templates, datasets, evals, agent guidance, and local/cloud execution constraints. This RFC requires those facts to appear in inspection output when they are project-relevant and available.

Prompt templates and system messages that affect project behavior must be inspectable as artifacts. Agent guidance must remain descriptive and must not cause implicit agent execution.

### Relationship to RFC 085, RFC 086, RFC 087, and RFC 096

Those RFCs own field metadata, schema descriptors, reusable field contracts, model composition, and declaration metadata blocks. This RFC requires their normalized checked facts and provenance edges to be visible through the semantic package where supported.

Adapter outputs must remain projections of checked descriptors, not source truth. The semantic package should preserve edges from adapter outputs back to descriptor identities when available.

### Relationship to RFC 092 and RFC 097

RFC 092 owns interactive runtime target manifests and host capability contracts. RFC 097 owns the Rust-hosted caller boundary. This RFC allows those emitted manifests, host capability facts, generated Rust-facing artifacts, and caller metadata to appear in the semantic package when available, especially for LSP, docs, CI, and registry inspection.

## Alternatives considered

### Keep subsystem JSON outputs independent

Rejected because it preserves fragmentation. Independent outputs can be useful, but they must share identities and be joinable through a canonical project model.

### Make the LSP the only integration owner

Rejected because CI, docs tooling, registry tooling, and agents need the same facts outside an editor. LSP is the proving consumer, not the source of truth.

### Put the semantic layer in `incan.pub`

Rejected because local projects must remain inspectable without registry access, and local tooling owns receiver-side mutation plans and policy. Registry graph metadata can enrich inspection but must not be required for it.

### Use generated Rust as the inspection source

Rejected because Incan semantics include source-level facts, metadata, provenance, policy, capabilities, and actions that generated Rust either cannot represent or should not be authoritative for.

### Treat agent guidance as separate from normal tooling

Rejected because giving agents a special path would create drift and privilege confusion. Agents should consume the same semantic facts as IDEs and CI, subject to the same policy boundaries.

## Drawbacks

This RFC adds an integration obligation across many subsystems. Each subsystem must preserve identities and enough structured data for the semantic package, which can slow early implementation.

A broad semantic package can become too large or too slow if every command eagerly computes every fact. Implementations will need focused views, lazy computation, or scope selection while preserving the same identity and degraded-state contract.

Versioning the inspection schema creates compatibility work. Once tools and agents depend on the JSON shape, changes need migration discipline.

There is a risk of overpromising if implementation work tries to expose every artifact kind at once. Implementation sequencing should prove the local compiler and lifecycle join while preserving the full 1.0 contract described by this RFC.

## Implementation architecture

This section is non-normative.

A practical implementation shape is to treat the semantic inspection surface as a join over two fact domains:

- compiler facts: modules, declarations, types, contracts, diagnostics, checked metadata, schema descriptors, and stable source identities;
- project facts: manifests, workspaces, lock state, capabilities, actions, templates, generated-file provenance, policy, artifacts, AI assets, and registry-derived local metadata.

The join should happen through stable identities and graph edges rather than by embedding subsystem-specific blobs that consumers must reinterpret. Subsystems may still own their specialized payloads, but the semantic package should expose enough shared fields for generic tooling to navigate the project.

Implementations should support focused queries so LSP and CI can request only the facts they need. Focused query output should remain a semantic package fragment with the same schema version, identity rules, degraded-state model, and edge vocabulary as full inspection output.

## Layers affected

- **Compiler semantic analysis**: must expose checked source facts, diagnostics, stable identities, and degraded states in a form that the semantic package can consume.
- **Project model / lifecycle tooling**: must expose manifest, workspace, lock, capability, action, template, policy, provenance, and AI asset facts through shared identities.
- **CLI / tooling**: must provide machine-readable inspection and graph explanation commands, plus focused views where needed.
- **LSP / IDE tooling**: should consume semantic package facts for project views, hovers, definitions, diagnostics, run actions, generated-file status, policy status, and agent guidance discovery.
- **Docs tooling**: should consume checked declarations, schema descriptors, provenance, and artifact edges from the semantic package where useful.
- **CI / automation**: should consume action, policy, stale-artifact, eval, and degraded-state facts without parsing human output.
- **Registry / package integration**: should map local artifact identities and relationship edges to registry artifact graph metadata when publishing or inspecting packages.
- **Agentic tooling**: may consume the semantic package for context selection and proposal generation, but must respect policy outcomes and approval boundaries.

## Unresolved questions

- Should the canonical command be `incan inspect`, `incan project inspect`, `incan graph inspect`, or another spelling?
- Should graph explanation be a subcommand of inspection, such as `incan inspect explain`, or a separate `incan graph explain` command?
- Which semantic package schema fields are mandatory for the 1.0 north-star contract, and which unsupported domains should appear as explicit degraded facts until their owning RFCs land?
- Which identity formats should be stable across machines, packages, and versions, and which should be explicitly local-only?
- Should focused LSP queries use the same JSON schema directly or a protocol-specific projection that preserves semantic package identities?
- How should semantic package fragments be cached and invalidated without standardizing compiler-internal storage?
- Should exported package artifacts embed a semantic package fragment, or should they embed only RFC 048 metadata plus artifact graph metadata until a later publishing RFC?
- What compatibility policy should apply when an older tool consumes a newer semantic package with unknown object or edge kinds?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
