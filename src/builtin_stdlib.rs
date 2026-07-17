//! Compiler-owned view of the modules published by the built-in stdlib artifact.
//!
//! The stdlib library entrypoint and the normal source-module resolver define the artifact module graph. Library
//! publication persists that resolved graph in checked API metadata; consumers derive ownership from that metadata
//! instead of maintaining a second Rust inventory.

use std::collections::BTreeSet;

use crate::library_manifest::LibraryManifest;
use incan_core::lang::stdlib;

/// Synthetic module name assigned to a library entrypoint by source collection.
const ARTIFACT_ENTRY_MODULE: &str = "main";

/// Canonical module paths supplied by the compiled built-in stdlib artifact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BuiltinStdlibModules {
    relative_paths: BTreeSet<Vec<String>>,
}

impl BuiltinStdlibModules {
    /// Build an inventory from module paths relative to the artifact crate root.
    #[must_use]
    pub(crate) fn from_relative_paths(paths: impl IntoIterator<Item = Vec<String>>) -> Self {
        Self {
            relative_paths: paths.into_iter().collect(),
        }
    }

    /// Derive artifact ownership from the resolved modules persisted in a checked library manifest.
    #[must_use]
    pub(crate) fn from_manifest(manifest: &LibraryManifest) -> Self {
        let relative_paths = manifest
            .contract_metadata
            .api
            .iter()
            .flat_map(|api| api.modules.iter())
            // The artifact's library entrypoint participates in checked API collection as synthetic `main`, but it
            // is not a public `std.main` module. Every other resolved module is owned by this dedicated artifact.
            .filter(|module| module.module_path.as_slice() != [ARTIFACT_ENTRY_MODULE])
            .map(|module| module.module_path.clone())
            .collect();
        Self { relative_paths }
    }

    /// Return whether a public `std.*` source path is supplied by the artifact.
    #[must_use]
    pub(crate) fn contains_source_path(&self, path: &[String]) -> bool {
        path.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) && self.relative_paths.contains(&path[1..])
    }

    /// Return whether an emitted `__incan_std.*` path is supplied by the artifact.
    #[must_use]
    pub(crate) fn contains_emission_path(&self, path: &[String]) -> bool {
        path.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE)
            && self.relative_paths.contains(&path[1..])
    }

    /// Iterate over artifact-owned paths relative to the public `std` namespace.
    pub(crate) fn relative_paths(&self) -> impl Iterator<Item = &[String]> {
        self.relative_paths.iter().map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::BuiltinStdlibModules;
    use crate::frontend::api_metadata::{
        CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata, CheckedApiMetadataPackage,
    };
    use crate::library_manifest::LibraryManifest;

    fn manifest_with_modules(paths: &[&[&str]]) -> LibraryManifest {
        let mut manifest = LibraryManifest::new("incan_builtin_stdlib", "0.5.0");
        manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: paths
                .iter()
                .map(|path| CheckedApiMetadata {
                    schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                    module_path: path.iter().map(|segment| (*segment).to_string()).collect(),
                    declarations: Vec::new(),
                })
                .collect(),
        });
        manifest
    }

    #[test]
    fn inventory_is_derived_from_checked_artifact_modules() {
        let manifest = manifest_with_modules(&[&["fs", "locking"], &["traits"], &["main"]]);
        let modules = BuiltinStdlibModules::from_manifest(&manifest);

        assert!(modules.contains_source_path(&["std".to_string(), "fs".to_string(), "locking".to_string()]));
        assert!(modules.contains_emission_path(&["__incan_std".to_string(), "traits".to_string(),]));
        assert!(!modules.contains_source_path(&["std".to_string(), "main".to_string()]));
    }
}
