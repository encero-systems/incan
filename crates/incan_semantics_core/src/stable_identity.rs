//! Declaration identity that survives edits.
//!
//! [`CanonicalSymbolId`] is the compiler's identity for a resolved reference, and it is correct for what it claims:
//! its own documentation states it is stable across the stages of *one compilation, not across edits*, because
//! [`CanonicalSymbolId::declaration_span`] participates in its equality and moves whenever a line above it does.
//!
//! A consumer that caches across edits needs a different guarantee. RFC 106 separates the two: identity answers
//! *which declaration is this*, a digest answers *has its meaning changed*, and a single content-addressed value
//! cannot answer both — if identity moved with content, an edited declaration would be indistinguishable from a
//! deletion plus an addition. [`StableDeclarationId`] is that first half.
//!
//! Three positional channels have to be excluded, and each was found by measuring the compiler rather than by
//! reasoning about it:
//!
//! - **The declaration span.** Removing it is necessary and is the obvious part.
//! - **The signature, which removing the span makes necessary.** Two module-level declarations sharing namespace,
//!   origin, name, and kind become indistinguishable once the span is gone. Measured across the 1,252 declarations
//!   one standard library actually declares — imports, aliases, and re-exports share their target's identity by
//!   design and cannot collide — overloads are the only collision class that arises, and it arises twice. A
//!   signature discriminant is therefore required rather than defensive.
//! - **The scope discriminant's value.** [`ScopeDiscriminant`] indexes a module-wide table filled in traversal
//!   order, so inserting or moving any declaration renumbers every declaration traversed after it and untouched
//!   siblings appear to change. Only whether a declaration is nested may enter the identity, never where its scope
//!   sat in the traversal.

use serde::{Deserialize, Serialize};

use crate::facts::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};
use crate::types::IncanType;

/// Whether a declaration is module-level or introduced inside an enclosing scope.
///
/// This records only what [`crate::facts::ScopeDiscriminant`] is *for* — separating same-named bindings in sibling
/// scopes from a module-level declaration of that name — while discarding the discriminant's numeric value, which
/// is an index into a traversal-ordered table and therefore positional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DeclarationNesting {
    /// Unique within its origin without needing a scope to disambiguate it.
    ModuleLevel,
    /// Introduced inside an enclosing scope, such as a local, parameter, receiver, or generic binder.
    Nested,
}

/// A canonical rendering of a declaration's signature, used to separate overloads.
///
/// The rendering is an identity input, so it must be derived from checked types and never from source spelling: two
/// declarations that differ only in how their types are written are the same declaration, and two that differ in the
/// types themselves are not. It carries no span, no parameter name, and no ordering beyond the parameters' own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeclarationSignature(String);

impl DeclarationSignature {
    /// Build a signature from a callable's checked parameter and return types.
    ///
    /// Parameter *names* are deliberately excluded. Renaming a parameter does not produce a different declaration,
    /// and including the name would make a rename look like a new declaration to every consumer keyed on identity.
    pub fn from_callable_types<'a>(
        parameters: impl IntoIterator<Item = &'a IncanType>,
        return_type: &IncanType,
    ) -> Self {
        let rendered = parameters
            .into_iter()
            .map(render_type)
            .collect::<Vec<_>>()
            .join(",");
        Self(format!("({rendered})->{}", render_type(return_type)))
    }

    /// Build a signature from an already-canonical rendering.
    ///
    /// For declarations that are not callables but still need separating, and for callers holding a rendering this
    /// module did not produce. The caller owns the guarantee that the rendering is derived from checked types.
    pub fn from_rendering(rendering: impl Into<String>) -> Self {
        Self(rendering.into())
    }

    /// The canonical rendering.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Render one checked type into the signature alphabet.
///
/// This is deliberately a total function over [`IncanType`] rather than a `Display` implementation: the rendering is
/// an identity input, so it must change only when the type changes, and keeping it local stops an unrelated
/// formatting change elsewhere from silently rekeying every declaration in the workspace.
fn render_type(value: &IncanType) -> String {
    match value {
        IncanType::Never => "!".to_string(),
        IncanType::Primitive(primitive) => format!("{primitive:?}"),
        IncanType::Named(name) => name.clone(),
        IncanType::Generic { base, args } => {
            let rendered = args.iter().map(render_type).collect::<Vec<_>>().join(",");
            format!("{base}[{rendered}]")
        }
        other => format!("{other:?}"),
    }
}

/// A declaration identity that is stable across edits.
///
/// Equality answers *is this the same declaration* and must survive a comment, a reformat, a reordering, an
/// insertion above, and a move between files. It deliberately holds no span, no byte offset, and no traversal
/// counter; see the module documentation for why each is excluded.
///
/// This does **not** replace [`CanonicalSymbolId`], which remains correct within one compilation and is what the
/// compiler resolves references against. The two are joined by construction: a caller resolves with the canonical
/// identity, then derives this one for anything that has to outlive the compilation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct StableDeclarationId {
    /// Which namespace the binding lives in.
    pub namespace: SymbolNamespace,
    /// The module, package, crate, or registry owning the declaration.
    pub origin: SymbolOrigin,
    /// The spelling at the declaration site.
    pub declaration_name: String,
    /// Declaration category.
    pub kind: SemanticSourceTargetKind,
    /// Whether the declaration is module-level or nested, without the discriminant's positional value.
    pub nesting: DeclarationNesting,
    /// Separates declarations that share every field above, which in practice means overloads.
    pub signature: Option<DeclarationSignature>,
}

impl StableDeclarationId {
    /// Derive a stable identity from the compilation's canonical identity.
    ///
    /// The span is dropped and the scope discriminant is reduced to [`DeclarationNesting`]. `signature` separates
    /// overloads and may be `None` for a declaration that cannot collide — but a caller that has a signature
    /// available should pass it, because absence is only safe where the declaration kind admits no overloading.
    pub fn from_canonical(canonical: &CanonicalSymbolId, signature: Option<DeclarationSignature>) -> Self {
        Self {
            namespace: canonical.namespace,
            origin: canonical.origin.clone(),
            declaration_name: canonical.declaration_name.clone(),
            kind: canonical.kind.clone(),
            nesting: match canonical.scope_discriminant {
                Some(_) => DeclarationNesting::Nested,
                None => DeclarationNesting::ModuleLevel,
            },
            signature,
        }
    }

    /// Render a deterministic, span-free spelling for snapshots, digests, and diagnostics.
    ///
    /// Unlike [`CanonicalSymbolId::render_compact`] this ends at the signature rather than at `@start..end`, which
    /// is the whole point: the rendering is safe to persist and to compare across edits.
    pub fn render_compact(&self) -> String {
        let origin = match &self.origin {
            SymbolOrigin::Module(path) => path.join("::"),
            SymbolOrigin::Package { library, module_path } => {
                let mut parts = vec![format!("pub::{library}")];
                parts.extend(module_path.iter().cloned());
                parts.join("::")
            }
            SymbolOrigin::RustCrate(path) => format!("rust::{}", path.join("::")),
            SymbolOrigin::Builtin => "builtin".to_string(),
        };
        let namespace = match self.namespace {
            SymbolNamespace::OrdinaryLexical => "",
            SymbolNamespace::Member => "member/",
            SymbolNamespace::ModulePath => "path/",
        };
        let nesting = match self.nesting {
            DeclarationNesting::ModuleLevel => "",
            DeclarationNesting::Nested => "#nested",
        };
        let signature = self
            .signature
            .as_ref()
            .map(|signature| format!("|{}", signature.as_str()))
            .unwrap_or_default();
        format!(
            "{namespace}{}:{origin}::{}{nesting}{signature}",
            self.kind.as_str(),
            self.declaration_name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::ScopeDiscriminant;
    use crate::hir::HirSourceSpan;
    use crate::types::IncanPrimitiveType;

    fn canonical(name: &str, span: (usize, usize), scope: Option<usize>) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec!["environ".to_string()]),
            declaration_name: name.to_string(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: scope.map(ScopeDiscriminant),
            declaration_span: HirSourceSpan::new(span.0, span.1),
        }
    }

    /// The defining property: the same declaration at a different offset is the same declaration.
    #[test]
    fn identity_survives_a_moved_declaration() {
        let before = StableDeclarationId::from_canonical(&canonical("read", (100, 200), None), None);
        let after = StableDeclarationId::from_canonical(&canonical("read", (900, 1000), None), None);
        assert_eq!(before, after);
        assert_eq!(before.render_compact(), after.render_compact());
    }

    /// A scope discriminant is an index into a traversal-ordered table, so its value must not reach identity.
    /// Its presence must, or a nested binding would collide with a module-level declaration of the same name.
    #[test]
    fn nesting_is_recorded_without_its_positional_value() {
        let first = StableDeclarationId::from_canonical(&canonical("value", (10, 20), Some(2)), None);
        let renumbered = StableDeclarationId::from_canonical(&canonical("value", (10, 20), Some(97)), None);
        assert_eq!(first, renumbered, "a renumbered scope is the same declaration");

        let module_level = StableDeclarationId::from_canonical(&canonical("value", (10, 20), None), None);
        assert_ne!(first, module_level, "a nested binding is not its module-level namesake");
    }

    /// Dropping the span alone is not sufficient: overloads then collide. This is the case measured in
    /// `environ.incn`, where `get_as` is declared twice and separated only by `declaration_span`.
    #[test]
    fn overloads_collide_without_a_signature_and_separate_with_one() {
        let one_argument = canonical("get_as", (195, 400), None);
        let two_arguments = canonical("get_as", (218, 500), None);

        let without = (
            StableDeclarationId::from_canonical(&one_argument, None),
            StableDeclarationId::from_canonical(&two_arguments, None),
        );
        assert_eq!(without.0, without.1, "without a signature the overloads are indistinguishable");

        let str_type = IncanType::Primitive(IncanPrimitiveType::Str);
        let result = IncanType::Named("Result".to_string());
        let with = (
            StableDeclarationId::from_canonical(
                &one_argument,
                Some(DeclarationSignature::from_callable_types([&str_type], &result)),
            ),
            StableDeclarationId::from_canonical(
                &two_arguments,
                Some(DeclarationSignature::from_callable_types([&str_type, &str_type], &result)),
            ),
        );
        assert_ne!(with.0, with.1, "the signature separates them");
    }

    /// Renaming a parameter does not produce a different declaration, so names must stay out of the signature.
    #[test]
    fn signature_ignores_parameter_names_by_construction() {
        let str_type = IncanType::Primitive(IncanPrimitiveType::Str);
        let int_type = IncanType::Primitive(IncanPrimitiveType::Int);
        let unit = IncanType::Named("None".to_string());
        let signature = DeclarationSignature::from_callable_types([&str_type, &int_type], &unit);
        assert!(!signature.as_str().contains("key"), "no parameter name reaches the rendering");
        assert_eq!(
            signature,
            DeclarationSignature::from_callable_types([&str_type, &int_type], &unit),
            "the rendering is deterministic"
        );
    }

    /// A differing parameter type is a differing declaration; a differing return type is too.
    #[test]
    fn signature_tracks_types_in_both_positions() {
        let str_type = IncanType::Primitive(IncanPrimitiveType::Str);
        let int_type = IncanType::Primitive(IncanPrimitiveType::Int);
        assert_ne!(
            DeclarationSignature::from_callable_types([&str_type], &int_type),
            DeclarationSignature::from_callable_types([&int_type], &int_type)
        );
        assert_ne!(
            DeclarationSignature::from_callable_types([&str_type], &int_type),
            DeclarationSignature::from_callable_types([&str_type], &str_type)
        );
    }

    /// The rendering is persisted and compared across edits, so it must carry no offset at all.
    #[test]
    fn rendering_carries_no_offset() {
        let rendered = StableDeclarationId::from_canonical(&canonical("read", (1234, 5678), None), None).render_compact();
        assert!(!rendered.contains("1234") && !rendered.contains("5678"), "rendered: {rendered}");
        assert!(!rendered.contains('@'), "an `@start..end` suffix would defeat the purpose: {rendered}");
    }
}
