//! Load a Cargo tree into rust-analyzer's `RootDatabase`.
//!
//! This module is intentionally behind the rust-inspect preparation/cache boundary. It owns the unstable rust-analyzer
//! embedding details so parser/typechecker/codegen code does not load Cargo workspaces directly.

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

    /// Build Cargo configuration for one Rust metadata workspace.
    ///
    /// rust-analyzer may run `cargo check` to discover build-script output. Keep those nested Cargo artifacts inside
    /// the generated workspace target selected by Incan instead of inheriting a caller-level target or unstable
    /// Cargo `build-dir` override.
    fn metadata_cargo_config(target_dir: &Path) -> CargoConfig {
        let target_dir = target_dir.to_string_lossy().into_owned();
        let mut config = CargoConfig::default();
        config
            .extra_env
            .insert("CARGO_TARGET_DIR".to_string(), Some(target_dir.clone()));
        config
            .extra_env
            .insert("CARGO_BUILD_BUILD_DIR".to_string(), Some(target_dir));
        config
    }

    /// Load the Cargo project rooted at `manifest_dir` (directory containing `Cargo.toml`).
    ///
    /// `progress` is forwarded to rust-analyzer while discovering workspace members. Call this only from explicit
    /// inspection preparation paths, not from ordinary semantic lookups.
    pub fn load(manifest_dir: &Path, progress: &(dyn Fn(String) + Sync)) -> Result<Self, RustMetadataError> {
        Self::load_with_options(manifest_dir, progress, false)
    }

    /// Load the Cargo project rooted at `manifest_dir` with optional build-script OUT_DIR support.
    pub fn load_with_options(
        manifest_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        let target_dir = crate::cache::cargo_configured_target_dir(manifest_dir);
        Self::load_with_options_and_target(manifest_dir, &target_dir, progress, load_out_dirs_from_check)
    }

    /// Load a Cargo project while keeping any nested build-script discovery in the owner workspace's target.
    pub(crate) fn load_with_options_and_target(
        manifest_dir: &Path,
        target_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        let manifest_dir = manifest_dir.canonicalize()?;
        let cargo_config = Self::metadata_cargo_config(target_dir);
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
    use std::fs;

    use super::RustWorkspace;

    use tempfile::tempdir;

    #[test]
    fn metadata_loader_allows_cargo_to_resolve_uncached_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let cargo_config = RustWorkspace::metadata_cargo_config(&workspace.path().join("target"));
        assert!(
            !cargo_config.extra_args.iter().any(|arg| arg == "--offline"),
            "rust-inspect workspace loads must not force offline metadata resolution"
        );
        assert_eq!(
            cargo_config.extra_env.get("CARGO_NET_OFFLINE"),
            None,
            "rust-inspect workspace loads must not force Cargo into offline mode"
        );
        Ok(())
    }

    #[test]
    fn metadata_loader_contains_nested_cargo_output_in_configured_target() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let configured_target = workspace.path().join("managed-target");
        fs::create_dir_all(workspace.path().join(".cargo"))?;
        fs::write(
            workspace.path().join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = {:?}\n", configured_target),
        )?;

        let resolved_target = crate::cache::cargo_configured_target_dir(workspace.path());
        assert_eq!(resolved_target, configured_target);
        let cargo_config = RustWorkspace::metadata_cargo_config(&resolved_target);
        let expected = Some(configured_target.to_string_lossy().into_owned());
        assert_eq!(cargo_config.extra_env.get("CARGO_TARGET_DIR"), Some(&expected));
        assert_eq!(cargo_config.extra_env.get("CARGO_BUILD_BUILD_DIR"), Some(&expected));
        Ok(())
    }
}
