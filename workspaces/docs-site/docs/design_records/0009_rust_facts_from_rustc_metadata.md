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

1. **One extractor.** The native driver extracts facts from linked rustc metadata through public paths, using the checker's metadata shape; each nonstandard root requires one explicit extern artifact in the unit's closure, with missing/conflicting artifacts refused and search paths limited to transitive dependencies.
2. **ABI caches are the primary source.**
   - **Contents:** an ABI cache holds the facts of every public item of one or more compiled Rust crates, plus restricted type and trait dependencies needed to interpret that API. Restricted facts support compiler-selected calls and adjustments but never make a private path importable or source-nameable.
   - **Who writes them:** the same extractor, run as `incan-rustc-driver --rust-abi`, when a crate is compiled or a toolchain is installed.
   - **Coverage:** the toolchain ships the cache of `std`, `core` and `alloc`. A compiled Rust unit carries its own cache, from incan.pub's bake or Oven's build.
   - **How they reach the compiler:** Oven passes the caches with the unit's native closure (`--rust-abi`).
3. **Check-time extraction is the fallback.** Lookup is ordered: the Loaf's shipped ABI, caches, then the same extractor over the unit's crates; covered paths are never re-extracted. For a crate the unit declares, private indexed Loaf facts yield to the unit's own caches and extraction; explicit dependency reexports retain their declared ABI. Selected public layouts include transitive records through Options and collections, while method-signature expansion is independently bounded.
   Bounded fallback results may be retained in the same indexed layout for the exact request and native generation, including absent paths; they remain separate from shipped complete-crate caches and grant no crate-wide coverage.
   Loaf publication does not republish private foreign-trait snapshots from the producer's closure; those lookups use the consumer's own native closure, which may add implementations and associated definitions.
4. **Storage.**
   - **Layout:** a version comes first, then three Snappy-compressed sections: an index naming each path's block and range, the string table the records share, and the directory of record blocks. The records follow in Snappy-compressed blocks of about 64 KiB, each record postcard with its strings stored as table indexes. Crate facts repeat a few hundred thousand distinct strings and many identical signatures across items: the toolchain's own caches shrink from 904 MiB in the plain postcard layout (cache version 9) to 42 MiB (version 10), and a cold lookup stays near 0.1 ms.
   - **Reading:** a reader decodes the sections once, and decompresses a block only when checking asks for one of its items; it keeps the eight most recently used blocks.
   - **Ownership:** the cache codec belongs to the compiler frontend, not the kernel vocabulary, so the compressor never enters a compiled program's dependency closure.
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

Derives from DD-0004 (the compiler is a rustc driver), DD-0006 (Rust interop on the direct route), DD-0007 (Loaf artifacts) and RFC 041 (Rust interop metadata). It replaces the rust-analyzer inspection that Incan 0.6 removed.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- DD-0004, DD-0006, DD-0007, RFC 041
