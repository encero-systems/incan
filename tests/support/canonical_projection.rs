// Test-only inspection helpers for canonical identities embedded in generated Rust spellings.
//
// Semantic compiler paths may only project checked identities into generated names; they must never recover source
// meaning by decoding those names. Artifact-facing tests still need to inspect the projection, so that deliberately
// reverse-facing work lives outside `src/` at the same boundary as other generated-artifact assertions.

// Each including target uses the subset of these helpers its own assertions need, so the decode helpers below carry
// their own `allow(dead_code)`. That has to be per item rather than a module-wide inner attribute, because
// `src/backend/ir/codegen.rs` pulls this file in through `include!`, which does not permit inner attributes.

use std::collections::HashSet;

use incan_semantics_core::{
    CanonicalSymbolId, SemanticSourceTargetKind, decode_incan_symbol_identity, encode_incan_symbol_identity,
};

/// Recover every projected identity for one source declaration from generated Rust.
#[allow(dead_code)]
pub(crate) fn projected_identities(
    code: &str,
    source_name: &str,
    kind: SemanticSourceTargetKind,
) -> HashSet<CanonicalSymbolId> {
    code.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|token| token.starts_with("__incan_v"))
        .filter_map(|token| decode_incan_symbol_identity(token).ok().flatten())
        .filter(|identity| identity.kind == kind && identity.declaration_name == source_name)
        .collect()
}

/// Recover the one projected identity expected for a source declaration.
#[allow(dead_code)]
pub(crate) fn projected_identity(code: &str, source_name: &str, kind: SemanticSourceTargetKind) -> CanonicalSymbolId {
    let identities = projected_identities(code, source_name, kind.clone());
    assert_eq!(
        identities.len(),
        1,
        "expected exactly one {kind:?} identity for `{source_name}`, got {identities:?} in:\n{code}"
    );
    identities
        .into_iter()
        .next()
        .unwrap_or_else(|| unreachable!("identity count checked above"))
}

/// Recover the exact generated Rust projection for one source declaration.
#[allow(dead_code)]
pub(crate) fn projected_name(code: &str, source_name: &str, kind: SemanticSourceTargetKind) -> String {
    encode_incan_symbol_identity(&projected_identity(code, source_name, kind))
}

/// Present generated Rust with every RFC 120 projection decoded back to its source spelling.
///
/// Artifact assertions and golden files describe declarations the way the source spells them, but a linker-visible
/// declaration reaches generated Rust as an encoded projection. Decoding here is presentation-only and never feeds a
/// compiler path; a caller that needs the physical projection asserts against the undecoded string instead.
///
/// This decodes without reformatting, so comments and the generated header survive. An encoded projection is far
/// longer than the source name it stands for, so `prettyplease` has already wrapped signatures that the source writes
/// on one line; a caller asserting against a one-line source spelling wants [`reformatted_after_decode`] as well.
#[allow(dead_code)]
pub(crate) fn decoded_source_spellings(code: &str) -> String {
    let mut decoded = String::with_capacity(code.len());
    let mut token = String::new();
    let flush = |token: &mut String, decoded: &mut String| {
        if token.starts_with("__incan_v")
            && let Ok(Some(identity)) = decode_incan_symbol_identity(token)
        {
            decoded.push_str(&identity.declaration_name);
        } else {
            decoded.push_str(token);
        }
        token.clear();
    };

    for character in code.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            token.push(character);
        } else {
            flush(&mut token, &mut decoded);
            decoded.push(character);
        }
    }
    flush(&mut token, &mut decoded);
    decoded
}

/// Re-wrap already-decoded generated Rust the way the source-level shape would have been wrapped.
///
/// Decoding shortens every projected identifier, which leaves behind line breaks `prettyplease` chose for the long
/// encoded spelling. Re-formatting restores the one-line signatures a source-shaped assertion expects. Comments do
/// not survive `prettyplease`, so this is a second view of the artifact rather than a replacement for
/// [`decoded_source_spellings`], and callers check both. Input that does not parse as a whole file yields `None`.
#[allow(dead_code)]
pub(crate) fn reformatted_after_decode(decoded: &str) -> Option<String> {
    syn::parse_file(decoded)
        .ok()
        .map(|syntax| prettyplease::unparse(&syntax))
}
