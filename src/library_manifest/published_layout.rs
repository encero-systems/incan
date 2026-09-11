//! Manifest-selected immutable executable sidecars in the package's semantic artifact slot.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use incan_semantics_core::CanonicalSymbolId;

use super::{CanonicalIdentityExport, FieldExport, FieldVisibilityExport, LibraryManifest};

/// Directory reserved for executable semantic package fragments.
pub const EXECUTABLE_SURFACE_DIRECTORY: &str = "semantic";

/// Locate the exact immutable surface selected by a finalized manifest.
///
/// Digest syntax is checked before path construction so manifest contents cannot escape their artifact root.
pub fn executable_surface_path(manifest_path: &Path, manifest: &LibraryManifest) -> Option<PathBuf> {
    let descriptor = manifest.contract_metadata.executable_representation.as_ref()?;
    let digest = &descriptor.content_digest;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Some(
        manifest_path
            .parent()?
            .join(EXECUTABLE_SURFACE_DIRECTORY)
            .join(format!("{digest}.incnsem")),
    )
}

/// Canonical membership of the manifest's public surface, including public fields, variants and methods.
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
    use crate::frontend::api_metadata::ApiDeclaration;

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
    declaration: &crate::frontend::api_metadata::ApiDeclaration,
) -> Option<(
    &crate::frontend::api_metadata::SourceAnchor,
    incan_semantics_core::SemanticSourceTargetKind,
)> {
    use crate::frontend::api_metadata::ApiDeclaration;
    use incan_semantics_core::SemanticSourceTargetKind as Kind;
    match declaration {
        ApiDeclaration::Model(value) => Some((&value.anchor, Kind::Model)),
        ApiDeclaration::Class(value) => Some((&value.anchor, Kind::Class)),
        ApiDeclaration::Trait(value) => Some((&value.anchor, Kind::Trait)),
        ApiDeclaration::Enum(value) => Some((&value.anchor, Kind::Enum)),
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
    anchor: &crate::frontend::api_metadata::SourceAnchor,
    manifest_name: &str,
    module_path: &[String],
) -> bool {
    exported_types.iter().any(|identity| {
        identity.kind == kind
            && identity.declaration_span.start == anchor.span.start
            && identity.declaration_span.end == anchor.span.end
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
    use super::executable_surface_path;
    use crate::library_manifest::{ExecutableRepresentationExport, LibraryManifest};
    use std::path::Path;

    /// Linking-only packages never invent a semantic artifact path.
    #[test]
    fn no_representation_is_a_valid_absence() {
        assert!(executable_surface_path(Path::new("/pkg/lib.incnlib"), &LibraryManifest::new("pkg", "1.0")).is_none());
    }

    /// Only the declared content identity selects the sidecar; arbitrary manifest paths cannot escape the root.
    #[test]
    fn a_manifest_selects_a_safe_immutable_sidecar() -> Result<(), Box<dyn std::error::Error>> {
        let mut manifest = LibraryManifest::new("pkg", "1.0");
        manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
            representation_version: 2,
            content_digest: "a".repeat(64),
        });
        assert_eq!(
            executable_surface_path(Path::new("/pkg/lib.incnlib"), &manifest).ok_or("path missing")?,
            Path::new("/pkg/semantic").join(format!("{}.incnsem", "a".repeat(64)))
        );
        manifest.contract_metadata.executable_representation = Some(ExecutableRepresentationExport {
            representation_version: 2,
            content_digest: "../escape".into(),
        });
        assert!(executable_surface_path(Path::new("/pkg/lib.incnlib"), &manifest).is_none());
        Ok(())
    }
}
