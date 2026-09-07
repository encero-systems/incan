//! Author-facing companion crate for the pro-level `scriptkit` embedded-fragment example (RFC 081, `#1023`).
//!
//! This example is intentionally narrow. It demonstrates the RFC 081 contract for script/type-shaped blocks using
//! the compiler's fixed `RegexTemplate` and `TypePosition` embedded-fragment submodes, without pretending to be a
//! complete JavaScript/TypeScript implementation -- only a bare regex literal, a template string with expression
//! holes, and a minimal representative type-shaped grammar (namespace-qualified names, generics, nullable, array,
//! union) are accepted. Anything else inside `pattern:`/`shape:` is a parse error, never a silent reinterpretation.

use incan_vocab::{
    DeclarationSurface, DslSurface, EmbeddedFragmentDescriptor, EmbeddedFragmentSubmode, LibraryManifest,
    VocabRegistration,
};

/// Import namespace that activates this vocab.
pub const NAMESPACE: &str = "scriptkit";

/// Pattern block keyword introduced by this example DSL: a regex literal or template string fragment.
pub const PATTERN_KW: &str = "pattern";

/// Shape block keyword introduced by this example DSL: a minimal type-shaped grammar fragment.
pub const SHAPE_KW: &str = "shape";

/// Stable descriptor key for the `pattern:` block body.
pub const PATTERN_FRAGMENT_DESCRIPTOR: &str = "pattern.fragment";

/// Stable descriptor key for the `shape:` block body.
pub const SHAPE_FRAGMENT_DESCRIPTOR: &str = "shape.fragment";

/// Return the complete vocabulary registration for the example companion crate.
///
/// No desugarer is registered here: RFC 081 embedded fragments do not need one to prove the parser-to-lowering
/// contract -- their typed artifact reaches typecheck/lowering directly (see `EmbeddedFragmentExpr`'s rustdoc in
/// `incan_syntax`), unlike RFC 040/045 scoped surfaces. Giving `pattern:`/`shape:` real runtime meaning is
/// downstream tooling's job (RFC 081 §Semantics), not part of what `#1023` delivers.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import(NAMESPACE)
                .with_declaration(DeclarationSurface::named(PATTERN_KW).with_statement_body())
                .with_declaration(DeclarationSurface::named(SHAPE_KW).with_statement_body())
                .with_embedded_fragment(
                    EmbeddedFragmentDescriptor::new(
                        PATTERN_FRAGMENT_DESCRIPTOR,
                        EmbeddedFragmentSubmode::RegexTemplate,
                        "pattern_nodes",
                    )
                    .in_declaration_body(PATTERN_KW),
                )
                .with_embedded_fragment(
                    EmbeddedFragmentDescriptor::new(
                        SHAPE_FRAGMENT_DESCRIPTOR,
                        EmbeddedFragmentSubmode::TypePosition,
                        "shape_node",
                    )
                    .in_declaration_body(SHAPE_KW),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
}
