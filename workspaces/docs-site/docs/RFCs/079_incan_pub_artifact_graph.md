# RFC 079: `loaves.pub` artifact graph

- **Status:** Planned
- **Created:** 2026-04-26
- **Author(s):** Danny Meijer (@dannymeijer)
- **Related:**
    - RFC 020 (Cargo offline and locked policy)
    - RFC 027 (incan-vocab and library metadata)
    - RFC 031 (Incan library system phase 1)
    - RFC 125 (`loaves.pub` Loaf registry; supersedes RFC 034)
    - RFC 074 (template rendering and boilerplate provenance)
    - RFC 075 (starter profiles and mixes)
    - RFC 076 (project mutation policy and recovery)
    - RFC 077 (workspace and multi-package projects)
    - RFC 078 (tool execution and typed workflow actions)
    - RFC 080 (declared AI assets)
- **Issue:** <https://github.com/encero-systems/incan/issues/407>
- **RFC PR:** —
- **Written against:** v0.3
- **Shipped in:** —

## Summary

This RFC defines the north-star model for `loaves.pub` as an artifact graph rather than only a package file registry. Packages remain central, but the registry should also understand related templates, starters, mixes, docs, examples, actions, policies, advisories, workspaces, AI assets, evals, and agent guidance so users and tools can discover coherent project capabilities instead of isolated tarballs.

## Core model

Read this RFC as seven foundations:

1. **Packages are one artifact type:** packages remain publishable units, but not every useful ecosystem object is only a package.
2. **Relationships are first-class:** a package may advertise mixes, templates, actions, AI assets, docs, examples, and policies.
3. **Discovery follows intent:** users search for "CLI app", "HTTP client", "data quality", "local embedding model", or "workspace starter", not only package names.
4. **Trust metadata travels with artifacts:** identity, integrity, provenance, yanking, advisories, compatibility, and policy-relevant metadata are graph data.
5. **Local tooling executes decisions:** the registry may discover and verify artifacts, but local lifecycle tooling plans and applies receiver-side mutations.
6. **Cards explain artifacts:** human-readable docs plus structured metadata should describe intended use, limitations, license, compatibility, examples, and safety.
7. **The graph supports AI-native artifacts:** model references, prompt templates, evals, datasets, adapters, and agent guidance need the same discovery and trust treatment as packages.

## Motivation

npm's strength is not only that it stores packages; it makes packages discoverable, scriptable, auditable, and operationally useful. Cargo and crates.io make package identity, docs, versions, features, and lockfiles feel coherent. Hugging Face shows how model and dataset cards turn artifacts into documented, searchable ecosystem objects. Incan should combine those lessons without treating the registry as a remote project mutator.

RFC 125 defines the Loaf registry foundation, superseding RFC 034, while RFC 117 owns project and dependency authority. RFC 074, RFC 075, RFC 076, RFC 077, RFC 078, and RFC 080 all introduce related objects that need discovery and trust: templates, mixes, policies, workspaces, actions, and AI assets. This RFC captures the long-term registry graph that connects them.

## Goals

- Define registry-visible artifact kinds beyond packages.
- Define artifact relationships such as advertises, requires, renders, provides, evaluates, supersedes, yanks, derives-from, and compatible-with.
- Define structured card metadata for packages, mixes, templates, tools, AI assets, datasets, evals, examples, and workspaces.
- Define discovery and ranking inputs without specifying marketplace UI details.
- Define graph metadata that local tooling can use for status, audit, update proposal, policy evaluation, and recovery.
- Keep project mutation local and receiver-owned.
- Let public and private catalogs share descriptor, identity, and trust semantics.
- Define who signs each kind of registry event.

## Non-Goals

- Defining the registry transport protocol or signing mechanics, which RFC 125 owns.
- Defining commercial marketplace ranking, billing, telemetry, or recommendation algorithms.
- Replacing RFC 125 Loaf package identity, trust, or transport semantics, or RFC 117 project and dependency authority.
- Letting `loaves.pub` compute or apply receiver-side project mutation plans.
- Defining AI model execution semantics. RFC 080 owns AI asset metadata and execution constraints.

## Guide-level explanation

### Artifact graph view

A package page might expose relationships:

```text
Package: app-cli 0.3.1

Provides:
  mix cli
  template cli.main
  action run-cli
  docs cli.quickstart

Compatible with:
  Incan >=0.3,<0.4

Advisory state:
  current
```

A user can discover by mix rather than package:

```text
oven registry search kind:mix cli
oven mix show cli --source loaves.pub
```

The local CLI still owns the dry-run and mutation plan.

### Cards

Every artifact has a card: structured metadata plus human-readable explanation, like a model card on Hugging Face or a crate's page on crates.io. The publisher writes it into the source Loaf, next to the artifact it describes:

```text
app-cli 0.3.1 (source Loaf, signed by its publisher)
  loaf.toml
  mixes/cli.toml
  cards/cli.toml        # the mix's card
```

A mix card might include:

```toml
[card]
kind = "mix"
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

Because the card is inside the Loaf, it is covered by the publisher's signature and the Loaf's digest. `loaves.pub` reads it to build search entries and artifact pages, and each of those points back at the card's digest; the registry never keeps an editable copy that could drift from what the publisher signed. To correct a card for a version that is already published, such as fixing a typo, the publisher publishes a signed card update carrying the corrected card. The artifact itself does not change, and the registry shows the newest accepted card while keeping the earlier ones.

### Who signs what

Every change to registry state is an event. The publisher signs what they write; `loaves.pub` countersigns every event it accepts, recording where it sits in the log; and the registry signs alone only for its own interventions:

| Event | Publisher signs | Registry signs |
| --- | --- | --- |
| publish, yank or unyank of one's own version, card update, supersede, compatibility claim | yes | yes, as acceptance |
| ownership transfer | the old and the new owner | yes |
| advisory attach or resolve | the advisory's author | yes |
| malware takedown, legal removal, revocation | no | yes, with a reason |

A compromised registry therefore cannot forge a publisher's claim, and a client can check both who said something and when it was accepted.

## Reference-level explanation

### Artifact kinds

The artifact graph must support these kinds:

- `package`
- `library`
- `template`
- `starter`
- `mix`
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

Each kind must have a versioned, typed descriptor defined by the RFC that owns it: packages and libraries by RFCs 117 and 125, templates by RFC 074, starters and mixes by RFC 075, policies by RFC 076, workspaces by RFC 077, actions and tools by RFC 078, AI assets, datasets, evals and agent guidance by RFC 080, and docs bundles by RFC 082. This RFC defines `advisory`. The graph must not use generic untyped nodes to suggest support for a kind that has no descriptor. A client that meets a kind it does not recognize must keep it visible in machine-readable output as unknown and must not interpret it.

### Relationship kinds

The graph must support these relationship kinds:

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

A card must be a file at `cards/<id>.toml` in the source Loaf that publishes its artifact, so it is covered by the publisher's signature and the Loaf's digest. Registry search entries, indexes, and pages must be projections tied to the card's digest; the registry must not hold an editable copy that can replace the authored card. A correction to a published version's card must be a publisher-signed card update that carries the new card and leaves the artifact's identity unchanged.

Artifact cards must include:

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

For AI assets, cards should also show the declared kind, digest, license, execution and data retention from RFC 080, and may carry richer metadata such as lineage, evals and limitations from a higher-level framework.

### Local tooling boundary

The artifact graph may provide descriptor payloads or descriptor references. It may provide source diffs, compatibility information, advisories, yanking state, integrity metadata, and recommended replacements.

The artifact graph must not be required to compute or apply receiver-side mutation plans. Local lifecycle tooling remains responsible for project inspection, policy evaluation, dry-run output, rendered diffs, conflict handling, and file writes.

### Discovery

Every search result must carry the artifact's identity, kind, version, description, declared compatibility, provenance, and trust status, including yanking, revocation, and advisory state. Discovery must support filtering by artifact kind, package, mix, action, domain, tag, license, compatibility, risk category, AI task, model family, dataset, and advisory state.

Discovery output must be machine-readable and should expose why a result matched the query. Ranking orders results; it must never become semantic authority, so a rank never stands in for compatibility, trust, or identity.

### Advisories and yanking

The graph must represent advisories and yanking as relationships rather than opaque package flags. An advisory must be its own separately attributable, signed record that references the immutable artifact versions it affects; it must never rewrite the affected artifact or its card. A later advisory that corrects or replaces an earlier one must say so with an explicit supersession. A yanked mix descriptor, revoked model, or vulnerable template source can then be connected to affected packages, templates, actions, and projects through local provenance.

### Signatures

Publisher-authored events must carry the publisher's signature, and every accepted event must carry the registry's signature recording its acceptance and position in the log:

- publish, yank and unyank of one's own version, card update, supersession, and compatibility assertions: signed by the publisher and countersigned by the registry;
- ownership transfer: signed by the previous and the new owner and countersigned by the registry;
- advisory attach and resolve: signed by the advisory's author and countersigned by the registry;
- malware takedown, legal removal, and revocation: signed by the registry alone, with a recorded reason.

Signing identities, keyless signing, the transparency log, and registry key delegation are RFC 125's.

### Private catalogs

A private catalog must use the same descriptors, identities, card rules, event kinds, and signature rules as `loaves.pub`. It differs only in visibility and authorization: who may read, publish, and administer its artifacts. A private artifact referencing a public one, or the reverse, must use the same relationship kinds and identities, and RFC 075's source-precedence rules govern which catalog provides an ambiguous id.

### Integrity and event log model

The artifact graph should be built from immutable artifact versions and signed registry events rather than from a mutable ad-hoc catalog. Package archives, template bundles, model weights, docs bundles, datasets, eval suites, generated artifacts, and similar versioned payloads should have content-addressed identities. A stable asset name or channel such as `latest`, `stable`, or `recommended` is a registry pointer, not the artifact version itself.

Registry state changes should be represented as signed events. Event kinds include publish, yank, unyank, supersede, ownership transfer, advisory attach, advisory resolve, revocation, compatibility assertion, and metadata/card update. The queryable artifact graph is then a projection over the accepted event stream plus the immutable artifact payloads it references.

This keeps the useful properties usually sought from blockchain-style designs: tamper evidence, provenance, auditable state transitions, and independently verifiable artifact identity. It avoids making consensus, token economics, or irreversible public-chain writes part of the core `loaves.pub` contract. Moderation, malware response, accidental secret publication, legal takedowns, private catalogs, and key rotation all require registry-controlled state transitions that remain auditable without pretending every piece of state should be globally immutable.

The event log must be append-only from the registry's perspective, with corrections represented by later events rather than in-place rewrites. RFC 125's signed periodic checkpoints cover the log; anchoring them in an external transparency system is not required by the artifact graph.

## Design details

### Relationship to RFC 125

RFC 125 supersedes RFC 034 and owns core Loaf registry semantics. This RFC extends the registry's conceptual model from package versions to related artifact nodes and relationships.

This RFC consumes RFC 125's source-Loaf and baked-asset identity chain and RFC 117's checked package authority. Package semantics must remain grounded in Incan manifests, semantic metadata, ABI/package metadata, and registry artifact relationships, never in implementation artifacts a node may describe for inspection or provenance.

### Relationship to RFC 074 and RFC 075

Template, starter, and mix descriptors are local tooling contracts. The graph can distribute and index them, but local lifecycle tooling owns rendering and mutation planning.

### Relationship to RFC 076

Policy evaluation consumes graph metadata such as source identity, publisher, trust tier, advisory state, yanking state, and recommended recovery paths.

### Relationship to RFC 080

AI assets become first-class graph nodes so model, dataset, prompt, adapter, eval, and agent metadata can be discovered and governed alongside packages and mixes. What AI assets need from the graph is immutable identities and typed relationships, such as `uses-model`, `uses-dataset`, `evaluated-by`, and `derived-from`, to their sources, profiles, and observations. A recommendation marketplace is not part of that need.

## Alternatives considered

### Keep `loaves.pub` package-only

Rejected because templates, mixes, actions, policies, and AI assets would then need parallel registries or ad-hoc package conventions.

### Treat every artifact as a package

Rejected because not every artifact has package semantics. A prompt template, eval suite, mix descriptor, or advisory is meaningfully related to packages but should not necessarily be published as its own dependency.

### Let the registry apply project mutations

Rejected because receiver-side mutation must remain local, reviewable, and policy-controlled.

### Use a blockchain as the core registry ledger

Rejected for the core `loaves.pub` design. Blockchain infrastructure adds operational, cost, latency, governance, and user-experience complexity without matching the registry's primary trust boundary. `loaves.pub` needs signed publishers, content-addressed artifacts, auditable registry events, moderation, yanking, recovery, private-catalog compatibility, and predictable EU-hosted operations. A signed append-only event log with optional external checkpoints provides the relevant integrity properties without binding the registry to a token, chain, or external consensus layer.

## Drawbacks

- A graph model is more complex than a package index.
- Artifact cards require authors to maintain metadata.
- Discovery quality depends on consistent metadata and relationship hygiene.

## Implementation architecture

The recommended implementation shape indexes package-provided relationships that already exist in descriptors and manifests, and adds explicit card metadata, artifact pages, advisory relationships, AI asset nodes, and richer graph queries on top of them.

The local lifecycle does not depend on this RFC. RFC 074, RFC 075, RFC 076, and RFC 078 must be able to work with built-in descriptors and explicit local sources without `loaves.pub` graph semantics.

## Layers affected

- **Registry / package integration:** `loaves.pub` needs artifact kinds, relationship metadata, cards, advisories, and graph queries.
- **CLI / tooling:** lifecycle commands need graph-backed discovery, status, audit, and source resolution.
- **Manifest schema / configuration validation:** packages and descriptors need a way to declare graph relationships and cards.
- **LSP / IDE tooling:** editor tooling may consume graph metadata for discovery, code actions, docs, and project diagnostics.
- **Agentic tooling:** agents may use graph metadata to select mixes, actions, docs, evals, and skills, subject to policy.
- **Documentation:** docs must explain artifact kinds, cards, relationships, and local mutation boundaries.

## Design Decisions

- The graph covers the artifact kinds that agreed RFCs define, each with a versioned typed descriptor from its owning RFC; there are no generic untyped nodes, and an unrecognized kind stays visible as unknown.
- Cards live in the source Loaf at `cards/<id>.toml`, covered by the publisher's signature and the Loaf's digest. Registry indexes and pages are projections tied to the card's digest, and corrections are publisher-signed card updates rather than registry edits.
- Publishers sign what they author; the registry countersigns every accepted event and signs alone only for its own interventions, such as takedowns and revocations, with a reason. Ownership transfers carry both owners' signatures. Signing mechanics are RFC 125's.
- Every search result carries identity, kind, version, description, declared compatibility, provenance, and trust status. Ranking never becomes semantic authority.
- Advisories are separately attributable signed records that reference immutable artifact versions, with explicit supersession; they never rewrite an artifact or its card.
- Private catalogs use the same descriptors, identities, card rules, events, and signatures as `loaves.pub`, with their own visibility and authorization.
- AI assets need immutable identities and typed relationships to their sources, profiles, and observations; a recommendation marketplace is not part of that need.
