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

### Canonical entrypoint

Companion crates should export one obvious Rust function:

```rust
use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new().with_surface(
        DslSurface::on_import("demo.surface").with_declaration(
            DeclarationSurface::named("query")
                .with_clause_body()
                .desugars_to_expression()
                .with_clauses([
                    ClauseSurface::expr("FROM").required(),
                    ClauseSurface::expr_list("SELECT").required().after("FROM"),
                ]),
        ),
    )
}
```

`VocabRegistration` is the source of truth for one library's activated DSL surfaces, machine-readable manifest metadata, and optional Rust desugarer.

### High-level surface types

These are the main author-facing types:

- `VocabRegistration`: the canonical bundle returned by `library_vocab()`
- `DslSurface`: one activation-scoped group of DSL declarations
- `DeclarationSurface`: one top-level DSL declaration such as `query`, `step`, or `route`
- `ClauseSurface`: one declaration-owned clause such as `FROM`, `SELECT`, or `middleware`
- `LibraryManifest`: exported module metadata plus any required Cargo or stdlib requirements

### Public desugaring contract

The `ast` and `desugar` modules define the stable bridge between the compiler and library desugarers:

- `VocabSyntaxNode`: the public AST node handed to desugarers
- `DesugarOutput`: statement-valued or expression-valued output
- `VocabDesugarer`: the trait implemented by companion crates that need custom lowering

These types are intentionally separate from the compiler's internal AST so companion crates do not need to track every internal refactor.

### Low-level transport DTOs

`KeywordRegistration`, `KeywordSpec`, and `VocabMetadata` still exist, but they are lower-level transport and escape-hatch types. Tooling may derive or serialize them as part of packaging, yet the intended authoring flow starts from `library_vocab() -> VocabRegistration`.

## Serialization

The `serde` feature is enabled by default because the compiler serializes vocab metadata into library artifacts. Companion crates can construct the types directly in Rust, and the compiler can persist the resulting `VocabMetadata` as part of a `.incnlib` payload.

## Design constraints

- No dependency on the full compiler crate.
- No dependency on compiler-internal AST or typechecker structures.
- Small, explicit, library-author-facing DTOs instead of leaking implementation details.
- Evolves as a contract crate first, not as an internal convenience module.

## Status

This crate is currently hosted inside the Incan repository and is intended to become publishable on crates.io once the API has settled enough for external library authors.
