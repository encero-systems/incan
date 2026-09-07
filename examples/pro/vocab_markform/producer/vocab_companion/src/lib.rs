//! Author-facing companion crate for the pro-level `markform` embedded-fragment example (RFC 081, `#1023`).
//!
//! This example is intentionally narrow. It demonstrates the RFC 081 contract for an HTML/XML-shaped `markup:`
//! block using the compiler's fixed `Markup` embedded-fragment submode, without pretending to be a complete HTML
//! or XML implementation -- only tags, attributes, text nodes, entity references, comments, and expression holes
//! are accepted. Anything else inside `markup:` is a parse error, never a silent reinterpretation.

use incan_vocab::{
    DeclarationSurface, DslSurface, EmbeddedFragmentDescriptor, EmbeddedFragmentSubmode, LibraryManifest,
    VocabRegistration,
};

/// Import namespace that activates this vocab.
pub const NAMESPACE: &str = "markform";

/// Markup block keyword introduced by this example DSL.
pub const MARKUP_KW: &str = "markup";

/// Stable descriptor key for the `markup:` block body.
pub const MARKUP_FRAGMENT_DESCRIPTOR: &str = "markup.fragment";

/// Return the complete vocabulary registration for the example companion crate.
///
/// No desugarer is registered here: RFC 081 embedded fragments do not need one to prove the parser-to-lowering
/// contract -- their typed artifact reaches typecheck/lowering directly (see `EmbeddedFragmentExpr`'s rustdoc in
/// `incan_syntax`), unlike RFC 040/045 scoped surfaces. Giving `markup:` real runtime meaning (rendering a
/// template to a string, for example) is downstream tooling's job (RFC 081 §Semantics), not part of what `#1023`
/// delivers.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import(NAMESPACE)
                .with_declaration(DeclarationSurface::named(MARKUP_KW).with_statement_body())
                .with_embedded_fragment(
                    EmbeddedFragmentDescriptor::new(
                        MARKUP_FRAGMENT_DESCRIPTOR,
                        EmbeddedFragmentSubmode::Markup,
                        "markup_nodes",
                    )
                    .in_declaration_body(MARKUP_KW),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
}
