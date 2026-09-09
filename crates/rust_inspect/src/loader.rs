//! Load an explicitly selected projection into rust-analyzer's database.
//!
//! This module owns physical source loading. Oven supplies selection and retains the admitted input leases;
//! no manifest discovery, dependency resolution, build-script execution or ambient sysroot loading occurs here.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{ProjectJson, ProjectWorkspace, ProjectWorkspaceKind, Sysroot};
use ra_ap_vfs::Vfs;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::RustMetadataError;

/// A loaded selected projection suitable for `hir` queries.
///
/// The `Vfs` handle is retained so file-backed state remains consistent with the database for the lifetime of this
/// value.
pub struct RustWorkspace {
    pub(crate) db: RootDatabase,
    pub(crate) selection_fingerprint: String,
    crate_index: HashMap<String, Crate>,
    #[allow(dead_code)]
    vfs: Vfs,
}

/// A sequence scoped to this process keeps generated direct-project descriptions independent when libtests run in
/// parallel. The descriptions live under the caller-managed inspection output, never beside an inspected source
/// tree or in Cargo's cache.
static OVEN_PROJECT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Compiler-authored marker for an inspection projection that must use the direct rust-project loader.
///
/// The marker lives only in a generated inspection directory. It keeps the selection local to the prepared Oven
/// invocation. It is retained as historical wire data; it no longer selects a loader or grants source authority.
pub const OVEN_DIRECT_INSPECTION_MARKER: &str = ".incan_oven_direct_rust_project";
/// Compiler-authored source authority consumed by the direct Oven inspection loader.
pub const OVEN_DIRECT_INSPECTION_AUTHORITY_FILE: &str = ".incan_oven_rust_sources.json";
const OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION: u32 = 2;

/// Historical source-catalog validation label, retained when reading or writing its schema.
///
/// The selected physical loader never consumes this label. Setting it cannot bypass input validation or supply
/// provider admission; those facts must come from the caller's selected projection and retained leases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum OvenInspectionSourceValidation {
    #[default]
    FullTreeDigest,
    SealedOvenSelection,
}

/// Exact registry source selected from one leased Loaf or the explicit baker's locked metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenInspectionRegistrySource {
    /// Cargo package name recorded by the publisher.
    pub package: String,
    /// Exact package version selected by the publisher.
    pub version: String,
    /// Cargo registry identity from the publisher lock.
    pub registry: String,
    /// Registry archive checksum from the publisher lock.
    pub checksum: String,
    /// Exact unified feature set compiled into the selected Loaf leaf.
    pub features: Vec<String>,
    /// Immutable source directory retained by the selected Loaf.
    pub source_root: PathBuf,
    /// Digest of every portable path and regular file below `source_root`.
    pub source_digest: String,
}

/// Historical source catalog; its entries and validation label cannot select an inspection graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenInspectionSourceAuthority {
    schema_version: u32,
    #[serde(default)]
    source_validation: OvenInspectionSourceValidation,
    sources: Vec<OvenInspectionRegistrySource>,
}

/// Write the historical source catalog with its full-tree validation label.
///
/// Serialization does not validate or admit the sources. Selected loading requires separate explicit inputs.
pub fn write_oven_inspection_source_authority(
    manifest_dir: &Path,
    sources: Vec<OvenInspectionRegistrySource>,
) -> Result<PathBuf, RustMetadataError> {
    write_oven_inspection_source_authority_with_validation(
        manifest_dir,
        sources,
        OvenInspectionSourceValidation::FullTreeDigest,
    )
}

/// Write the historical source catalog with its selected-Loaf validation label.
///
/// This label grants no trust bypass. The physical loader requires explicit expected content bindings, while
/// provider admission and source leases remain the caller's responsibility.
pub fn write_sealed_oven_inspection_source_authority(
    manifest_dir: &Path,
    sources: Vec<OvenInspectionRegistrySource>,
) -> Result<PathBuf, RustMetadataError> {
    write_oven_inspection_source_authority_with_validation(
        manifest_dir,
        sources,
        OvenInspectionSourceValidation::SealedOvenSelection,
    )
}

/// Return the exact source roots named by a prepared Oven inspection authority.
///
/// This lightweight helper validates only schema and source-root accessibility. Callers that need full identity and
/// digest validation must use explicit selected inputs. This historical catalog does not establish graph selection
/// or authorize metadata cache access.
pub fn oven_inspection_registry_source_roots(manifest_dir: &Path) -> Result<Vec<PathBuf>, RustMetadataError> {
    let path = manifest_dir.join(OVEN_DIRECT_INSPECTION_AUTHORITY_FILE);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let authority = serde_json::from_slice::<OvenInspectionSourceAuthority>(&fs::read(&path)?).map_err(|error| {
        RustMetadataError::LoadWorkspace {
            path: path.clone(),
            message: format!("invalid Oven Rust source authority: {error}"),
        }
    })?;
    if !(1..=OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION).contains(&authority.schema_version) {
        return Err(RustMetadataError::LoadWorkspace {
            path,
            message: format!(
                "unsupported Oven Rust source authority schema {}",
                authority.schema_version
            ),
        });
    }
    let mut roots = authority
        .sources
        .into_iter()
        .map(|source| source.source_root.canonicalize())
        .collect::<Result<Vec<_>, _>>()?;
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Serialize deterministic inspection authority using the caller's declared validation boundary.
fn write_oven_inspection_source_authority_with_validation(
    manifest_dir: &Path,
    mut sources: Vec<OvenInspectionRegistrySource>,
    source_validation: OvenInspectionSourceValidation,
) -> Result<PathBuf, RustMetadataError> {
    for source in &mut sources {
        source.features.sort();
        source.features.dedup();
    }
    sources.sort_by(|left, right| {
        (
            &left.package,
            &left.version,
            &left.registry,
            &left.checksum,
            &left.source_root,
        )
            .cmp(&(
                &right.package,
                &right.version,
                &right.registry,
                &right.checksum,
                &right.source_root,
            ))
    });
    let authority = OvenInspectionSourceAuthority {
        schema_version: OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        source_validation,
        sources,
    };
    let path = manifest_dir.join(OVEN_DIRECT_INSPECTION_AUTHORITY_FILE);
    let payload = serde_json::to_vec_pretty(&authority).map_err(|error| RustMetadataError::LoadWorkspace {
        path: path.clone(),
        message: format!("failed to encode Oven Rust source authority: {error}"),
    })?;
    fs::write(&path, payload)?;
    Ok(path)
}

/// Hash one source tree by portable path and exact bytes, matching Oven's Loaf source identity.
pub(crate) fn digest_oven_source_tree(root: &Path) -> Result<String, RustMetadataError> {
    /// Collect portable source-tree records while rejecting links and special files.
    fn collect(root: &Path, current: &Path, records: &mut BTreeMap<String, String>) -> Result<(), RustMetadataError> {
        let mut entries = fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(RustMetadataError::LoadWorkspace {
                    path,
                    message: "sealed Oven registry source contains a symbolic link".to_string(),
                });
            }
            if metadata.is_dir() {
                collect(root, &path, records)?;
                continue;
            }
            if !metadata.is_file() {
                return Err(RustMetadataError::LoadWorkspace {
                    path,
                    message: "sealed Oven registry source contains a non-regular file".to_string(),
                });
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|error| RustMetadataError::LoadWorkspace {
                    path: path.clone(),
                    message: format!("sealed Oven registry source escaped its root: {error}"),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let digest = format!("sha256:{}", hex::encode(Sha256::digest(fs::read(&path)?)));
            if records.insert(relative, digest).is_some() {
                return Err(RustMetadataError::LoadWorkspace {
                    path,
                    message: "sealed Oven registry source contains a duplicate portable path".to_string(),
                });
            }
        }
        Ok(())
    }

    let root = root.canonicalize()?;
    let metadata = fs::symlink_metadata(&root)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(RustMetadataError::LoadWorkspace {
            path: root,
            message: "sealed Oven registry source must be a real directory".to_string(),
        });
    }
    let mut records = BTreeMap::new();
    collect(&root, &root, &mut records)?;
    if records.is_empty() {
        return Err(RustMetadataError::LoadWorkspace {
            path: root,
            message: "sealed Oven registry source must contain regular files".to_string(),
        });
    }
    let payload = serde_json::to_vec(&records).map_err(|error| RustMetadataError::LoadWorkspace {
        path: root,
        message: format!("failed to encode sealed Oven source digest: {error}"),
    })?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(payload))))
}

impl RustWorkspace {
    /// Refuse a path-only load: a directory is not a selected inspection graph.
    pub fn load(manifest_dir: &Path, _progress: &(dyn Fn(String) + Sync)) -> Result<Self, RustMetadataError> {
        Err(RustMetadataError::SelectedInputUnavailable {
            path: manifest_dir.to_path_buf(),
        })
    }

    /// Refuse the former path-and-build-script option entrypoint without discovering replacement inputs.
    pub fn load_with_options(
        manifest_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        _load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        Self::load(manifest_dir, progress)
    }

    /// Load one validated physical projection under the caller's temporary root.
    ///
    /// The caller retains the source/artifact leases for the database lifetime. This host operation verifies physical
    /// bindings, but does not admit providers or decide dependencies. All sysroot crates, cfg and generated sources
    /// must already occur in the projection. Unsupported macro execution is refused during validation.
    pub fn load_selected(
        projection: &crate::selection::ValidatedInspectionProject,
        temporary_root: &Path,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Self, RustMetadataError> {
        projection.verify_inputs()?;
        let temporary_root = temporary_root.canonicalize()?;
        if projection.contains_input_path(&temporary_root) {
            return Err(RustMetadataError::InvalidSelectedInput {
                path: temporary_root,
                message: "inspection temporary output must be outside selected input roots".to_string(),
            });
        }
        let temporary = InspectionTemporaryProject::create(&temporary_root, projection.payload())?;
        let base_text = temporary
            .directory
            .to_str()
            .ok_or_else(|| RustMetadataError::InvalidSelectedInput {
                path: temporary.directory.clone(),
                message: "inspection temporary path must be UTF-8".to_string(),
            })?;
        let base = AbsPathBuf::try_from(base_text).map_err(|_| RustMetadataError::InvalidSelectedInput {
            path: temporary.directory.clone(),
            message: "inspection temporary path must be absolute".to_string(),
        })?;
        let data = projection.project_data()?;
        let project = ProjectJson::new(None, &base, data);
        // Construct the existing neutral consumer directly. load_workspace_at/load_inline also discover sysroot
        // metadata and can invoke Cargo even when the outer project is JSON.
        let workspace = ProjectWorkspace {
            kind: ProjectWorkspaceKind::Json(project),
            sysroot: Sysroot::empty(),
            rustc_cfg: Vec::new(),
            toolchain: Some(Version::parse(projection.toolchain_version()).map_err(|error| {
                RustMetadataError::InvalidSelectedInput {
                    path: temporary.directory.clone(),
                    message: format!("invalid selected toolchain version: {error}"),
                }
            })?),
            target: Ok(projection.target_data()?),
            cfg_overrides: Default::default(),
            extra_includes: Vec::new(),
            set_test: false,
        };
        let config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::None,
            prefill_caches: false,
            num_worker_threads: 1,
            proc_macro_processes: 1,
        };
        progress("loading selected Rust inspection inputs".to_string());
        let (db, vfs, _client) = load_workspace(workspace, &Default::default(), &config).map_err(|error| {
            RustMetadataError::LoadWorkspace {
                path: temporary.directory.clone(),
                message: error.to_string(),
            }
        })?;
        projection.verify_inputs()?;
        let mut crate_index = HashMap::new();
        for (alias, root) in projection.query_roots() {
            let mut matches = Crate::all(&db).into_iter().filter(|krate| {
                vfs.file_path(krate.root_file(&db))
                    .as_path()
                    .is_some_and(|path| path == root.as_path())
            });
            let Some(krate) = matches.next() else {
                return Err(RustMetadataError::InvalidSelectedInput {
                    path: root.clone(),
                    message: format!("selected query binding `{alias}` was not loaded"),
                });
            };
            if matches.next().is_some() {
                return Err(RustMetadataError::InvalidSelectedInput {
                    path: root.clone(),
                    message: format!("selected query binding `{alias}` identifies multiple units"),
                });
            }
            crate_index.insert(alias.clone(), krate);
        }
        temporary.close()?;
        Ok(Self {
            db,
            selection_fingerprint: projection.fingerprint().to_string(),
            crate_index,
            vfs,
        })
    }

    /// Shared read-only access to the underlying database.
    pub fn db(&self) -> &RootDatabase {
        &self.db
    }

    /// Resolve only an explicitly selected query alias; no package-name fallback is available.
    pub fn crate_by_name(&self, crate_name: &str) -> Option<Crate> {
        self.crate_index.get(crate_name).copied()
    }
}

/// An owned temporary projection; every error path removes only this invocation's files.
struct InspectionTemporaryProject {
    directory: PathBuf,
}

impl InspectionTemporaryProject {
    /// Create a unique directory without reusing another process's projection path.
    fn create(root: &Path, payload: &[u8]) -> Result<Self, RustMetadataError> {
        loop {
            let sequence = OVEN_PROJECT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let directory = root.join(format!("incan-inspect-{}-{sequence}", std::process::id()));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let owned = Self { directory };
                    // Keep the recognized filename even though loading never performs path discovery.
                    fs::write(owned.directory.join("rust-project.json"), payload)?;
                    return Ok(owned);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Remove the owned descriptor and directory, reporting cleanup failure on a successful load.
    fn close(&self) -> Result<(), RustMetadataError> {
        fs::remove_file(self.directory.join("rust-project.json"))?;
        fs::remove_dir(&self.directory)?;
        Ok(())
    }
}

impl Drop for InspectionTemporaryProject {
    /// Best-effort cleanup also runs when projection creation or database loading fails.
    fn drop(&mut self) {
        let _ = fs::remove_file(self.directory.join("rust-project.json"));
        let _ = fs::remove_dir(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    include!("selection_tests.rs");
}
