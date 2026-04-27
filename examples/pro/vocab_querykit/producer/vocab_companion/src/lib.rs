//! Author-facing companion crate for the pro-level synthetic querykit scoped surface example.
//!
//! This example is intentionally narrow. It demonstrates the RFC 040 contract for query-like leading-dot
//! field references without pretending to be a complete query engine.

mod desugar;

use incan_vocab::{
    DeclarationSurface, DslSurface, LibraryManifest, ScopedSurfaceDescriptor, ScopedSurfaceReceiver, VocabRegistration,
};

pub use desugar::QuerykitDesugarer;

/// Import namespace that activates this vocab.
pub const NAMESPACE: &str = "querykit";

/// Query block keyword introduced by the example DSL.
pub const QUERY_KW: &str = "query";

/// Stable descriptor key preserved on accepted leading-dot query fields.
pub const QUERY_FIELD_DESCRIPTOR: &str = "query.field";

/// Stable descriptor key for leading-dot fields inside query method arguments.
pub const QUERY_METHOD_FIELD_DESCRIPTOR: &str = "query.method_field";

/// Stable descriptor key for query pipeline composition.
pub const QUERY_PIPE_DESCRIPTOR: &str = "query.pipe";

/// Return the complete vocabulary registration for the example companion crate.
///
/// The scoped surface descriptors are the important part: they tell the compiler that leading-dot paths
/// such as `.amount` and `.customer_id` are query-owned expression forms inside `query:` bodies and in
/// registered query method arguments such as `filter(...)` and `select(...)`.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import(NAMESPACE)
                .with_declaration(DeclarationSurface::named(QUERY_KW).with_statement_body())
                .with_scoped_surfaces([
                    ScopedSurfaceDescriptor::leading_dot_path(QUERY_FIELD_DESCRIPTOR)
                        .in_declaration_body(QUERY_KW)
                        .with_receiver(ScopedSurfaceReceiver::OwningDeclaration),
                    ScopedSurfaceDescriptor::operator(QUERY_PIPE_DESCRIPTOR, "|>")
                        .in_declaration_body(QUERY_KW)
                        .pairwise_chain(),
                ])
                .with_scoped_surface(
                    ScopedSurfaceDescriptor::leading_dot_path(QUERY_METHOD_FIELD_DESCRIPTOR)
                        .with_eligibilities([
                            incan_vocab::ScopedSurfaceEligibility::call_argument(QUERY_KW, "filter"),
                            incan_vocab::ScopedSurfaceEligibility::call_argument(QUERY_KW, "select"),
                        ])
                        .with_receiver(ScopedSurfaceReceiver::custom("method-receiver")),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
        .with_desugarer(QuerykitDesugarer)
}

incan_vocab::export_wasm_desugarer!(QuerykitDesugarer);
