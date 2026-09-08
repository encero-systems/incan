//! Manifest-selected immutable executable sidecars in the package's semantic artifact slot.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use incan_semantics_core::CanonicalSymbolId;

use super::{CanonicalIdentityExport, FieldVisibilityExport, LibraryManifest};

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
pub fn public_executable_identities(manifest: &LibraryManifest) -> BTreeSet<CanonicalSymbolId> {
    let mut identities = manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter_map(|export| export.canonical.as_ref()?.hydrate())
        .collect::<BTreeSet<_>>();
    let exported_types = identities.clone();
    let mut add = |identity: &Option<CanonicalIdentityExport>| {
        if let Some(identity) = identity.as_ref().and_then(CanonicalIdentityExport::hydrate) {
            identities.insert(identity);
        }
    };
    for model in &manifest.exports.models {
        for field in &model.fields {
            if field.visibility == FieldVisibilityExport::Public {
                add(&field.canonical);
            }
        }
        for method in &model.methods {
            add(&method.canonical);
        }
        for property in &model.properties {
            add(&property.canonical);
        }
    }
    for class in &manifest.exports.classes {
        for field in &class.fields {
            if field.visibility == FieldVisibilityExport::Public {
                add(&field.canonical);
            }
        }
        for method in &class.methods {
            add(&method.canonical);
        }
        for property in &class.properties {
            add(&property.canonical);
        }
    }
    for value in &manifest.exports.enums {
        for variant in &value.variants {
            add(&variant.canonical);
        }
        for method in &value.methods {
            add(&method.canonical);
        }
    }
    for value in &manifest.exports.traits {
        for method in &value.methods {
            add(&method.canonical);
        }
    }
    if let Some(api) = &manifest.contract_metadata.api {
        for module in &api.modules {
            for declaration in &module.declarations {
                use crate::frontend::api_metadata::ApiDeclaration;
                let (anchor, kind) = match declaration {
                    ApiDeclaration::Model(value) => {
                        (&value.anchor, incan_semantics_core::SemanticSourceTargetKind::Model)
                    }
                    ApiDeclaration::Class(value) => {
                        (&value.anchor, incan_semantics_core::SemanticSourceTargetKind::Class)
                    }
                    ApiDeclaration::Trait(value) => {
                        (&value.anchor, incan_semantics_core::SemanticSourceTargetKind::Trait)
                    }
                    ApiDeclaration::Enum(value) => {
                        (&value.anchor, incan_semantics_core::SemanticSourceTargetKind::Enum)
                    }
                    _ => continue,
                };
                let exported = exported_types.iter().any(|identity| {
                    identity.kind == kind && identity.declaration_span.start == anchor.span.start && identity.declaration_span.end == anchor.span.end
                        && matches!(&identity.origin, incan_semantics_core::SymbolOrigin::Package { library, module_path } if library == &manifest.name && module_path == &module.module_path)
                });
                if !exported {
                    continue;
                }
                match declaration {
                    ApiDeclaration::Model(value) => {
                        for field in &value.fields {
                            if field.visibility == FieldVisibilityExport::Public {
                                add(&field.canonical);
                            }
                        }
                        for method in &value.methods {
                            add(&method.canonical);
                        }
                        for property in &value.properties {
                            add(&property.canonical);
                        }
                    }
                    ApiDeclaration::Class(value) => {
                        for field in &value.fields {
                            if field.visibility == FieldVisibilityExport::Public {
                                add(&field.canonical);
                            }
                        }
                        for method in &value.methods {
                            add(&method.canonical);
                        }
                        for property in &value.properties {
                            add(&property.canonical);
                        }
                    }
                    ApiDeclaration::Trait(value) => {
                        for method in &value.methods {
                            add(&method.canonical);
                        }
                    }
                    ApiDeclaration::Enum(value) => {
                        for variant in &value.variants {
                            add(&variant.canonical);
                        }
                        for method in &value.methods {
                            add(&method.canonical);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    identities
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
