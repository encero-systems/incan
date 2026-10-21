---
id: DD-0009
title: Rust facts are ABI caches read from rustc's metadata
status: Accepted
type: design-decision
date: 2026-10-20
review_target: v0.6
sources:
  - Maintainer decision, 2026-10-20 (dev.8, after a check-time spike)
  - https://github.com/encero-systems/incan/issues/2256
  - DD-0004
  - DD-0006
  - DD-0007
  - RFC 041
---

<!-- markdownlint-configure-file {"MD025": {"front_matter_title": ""}} -->

# DD-0009: Rust facts are ABI caches read from rustc's metadata

## Context

The checker types `rust::` imports from Rust facts. These cover:

- a type's generics, public methods, fields and variants;
- a function's signature;
- a constant's type;
- a trait's functions;
- a module's children.

The facts used to come from a rust-analyzer inspection of a generated Cargo workspace. 0.6.0-dev.8 removed that inspection together with Cargo. Without facts, every call into a Rust crate is untyped, and with it every standard package that wraps a Rust crate.

The compiler is a rustc driver (DD-0004). rustc's own crate metadata therefore describes every compiled Rust crate exactly as the compilation will link it.

## Decision

1. **One extractor.** `incan-rustc-driver` reads Rust facts from rustc's crate metadata in one rustc session. The session runs over a stub crate that links the crates in question. Each nonstandard root must have one explicit `--extern CRATE=RLIB` artifact in the unit's native closure; absent or conflicting artifacts refuse before extraction. Search directories locate only transitive dependencies, never choose the root artifact. Each requested path resolves through public module children. Each item is described in the checker's existing metadata shape (`RustItemMetadata`).
2. **ABI caches are the primary source.**
   - **Contents:** an ABI cache holds the facts of every public item of one or more compiled Rust crates.
   - **Who writes them:** the same extractor, run as `incan-rustc-driver --rust-abi`, when a crate is compiled or a toolchain is installed.
   - **Coverage:** the toolchain ships the cache of `std`, `core` and `alloc`. A compiled Rust unit carries its own cache, from incan.pub's bake or Oven's build.
   - **How they reach the compiler:** Oven passes the caches with the unit's native closure (`--rust-abi`).
3. **Check-time extraction is the fallback.** A path that neither a Loaf's shipped ABI nor any cache covers is read by the same extractor during checking, from the unit's own crates. This applies to crates without a cache, such as a local Rust crate Oven has not described yet. The lookup order is fixed: the Loaf, then the caches, then the fallback. A path a cache covers is never extracted again, so a crate never has two answers.
4. **Storage.**
   - **Layout:** a cache uses `.incnbody`'s layout. A version comes first, then a postcard index of each path's payload range, then one postcard payload per item.
   - **Reading:** a reader decodes the index once, and decodes an item only when checking asks for it.
   - **Shape:** the metadata types serialize field by field (no skipped fields), so JSON and the binary cache use one shape. Layout version 4 retains trait impl templates, their conditions and associated type definitions, function trait predicates with projection equalities, and whether a trait belongs to Rust's edition-2024 prelude. Callable predicates retain inputs and `FnOnce::Output` without guessing a closure expression's signature. Earlier caches must be regenerated. Conditional impls are candidates, not unconditional implementation proof; a selected closed callee remains subject to rustc's signature and predicate validation.
5. **Clients without rustc use caches only.** The language server opens the same `CompilationSession` as checking, loads the unit's shipped ABI caches, and installs no fallback extractor. For declared Rust dependencies without shipped caches, `incan_driver::rust_abi::cached_rust_facts` asks the installed toolchain's driver to run `--rust-abi` over the unit's exact extern aliases and transitive search paths. The toolchain is located from its shipped cache under `<toolchain>/rust-abi` and its owning `toolchain.json`, rather than from an independently selected rustc. Extracted caches live under `<toolchain>/rust-abi/units/<identity>.rustabi`. The identity covers the cache layout version, driver and shipped-cache contents, sysroot path, declared extern aliases and artifact contents, and search paths. A file lock serializes extraction for the same identity; atomic publication exposes only complete, decodable caches. Reopening or editing an Incan document with the same extern identities reuses the cache, while rebuilding a Rust dependency changes its identity. An extraction failure becomes a document diagnostic, never untyped acceptance.

## Consequences

- Checking `rust::` imports needs neither rust-analyzer nor Cargo. Every fact matches the crate rustc links.
- Measured on aarch64-apple-darwin with rustc 1.98.0:
  - **`std`, `core` and `alloc` cache:** 24,661 items, 5.3 MB, about 570 KB compressed, written in 1.5 s.
  - **Fallback extraction:** 15 to 30 ms for a unit's handful of imports.
- Registry Loaves gain an ABI cache asset beside each compiled unit. incan.pub's existing `[[rust.facts]]` remain build facts (cfg and features).

## Non-goals

- Receiver contracts (returned borrows) are not extracted yet. A missing contract authorizes no inferred borrowing.
- How calls on Rust values lower to the plan is DD-0006's.

## Revisit condition

Revisit when the cache layout changes. A reader refuses a cache of another layout version. Also revisit when checking needs facts the extractor does not yet provide.

## Provenance

Derives from DD-0004 (the compiler is a rustc driver), DD-0006 (Rust interop on the direct route), DD-0007 (Loaf artifacts) and RFC 041 (Rust interop metadata). It replaces the rust-analyzer inspection removed in 0.6.0-dev.8.

## References

- [Issue #2256](https://github.com/encero-systems/incan/issues/2256)
- DD-0004, DD-0006, DD-0007, RFC 041
