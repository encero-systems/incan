# incan_vocab

`incan_vocab` is the stable contract crate for Incan library companion crates.

Libraries that want to contribute import-activated keywords, compatibility soft-keyword metadata, or future desugaring hooks depend on this crate instead of depending on the full Incan compiler. The goal is to give library authors a small, well-documented API surface that stays stable even as the compiler itself keeps evolving.

## What this crate is for

- Define keyword registrations through `KeywordRegistration`, `KeywordSpec`, and `KeywordActivation`.
- Describe machine-readable library metadata through the DTOs in `manifest`.
- Provide a public AST and desugaring interface for future library-driven syntax lowering.
- Give the compiler a serializable `VocabMetadata` payload that can be written into `.incnlib` artifacts.

## Stability contract

This crate is intended to be versioned independently from the main Incan compiler crates.

- The `incan_vocab` crate follows its own semver lifecycle.
- Additive DTO changes should prefer backwards-compatible evolution.
- Breaking changes to library-author-facing traits, enums, or serialized shapes should be rare and deliberate.
- The compiler may evolve faster than this crate, but it should continue consuming older compatible `incan_vocab` payloads whenever practical.

In other words: library authors should not need to rewrite their vocab companion crates every time the compiler's own version changes.

## Public API overview

### `VocabProvider`

Implement this trait in a companion crate to describe the vocabulary surface your library exports.

- `keyword_registrations()` returns the parser-facing keyword declarations.
- `library_manifest()` returns any additional machine-readable metadata your library wants serialized.
- `desugarer()` reserves a future hook for block desugaring without forcing every provider to implement it.
- `metadata()` assembles the stable serialized output shape consumed by the compiler.

### `keywords`

The `keywords` module contains the core registration model:

- `KeywordRegistration`: a set of keywords that share one activation rule
- `KeywordActivation`: when the keywords become active
- `KeywordSpec`: the shape of an individual keyword
- `KeywordPlacement`: where the keyword is valid
- `KeywordSurfaceKind`: what parser surface the keyword occupies

### `manifest`

The `manifest` module contains stable, serde-friendly DTOs for machine-readable library metadata. These types are intentionally plain data structures so that companion crates can construct them without depending on compiler internals.

### `ast` and `desugar`

These modules define the public syntax/desugaring contract. They are intentionally separate from the compiler's internal AST so the compiler can change implementation details without forcing companion crates to follow every internal refactor.

## Serialization

The `serde` feature is enabled by default because the compiler serializes vocab metadata into library artifacts. Companion crates can construct the types directly in Rust, and the compiler can persist the resulting `VocabMetadata` as part of a `.incnlib` payload.

## Design constraints

- No dependency on the full compiler crate.
- No dependency on compiler-internal AST or typechecker structures.
- Small, explicit, library-author-facing DTOs instead of leaking implementation details.
- Evolves as a contract crate first, not as an internal convenience module.

## Status

This crate is currently hosted inside the Incan repository and is intended to become publishable on crates.io once the API has settled enough for external library authors.
