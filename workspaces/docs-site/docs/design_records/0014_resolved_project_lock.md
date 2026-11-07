---
id: DD-0014
title: Resolved project lock
status: Accepted
type: design-decision
date: 2026-11-07
review_target: v0.6
sources:
  - RFC 106
  - RFC 117
  - RFC 118
  - RFC 119
  - RFC 125
  - https://github.com/encero-systems/incan/issues/2706
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0014: Resolved project lock

## Context

RFC 117 makes `oven.lock` the canonical generated resolution state. RFC 125 requires it to record a sticky version-selection mode and exact registry selections. The installed toolchain can already contain several coherent compiled producers for one requirement, while the project manifest previously had a second selection fact, `[build] registry-lock`. A generated lock must remain portable, authenticate the whole graph and distinguish coexisting compatibility classes without inheriting the removed Cargo lock format.

## Decision

1. **One root lock.** A command's root project owns one generated `oven.lock`; a path dependency's lock is not an input. `oven lock` writes it explicitly. Run, build and test create or refresh it silently. Check and inspection commands read it without writing. `[build] registry-lock` is retired because exact compiled producer entries now live in the lock.
2. **Format 1 is closed TOML.** The format records its version, selection mode, graph identity, toolchain identity and a table for every reached root, path, registry or toolchain Loaf. Origin-specific fields reserve registry protocol, revision, source, digest, trust and local adoption. Paths are relative to the lock directory. Unknown fields and format versions are refused.
3. **The graph authenticates semantics, not layout.** Each entry has a semantic digest over length-prefixed fields. Dependency edges are ordered by their exact identity, and each contributes the closure digest of the entry it reaches, following RFC 106. The root closure digest is `graph`. Table order, edge order and feature-name order do not affect it; a semantic edit does. Every serialized entry must be reached exactly once through resolvable edges from the single root.
4. **Toolchain units are exact selections.** Until a registry provider can answer resolution requests, a shipped registry Loaf is recorded with `origin = "toolchain"` and its compiled entry identity. Those entries feed the existing compiled-closure selection boundary. The installation's own producer record remains an installation default.
5. **Modes are sticky.** `lowest` is the default; `lowest-direct` and `newest` are explicit alternatives. A later `oven lock` keeps the recorded mode. A toolchain change refreshes in `lowest`; `--locked` refuses the old toolchain or any other change and names `oven lock`.
6. **Satisfaction is build evidence.** Whether a selected unit was baked, reused or imported depends on the build and remains in its receipt rather than the portable lock.

## Consequences

- A committed lock selects the same compiled producer graph until an explicit or necessary refresh changes it.
- Two versions of one Loaf remain distinct because every edge records an exact version and, for toolchain content, an exact entry.
- Lock authentication detects hand edits and incomplete graphs without making generated table order significant.
- Registry-origin fields can be populated by the provider implementation without changing format 1.

## Non-goals

- Registry fetching, provider trust establishment, advisories or local adoption.
- A general policy or audit store.
- `--offline`, `--frozen` or Cargo lock compatibility.

## Provenance

This decision applies RFC 106's order-free closure fold and RFCs 117, 118, 119 and 125 to the v0.6 registry implementation tracked by issue #2706. It replaces the project-level producer-selection fact with the generated lock; it does not replace the installation's standard producer selection.
