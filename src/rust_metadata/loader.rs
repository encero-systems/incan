//! Load a Cargo tree into rust-analyzer's `RootDatabase`.

use std::path::Path;

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
    #[allow(dead_code)]
    vfs: Vfs,
}

impl RustWorkspace {
    fn metadata_cargo_config() -> CargoConfig {
        let mut cargo_config = CargoConfig::default();
        // The generated `target/incan_lock` workspace is a semantic probe, not the user's real build. Keep metadata
        // loading offline-first so missing registry state fails fast instead of burning tens of seconds on network
        // retries during ordinary typechecking/codegen.
        cargo_config.extra_args.push("--offline".to_string());
        cargo_config
            .extra_env
            .insert("CARGO_NET_OFFLINE".to_string(), Some("true".to_string()));
        cargo_config
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
        Ok(RustWorkspace { db, vfs })
    }

    /// Shared read-only access to the underlying database.
    pub fn db(&self) -> &RootDatabase {
        &self.db
    }
}

#[cfg(test)]
mod tests {
    use super::RustWorkspace;

    #[test]
    fn metadata_loader_forces_offline_cargo_queries() {
        let cargo_config = RustWorkspace::metadata_cargo_config();
        assert!(
            cargo_config.extra_args.iter().any(|arg| arg == "--offline"),
            "rust-metadata workspace loads should pass --offline to cargo metadata"
        );
        assert_eq!(
            cargo_config.extra_env.get("CARGO_NET_OFFLINE"),
            Some(&Some("true".to_string())),
            "rust-metadata workspace loads should force offline cargo resolution"
        );
    }
}
