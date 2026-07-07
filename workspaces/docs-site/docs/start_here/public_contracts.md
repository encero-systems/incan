# 1.0 public contracts

Incan 1.0 should be judged by the public contracts it stabilizes, not by every implementation detail that happened to exist during the beta.

This page is a planning target for the 1.0 stabilization pass. It names the contract areas that need explicit docs before 1.0 can be presented as a coherent public surface.

## Contract checklist

| Area | 1.0 question to answer | Public contract shape |
| --- | --- | --- |
| Source compatibility | Which source programs should keep working across compatible releases? | Define compatibility promises for syntax, typechecking, stdlib behavior, diagnostics that are part of user workflows, and allowed breaking-change process. |
| Package metadata | What facts travel with a package? | Version package manifests, dependency metadata, checked API metadata, model/contract metadata, semantic facts, and artifact metadata. |
| Rust interop | What is stable for Rust-facing users? | State the stable Rust-facing ABI/package direction, and separate it from generated Rust source as an implementation or inspection artifact. |
| Diagnostics | What can tools rely on? | Version diagnostic JSON, stable diagnostic codes, severity, phase, spans, notes, hints, and `incan explain` records. |
| Build reports | What does a successful build report guarantee? | Version build-report JSON and document compiler version, profile, project identity, source breadcrumbs, artifacts, dependencies, interop summary, timings, notes, and degraded/partial report rules. |
| Codegraph and inspection | Which compiler facts are public? | Version codegraph records, provenance, degraded-state behavior, language coverage, and unsupported fact categories. |
| Artifact inspection | Which artifacts are stable enough for tools? | Separate native artifacts, package artifacts, checked metadata, current backend output, and debug-only generated files. |
| Experimental surfaces | How are future features labeled? | Every public feature page should say current, beta, experimental, planned, or deferred. |

## Generated Rust boundary

The current beta compiler path builds through Cargo/rustc and can expose generated Rust for inspection. That is useful and should remain documented as current backend output.

For 1.0, generated Rust source should not be the public package compatibility contract. Public compatibility should be based on Incan source, manifests, checked metadata, semantic facts, package metadata, native artifacts, CLI report schemas, and explicit Rust-facing interop contracts.

## Stabilization rules

Before declaring an area stable, the docs should answer:

1. What command, file, or API exposes the contract?
2. What schema or version field lets consumers identify it?
3. Which fields or behaviors are stable?
4. Which fields or behaviors are best-effort or diagnostic-only?
5. How should tools behave when a record is degraded, partial, or from a newer schema?
6. What is explicitly out of scope?

## Current docs that already contribute

- [Roadmap](../roadmap.md)
- [CLI reference](../tooling/reference/cli_reference.md)
- [Codegraph inspection](../tooling/reference/codegraph_inspection.md)
- [Checked API metadata](../tooling/reference/checked_api_metadata.md)
- [Checked contract metadata](../tooling/reference/contract_metadata.md)
- [Rust interop](../language/how-to/rust_interop.md)
- [Agent and tooling documentation surfaces](../tooling/reference/agent_docs_surfaces.md)
