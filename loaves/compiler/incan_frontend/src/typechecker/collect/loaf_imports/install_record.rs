//! The install record of a compiled Loaf: what installing it reads of its manifest
//! ([`crate::library_manifest::install_record`]).

use super::*;
use crate::library_manifest::CanonicalIdentityExport;
use crate::library_manifest::install_record::{
    INSTALL_RECORD_SCHEMA, InstallCanonicalType, InstallDeclaration, InstallDeclarationKind, InstallModule,
    InstallRootExports, LoafInstallRecord,
};

impl TypeChecker {
    /// The install record of the compiled Loaf indexed as `library`, whose manifest is `manifest`: its nominal public
    /// types as its canonical type remapping admits them, its root exports by kind and its type origins.
    pub fn loaf_install_record(&self, library: &str, manifest: &LibraryManifest) -> LoafInstallRecord {
        let canonical_types = self
            .prepared_canonical_type_remapping(library, manifest)
            .iter()
            .map(|entry| InstallCanonicalType {
                public_name: entry.public_name.clone(),
                source_path: entry.identity.source_path.clone(),
                canonical: entry
                    .identity
                    .canonical
                    .as_ref()
                    .and_then(|canonical| CanonicalIdentityExport::from_canonical(&manifest.name, canonical)),
            })
            .collect();
        let exports = &manifest.exports;
        LoafInstallRecord {
            schema: INSTALL_RECORD_SCHEMA,
            canonical_types,
            root_exports: InstallRootExports {
                models: exports.models.iter().map(|item| item.name.clone()).collect(),
                classes: exports.classes.iter().map(|item| item.name.clone()).collect(),
                enums: exports.enums.iter().map(|item| item.name.clone()).collect(),
                newtypes: exports.newtypes.iter().map(|item| item.name.clone()).collect(),
                traits: exports.traits.iter().map(|item| item.name.clone()).collect(),
            },
            type_origins: self.loaf_type_origins(library, manifest).values().cloned().collect(),
            modules: install_modules(manifest),
        }
    }
}

/// Each checked API module of `manifest` with the models, classes, enums, newtypes, type aliases and traits install
/// registers from it, each with the canonical identity the identity graph publishes at its public path.
fn install_modules(manifest: &LibraryManifest) -> Vec<InstallModule> {
    let Some(api) = manifest.contract_metadata.api.as_ref() else {
        return Vec::new();
    };
    let graph = &manifest.contract_metadata.identity_graph;
    let module_declarations = |module: &crate::api_metadata::CheckedApiMetadata| {
        module
            .declarations
            .iter()
            .enumerate()
            .filter_map(|(position, declaration)| {
                let (name, kind, alias_target, type_params) = match declaration {
                    ApiDeclaration::Model(item) => (
                        &item.name,
                        InstallDeclarationKind::Model,
                        None,
                        item.type_params.clone(),
                    ),
                    ApiDeclaration::Class(item) => (
                        &item.name,
                        InstallDeclarationKind::Class,
                        None,
                        item.type_params.clone(),
                    ),
                    ApiDeclaration::Enum(item) => (&item.name, InstallDeclarationKind::Enum, None, Vec::new()),
                    ApiDeclaration::Newtype(item) => (&item.name, InstallDeclarationKind::Newtype, None, Vec::new()),
                    ApiDeclaration::Trait(item) => (&item.name, InstallDeclarationKind::Trait, None, Vec::new()),
                    ApiDeclaration::TypeAlias(item) => (
                        &item.name,
                        InstallDeclarationKind::TypeAlias,
                        Some(item.type_alias.target.clone()),
                        item.type_alias.type_params.clone(),
                    ),
                    _ => return None,
                };
                let public_path = std::iter::once(manifest.name.clone())
                    .chain(module.module_path.iter().cloned())
                    .chain(std::iter::once(name.clone()))
                    .collect::<Vec<_>>();
                let canonical = graph
                    .canonical_for_public_path(&public_path)
                    .and_then(|canonical| CanonicalIdentityExport::from_canonical(&manifest.name, &canonical));
                Some(InstallDeclaration {
                    position,
                    name: name.clone(),
                    kind,
                    canonical,
                    alias_target,
                    type_params,
                })
            })
            .collect()
    };
    api.modules
        .iter()
        .map(|module| InstallModule {
            module_path: module.module_path.clone(),
            declarations: module_declarations(module),
        })
        .collect()
}
