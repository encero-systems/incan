# RFC 106: Compiler-backed agent context graph

- **Status:** Draft
- **Created:** 2026-05-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 015 (project lifecycle and CLI tooling)
    - RFC 031 (library system phase 1)
    - RFC 048 (checked contract metadata, Incan emit, and interrogation tooling)
    - RFC 077 (workspace and multi-package projects)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 080 (AI assets, models, prompts, evals, and agent metadata)
    - RFC 082 (checked API documentation generation)
    - RFC 096 (declaration metadata blocks)
    - RFC 105 (architect rule engine)
- **Issue:** —
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines an Incan-owned agent context graph: a deterministic, compiler-backed graph of package, workspace, module, declaration, import, API, body-level, reference, diagnostic, process-risk, and architecture-advice facts that agents and developer tools can query directly instead of rediscovering source structure through repeated grep/read loops. The graph is not a runtime `std.graph` value and is not a dependency on any single external codegraph product; it is a stable Incan tooling contract with checked provenance, tolerant work-in-progress export, compact context packing, LSP integration, and MCP/CLI surfaces designed for agentic development.

## Core model

1. **The compiler owns facts:** Incan graph facts should come from the parser, module resolver, typechecker, RFC 048 checked API metadata, manifests, and package/workspace metadata rather than from a generic syntax scraper.
2. **Incan specialization is a feature:** the graph optimizes for Incan source, stdlib, manifests, diagnostics, and checked metadata rather than cross-language breadth. Generic adapters may consume or export the graph, but the native model may include Incan-specific facts such as match dispatch, call sites, reference sites, declaration metadata blocks, and checked API contracts.
3. **Graph consumers are separate:** storage engines, embeddings, MCP daemons, visualizers, hosted search, LSP features, architecture-advice tooling, and external CodeGraph-style tools consume the graph; they do not define Incan source semantics.
4. **Provenance is first-class:** every fact must identify whether it is checked, unchecked, syntax-derived, manifest-derived, generated, external, derived-advisory, or runtime-observed so agents can reason about trust.
5. **Broken code remains navigable:** the graph must support a tolerant source-graph mode that emits available facts from parseable modules even when semantic checking fails.
6. **Risk is evidence, not vibes:** process signals such as churn, ownership concentration, co-change, coverage gaps, stale decisions, and recent volatility may influence agent context and architecture advice, but they must be deterministic records with cited evidence and visible limitations.
7. **Task context is a product surface:** the primary agent workflow is not raw graph browsing; it is task-ranked, budgeted context packing that returns the most useful symbols, relationships, diagnostics, risk signals, architecture signals, and source anchors for a concrete change.
8. **LSP and CLI share one fact model:** live editor features, command-line graph export, MCP context tools, and architecture-advice tools should reuse the same source-analysis substrate instead of building parallel extractors with different semantics.
9. **Identity is durable:** nodes, edges, modules, files, and context packs should have stable content-addressed identities so stale data, cache keys, feedback expiry, graph diffs, and audit output are mechanically checkable.
10. **Compact output matters:** graph output intended for agents must be token-aware and deterministic, with a compact textual format in addition to JSON/JSONL.
11. **Evaluation is part of the feature:** Incan should define a small benchmark harness for agent-context quality so claims are measured on Incan packages and workspaces rather than inherited from generic tool marketing.

## Motivation

Agents working in an Incan package currently reconstruct codebase shape by searching file names, grepping symbols, reading source, following imports by hand, and retrying when the first path misses. This wastes tokens and makes behavior sensitive to the model's exploration choices. It is also worst exactly when the project is large, modular, generated in part, or mid-refactor.

Incan already has information that generic code-indexing tools do not: parsed modules, checked declarations, resolved imports, package manifests, stdlib manifests, Rust interop manifests, RFC 048 checked API metadata, declaration metadata, diagnostics, and generated artifact metadata. Letting external tools rediscover that structure from `.incn` text would be slower and less trustworthy than exposing it directly.

Recent code-context tools show that the useful layer is moving beyond keyword search. Aider's repository map summarizes definitions and uses graph ranking to fit important symbols into a token budget. CodeGraph and similar MCP tools pre-index symbol relationships so agents can query structure rather than scan files. Codebase-memory and GitNexus expose persistent knowledge graphs through MCP with impact analysis, architecture summaries, and graph queries. Knowing adds content-addressed graph identity, task-ranked context packing, feedback expiry, and Merkle-style integrity claims. Sourcegraph's SCIP demonstrates the value of language-agnostic code-intelligence interchange for definitions and references. LSP and LSIF demonstrate the editor/code-navigation lineage. Repomix demonstrates the opposite baseline: full-repo packing is useful for portability but too blunt as the only agent context mechanism.

Those systems are useful prior art, but Incan should not copy them wholesale. Generic tree-sitter systems optimize for broad language coverage and approximate relationships. Incan can provide narrower but higher-trust facts for Incan source, including checked public API structure and source-compatible diagnostics. The best Incan solution should therefore combine compiler authority with agent-native retrieval and packaging.

The same graph also benefits non-agent developer workflows. The language server already has live source positions, diagnostics, and semantic navigation needs; architecture-advice tooling needs stable evidence for repeated dispatch, call patterns, references, and public API pressure. If those tools each reimplement source discovery, they will drift. A shared graph contract lets editor features, agent context, and architecture review use the same facts with different latency and presentation requirements.

Repowise adds a useful adjacent lesson for architecture tooling: structural source metrics are not the whole story. Its code-health layer combines AST-derived complexity, duplication, coverage, git ownership, churn, hidden coupling, and decision records, and its public benchmark reports statistically significant relationships between health scores and future bug-fix touches. The benchmark also reports that controlling for file size weakens the relationship, which is exactly the kind of caveat Incan should preserve. Incan should use deterministic risk signals as explainable evidence for prioritization, not as opaque bug prophecy.

## Goals

- Define a stable graph fact model for Incan packages and workspaces.
- Expose compiler-backed source, module, declaration, import, public API, reference, call, diagnostic, manifest, and artifact facts where available.
- Expose Incan-specific body facts such as match dispatch, call sites, reference sites, metadata annotations, and derived architecture signals where available.
- Expose optional process-risk facts such as churn, ownership, co-change, coverage, decision staleness, and trend signals when the required local inputs are available.
- Include provenance, confidence, source span, package identity, module path, and graph-schema version on graph facts.
- Support strict checked export and tolerant work-in-progress export.
- Define CLI and MCP surfaces for raw graph exploration and task-ranked context packing.
- Make the same graph facts available to LSP/editor features and architecture-advice tooling without requiring separate extraction logic.
- Define a compact, deterministic agent context format alongside JSON and JSONL.
- Define content-addressed identities for graph objects and graph snapshots.
- Allow optional feedback and task-memory signals while requiring automatic staleness/expiry when the underlying graph changes.
- Support stdlib, package, workspace, and generated-artifact exploration.
- Keep external storage, embeddings, hosted services, visualization, and product-specific agent integrations outside the compiler boundary.
- Define evaluation expectations for retrieval quality, token efficiency, freshness, determinism, and failure modes on Incan code.

## Non-Goals

- Adding a runtime `std.graph` data structure. Runtime graph collections are a separate concern from tooling graph facts.
- Making Incan depend on CodeGraph, Knowing, GitNexus, codebase-memory, Sourcegraph, SurrealDB, SQLite, Neo4j, KuzuDB, or any single storage/query engine.
- Requiring embeddings, vector search, hosted indexing, or remote inference.
- Guaranteeing that an agent will make correct edits after receiving graph context.
- Claiming defect prediction or risk prioritization without Incan-specific evaluation, visible controls, and caveats.
- Standardizing every possible edge kind in the first release.
- Replacing LSP. The agent context graph complements editor interactions rather than replacing live language-server diagnostics and completions.
- Matching generic cross-language indexers on language count. Incan may support adapters, but the native value is language-specific precision.
- Freezing implementation crate names, CLI subcommand spelling, or MCP tool names before the Draft is accepted.
- Indexing arbitrary non-Incan languages. Mixed-language workspaces may expose foreign artifacts through future adapters, but this RFC's normative scope is Incan source and Incan-owned metadata.

## Guide-level explanation

### Exporting a graph

A developer can export graph facts for a source file, package, workspace, or directory:

```bash
incan inspect codegraph src --format jsonl
```

By default, export is checked. If the project contains semantic errors, the command fails with diagnostics rather than pretending the graph is fully trusted.

During active development, the same developer can request a tolerant graph:

```bash
incan inspect codegraph src --format jsonl --allow-errors
```

In tolerant mode, parseable modules still produce package, file, module, declaration, import, and source-span facts. Facts that require successful semantic checking are omitted or marked with lower provenance. Diagnostics become graph facts so an agent can see why the graph is partial.

The v0.4 implementation is this checked/tolerant JSONL export only. It does not include MCP serving, task-ranked context packing, process-risk scoring, architecture findings, or first-class Rust records yet; those are consumers or follow-up graph layers that build on the same schema contract.

### Exploring graph structure

A human or agent can ask direct graph questions:

```bash
incan tools agent-graph summary
incan tools agent-graph find "Base32"
incan tools agent-graph module "encoding"
incan tools agent-graph neighbors "encoding::base32::decode"
```

These commands are useful for debugging the graph and for deterministic low-level exploration, but they are not the main agent path.

### Asking for task context

The main workflow is task-ranked context:

```bash
incan tools agent-context pack --task "add strict padding validation to base32 decoding" --budget 8000 --format compact
```

The output should include a ranked, token-budgeted context pack: relevant declarations, modules, imports, call/reference neighborhoods, checked signatures, diagnostics, tests when known, and source anchors. The pack should be deterministic for the same graph snapshot, task string, budget, and ranking configuration.

An MCP client sees the same capability through tools such as:

```text
context_for_task(task, budget, project_id)
context_for_files(files, budget, project_id)
explain_context(task, node_id, project_id)
graph_neighbors(node_id, project_id)
graph_summary(project_id)
```

### Using graph-backed IDE and architecture tools

Editor and architecture-advice features can use the same graph facts without changing the export format. An LSP feature may ask for the current graph neighborhood around a cursor position to power richer hover, references, impact hints, stale-index warnings, or code actions. Architecture-advice tooling may consume body-level facts such as match dispatch and call sites, then emit `architecture_finding` records that cite the exact graph facts and source spans that caused them.

Those derived findings should not pretend to be compiler errors. They are advisory records with evidence links back to checked or unchecked graph facts. That distinction matters for agents: an agent can treat a checked signature differently from an architecture smell, and it can inspect the evidence trail before editing code.

Risk-aware tools can layer `risk_signal` records onto the same evidence model. A risk signal is prioritization evidence, not a recommendation by itself. For example, a finding may say that a dispatcher is broad, frequently edited, weakly covered, and co-changes with a module that has no import edge. That is more useful than a generic “complex file” label because the agent can see exactly which structural, git, and coverage facts contributed to the recommendation.

The architecture-advice split is therefore explicit:

- `architecture_finding`: a specific advisory claim such as repeated dispatch, duplicated registry data, wrong-layer policy, public API drift, or hidden initialization behavior. It answers what design pressure is visible and cites graph evidence.
- `risk_signal`: deterministic context such as churn, ownership spread, coverage gaps, co-change, decision staleness, or volatility. It answers where attention may be most valuable and exposes raw measures and caveats.

An `architecture_finding` may reference nearby `risk_signal` records to explain priority, but risk must not replace the finding. Conversely, a high-risk file with no architectural evidence should remain a risk observation rather than an architecture recommendation.

### Using feedback

An agent may report that a symbol was useful for a task. That feedback can influence later ranking for similar tasks, but it must be tied to the graph snapshot or affected package root. If the module, package, or relevant graph neighborhood changes, stale feedback must stop applying automatically.

### External tools

External tools may ingest the exported graph. For example, an external CodeGraph-style importer can map Incan JSONL into its own node and edge tables. That is an integration path, not the source of truth. Incan remains responsible for the stable wire schema and provenance semantics.

## Reference-level explanation

### Graph document

An Incan agent graph document must declare:

- `schema_version`
- package or workspace identity when available
- languages represented by the export
- source root and path normalization mode
- graph generation mode: `checked` or `allow-errors`
- toolchain version
- graph snapshot identity when available
- records containing nodes, edges, diagnostics, and metadata

The document must be deterministic for equivalent inputs under the same toolchain version, ignoring timestamps unless explicitly requested. Every source-backed graph fact record should carry an explicit language, provenance tier, source identity where applicable, and degraded-state flag.

### Node kinds

The graph must support at least these node kinds:

- `package`
- `workspace`
- `file`
- `module`
- `declaration`
- `api_member`
- `import`
- `external`
- `diagnostic`

The graph should support these node kinds as the compiler exposes enough information:

- `callable`
- `method`
- `field`
- `enum_variant`
- `trait_requirement`
- `reference_site`
- `call_site`
- `match_dispatch`
- `dispatch_pattern`
- `test`
- `artifact`
- `generated_source`
- `rust_item`
- `stdlib_item`
- `architecture_finding`
- `risk_signal`
- `coverage_report`
- `decision_record`

Unknown node kinds must remain visible to consumers as opaque node records rather than causing a parse failure.

### Edge kinds

The graph must support at least these edge kinds:

- `contains`
- `defines`
- `imports`
- `references`
- `documents`
- `diagnoses`

The graph should support these edge kinds when available:

- `calls`
- `implements`
- `requires`
- `exports`
- `aliases`
- `overrides`
- `tests`
- `generated_from`
- `materializes`
- `uses_rust_item`
- `uses_stdlib_item`
- `dispatches_on`
- `matches_pattern`
- `evidences`
- `co_changes_with`
- `owned_by`
- `covered_by`
- `governed_by`

Unknown edge kinds must remain visible to consumers as opaque edge records rather than causing a parse failure.

### Provenance tiers

Every node and edge must carry a provenance tier. The minimum tiers are:

- `compiler_checked`: fact derived from successfully checked Incan source or checked metadata.
- `compiler_unchecked`: fact derived from parseable source in tolerant mode where semantic checking failed or was intentionally skipped.
- `syntax_inferred`: fact derived from syntax shape without full semantic confirmation.
- `manifest_declared`: fact derived from manifests, library manifests, lockfiles, or workspace descriptors.
- `generated`: fact derived from generated source or compiler materialization metadata.
- `external`: fact names a target outside the current graph.
- `derived_advisory`: fact derived from graph analysis rather than directly from the compiler, such as an architecture recommendation or impact finding.
- `process_observed`: fact derived from local process artifacts such as git history, coverage reports, issue/commit metadata, or explicit decision records.

Future implementations may add tiers such as `runtime_observed`, `lsp_resolved`, or `scip_imported`, but they must not collapse into `compiler_checked` unless the Incan compiler itself validated the fact.

### Identity

Graph object identity should be content-addressed. A node identity should include enough logical information to distinguish package, module path, stable declaration anchor, node kind, and relevant source identity. An edge identity should include source node identity, target node identity, edge kind, and provenance tier. A diagnostic identity should include diagnostic code, affected source span, module path, and message-stable details.

Physical file movement should not unnecessarily destroy identity for declarations that retain the same package, module, and stable anchor, but the RFC leaves exact hash fields to implementation design until the Draft advances.

### Checked and tolerant export

Strict export must fail when the graph would otherwise imply checked semantic facts from an invalid package. Tolerant export may continue after parseable module errors, but it must mark unchecked facts clearly and must not emit checked API members, checked signatures, resolved call edges, or resolved reference edges for modules that failed the required semantic phase.

Diagnostics produced during tolerant export should be emitted as graph records so tools can explain why the graph is partial.

### LSP and architecture consumers

The graph schema must be suitable for live editor and advisory consumers. Nodes and edges that point into source must carry enough range data for an LSP client to map a graph fact back to a document location. A graph consumer may operate against an in-memory live snapshot, a persisted snapshot, or an exported JSONL document, but fact meaning and provenance must be the same across those modes.

LSP features should consume graph facts through shared analysis services rather than defining a separate graph extractor. The language server may use live incremental state to avoid full export on every edit, but any graph records it exposes to external tools must still include schema version, provenance, source identity, and staleness information.

Architecture-advice tools may create `architecture_finding` records from graph facts. An `architecture_finding` should cite evidence nodes or edges with `evidences` relationships and should preserve the provenance of the underlying facts. For example, a repeated match-dispatch finding can cite the match-dispatch nodes, dispatch patterns, call sites, and source spans that caused the finding. If the underlying body facts are unchecked because the package is in tolerant mode, the derived finding must also be treated as unchecked advisory output.

Process-risk facts should be represented as `risk_signal` records. They may be attached to files, modules, declarations, tests, or architecture findings. They must identify their input source, such as git history, coverage reports, decision records, lockfiles, or local tool output. They should expose the raw contributing measures when practical rather than only a composite score. Composite scores may be useful for sorting, but they must not hide the evidence or normalization rules that produced them.

`architecture_finding` and `risk_signal` records serve different jobs. The former carries an advisory claim that can be accepted, rejected, or fixed; the latter carries deterministic evidence that can rank context, explain priority, or sharpen a finding. A consumer must be able to filter and inspect them independently.

### Context packing

Task context packing must use the graph as its primary retrieval surface. A conforming implementation should:

- extract exact identifiers and natural-language keywords from the task
- seed candidate nodes from name, module, doc, diagnostic, and metadata matches
- expand candidates through graph relationships with edge-aware weights
- rank nodes using graph proximity, provenance, public API relevance, diagnostics, architecture signals, process-risk signals, recency when available, and optional feedback
- pack results into a token budget using deterministic ordering and stable tie-breakers
- include enough source anchors for an agent to open or inspect original files
- explain why a node was included when requested

Embedding-based re-ranking may be offered, but it must be optional and local/remote execution must be policy-visible under RFC 080 when models are involved.

### Formats

Graph export must support JSONL for streaming ingestion and should support pretty JSON for debugging. Agent context packing should support a compact text format designed for LLM consumption. The compact format should avoid repeated fully qualified names, preserve local IDs, expose edge direction, include provenance, and be deterministic.

The compact format is not required to be stable in the same way as the graph JSONL schema until explicitly versioned.

### MCP surface

The MCP surface should expose task-level tools before raw graph tools. Raw graph tools are still required for debugging, but agents should be able to make one high-level call for task context instead of a sequence of low-level searches.

MCP resources should expose read-only orientation such as graph summary, schema, indexed packages, stale status, and current snapshot. MCP tools that mutate index state, record feedback, or trigger reindexing must be explicit and policy-visible.

### Evaluation

The feature should include an Incan-specific evaluation harness before being considered complete. The harness should measure at least:

- task-context precision at a fixed budget
- recall of known relevant declarations
- token cost compared with file-by-file exploration
- determinism across repeated runs
- freshness after adding or editing source
- behavior on broken work-in-progress packages
- query latency on stdlib-sized and workspace-sized projects
- risk-signal usefulness for architecture review tasks, including at least one control for file or module size when making predictive claims

Evaluation tasks should be hand-labeled where possible and should not derive ground truth from the graph output being evaluated.

## Design details

### Prior art and lessons

[Aider's repository map](https://aider.chat/docs/repomap.html) demonstrates that a compact cross-repository summary can help agents work without reading every file. Its public docs describe a map containing key classes/functions and signatures, then shrinking that map to fit a token budget through graph ranking over file dependencies. The lesson for Incan is that token-budgeted context should be a first-class output, but file-level PageRank is too coarse for compiler-backed Incan declarations.

[CodeGraph](https://github.com/colbymchenry/codegraph) demonstrates the practical value of a local pre-indexed code graph for reducing agent exploration calls. Its README frames the problem as agents scanning with grep/glob/read and proposes instant graph queries over symbol relationships, call graphs, and structure. The lesson for Incan is that local agent integration matters, but Incan should provide authoritative facts rather than requiring an external parser to guess `.incn` semantics.

[Knowing](https://github.com/blackwell-systems/knowing) demonstrates the strongest architectural pattern in this space: content-addressed graph identity, task-ranked context, compact wire formats, feedback expiry, and graph snapshots. Its docs describe a [context pipeline](https://github.com/blackwell-systems/knowing/blob/main/docs/architecture/context-packing.md) that seeds from task text, performs graph-aware expansion such as random walk with restart, ranks results, packs into a budget, and emits compact formats. The lesson for Incan is that the graph export alone is not enough; task-context packing is the agent-facing product.

[Codebase-memory](https://github.com/DeusData/codebase-memory-mcp) demonstrates a different local-first product shape: a persistent knowledge graph served through MCP, broad tree-sitter language coverage, structural queries, impact analysis, architecture summaries, file watchers, agent installer integration, and optional graph visualization. The lesson for Incan is to keep operation local and ergonomic while avoiding the false precision of generic grammar coverage where compiler facts exist.

[GitNexus](https://github.com/nxpatterns/gitnexus) demonstrates deep MCP/editor workflow integration: graph query tools, context/impact tools, generated agent guidance, and hooks that remind agents to use the graph or detect stale indexes. The lesson for Incan is that MCP alone is not enough; agent guidance and stale-index signals shape behavior.

[Gortex](https://gortex.dev/) demonstrates the appeal and risk of broad multi-language graph indexing. Its public site emphasizes a single static binary, in-memory graph, language-server integration, many language extractors, communities, and precomputed blast-radius indexes. The lesson for Incan is to learn from graph algorithms and operational UX, but not to dilute the Incan RFC into a generic polyglot indexer.

[Repowise](https://github.com/repowise-dev/repowise) demonstrates a broader codebase-intelligence product shape: graph intelligence, git intelligence, generated documentation, decision records, code-health scoring, MCP tools, and automatic sync. Its code-health docs describe deterministic biomarkers over AST, git, duplication, coverage, ownership, and trends; its public benchmark reports a time-window experiment over FastAPI, Django, and Pydantic with significant health/defect correlations while also documenting limitations such as file-size confounds and commit-message defect-label heuristics. The lesson for Incan is that architecture advice should combine structural compiler facts with process facts, but any predictive or prioritization claim must expose its evidence and controls.

[Sourcegraph SCIP](https://github.com/scip-code/scip) and [LSIF](https://code.visualstudio.com/blogs/2019/02/19/lsif) demonstrate durable code-intelligence interchange for definitions, references, and implementations. SCIP is language-agnostic and supports code navigation; LSIF was designed to dump language-server knowledge for rich navigation without a local source checkout. The lesson for Incan is that a stable interchange schema can be more valuable than one database, but Incan's schema must also carry agent-specific provenance, diagnostics, and context-packing semantics.

LSP demonstrates the live editor model: diagnostics, go-to-definition, references, hover, and code actions. The lesson for Incan is that live interactivity and durable graph indexing are complementary. The agent context graph should not make every agent simulate an editor session.

[Repomix](https://repomix.com/guide/) and related repo-pack tools demonstrate portability and simplicity: pack the repository into an AI-friendly file. The lesson for Incan is that full-project packing can be an escape hatch, but graph-ranked packs should be the default because they preserve token budget and structural relevance.

### Incan-specific differentiators

Incan can distinguish checked facts from approximate facts. That is the core differentiator. A generic indexer can detect that text looks like a declaration or call; the Incan compiler can know whether a declaration is public, what its checked signature is, what metadata it carries, whether an alias target resolved, whether a stdlib import exists, and which diagnostic invalidated a module.

Incan can also expose body-level facts that are meaningful to the language rather than merely textual. Match dispatch, call sites, reference sites, pattern families, metadata blocks, checked public API members, and stdlib/import relationships are all examples of facts that can be represented directly instead of inferred by a generic code-search system. These facts are especially useful for architecture-advice tooling because they let a rule cite structured evidence rather than a bag of matching lines.

Incan can also export graph facts for generated or materialized structures. RFC 048 metadata and future artifact metadata allow packages to expose public contracts even when source is generated, embedded, or inspected from a built artifact.

Finally, Incan can make graph context part of the project lifecycle. Workspace roots, lockfiles, manifests, stdlib sources, package artifacts, and future `incan.pub` metadata can all contribute to graph identity and staleness detection.

The language server is another differentiator. A generic graph system usually indexes a repository out of band and then tries to stay fresh. Incan can share one source-analysis substrate between CLI export, MCP context, architecture advice, and live editor features. That does not make LSP the graph protocol; it means the language server can become a low-latency consumer and producer of the same schema where appropriate.

Incan can also tie process signals to language facts more precisely than a file-only health score. A generic tool can say a file is high churn; Incan can say that a specific public API, dispatch family, declaration metadata block, generated artifact, or package boundary is high churn, poorly covered, or governed by a stale decision. That gives architecture advice better targets and gives agents a more defensible reason to inspect a region before editing it.

### Graph boundaries

The compiler-facing graph schema should remain narrow enough to be stable. Higher-level storage may add community detection, embeddings, visualization coordinates, feedback scores, or query caches as metadata, but those must not change the meaning of core compiler facts.

Derived advisory records occupy a middle ground. They are not compiler facts and must not be required for baseline graph export, but the schema should allow them because they are useful for agents and architecture tools. The key rule is evidence: derived records must point back to source graph facts so a consumer can audit why the recommendation exists.

Process-risk records follow the same boundary. Git history, coverage reports, and explicit decision records are local project facts, but they are not compiler validation. They should be optional, provenance-tagged, and separable from checked source facts. A missing coverage report should produce missing coverage facts, not a misleading zero-coverage claim.

### Security and policy

Graph export reads source code and may expose private structure to agents. Local export is the default. Remote indexing, remote embedding, hosted storage, or sharing graph artifacts must be explicit and policy-visible. MCP tools that can record feedback or mutate index state should be separated from read-only resources.

## Alternatives considered

### Depend directly on CodeGraph

Rejected. CodeGraph-style tools are useful consumers, but making one external tool the Incan source of truth would give away the compiler's semantic advantage and tie the language roadmap to another project's schema and release cycle.

### Use Knowing directly as the Incan graph system

Rejected as the default, but useful as an integration experiment. Knowing has strong architecture, but it does not currently understand Incan source. The right path is to learn from its content-addressed graph and context-packing model while keeping Incan facts compiler-owned. A bridge importer may still be valuable.

### Emit only SCIP

Rejected as the only format. SCIP is good prior art for code navigation interchange, but the Incan agent graph needs package/workspace facts, diagnostics, checked API metadata, tolerant provenance, compact context packs, and task-context semantics that go beyond ordinary go-to-definition/reference indexing. A future SCIP exporter may still be useful.

### Use only LSP

Rejected. LSP is optimized for live editor interactions, not durable agent context, snapshot identity, task-context packing, feedback expiry, or offline graph interchange.

### Adopt Repowise-style health scoring directly

Rejected as the default. Repowise is strong prior art for deterministic risk signals, but its current implementation is broad, tree-sitter-centered, and file-score oriented. Incan should borrow the evidence model, benchmark discipline, and process metrics while tying signals to compiler-backed declarations, dispatches, packages, and architecture findings. Any Incan health or risk scoring must be validated on Incan projects before being described as predictive.

### Build a generic tree-sitter Incan extractor

Rejected as the primary approach. A tree-sitter extractor may be useful for editor tooling or external tools, but it cannot replace compiler-backed graph facts without duplicating semantic logic and producing lower-trust results.

### Start with embeddings-first retrieval

Rejected. Embeddings may improve ranking within a candidate set, but the graph should first use compiler structure, names, docs, diagnostics, and metadata. Embeddings should be optional and policy-visible.

### Pack the whole repository

Rejected as the default. Full-repository packing is easy to understand but scales poorly and provides no structural ranking. It can remain a fallback export mode for small packages or archival review.

## Drawbacks

This feature adds another public machine-readable contract to maintain. Once external tools depend on the graph schema, schema evolution must be careful.

Compiler-backed facts are only as complete as the compiler exposes. Early versions may have excellent module/declaration/import/API facts but limited call/reference edges.

Tolerant export is easy to misuse. Agents may over-trust unchecked facts unless provenance is visible and compact formats preserve it.

Task-ranking quality is hard to prove. The RFC therefore requires an Incan-specific evaluation harness, but even that will not guarantee performance on every future package.

Risk scoring can be misleading if presented without caveats. Churn, ownership, coverage, and size are correlated with each other; composite scores must remain explainable and should report controls when used for predictive claims.

Content-addressed identity can become brittle if fields are chosen poorly. The identity model must balance stability across harmless movement with accurate invalidation when semantics change.

MCP integration increases the security surface. Read-only graph tools are low risk, but indexing, feedback, hooks, and remote model re-ranking require explicit policy controls.

## Implementation architecture

This section is non-normative.

The recommended shape is four layers. The first layer is a small schema crate that owns graph record types, versioning, serialization, provenance vocabulary, and compact-format helpers. The second layer is the compiler/tooling exporter that converts parser, resolver, typechecker, manifest, stdlib, artifact, and diagnostic facts into schema records. The third layer is a derived-analysis layer for architecture findings, process-risk findings, impact hints, and other advisory records that cite graph evidence. The fourth layer is an optional agent-context service that stores records, computes graph neighborhoods, ranks context, serves MCP, records feedback, and integrates with external stores or importers.

The compiler layer should remain storage-agnostic. It may emit JSONL to stdout or write a graph artifact, but it should not require a database server. The agent-context layer may use SQLite, SurrealDB, an embedded graph engine, or another storage backend as an implementation detail.

The LSP should share the same source-analysis services with the exporter where possible. Live editor state may require incremental caches and partial snapshots, but those caches should materialize the same graph records instead of defining parallel fact shapes.

The task-context ranker should start simple: exact identifiers, module/name/doc search, graph expansion over containment/import/reference/call/dispatch/evidence edges, provenance weights, public API boosts, diagnostic and architecture-signal proximity, and deterministic budget packing. Embedding re-ranking, community detection, feedback, and cross-package memory can come later without changing the core graph schema.

## Layers affected

- **Parser / AST**: parseable module structure, source spans, match dispatch, call sites, reference sites, and metadata syntax provide syntax-backed graph nodes even in tolerant mode.
- **Typechecker / Symbol resolution**: checked declarations, imports, aliases, references, calls, public API signatures, dispatch targets where known, and diagnostics provide high-provenance graph facts.
- **IR Lowering**: lowering does not define the graph, but generated or materialized structures may need source/metadata anchors so tools can connect emitted behavior back to source.
- **Emission**: generated Rust is not the graph source of truth, but emission may expose artifact metadata that can be linked to source graph identities.
- **Stdlib / Runtime (`incan_stdlib`)**: stdlib source and manifests should be graphable as ordinary Incan packages, and stdlib builtins should appear as external or stdlib nodes when referenced.
- **Formatter**: no syntax changes are required by this RFC, but compact graph/context output should have deterministic formatting.
- **LSP / Tooling**: CLI, MCP, editor integrations, graph export, graph stale checks, live graph snapshots, context packing, architecture-advice tooling, risk scoring, and checked metadata/documentation tooling are directly affected.
- **Packaging / Workspaces**: package identity, workspace roots, lockfiles, generated artifacts, and future registry metadata contribute to graph identity and staleness.
- **VCS / Coverage inputs**: git history, ownership signals, co-change data, coverage reports, and explicit decision records may contribute optional process-risk facts, but they must remain provenance-tagged and absent when inputs are unavailable.

## Unresolved questions

- Should higher-level graph service commands reuse `codegraph` or introduce a separate `context`/`agent` namespace once MCP and task packing exist?
- Which graph JSONL fields are stable enough for v0.4, and which should remain experimental?
- Should Incan provide a SCIP exporter in addition to the native graph schema?
- What exact content-addressing inputs should define node, edge, diagnostic, and context-pack identity?
- Should graph snapshots be Merkle roots in the first release or should v0.4 start with simpler content hashes?
- Which edge kinds can the compiler expose accurately enough in v0.4: calls, references, aliases, implements, tests, generated-from, or a smaller subset?
- Which body-level facts should be stable in the first release: match dispatch, call sites, reference sites, metadata annotations, pattern families, or a smaller subset?
- Should derived architecture findings be persisted as graph records, emitted only as reports, or supported in both forms with the same evidence model?
- Which process-risk facts belong in v0.4: churn, ownership, co-change, coverage, decision staleness, trend snapshots, or only the schema hooks for them?
- What local benchmark can validate Incan-specific risk signals without inheriting Python-only or file-size-biased claims from prior art?
- How should live LSP graph snapshots relate to persisted graph snapshots, especially for dirty unsaved editor buffers?
- Should feedback be stored by the Incan toolchain, by an MCP service, or only by external consumers?
- What compact context format should Incan standardize, and when should it become compatibility-stable?
- What minimum evaluation corpus is required before moving this RFC from Draft to Planned?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
