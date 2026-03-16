//! Author-facing companion crate for the pro-level `routekit` example.
//!
//! The goal of this file is to show the intended library-author experience:
//! 1. Declare the DSL surface and optional Rust desugarer in one obvious place.
//! 2. Then, let Incan tooling handle extraction and packaging later.

mod desugar;

use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, LibraryManifest, VocabRegistration};

pub use desugar::RoutekitDesugarer;

/// Import namespace that activates this vocab.
pub const NAMESPACE: &str = "routekit";

/// Top-level block keyword introduced by the example DSL.
pub const ROUTE_KW: &str = "route";

/// Nested sub-block supported inside a `route` block.
pub const MIDDLEWARE_KW: &str = "middleware";

/// Return the complete vocabulary registration for the example companion crate.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import(NAMESPACE).with_declaration(
                DeclarationSurface::named(ROUTE_KW)
                    .with_header_args()
                    .with_mixed_body()
                    .with_clause(ClauseSurface::nested_items(MIDDLEWARE_KW).optional()),
            ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(RoutekitDesugarer)
}
