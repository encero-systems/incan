//! Author-facing companion crate for the pro-level `styleforge` embedded-fragment example (RFC 081, `#1023`).
//!
//! This example is intentionally narrow. It demonstrates the RFC 081 contract for a CSS-shaped `style:` block
//! using the compiler's fixed `Style` and `SelectorDeclarationValue` embedded-fragment submodes, without
//! pretending to be a complete CSS implementation -- only the constructs those two submodes explicitly enumerate
//! (selector lists, declaration blocks, custom properties, colors, dimensions, `var()` references) are accepted.
//! Anything else inside `style:`/`spacing:` is a parse error, never a silent reinterpretation.

use incan_vocab::{
    DeclarationSurface, DslSurface, EmbeddedFragmentDescriptor, EmbeddedFragmentSubmode, LibraryManifest,
    VocabRegistration,
};

/// Import namespace that activates this vocab.
pub const NAMESPACE: &str = "styleforge";

/// Style block keyword introduced by this example DSL: a full selector-list-plus-declaration-block fragment.
pub const STYLE_KW: &str = "style";

/// Spacing block keyword introduced by this example DSL: a single bare declaration-value fragment.
pub const SPACING_KW: &str = "spacing";

/// Stable descriptor key for the `style:` block body.
pub const STYLE_FRAGMENT_DESCRIPTOR: &str = "style.fragment";

/// Stable descriptor key for the `spacing:` block body.
pub const SPACING_FRAGMENT_DESCRIPTOR: &str = "spacing.fragment";

/// Return the complete vocabulary registration for the example companion crate.
///
/// No desugarer is registered here: RFC 081 embedded fragments do not need one to prove the parser-to-lowering
/// contract -- their typed artifact reaches typecheck/lowering directly (see `EmbeddedFragmentExpr`'s rustdoc in
/// `incan_syntax`), unlike RFC 040/045 scoped surfaces. Giving `style:`/`spacing:` real runtime meaning is
/// downstream tooling's job (RFC 081 §Semantics), not part of what `#1023` delivers.
#[must_use]
pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_surface(
            DslSurface::on_import(NAMESPACE)
                .with_declaration(DeclarationSurface::named(STYLE_KW).with_statement_body())
                .with_declaration(DeclarationSurface::named(SPACING_KW).with_statement_body())
                .with_embedded_fragment(
                    EmbeddedFragmentDescriptor::new(
                        STYLE_FRAGMENT_DESCRIPTOR,
                        EmbeddedFragmentSubmode::Style,
                        "style_rules",
                    )
                    .in_declaration_body(STYLE_KW),
                )
                .with_embedded_fragment(
                    EmbeddedFragmentDescriptor::new(
                        SPACING_FRAGMENT_DESCRIPTOR,
                        EmbeddedFragmentSubmode::SelectorDeclarationValue,
                        "spacing_value",
                    )
                    .in_declaration_body(SPACING_KW),
                ),
        )
        .with_library_manifest(LibraryManifest::default())
}
