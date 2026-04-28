//! Load a Cargo tree into rust-analyzer's `RootDatabase`.

use std::collections::HashMap;
use std::path::Path;

use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::Vfs;

use super::error::RustMetadataError;

/// A loaded Cargo workspace suitable for `hir` queries.
///
/// The `Vfs` handle is retained so file-backed state remains consistent with the database for the lifetime of this
/// value.
pub struct RustWorkspace {
    pub(crate) db: RootDatabase,
    crate_index: HashMap<String, Crate>,
    #[allow(dead_code)]
    vfs: Vfs,
}

impl RustWorkspace {
    fn normalize_crate_name(name: &str) -> String {
        name.replace('-', "_")
    }

    fn build_crate_index(db: &RootDatabase) -> HashMap<String, Crate> {
        let mut index = HashMap::new();
        for krate in Crate::all(db) {
            if let Some(display_name) = krate.display_name(db) {
                index
                    .entry(Self::normalize_crate_name(display_name.to_string().as_str()))
                    .or_insert(krate);
                index
                    .entry(Self::normalize_crate_name(display_name.crate_name().as_str()))
                    .or_insert(krate);
                index
                    .entry(Self::normalize_crate_name(display_name.canonical_name().as_str()))
                    .or_insert(krate);
            }
        }
        index
    }

    fn metadata_cargo_config() -> CargoConfig {
        CargoConfig::default()
    }

    /// Load the Cargo project rooted at `manifest_dir` (directory containing `Cargo.toml`).
    ///
    /// `progress` is forwarded to rust-analyzer while discovering workspace members.
    pub fn load(manifest_dir: &Path, progress: &(dyn Fn(String) + Sync)) -> Result<Self, RustMetadataError> {
        Self::load_with_options(manifest_dir, progress, false)
    }

    /// Load the Cargo project rooted at `manifest_dir` with optional build-script OUT_DIR support.
    pub fn load_with_options(
        manifest_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        let manifest_dir = manifest_dir.canonicalize()?;
        let cargo_config = Self::metadata_cargo_config();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check,
            // Proc macros are optional for many crates; `None` keeps CI fast.
            with_proc_macro_server: ProcMacroServerChoice::None,
            prefill_caches: false,
            num_worker_threads: 1,
            proc_macro_processes: 1,
        };
        let (db, vfs, _pm) = load_workspace_at(&manifest_dir, &cargo_config, &load_config, progress).map_err(|e| {
            RustMetadataError::LoadWorkspace {
                path: manifest_dir.clone(),
                message: e.to_string(),
            }
        })?;
        let crate_index = Self::build_crate_index(&db);
        Ok(RustWorkspace { db, crate_index, vfs })
    }

    /// Shared read-only access to the underlying database.
    pub fn db(&self) -> &RootDatabase {
        &self.db
    }

    pub fn crate_by_name(&self, crate_name: &str) -> Option<Crate> {
        self.crate_index
            .get(Self::normalize_crate_name(crate_name).as_str())
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::RustWorkspace;

    #[test]
    fn metadata_loader_allows_cargo_to_resolve_uncached_dependencies() {
        let cargo_config = RustWorkspace::metadata_cargo_config();
        assert!(
            !cargo_config.extra_args.iter().any(|arg| arg == "--offline"),
            "rust-inspect workspace loads must not force offline metadata resolution"
        );
        assert_eq!(
            cargo_config.extra_env.get("CARGO_NET_OFFLINE"),
            None,
            "rust-inspect workspace loads must not force Cargo into offline mode"
        );
    }
}
