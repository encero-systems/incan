//! The install record of a compiled Loaf: what installing it reads of its manifest
//! ([`crate::library_manifest::install_record`]).

use super::*;
use crate::library_manifest::CanonicalIdentityExport;
use crate::library_manifest::install_record::{
    INSTALL_RECORD_SCHEMA, InstallCanonicalType, InstallRootExports, LoafInstallRecord,
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
        }
    }
}
