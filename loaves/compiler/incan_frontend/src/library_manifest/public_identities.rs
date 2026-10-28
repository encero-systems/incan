//! The public identities a Loaf's executable representation is projected onto.

use std::collections::BTreeSet;

use incan_semantics_core::CanonicalSymbolId;

use super::{CanonicalIdentityExport, FieldExport, FieldVisibilityExport, LibraryManifest};

/// Canonical membership of the manifest's public surface: exported types plus their published public members.
///
/// Newtype methods use the same finalized canonical records as model, class, enum and trait methods. The public
/// API already exposes these members; RFC 123 executable membership must retain them without admitting private
/// declarations or inventing identities from producer source.
///
/// Top-level and facade targets come from the finalized identity graph. Member visibility comes from the published
/// export records, rather than declaration spans or names in producer source.
///
/// Two sources contribute members and both are needed. The export records describe what this package declares
/// directly. The API metadata additionally describes declarations reached through a facade, which have no export
/// record of their own, so those are admitted only when the identity graph already proves the owning type public.
pub fn public_executable_identities(manifest: &LibraryManifest) -> BTreeSet<CanonicalSymbolId> {
    let mut identities = manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter_map(|export| export.canonical.as_ref()?.hydrate())
        .collect::<BTreeSet<_>>();
    let exported_types = identities.clone();
    add_export_record_members(manifest, &mut identities);
    add_api_declaration_members(manifest, &exported_types, &mut identities);
    identities
}

/// Admit the public members of every type this package declares through its own export records.
fn add_export_record_members(manifest: &LibraryManifest, identities: &mut BTreeSet<CanonicalSymbolId>) {
    for model in &manifest.exports.models {
        add_public_fields(&model.fields, identities);
        add_each(model.methods.iter().map(|method| &method.canonical), identities);
        add_each(model.properties.iter().map(|property| &property.canonical), identities);
    }
    for class in &manifest.exports.classes {
        add_public_fields(&class.fields, identities);
        add_each(class.methods.iter().map(|method| &method.canonical), identities);
        add_each(class.properties.iter().map(|property| &property.canonical), identities);
    }
    for value in &manifest.exports.enums {
        add_each(value.variants.iter().map(|variant| &variant.canonical), identities);
        add_each(value.methods.iter().map(|method| &method.canonical), identities);
    }
    for value in &manifest.exports.traits {
        add_each(value.methods.iter().map(|method| &method.canonical), identities);
    }
    for value in &manifest.exports.newtypes {
        add_each(value.methods.iter().map(|method| &method.canonical), identities);
    }
}

/// Admit the public members of API-described declarations whose owning type the identity graph already proves public.
///
/// A facade target has no export record of its own, so its members would otherwise be missed. The export check is
/// what keeps this from widening the surface: a declaration the graph does not carry contributes nothing.
fn add_api_declaration_members(
    manifest: &LibraryManifest,
    exported_types: &BTreeSet<CanonicalSymbolId>,
    identities: &mut BTreeSet<CanonicalSymbolId>,
) {
    use crate::api_metadata::ApiDeclaration;

    let Some(api) = &manifest.contract_metadata.api else {
        return;
    };
    for module in &api.modules {
        for declaration in &module.declarations {
            let Some((anchor, kind)) = api_declaration_anchor(declaration) else {
                continue;
            };
            if !is_exported_type(exported_types, kind, anchor, &manifest.name, &module.module_path) {
                continue;
            }
            match declaration {
                ApiDeclaration::Model(value) => {
                    add_public_fields(&value.fields, identities);
                    add_each(value.methods.iter().map(|method| &method.canonical), identities);
                    add_each(value.properties.iter().map(|property| &property.canonical), identities);
                }
                ApiDeclaration::Class(value) => {
                    add_public_fields(&value.fields, identities);
                    add_each(value.methods.iter().map(|method| &method.canonical), identities);
                    add_each(value.properties.iter().map(|property| &property.canonical), identities);
                }
                ApiDeclaration::Trait(value) => {
                    add_each(value.methods.iter().map(|method| &method.canonical), identities);
                }
                ApiDeclaration::Newtype(value) => {
                    add_each(value.methods.iter().map(|method| &method.canonical), identities);
                }
                ApiDeclaration::Enum(value) => {
                    add_each(value.variants.iter().map(|variant| &variant.canonical), identities);
                    add_each(value.methods.iter().map(|method| &method.canonical), identities);
                }
                _ => {}
            }
        }
    }
}

/// Return the declaration anchor and semantic kind for the API declarations that can own public members.
///
/// Anything else — a function, a constant, an alias — has no members to admit, so it is not a kind this pass
/// recognizes rather than a kind it skips for a reason.
fn api_declaration_anchor(
    declaration: &crate::api_metadata::ApiDeclaration,
) -> Option<(
    &crate::api_metadata::SourceAnchor,
    incan_semantics_core::SemanticSourceTargetKind,
)> {
    use crate::api_metadata::ApiDeclaration;
    use incan_semantics_core::SemanticSourceTargetKind as Kind;
    match declaration {
        ApiDeclaration::Model(value) => Some((&value.anchor, Kind::Model)),
        ApiDeclaration::Class(value) => Some((&value.anchor, Kind::Class)),
        ApiDeclaration::Trait(value) => Some((&value.anchor, Kind::Trait)),
        ApiDeclaration::Enum(value) => Some((&value.anchor, Kind::Enum)),
        ApiDeclaration::Newtype(value) => Some((
            &value.anchor,
            if value.is_rusttype {
                Kind::Rusttype
            } else {
                Kind::Newtype
            },
        )),
        _ => None,
    }
}

/// Return whether the identity graph already carries this declaration as a public type of this package.
///
/// Matching is on kind, declaration span and package-scoped origin together. Span alone is not an identity: two
/// packages can declare at the same offsets, and the same offsets in one package can belong to different kinds.
fn is_exported_type(
    exported_types: &BTreeSet<CanonicalSymbolId>,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    anchor: &crate::api_metadata::SourceAnchor,
    manifest_name: &str,
    module_path: &[String],
) -> bool {
    exported_types.iter().any(|identity| {
        identity.kind == kind
            && identity.declaration_span == anchor.span
            && matches!(
                &identity.origin,
                incan_semantics_core::SymbolOrigin::Package { library, module_path: path }
                    if library == manifest_name && path == module_path
            )
    })
}

/// Admit each hydrated identity in a sequence, skipping entries the manifest left unproven.
fn add_each<'a>(
    canonicals: impl Iterator<Item = &'a Option<CanonicalIdentityExport>>,
    identities: &mut BTreeSet<CanonicalSymbolId>,
) {
    for canonical in canonicals {
        if let Some(identity) = canonical.as_ref().and_then(CanonicalIdentityExport::hydrate) {
            identities.insert(identity);
        }
    }
}

/// Admit the canonical identity of each field a producer published as public.
///
/// Field visibility is read from the published record rather than inferred, so a private field never reaches the
/// executable surface even when its declaration sits inside a public type.
fn add_public_fields(fields: &[FieldExport], identities: &mut BTreeSet<CanonicalSymbolId>) {
    add_each(
        fields
            .iter()
            .filter(|field| field.visibility == FieldVisibilityExport::Public)
            .map(|field| &field.canonical),
        identities,
    );
}

#[cfg(test)]
mod tests {
    use super::public_executable_identities;
    use crate::library_manifest::{
        CanonicalIdentityExport, ExportIdentity, ExportIdentityKind, ExportIdentityProjection, FieldExport,
        FieldVisibilityExport, LibraryManifest, ModelExport, TypeRef,
    };
    use incan_semantics_core::{
        CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
    };

    /// Linking-only packages never invent executable members from their package name.
    #[test]
    fn no_exports_is_a_valid_empty_surface() {
        assert!(public_executable_identities(&LibraryManifest::new("pkg", "1.0")).is_empty());
    }

    /// Published canonical identity admits an export; public and source spellings cannot manufacture one.
    #[test]
    fn a_manifest_selects_only_authenticated_export_identities() -> Result<(), Box<dyn std::error::Error>> {
        let mut manifest = LibraryManifest::new("pkg", "1.0");
        let mut export = ExportIdentity {
            public_name: "alias".into(),
            public_path: vec!["pkg".into(), "alias".into()],
            source_path: vec!["different".into(), "spelling".into()],
            kind: ExportIdentityKind::Function,
            projection: ExportIdentityProjection::Direct,
            canonical: None,
        };
        manifest.contract_metadata.identity_graph.exports.push(export.clone());
        assert!(public_executable_identities(&manifest).is_empty());
        let canonical = CanonicalSymbolId {
            namespace: SymbolNamespace::OrdinaryLexical,
            origin: SymbolOrigin::Package {
                library: "pkg".into(),
                module_path: vec!["implementation".into()],
            },
            declaration_name: "function".into(),
            kind: SemanticSourceTargetKind::Function,
            scope_discriminant: None,
            declaration_span: HirSourceSpan::new(1, 2),
        };
        export.canonical = Some(CanonicalIdentityExport::from_canonical("pkg", &canonical).ok_or("identity missing")?);
        assert_borrowed_identity_membership(export.canonical.as_ref().ok_or("identity missing")?, &canonical);
        manifest.contract_metadata.identity_graph.exports.push(export);
        assert_eq!(
            public_executable_identities(&manifest),
            [canonical.clone()].into_iter().collect()
        );

        let owner = published_identity("Value", SemanticSourceTargetKind::Model, 10)?;
        let visible = published_identity("Value.visible", SemanticSourceTargetKind::Field, 20)?;
        let private = published_identity("Value.secret", SemanticSourceTargetKind::Field, 30)?;
        let expected = [
            canonical,
            owner.hydrate().ok_or("owner identity missing")?,
            visible.hydrate().ok_or("public field identity missing")?,
        ]
        .into_iter()
        .collect();
        manifest.contract_metadata.identity_graph.exports.push(ExportIdentity {
            public_name: "Value".into(),
            public_path: vec!["Value".into()],
            source_path: vec!["main".into(), "Value".into()],
            kind: ExportIdentityKind::Model,
            projection: ExportIdentityProjection::Direct,
            canonical: Some(owner),
        });
        manifest.exports.models.push(ModelExport {
            name: "Value".into(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: vec![
                field("visible", Some(visible), FieldVisibilityExport::Public),
                field("secret", Some(private), FieldVisibilityExport::Private),
                field("unproven", None, FieldVisibilityExport::Public),
            ],
            properties: Vec::new(),
            methods: Vec::new(),
        });
        assert_eq!(public_executable_identities(&manifest), expected);
        Ok(())
    }

    /// Borrowed membership must equal hydrated structural equality for every identity dimension and refused shape.
    fn assert_borrowed_identity_membership(published: &CanonicalIdentityExport, canonical: &CanonicalSymbolId) {
        let mut candidates = vec![canonical.clone()];
        let mut changed = canonical.clone();
        changed.namespace = SymbolNamespace::Member;
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.declaration_name.push_str("different");
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.origin = SymbolOrigin::Builtin;
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.kind = SemanticSourceTargetKind::Model;
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.declaration_span.start += 1;
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.declaration_span.end += 1;
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.declaration_span.expansion = Some([1; 32]);
        candidates.push(changed);
        let mut changed = canonical.clone();
        changed.scope_discriminant = Some(incan_semantics_core::ScopeDiscriminant(1));
        candidates.push(changed);
        for candidate in candidates {
            assert_eq!(
                published.matches_canonical(&candidate),
                published.hydrate().as_ref() == Some(&candidate)
            );
        }
        let mut unknown = published.clone();
        unknown.kind = "unrecognized".into();
        assert!(!unknown.matches_canonical(canonical));
        assert!(unknown.hydrate().is_none());
    }

    /// Project a source declaration fixture into the package identity domain used by manifest consumers.
    fn published_identity(
        name: &str,
        kind: SemanticSourceTargetKind,
        start: usize,
    ) -> Result<CanonicalIdentityExport, &'static str> {
        let member = matches!(kind, SemanticSourceTargetKind::Field);
        let mut identity = CanonicalSymbolId::module_declaration(
            vec!["main".into()],
            name,
            kind,
            HirSourceSpan::new(start, start + 5),
        );
        if member {
            identity.namespace = SymbolNamespace::Member;
        }
        CanonicalIdentityExport::from_canonical("pkg", &identity).ok_or("fixture identity cannot be published")
    }

    /// Retain a field's explicit publication proof and visibility independently of its source spelling.
    fn field(name: &str, canonical: Option<CanonicalIdentityExport>, visibility: FieldVisibilityExport) -> FieldExport {
        FieldExport {
            name: name.into(),
            canonical,
            visibility,
            ty: TypeRef::Named {
                name: "int".into(),
                origin: None,
            },
            surface_type_name: None,
            has_default: false,
            default: None,
            alias: None,
            description: None,
        }
    }
}
