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

The checker needs Rust facts for `rust::` imports: types, generics, methods, fields, variants, function signatures, constants, traits, and module children. Generated Cargo workspace inspection by rust-analyzer disappeared with dev.8; rustc metadata describes the compiled crates the native driver actually links (DD-0004).

## Decision

1. **One extractor.** The native driver extracts facts from linked rustc metadata through public paths, using the checker's metadata shape; each nonstandard root requires one explicit extern artifact in the unit's closure, with missing/conflicting artifacts refused and search paths limited to transitive dependencies.
2. **ABI caches are the primary source.**
   - **Contents:** an ABI cache holds the facts of every public item of one or more compiled Rust crates.
   - **Who writes them:** the same extractor, run as `incan-rustc-driver --rust-abi`, when a crate is compiled or a toolchain is installed.
   - **Coverage:** the toolchain ships the cache of `std`, `core` and `alloc`. A compiled Rust unit carries its own cache, from incan.pub's bake or Oven's build.
   - **How they reach the compiler:** Oven passes the caches with the unit's native closure (`--rust-abi`).
3. **Check-time extraction is the fallback.** Lookup is ordered: the Loaf's shipped ABI, caches, then the same extractor over the unit's crates; covered paths are never re-extracted. Selected public layouts include transitive records through Options and collections, while method-signature expansion is independently bounded.
4. **Storage.**
   - **Layout:** a cache uses `.incnbody`'s layout. A version comes first, then a postcard index of each path's payload range, then one postcard payload per item.
   - **Reading:** a reader decodes the index once, and decodes an item only when checking asks for it.
   - **Shape:** JSON and binary storage serialize every metadata field identically, preserving native widths, references, owner binders, and enum payloads independently of source normalization. Facts retain conditional impl templates on owners and traits, associated definitions, function/supertrait projection predicates, edition-2024 prelude membership, callable inputs/output equalities, and opaque async futures' logical outputs. Extension traits supplement older owner caches without replacing them. Conditional impls remain candidates subject to rustc signature/predicate validation; callable predicates never authorize guessed closure signatures.
5. **Clients without rustc use caches only.** Editor analysis shares checking's compilation context and shipped caches without an in-process fallback; missing declared dependency caches are extracted by the installed toolchain's own driver using exact extern aliases and transitive search paths, never an independently selected rustc. Per-unit caches belong to that toolchain and are keyed by layout version, driver/shipped-cache contents, sysroot path, extern aliases/artifact contents, and search paths; unchanged identities permit reuse, while rebuilt dependencies invalidate it. Extraction is serialized per identity and publication is atomic and limited to complete, decodable caches; failure produces document diagnostics rather than untyped acceptance.

## Consequences

- Checking `rust::` imports needs neither rust-analyzer nor Cargo. Every fact matches the crate rustc links.
- Indexed caches avoid decoding unused items; fallback extraction adds work only for uncovered paths.
- Registry Loaves gain an ABI cache asset beside each compiled unit. incan.pub's existing `[[rust.facts]]` remain build facts (cfg and features).

## Non-goals

- Receiver contracts (returned borrows); a missing contract authorizes no inferred borrowing.
- How calls on Rust values lower to the plan is DD-0006's.

## Revisit condition

Revisit when the cache layout changes or checking needs additional facts; readers refuse other layout versions, requiring cache regeneration.

## Provenance

Derives from DD-0004 (the compiler is a rustc driver), DD-0006 (Rust interop on the direct route), DD-0007 (Loaf artifacts) and RFC 041 (Rust interop metadata). It replaces the rust-analyzer inspection removed in 0.6.0-dev.8.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- DD-0004, DD-0006, DD-0007, RFC 041
