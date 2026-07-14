# RFC 079: `incan.pub` artifact graph

- **Status:** Draft
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (Cargo offline and locked policy)
    - RFC 027 (incan-vocab and library metadata)
    - RFC 031 (Incan library system phase 1)
    - RFC 034 (`incan.pub` package registry)
    - RFC 074 (template rendering and boilerplate provenance)
    - RFC 075 (starter profiles and capability packs)
    - RFC 076 (project mutation policy and recovery)
    - RFC 077 (workspace and multi-package projects)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 080 (AI assets and agent metadata)
- **Issue:** https://github.com/encero-systems/incan/issues/407
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines the north-star model for `incan.pub` as an artifact graph rather than only a package file registry. Packages remain central, but the registry should also understand related templates, starters, capabilities, docs, examples, actions, policies, advisories, workspaces, AI assets, evals, and agent guidance so users and tools can discover coherent project capabilities instead of isolated tarballs.

## Core model

Read this RFC as seven foundations:

1. **Packages are one artifact type:** packages remain publishable units, but not every useful ecosystem object is only a package.
2. **Relationships are first-class:** a package may advertise capabilities, templates, actions, AI assets, docs, examples, and policies.
3. **Discovery follows intent:** users search for "CLI app", "HTTP client", "data quality", "local embedding model", or "workspace starter", not only package names.
4. **Trust metadata travels with artifacts:** identity, integrity, provenance, yanking, advisories, compatibility, and policy-relevant metadata are graph data.
5. **Local tooling executes decisions:** the registry may discover and verify artifacts, but local lifecycle tooling plans and applies receiver-side mutations.
6. **Cards explain artifacts:** human-readable docs plus structured metadata should describe intended use, limitations, license, compatibility, examples, and safety.
7. **The graph supports AI-native artifacts:** model references, prompt templates, evals, datasets, adapters, and agent guidance need the same discovery and trust treatment as packages.

## Motivation

npm's strength is not only that it stores packages; it makes packages discoverable, scriptable, auditable, and operationally useful. Cargo and crates.io make package identity, docs, versions, features, and lockfiles feel coherent. Hugging Face shows how model and dataset cards turn artifacts into documented, searchable ecosystem objects. Incan should combine those lessons without treating the registry as a remote project mutator.

RFC 034 defines the package registry foundation. RFC 074, RFC 075, RFC 076, RFC 077, RFC 078, and RFC 080 all introduce related objects that need discovery and trust: templates, capabilities, policies, workspaces, actions, and AI assets. This RFC captures the long-term registry graph that connects them.

## Goals

- Define registry-visible artifact kinds beyond packages.
- Define artifact relationships such as advertises, requires, renders, provides, evaluates, supersedes, yanks, derives-from, and compatible-with.
- Define structured card metadata for packages, capabilities, templates, tools, AI assets, datasets, evals, examples, and workspaces.
- Define discovery and ranking inputs without specifying marketplace UI details.
- Define graph metadata that local tooling can use for status, audit, update proposal, policy evaluation, and recovery.
- Keep project mutation local and receiver-owned.
- Leave room for public and private catalogs to share descriptor semantics.

## Non-Goals

- Defining the complete public registry protocol.
- Defining commercial marketplace ranking, billing, telemetry, or recommendation algorithms.
- Replacing RFC 034 package identity and transport semantics.
- Letting `incan.pub` compute or apply receiver-side project mutation plans.
- Requiring every artifact type to ship in the first `incan.pub` implementation.
- Defining AI model execution semantics. RFC 080 owns AI asset metadata and execution constraints.

## Guide-level explanation

### Artifact graph view

A package page might expose relationships:

```text
Package: app-cli 0.3.1

Provides:
  capability cli
  template cli.main
  action run-cli
  docs cli.quickstart

Compatible with:
  Incan >=0.3,<0.4

Advisory state:
  current
```

A user can discover by capability rather than package:

```text
incan pub search capability:cli
incan capability show cli --source incan.pub
```

The local CLI still owns the dry-run and mutation plan.

### Cards

Artifacts should have a card: structured metadata plus human-readable explanation. A capability card might include:

```toml
[card]
kind = "capability"
id = "cli"
title = "Command-line application"
license = "Apache-2.0"
tags = ["cli", "application", "starter"]
requires-incan = ">=0.3,<0.4"

[card.risk]
mutates = ["source", "script", "test", "agent-guidance"]

[card.examples]
quickstart = "docs/cli.md"
```

The exact syntax is not normative in this Draft. The requirement is that artifact cards provide both discovery metadata and enough context for humans to judge intended use.

## Reference-level explanation

### Artifact kinds

The artifact graph should support these kinds over time:

- `package`
- `library`
- `template`
- `starter`
- `capability`
- `action`
- `tool`
- `workspace`
- `docs`
- `example`
- `policy`
- `advisory`
- `ai-model`
- `ai-adapter`
- `prompt-template`
- `dataset`
- `eval`
- `agent-guidance`

Implementations may add artifact kinds, but unknown kinds must remain visible in machine-readable output.

### Relationship kinds

The graph should support relationship kinds such as:

- `provides`
- `advertises`
- `requires`
- `depends-on`
- `dev-depends-on`
- `renders`
- `configures`
- `compatible-with`
- `supersedes`
- `derived-from`
- `evaluated-by`
- `uses-dataset`
- `uses-model`
- `implements-action`
- `affected-by-advisory`
- `yanked-by`
- `revoked-by`

Relationships should include source identity, version constraints, and compatibility information when available.

### Cards and metadata

Artifact cards should include:

- stable id
- kind
- title and short description
- publisher or owner
- license
- tags and domain labels
- compatibility constraints
- source repository or documentation links when available
- package or workspace relationships when applicable
- risk categories and mutation categories when applicable
- advisory state
- provenance and integrity metadata when available
- examples and quickstart references
- limitations or intended-use notes when applicable

For AI assets, cards should also include model lineage, datasets, evals, adapters, intended use, limitations, privacy concerns, and local/cloud execution requirements as defined by RFC 080.

### Local tooling boundary

The artifact graph may provide descriptor payloads or descriptor references. It may provide source diffs, compatibility information, advisories, yanking state, integrity metadata, and recommended replacements.

The artifact graph must not be required to compute or apply receiver-side mutation plans. Local lifecycle tooling remains responsible for project inspection, policy evaluation, dry-run output, rendered diffs, conflict handling, and file writes.

### Discovery

Discovery should support artifact kind, package, capability, action, domain, tag, license, compatibility, risk category, AI task, model family, dataset, and advisory state.

Discovery output should be machine-readable and should expose why a result matched the query when that information is available.

### Advisories and yanking

The graph should represent advisories and yanking as relationships rather than opaque package flags. A yanked capability descriptor, revoked model, or vulnerable template source can then be connected to affected packages, templates, actions, and projects through local provenance.

### Integrity and event log model

The artifact graph should be built from immutable artifact versions and signed registry events rather than from a mutable ad-hoc catalog. Package archives, template bundles, model weights, docs bundles, datasets, eval suites, generated artifacts, and similar versioned payloads should have content-addressed identities. A stable asset name or channel such as `latest`, `stable`, or `recommended` is a registry pointer, not the artifact version itself.

Registry state changes should be represented as signed events. Event kinds include publish, yank, unyank, supersede, ownership transfer, advisory attach, advisory resolve, revocation, compatibility assertion, and metadata/card update. The queryable artifact graph is then a projection over the accepted event stream plus the immutable artifact payloads it references.

This keeps the useful properties usually sought from blockchain-style designs: tamper evidence, provenance, auditable state transitions, and independently verifiable artifact identity. It avoids making consensus, token economics, or irreversible public-chain writes part of the core `incan.pub` contract. Moderation, malware response, accidental secret publication, legal takedowns, private catalogs, and key rotation all require registry-controlled state transitions that remain auditable without pretending every piece of state should be globally immutable.

The event log should be append-only from the registry's perspective, with corrections represented by later events rather than in-place rewrites. Implementations may publish Merkle checkpoints or anchor event-log roots in an external transparency system later, but that is an optional hardening layer. It must not be required for the first useful artifact graph.

## Design details

### Relationship to RFC 034

RFC 034 owns core package registry semantics. This RFC extends the registry's conceptual model from package versions to related artifact nodes and relationships.

This RFC inherits RFC 034's amended package artifact boundary: generated Rust source is not the public package compatibility path. Artifact graph nodes may describe generated implementation artifacts for inspection, provenance, compatibility reports, or migration, but package semantics must remain grounded in Incan manifests, semantic metadata, ABI/package metadata, and registry artifact relationships.

### Relationship to RFC 074 and RFC 075

Template, starter, and capability descriptors are local tooling contracts. The graph can distribute and index them, but local lifecycle tooling owns rendering and mutation planning.

### Relationship to RFC 076

Policy evaluation consumes graph metadata such as source identity, publisher, trust tier, advisory state, yanking state, and recommended recovery paths.

### Relationship to RFC 080

AI assets become first-class graph nodes so model, dataset, prompt, adapter, eval, and agent metadata can be discovered and governed alongside packages and capabilities.

## Alternatives considered

### Keep `incan.pub` package-only

Rejected because templates, capabilities, actions, policies, and AI assets would then need parallel registries or ad-hoc package conventions.

### Treat every artifact as a package

Rejected because not every artifact has package semantics. A prompt template, eval suite, capability descriptor, or advisory is meaningfully related to packages but should not necessarily be published as its own dependency.

### Let the registry apply project mutations

Rejected because receiver-side mutation must remain local, reviewable, and policy-controlled.

### Use a blockchain as the core registry ledger

Rejected for the core `incan.pub` design. Blockchain infrastructure adds operational, cost, latency, governance, and user-experience complexity without matching the registry's primary trust boundary. `incan.pub` needs signed publishers, content-addressed artifacts, auditable registry events, moderation, yanking, recovery, private-catalog compatibility, and predictable EU-hosted operations. A signed append-only event log with optional external checkpoints provides the relevant integrity properties without binding the registry to a token, chain, or external consensus layer.

## Drawbacks

- A graph model is more complex than a package index.
- Artifact cards require authors to maintain metadata.
- Discovery quality depends on consistent metadata and relationship hygiene.
- The first implementation must avoid overbuilding marketplace features before package basics are stable.

## Implementation architecture

The recommended implementation shape is to start by indexing package-provided relationships that already exist in descriptors and manifests. Over time, the registry can add explicit card metadata, artifact pages, advisory relationships, AI asset nodes, and richer graph queries.

This RFC is intentionally a registry graph direction, not a blocker for the local lifecycle slice. RFC 074, RFC 075, RFC 076, and RFC 078 must be able to work with built-in descriptors and explicit local sources before `incan.pub` provides rich graph semantics. The first useful registry implementation should therefore be narrow: index package-provided relationships, expose source identity and compatibility metadata, and avoid requiring the full artifact taxonomy before local lifecycle behavior can ship.

## Layers affected

- **Registry / package integration:** `incan.pub` needs artifact kinds, relationship metadata, cards, advisories, and graph queries.
- **CLI / tooling:** lifecycle commands need graph-backed discovery, status, audit, and source resolution.
- **Manifest schema / configuration validation:** packages and descriptors need a way to declare graph relationships and cards.
- **LSP / IDE tooling:** editor tooling may consume graph metadata for discovery, code actions, docs, and project diagnostics.
- **Agentic tooling:** agents may use graph metadata to select capabilities, actions, docs, evals, and skills, subject to policy.
- **Documentation:** docs must explain artifact kinds, cards, relationships, and local mutation boundaries.

## Unresolved questions

- Which artifact kinds should be included in the first graph implementation?
- Should cards live inside package archives, registry-side metadata, or both?
- Which graph event kinds require publisher signatures, registry signatures, or both?
- What discovery fields are required for v1 search?
- Should advisories be separate artifacts or metadata attached to packages and descriptors?
- How should private catalogs share graph semantics with public `incan.pub`?
- What minimum graph support is required before AI assets from RFC 080 become useful?

<!-- Rename this section to "Design Decisions" once all questions have been resolved.
     An RFC cannot move from Draft to Planned until no unresolved questions remain. -->
