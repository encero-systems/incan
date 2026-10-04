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
//!   origin, name, and kind become indistinguishable once the span is gone. Measured across the 1,252 declarations one
//!   standard library actually declares — imports, aliases, and re-exports share their target's identity by design and
//!   cannot collide — overloads are the only collision class that arises, and it arises twice. A signature discriminant
//!   is therefore required rather than defensive.
//! - **The scope discriminant's value.** [`ScopeDiscriminant`] indexes a module-wide table filled in traversal order,
//!   so inserting or moving any declaration renumbers every declaration traversed after it and untouched siblings
//!   appear to change. Nested declarations instead carry their stable named owner plus a collision-local ordinal; the
//!   module-global table position never enters this identity.

use serde::{Deserialize, Serialize};

use crate::facts::{CanonicalSymbolId, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin};
use crate::types::IncanType;

/// Stable location of a declaration within its semantic owner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StableDeclarationLocation {
    /// Unique within its origin without needing a lexical owner.
    ModuleLevel,
    /// Introduced inside a named declaration.
    Nested {
        /// Edit-stable identity of the nearest named semantic owner.
        owner: Box<StableDeclarationId>,
        /// Ordinal among same-name, same-kind bindings in this owner.
        ///
        /// This is deliberately not the module-global scope-table index. An unrelated differently-named insertion or
        /// root-declaration reorder cannot change it. Inserting another same-name, same-kind binding before an
        /// anonymous sibling can rekey later siblings; without persisted source ids or labels, identical anonymous
        /// blocks have no stronger edit-stable distinction.
        binding_ordinal: u32,
    },
}

/// Checked context required to project a nested declaration into an edit-stable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableDeclarationContext {
    /// Already-projected nearest named owner, including its final checked signature when applicable.
    pub owner: StableDeclarationId,
    /// Ordinal among the owner's same-name, same-kind bindings.
    pub binding_ordinal: u32,
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
        let rendered = parameters.into_iter().map(render_type).collect::<Vec<_>>().join(",");
        Self(format!("({rendered})->{}", render_type(return_type)))
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
    /// Semantic location, excluding raw scope-table indices.
    pub location: StableDeclarationLocation,
    /// Separates declarations that share every field above, which in practice means overloads.
    pub signature: Option<DeclarationSignature>,
}

impl StableDeclarationId {
    /// Derive a stable identity from the compilation's canonical identity.
    ///
    /// The span and raw scope discriminant are dropped. A nested declaration requires checked semantic owner context;
    /// absence fails closed instead of recreating the old colliding `Nested` key. `signature` separates overloads and
    /// should be supplied from final checked types whenever available.
    pub fn from_canonical(
        canonical: &CanonicalSymbolId,
        signature: Option<DeclarationSignature>,
        context: Option<StableDeclarationContext>,
    ) -> Option<Self> {
        let location = match canonical.scope_discriminant {
            None => StableDeclarationLocation::ModuleLevel,
            Some(_) => {
                let context = context?;
                StableDeclarationLocation::Nested {
                    owner: Box::new(context.owner),
                    binding_ordinal: context.binding_ordinal,
                }
            }
        };
        Some(Self {
            namespace: canonical.namespace,
            origin: canonical.origin.clone(),
            declaration_name: canonical.declaration_name.clone(),
            kind: canonical.kind.clone(),
            location,
            signature,
        })
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
        let location = match &self.location {
            StableDeclarationLocation::ModuleLevel => String::new(),
            StableDeclarationLocation::Nested { owner, binding_ordinal } => {
                format!("#in({})[{binding_ordinal}]", owner.render_compact())
            }
        };
        let signature = self
            .signature
            .as_ref()
            .map(|signature| format!("|{}", signature.as_str()))
            .unwrap_or_default();
        format!(
            "{namespace}{}:{origin}::{}{location}{signature}",
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
        canonical_of_kind(name, span, scope, SemanticSourceTargetKind::Function)
    }

    /// Mint one canonical record the way `SymbolTable::mint_identity_with_kind` does for a lexical binding: module
    /// origin, unqualified declaration name, the binding's own kind, a scope-table discriminant for anything not at
    /// module level, and the declaration span.
    fn canonical_of_kind(
        name: &str,
        span: (usize, usize),
        scope: Option<usize>,
        kind: SemanticSourceTargetKind,
    ) -> CanonicalSymbolId {
        CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Module(vec!["environ".to_string()]),
            declaration_name: name.to_string(),
            kind,
            scope_discriminant: scope.map(ScopeDiscriminant),
            declaration_span: HirSourceSpan::new(span.0, span.1),
        }
    }

    /// The two records from #1629: parameters both named `value`, minted in sibling functions of one module, so
    /// they share namespace, origin, name and kind and differ only in scope discriminant and declaration span. The
    /// projection used to drop both distinguishing fields and reduce every nested scope to one `#nested` key, so
    /// the two were published as one declaration. Each parameter is now owned by its own function's stable
    /// identity, and that ownership, not the span or the traversal index, is what separates them.
    #[test]
    fn sibling_parameters_named_alike_do_not_share_a_stable_identity_issue1629()
    -> Result<(), Box<dyn std::error::Error>> {
        let left_parameter = canonical_of_kind("value", (30, 35), Some(2), SemanticSourceTargetKind::Parameter);
        let right_parameter = canonical_of_kind("value", (80, 85), Some(3), SemanticSourceTargetKind::Parameter);
        assert_ne!(
            left_parameter, right_parameter,
            "the canonical identities were never the collision"
        );

        // The issue's own probe: convert without owner context. The old projection answered twice with
        // `parameter:...::value#nested`; the projection now refuses rather than inventing a colliding key.
        assert_eq!(
            StableDeclarationId::from_canonical(&left_parameter, None, None),
            None,
            "a nested binding has no stable identity without its checked owner"
        );
        assert_eq!(StableDeclarationId::from_canonical(&right_parameter, None, None), None);

        let owner = |name: &str, span: (usize, usize)| {
            StableDeclarationId::from_canonical(&canonical(name, span, None), None, None)
                .ok_or_else(|| format!("module-level owner `{name}` must project"))
        };
        let project = |parameter: &CanonicalSymbolId, owner: StableDeclarationId| {
            StableDeclarationId::from_canonical(
                parameter,
                None,
                Some(StableDeclarationContext {
                    owner,
                    binding_ordinal: 0,
                }),
            )
            .ok_or("an owned parameter must project")
        };
        let left = project(&left_parameter, owner("left", (0, 60))?)?;
        let right = project(&right_parameter, owner("right", (61, 120))?)?;
        assert_ne!(left, right, "sibling functions' parameters are distinct declarations");
        assert_ne!(left.render_compact(), right.render_compact());
        assert!(
            left.render_compact().contains("#in(function:environ::left)"),
            "the rendering names the lexical owner: {}",
            left.render_compact()
        );

        // Ownership is what carries the identity: moving both functions down the file and renumbering the scope
        // table leaves each parameter equal to itself, while the two stay apart.
        let moved_left = project(
            &canonical_of_kind("value", (530, 535), Some(41), SemanticSourceTargetKind::Parameter),
            owner("left", (500, 560))?,
        )?;
        let moved_right = project(
            &canonical_of_kind("value", (580, 585), Some(42), SemanticSourceTargetKind::Parameter),
            owner("right", (561, 620))?,
        )?;
        assert_eq!(
            left, moved_left,
            "an unrelated span and scope-table shift is the same declaration"
        );
        assert_eq!(right, moved_right);
        assert_ne!(moved_left, moved_right);
        Ok(())
    }

    /// The defining property: the same declaration at a different offset is the same declaration.
    #[test]
    fn identity_survives_a_moved_declaration() {
        let before = StableDeclarationId::from_canonical(&canonical("read", (100, 200), None), None, None);
        let after = StableDeclarationId::from_canonical(&canonical("read", (900, 1000), None), None, None);
        assert_eq!(before, after);
        assert_eq!(
            before.as_ref().map(StableDeclarationId::render_compact),
            after.as_ref().map(StableDeclarationId::render_compact)
        );
    }

    /// A scope discriminant is an index into a traversal-ordered table, so its value must not reach identity.
    /// Its presence must, or a nested binding would collide with a module-level declaration of the same name.
    #[test]
    fn nesting_requires_checked_owner_context_without_using_the_raw_scope_value() {
        let owner = StableDeclarationId::from_canonical(&canonical("read", (1, 100), None), None, None);
        let first = owner.clone().and_then(|owner| {
            StableDeclarationId::from_canonical(
                &canonical("value", (10, 20), Some(2)),
                None,
                Some(StableDeclarationContext {
                    owner,
                    binding_ordinal: 0,
                }),
            )
        });
        let renumbered = owner.clone().and_then(|owner| {
            StableDeclarationId::from_canonical(
                &canonical("value", (10, 20), Some(97)),
                None,
                Some(StableDeclarationContext {
                    owner,
                    binding_ordinal: 0,
                }),
            )
        });
        assert_eq!(first, renumbered, "a renumbered scope is the same declaration");

        let sibling = owner.and_then(|owner| {
            StableDeclarationId::from_canonical(
                &canonical("value", (30, 40), Some(3)),
                None,
                Some(StableDeclarationContext {
                    owner,
                    binding_ordinal: 1,
                }),
            )
        });
        assert_ne!(
            first, sibling,
            "same-named anonymous siblings need collision-local ordinals"
        );

        let module_level = StableDeclarationId::from_canonical(&canonical("value", (10, 20), None), None, None);
        assert_ne!(first, module_level, "a nested binding is not its module-level namesake");
        assert!(
            StableDeclarationId::from_canonical(&canonical("value", (10, 20), Some(2)), None, None).is_none(),
            "nested identity must fail closed when checked owner context is absent"
        );
    }

    /// Dropping the span alone is not sufficient: overloads then collide. This is the case measured in
    /// `environ.incn`, where `get_as` is declared twice and separated only by `declaration_span`.
    #[test]
    fn overloads_collide_without_a_signature_and_separate_with_one() {
        let one_argument = canonical("get_as", (195, 400), None);
        let two_arguments = canonical("get_as", (218, 500), None);

        let without = (
            StableDeclarationId::from_canonical(&one_argument, None, None),
            StableDeclarationId::from_canonical(&two_arguments, None, None),
        );
        assert_eq!(
            without.0, without.1,
            "without a signature the overloads are indistinguishable"
        );

        let str_type = IncanType::Primitive(IncanPrimitiveType::Str);
        let result = IncanType::Named("Result".to_string());
        let with = (
            StableDeclarationId::from_canonical(
                &one_argument,
                Some(DeclarationSignature::from_callable_types([&str_type], &result)),
                None,
            ),
            StableDeclarationId::from_canonical(
                &two_arguments,
                Some(DeclarationSignature::from_callable_types(
                    [&str_type, &str_type],
                    &result,
                )),
                None,
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
        assert!(
            !signature.as_str().contains("key"),
            "no parameter name reaches the rendering"
        );
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
        let rendered = StableDeclarationId::from_canonical(&canonical("read", (1234, 5678), None), None, None)
            .map(|identity| identity.render_compact())
            .unwrap_or_default();
        assert!(
            !rendered.contains("1234") && !rendered.contains("5678"),
            "rendered: {rendered}"
        );
        assert!(
            !rendered.contains('@'),
            "an `@start..end` suffix would defeat the purpose: {rendered}"
        );
    }
}
