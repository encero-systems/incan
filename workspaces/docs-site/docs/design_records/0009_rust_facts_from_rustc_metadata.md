---
id: DD-0009
title: Rust facts are ABI caches read from rustc's metadata
status: Accepted
type: design-decision
date: 2026-10-20
review_target: v0.6
sources:
  - Maintainer decision, 2026-10-20
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0004
  - DD-0006
  - DD-0007
  - RFC 041
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0009: Rust facts are ABI caches read from rustc's metadata

## Context

The checker needs Rust facts for `rust::` imports: types, generics, methods, fields, variants, function signatures, constants, traits, and module children. Inspection of generated Cargo workspaces by rust-analyzer ended when Incan 0.6 removed those workspaces; rustc metadata describes the compiled crates the native driver actually links (DD-0004).

## Decision

1. **One extractor owns Rust facts.** The native driver derives them from the exact linked rustc metadata. Missing or conflicting native inputs are refused; a source spelling or hand-maintained table is not an alternative authority.
2. **Shipped ABI caches are the primary source.** They cover public API and the restricted dependencies needed to interpret it, without granting access to private paths. Uncovered queries use the same extractor against the consumer's selected native closure; bounded results do not claim complete-crate coverage.
3. **Storage preserves meaning and supports selective reads.** The cache has a versioned, indexed encoding and retains the structural facts checking requires. The compiler frontend owns the codec so its dependencies do not enter compiled applications. Layout changes require cache regeneration.
4. **Editor clients share the same facts.** They consume caches and request missing extraction through the selected toolchain's driver, never an independently chosen rustc. Reuse is tied to the exact native generation; incomplete or invalid results are not published, and extraction failures remain diagnostics.

## Consequences

- Checking `rust::` imports needs neither rust-analyzer nor Cargo. Every fact matches the crate rustc links.
- Indexed caches avoid decoding unused items; fallback extraction adds work only for uncovered paths.
- Registry Loaves gain an ABI cache asset beside each compiled unit. loaves.pub's existing `[[rust.facts]]` remain build facts (cfg and features).

## Non-goals

- Receiver contracts (returned borrows); a missing contract authorizes no inferred borrowing.
- How calls on Rust values lower to the plan is DD-0006's.

## Supporting design

[Rust metadata extraction and cache design](../contributing/explanation/rust_metadata_cache_design.md) records the mechanisms and supporting evidence. Changes to those mechanisms do not change this decision unless they alter its stated boundaries or guarantees.

## Revisit condition

Revisit when the cache layout changes or checking needs additional facts; readers refuse other layout versions, requiring cache regeneration.

## Provenance

Derives from DD-0004 (the compiler is a rustc driver), DD-0006 (Rust interop on the direct route), DD-0007 (Loaf artifacts) and RFC 041 (Rust interop metadata). It replaces the rust-analyzer inspection that Incan 0.6 removed.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- DD-0004, DD-0006, DD-0007, RFC 041
