//! The install record of a compiled Loaf: what installing it reads of its manifest
//! ([`crate::library_manifest::install_record`]).

use super::*;
use crate::library_manifest::CanonicalIdentityExport;
use crate::library_manifest::install_record::{
    INSTALL_RECORD_SCHEMA, InstallCanonicalType, InstallDeclaration, InstallDeclarationKind, InstallExportKind,
    InstallModule, InstallNamedExport, InstallNestedLayout, InstallNestedLayoutKind, InstallRootExports,
    InstallScopeTarget, LoafInstallRecord,
};

impl TypeChecker {
    /// The compiled Loaf's install record, shared by the checkers of one module graph. Produce it for an older Loaf
    /// when the graph has no record for it yet.
    pub(in crate::typechecker::collect) fn prepared_install_record(
        &self,
        library: &str,
    ) -> Option<Arc<LoafInstallRecord>> {
        let prepared = &self.dependency_interface_preparation_cache.install_records;
        if let Some(record) = prepared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(library)
        {
            return Some(Arc::clone(record));
        }
        let manifest = self.loaves.manifest(library)?;
        let record = Arc::new(self.loaf_install_record(library, manifest));
        Some(Arc::clone(
            prepared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .entry(library.to_owned())
                .or_insert(record),
        ))
    }

    /// Find the admitted Loaf that publishes the package declaration, in index order. Its install record supplies
    /// membership; callers take the selected provider identity from the same entry's admitted inventory.
    pub(in crate::typechecker::collect) fn install_declaring_loaf(
        &self,
        canonical: &CanonicalSymbolId,
    ) -> Option<(&str, Arc<LoafInstallRecord>)> {
        let SymbolOrigin::Package { library, .. } = &canonical.origin else {
            return None;
        };
        self.loaves
            .package_names()
            .filter(|(_, name)| *name == library.as_str())
            .find_map(|(key, _)| {
                let record = self.prepared_install_record(key)?;
                record.identity_graph.publishes(canonical).then_some((key, record))
            })
    }

    /// The install record of the compiled Loaf indexed as `library`, whose manifest is `manifest`: its nominal public
    /// types as its canonical type remapping admits them, its root exports by kind and its type origins.
    pub fn loaf_install_record(&self, library: &str, manifest: &LibraryManifest) -> LoafInstallRecord {
        let canonical_types = self
            .manifest_nominal_public_types(library, manifest)
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
            modules: install_modules(manifest, self.manifest_declaration_scopes(library, manifest)),
            identity_graph: manifest.contract_metadata.identity_graph.clone(),
            export_kinds: install_export_kinds(manifest),
            aliases: manifest.exports.aliases.clone(),
            nested_layouts: install_nested_layouts(manifest),
        }
    }
}

/// For each public name the identity graph of `manifest` publishes, in first-occurrence order, the kind of root export
/// [`TypeChecker::find_manifest_export`] finds for it.
fn install_export_kinds(manifest: &LibraryManifest) -> Vec<InstallNamedExport> {
    let mut seen = HashSet::new();
    manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter(|export| seen.insert(export.public_name.as_str()))
        .map(|export| InstallNamedExport {
            public_name: export.public_name.clone(),
            kind: TypeChecker::find_manifest_export(manifest, &export.public_name).map(|found| match found {
                ManifestExportRef::Alias(_) => InstallExportKind::Alias,
                ManifestExportRef::Model(_) => InstallExportKind::Model,
                ManifestExportRef::Class(_) => InstallExportKind::Class,
                ManifestExportRef::Function(_) => InstallExportKind::Function,
                ManifestExportRef::Partial(_) => InstallExportKind::Partial,
                ManifestExportRef::Trait(_) => InstallExportKind::Trait,
                ManifestExportRef::Enum(_) => InstallExportKind::Enum,
                ManifestExportRef::EnumVariant { .. } => InstallExportKind::EnumVariant,
                ManifestExportRef::TypeAlias(_) => InstallExportKind::TypeAlias,
                ManifestExportRef::Newtype(_) => InstallExportKind::Newtype,
                ManifestExportRef::Const(_) => InstallExportKind::Const,
                ManifestExportRef::Static(_) => InstallExportKind::Static,
            }),
        })
        .collect()
}

/// Each checked API module of `manifest` with the models, classes, enums, newtypes, type aliases and traits install
/// registers from it, each with the canonical identity the identity graph publishes at its public path.
fn install_modules(
    manifest: &LibraryManifest,
    mut scopes: HashMap<Vec<String>, BTreeMap<String, InstallScopeTarget>>,
) -> Vec<InstallModule> {
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
            type_scope: scopes.remove(&module.module_path).unwrap_or_default(),
        })
        .collect()
}

/// Record which nested export sources have a nominal layout, and retain checked alias projections that can be
/// resolved from the loader's type tables. Declarations that yield no type have no layout recipe.
fn install_nested_layouts(manifest: &LibraryManifest) -> Vec<InstallNestedLayout> {
    let mut seen = HashSet::new();
    manifest
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .filter(|export| export.public_path.len() > 2 && seen.insert(export.source_path.as_slice()))
        .filter_map(|export| {
            let kind = match TypeChecker::api_declaration_for_target_path(manifest, &export.source_path)? {
                ApiDeclaration::Model(_)
                | ApiDeclaration::Class(_)
                | ApiDeclaration::Enum(_)
                | ApiDeclaration::Newtype(_) => InstallNestedLayoutKind::Nominal,
                ApiDeclaration::TypeAlias(_) => InstallNestedLayoutKind::TypeAlias,
                ApiDeclaration::Alias(alias) if alias.projected_function.is_none() => {
                    InstallNestedLayoutKind::Forwarded(alias.projected_type.clone()?)
                }
                _ => return None,
            };
            Some(InstallNestedLayout {
                source_path: export.source_path.clone(),
                kind,
            })
        })
        .collect()
}
