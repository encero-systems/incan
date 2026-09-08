//! Load a Cargo tree into rust-analyzer's `RootDatabase`.
//!
//! This module is intentionally behind the rust-inspect preparation/cache boundary. It owns the unstable rust-analyzer
//! embedding details so parser/typechecker/codegen code does not load Cargo workspaces directly.

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_ide_db::base_db::RootQueryDb;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{CargoConfig, RustLibSource};
use ra_ap_vfs::Vfs;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    /// Keep the external proc-macro process alive for lazy semantic queries such as derive expansion. The concrete
    /// client stays opaque here: rust-inspect owns its lifetime, but must not introduce a second direct instance of
    /// rust-analyzer's proc-macro API crate into Oven's self-hosted compiler graph.
    #[allow(dead_code)]
    proc_macro_client: Option<Box<dyn Any + Send + Sync>>,
}

/// A sequence scoped to this process keeps generated direct-project descriptions independent when libtests run in
/// parallel. The descriptions live under the caller-managed inspection output, never beside an inspected source
/// tree or in Cargo's cache.
static OVEN_PROJECT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Resolve the proc-macro server installed beside the active Rust compiler.
fn active_proc_macro_server() -> ProcMacroServerChoice {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let Ok(output) = std::process::Command::new(rustc).args(["--print", "sysroot"]).output() else {
        return ProcMacroServerChoice::Sysroot;
    };
    if !output.status.success() {
        return ProcMacroServerChoice::Sysroot;
    }
    let Some(sysroot) = std::str::from_utf8(&output.stdout)
        .ok()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return ProcMacroServerChoice::Sysroot;
    };
    for candidate in proc_macro_server_candidates(Path::new(sysroot), std::env::consts::EXE_SUFFIX) {
        if candidate.is_file() {
            return ProcMacroServerChoice::Explicit(AbsPathBuf::assert_utf8(candidate));
        }
    }
    ProcMacroServerChoice::Sysroot
}

/// Return the active-toolchain proc-macro-server locations in rustup and standalone-install order.
fn proc_macro_server_candidates(sysroot: &Path, executable_suffix: &str) -> [PathBuf; 2] {
    let server_name = format!("rust-analyzer-proc-macro-srv{executable_suffix}");
    [
        sysroot.join("libexec").join(&server_name),
        sysroot.join("lib").join(server_name),
    ]
}

/// Compiler-authored marker for an inspection projection that must use the direct rust-project loader.
///
/// The marker lives only in a generated inspection directory. It keeps the selection local to the prepared Oven
/// invocation instead of relying on an ambient environment variable that could accidentally make a legacy Cargo
/// inspection session lose build-script support.
pub const OVEN_DIRECT_INSPECTION_MARKER: &str = ".incan_oven_direct_rust_project";
/// Compiler-authored source authority consumed by the direct Oven inspection loader.
pub const OVEN_DIRECT_INSPECTION_AUTHORITY_FILE: &str = ".incan_oven_rust_sources.json";
/// Sealed build-script output directories (one absolute `OUT_DIR` per line) a direct-inspection workspace may read.
///
/// Normal Oven commands never run Cargo, so generated Rust such as prost's `include!`d modules is reachable only
/// through the immutable plan Loafs that compiled it. The installer lists those directories here and the
/// generated-code route scans them the way it scans a Cargo target directory.
pub const OVEN_GENERATED_OUT_DIRS_FILE: &str = ".incan_oven_generated_out_dirs";

/// One sealed build-script output directory a direct-inspection workspace may read, with the exact package version
/// whose build script wrote it when the sealing bake knew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedGeneratedOutDir {
    pub out_dir: PathBuf,
    pub version: Option<String>,
}

/// Record the sealed build-script output directories a direct-inspection workspace may read.
///
/// One directory per line, followed by a tab and the package version when known. A closure can hold several build
/// units of one package (two `substrait` versions, one for the project and one for a DataFusion adapter), and the
/// version is what lets the generated-code route read the unit that belongs to the dependency being inspected.
pub fn write_oven_generated_out_dirs(
    manifest_dir: &Path,
    out_dirs: &[SealedGeneratedOutDir],
) -> Result<PathBuf, RustMetadataError> {
    let mut lines: Vec<String> = out_dirs
        .iter()
        .map(|dir| match dir.version.as_deref() {
            Some(version) => format!("{}\t{version}", dir.out_dir.to_string_lossy()),
            None => dir.out_dir.to_string_lossy().into_owned(),
        })
        .collect();
    lines.sort();
    lines.dedup();
    let path = manifest_dir.join(OVEN_GENERATED_OUT_DIRS_FILE);
    fs::write(&path, format!("{}\n", lines.join("\n")))?;
    Ok(path)
}

/// Read the sealed build-script output directories recorded for a direct-inspection workspace, if any.
pub(crate) fn read_oven_generated_out_dirs(manifest_dir: &Path) -> Vec<SealedGeneratedOutDir> {
    fs::read_to_string(manifest_dir.join(OVEN_GENERATED_OUT_DIRS_FILE))
        .map(|text| {
            text.lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| match line.split_once('\t') {
                    Some((out_dir, version)) => SealedGeneratedOutDir {
                        out_dir: PathBuf::from(out_dir),
                        version: Some(version.trim().to_string()).filter(|version| !version.is_empty()),
                    },
                    None => SealedGeneratedOutDir {
                        out_dir: PathBuf::from(line),
                        version: None,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build-script output directories keyed by the exact package whose build script wrote them, written beside a
/// workspace by whichever component ran Cargo and therefore knows the mapping: `.incan_generated_out_dirs.json`.
///
/// Cargo names a build unit by a metadata hash the directory layout does not decode, so a directory scan alone cannot
/// tell two versions of one package apart. rust-analyzer's crate graph and Cargo's `build-script-executed` messages
/// both carry the package version beside the `OUT_DIR`; this file is that knowledge, persisted for the explicit bake.
pub const GENERATED_OUT_DIRS_MAP_FILE: &str = ".incan_generated_out_dirs.json";

/// One build-script output directory and the exact package whose build script wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedOutDirRecord {
    pub package: String,
    pub version: String,
    pub out_dir: PathBuf,
}

/// Write the package-keyed build-script output map beside a workspace.
pub fn write_generated_out_dirs_map(
    dir: &Path,
    records: &[GeneratedOutDirRecord],
) -> Result<PathBuf, RustMetadataError> {
    let mut records = records.to_vec();
    records.sort_by(|left, right| {
        (&left.package, &left.version, &left.out_dir).cmp(&(&right.package, &right.version, &right.out_dir))
    });
    records.dedup();
    let path = dir.join(GENERATED_OUT_DIRS_MAP_FILE);
    let payload =
        serde_json::to_string_pretty(&records).map_err(|error| RustMetadataError::Io(std::io::Error::other(error)))?;
    fs::write(&path, payload)?;
    Ok(path)
}

/// Read the package-keyed build-script output map beside a workspace, if one was written.
pub fn read_generated_out_dirs_map(dir: &Path) -> Vec<GeneratedOutDirRecord> {
    fs::read_to_string(dir.join(GENERATED_OUT_DIRS_MAP_FILE))
        .ok()
        .and_then(|payload| serde_json::from_str(&payload).ok())
        .unwrap_or_default()
}
const OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION: u32 = 2;

/// How a compiler-authored direct-inspection projection establishes its registry-source integrity boundary.
///
/// A standalone projection verifies the complete tree itself. A projection installed from the current Oven selection
/// relies on the selected, leased Loaf's verification instead: normal consumers already validate the receipt, plan,
/// artifact-root containment, and file shape there. Rehashing every retained Rust source tree again would turn a
/// prepared build into a multi-gigabyte integrity scan. Explicit `incan oven inspect` remains the full-closure audit.
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

/// Complete source authority for one build-system-neutral Rust inspection projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenInspectionSourceAuthority {
    schema_version: u32,
    #[serde(default)]
    source_validation: OvenInspectionSourceValidation,
    sources: Vec<OvenInspectionRegistrySource>,
}

/// Write a standalone direct-inspection authority that verifies every source tree before loading it.
///
/// This is deliberately the conservative default for callers outside the Oven selection path.
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

/// Write authority from the current selected Oven Loaf or publisher closure.
///
/// The caller must have obtained `sources` from the current receipt-bound selection or the baker's freshly verified
/// locked source closure. This avoids repeating that same full-tree validation inside the metadata loader.
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
/// digest validation must use the direct-inspection workspace loader. The typechecker uses these roots solely to
/// constrain later fast source-metadata lookup; it does not rediscover Cargo's ambient cache.
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

/// One local source crate in the direct rust-analyzer graph used by a sealed Oven consumer.
struct OvenProjectCrate {
    display_name: String,
    root_module: PathBuf,
    edition: String,
    is_proc_macro: bool,
    dependencies: Vec<OvenProjectDependency>,
    cfg: Vec<String>,
}

/// One resolved edge in the source graph used for direct Oven inspection.
///
/// A lockless compiler-owned Loaf may safely walk local path dependencies to their full closure, but it must not infer
/// a transitive registry closure from whichever sources happen to be cached on the machine. Keeping that distinction on
/// the edge rather than in a global recursion limit preserves intrinsic traits from local dependencies.
#[derive(Clone)]
struct OvenProjectDependency {
    name: String,
    source_dir: PathBuf,
    is_local_path: bool,
    features: Vec<String>,
}

/// One normal dependency declaration and its explicitly selected local feature inputs.
struct OvenDependencyDeclaration {
    name: String,
    package: String,
    path: Option<PathBuf>,
    version: Option<String>,
    features: Vec<String>,
}

/// A package entry from the already-resolved lockfile for a direct Oven project.
///
/// This is deliberately much smaller than Cargo's resolver model: direct inspection consumes the exact locked
/// graph and locally-present source trees; it does not resolve versions, update a lockfile, or contact a registry.
struct OvenLockedPackage {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
    dependencies: Vec<String>,
}

struct OvenProjectLock {
    packages: Vec<OvenLockedPackage>,
}

/// Hash one source tree by portable path and exact bytes, matching Oven's Loaf source identity.
fn digest_oven_source_tree(root: &Path) -> Result<String, RustMetadataError> {
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
    fn normalize_crate_name(name: &str) -> String {
        name.replace('-', "_")
    }

    /// Insert every rust-analyzer spelling for one crate without replacing a higher-authority entry.
    fn insert_crate_names(index: &mut HashMap<String, Crate>, krate: Crate, db: &RootDatabase) {
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

    /// Build the crate-name index with root direct dependencies taking precedence over duplicate transitive names.
    fn build_crate_index(db: &RootDatabase) -> HashMap<String, Crate> {
        let mut index = HashMap::new();
        // A Cargo graph may contain multiple versions of the same canonical crate name. Rust source at the generated
        // project root resolves that spelling through the root's direct dependency edge, not whichever transitive
        // version happens to appear first in rust-analyzer's crate enumeration. Seed those named edges before the
        // graph-wide fallback so inspection follows the same namespace authority as rustc.
        for root in Crate::all(db)
            .into_iter()
            .filter(|krate| !krate.is_builtin(db) && krate.reverse_dependencies(db).is_empty())
        {
            Self::insert_crate_names(&mut index, root, db);
            for dependency in root.dependencies(db) {
                index
                    .entry(Self::normalize_crate_name(dependency.name.as_str()))
                    .or_insert(dependency.krate);
                Self::insert_crate_names(&mut index, dependency.krate, db);
            }
        }
        for krate in Crate::all(db) {
            Self::insert_crate_names(&mut index, krate, db);
        }
        index
    }

    /// Whether this inspection workspace belongs to a receipt-bound direct-Rustc Oven consumer.
    ///
    /// The normal compiler has no reason to ask rust-analyzer to rediscover a Cargo graph: Oven either supplies a
    /// sealed provider ABI or rejects the unsupported dynamic request. The legacy publisher retains the historical
    /// Cargo loader. Every prepared direct-inspection route writes the workspace-local marker so concurrent legacy
    /// calls remain on their explicitly selected route.
    pub(crate) fn oven_direct_inspection_active(manifest_dir: &Path) -> bool {
        manifest_dir.join(OVEN_DIRECT_INSPECTION_MARKER).is_file()
    }

    /// Materialize a minimal rust-analyzer project description for one compiler-authored manifest without invoking
    /// Cargo. `rust-project.json` is rust-analyzer's documented build-system interface; absolute source paths keep
    /// the descriptor independent from the caller's working directory.
    #[cfg(test)]
    fn oven_project_json_payload(manifest_dir: &Path) -> Result<Vec<u8>, RustMetadataError> {
        Self::oven_project_json_payload_with_source_authority(manifest_dir)
    }

    /// Load the sealed compiler-suite source graph with rust-analyzer's build-system-neutral interface.
    fn load_oven_project(
        manifest_dir: &Path,
        target_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        _load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        let manifest_dir = manifest_dir.canonicalize()?;
        let payload = Self::oven_project_json_payload_with_source_authority(&manifest_dir)?;
        let sequence = OVEN_PROJECT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let project_dir = target_dir
            .join("incan-oven-rust-projects")
            .join(format!("{}-{sequence}", std::process::id()));
        fs::create_dir_all(&project_dir)?;
        // rust-analyzer recognizes only this exact filename when discovering a build-system-neutral graph. A suffix
        // such as `*.rust-project.json` makes it climb to an ancestor Cargo.toml and silently reintroduce Cargo.
        let project_path = project_dir.join("rust-project.json");
        fs::write(&project_path, payload)?;
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::None,
            prefill_caches: false,
            num_worker_threads: 1,
            proc_macro_processes: 1,
        };
        let result =
            load_workspace_at(&project_path, &CargoConfig::default(), &load_config, progress).map_err(|error| {
                RustMetadataError::LoadWorkspace {
                    path: manifest_dir.clone(),
                    message: error.to_string(),
                }
            });
        let _ = fs::remove_file(&project_path);
        let _ = fs::remove_dir(&project_dir);
        let (db, vfs, _pm) = result?;
        let crate_index = Self::build_crate_index(&db);
        Ok(RustWorkspace {
            db,
            crate_index,
            vfs,
            proc_macro_client: None,
        })
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

    /// Load only the explicitly selected Oven inspection project. Cargo discovery is unavailable.
    pub(crate) fn load_with_options_and_target(
        manifest_dir: &Path,
        target_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        Self::load_oven_project(manifest_dir, target_dir, progress, load_out_dirs_from_check)
    }

    /// Persist which package version each build-script `OUT_DIR` in the loaded crate graph belongs to.
    ///
    /// rust-analyzer ran the build scripts and injected `OUT_DIR` beside `CARGO_PKG_NAME` and `CARGO_PKG_VERSION`
    /// into every crate's environment. The explicit bake seals those directories for Cargo-free inspection and needs
    /// the version to seal them under, because a closure can hold several build units of one package.
    fn record_generated_out_dirs(manifest_dir: &Path, db: &RootDatabase) -> Result<(), RustMetadataError> {
        let mut records = Vec::new();
        for krate in db.all_crates().iter() {
            let env = krate.env(db);
            let (Some(out_dir), Some(package), Some(version)) = (
                env.get("OUT_DIR"),
                env.get("CARGO_PKG_NAME"),
                env.get("CARGO_PKG_VERSION"),
            ) else {
                continue;
            };
            records.push(GeneratedOutDirRecord {
                package,
                version,
                out_dir: PathBuf::from(out_dir),
            });
        }
        if records.is_empty() {
            return Ok(());
        }
        write_generated_out_dirs_map(manifest_dir, &records)?;
        Ok(())
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

    use super::{
        OVEN_DIRECT_INSPECTION_MARKER, OvenInspectionRegistrySource, RustWorkspace, digest_oven_source_tree,
        proc_macro_server_candidates, write_oven_inspection_source_authority,
        write_sealed_oven_inspection_source_authority,
    };

    use tempfile::tempdir;

    #[test]
    fn proc_macro_server_candidates_preserve_windows_executable_suffix() {
        let candidates = proc_macro_server_candidates(std::path::Path::new("toolchain"), ".exe");
        assert_eq!(
            candidates,
            [
                std::path::PathBuf::from("toolchain/libexec/rust-analyzer-proc-macro-srv.exe"),
                std::path::PathBuf::from("toolchain/lib/rust-analyzer-proc-macro-srv.exe"),
            ]
        );
    }

    #[test]
    fn metadata_loader_allows_cargo_to_resolve_uncached_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let cargo_config = RustWorkspace::metadata_cargo_config(&workspace.path().join("target"), false);
        assert!(
            cargo_config.sysroot.is_none(),
            "ordinary source inspection must not load the complete Rust sysroot"
        );
        assert!(
            !cargo_config.extra_args.iter().any(|arg| arg == "--offline"),
            "rust-inspect workspace loads must not force offline metadata resolution"
        );
        assert_eq!(
            cargo_config.extra_env.get("CARGO_NET_OFFLINE"),
            None,
            "rust-inspect workspace loads must not force Cargo into offline mode"
        );
        let bootstrap_config = RustWorkspace::metadata_cargo_config(&workspace.path().join("target"), true);
        assert_eq!(
            bootstrap_config.sysroot,
            Some(ra_ap_project_model::RustLibSource::Discover)
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
        let cargo_config = RustWorkspace::metadata_cargo_config(&resolved_target, false);
        let expected = Some(configured_target.to_string_lossy().into_owned());
        assert_eq!(cargo_config.extra_env.get("CARGO_TARGET_DIR"), Some(&expected));
        assert_eq!(cargo_config.extra_env.get("CARGO_BUILD_BUILD_DIR"), Some(&expected));
        Ok(())
    }

    #[test]
    fn direct_oven_inspection_uses_only_the_workspace_marker() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        assert!(
            !RustWorkspace::oven_direct_inspection_active(workspace.path()),
            "ambient suite state must not change an unrelated inspection workspace"
        );
        fs::write(workspace.path().join(OVEN_DIRECT_INSPECTION_MARKER), b"direct\n")?;
        assert!(
            RustWorkspace::oven_direct_inspection_active(workspace.path()),
            "the compiler-authored marker must select the direct rust-project route"
        );
        Ok(())
    }

    #[test]
    fn direct_oven_project_uses_binary_root_when_no_library_root_exists() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"oven-inspect-bin\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let binary_root = workspace.path().join("src/main.rs");
        fs::write(&binary_root, "fn main() {}\n")?;

        let payload = RustWorkspace::oven_project_json_payload(workspace.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        let root_module = graph["crates"][0]["root_module"]
            .as_str()
            .ok_or("direct Oven project omitted the binary root module")?;
        assert_eq!(root_module, binary_root.canonicalize()?.to_string_lossy());
        Ok(())
    }

    #[test]
    fn direct_oven_project_preserves_proc_macro_crate_identity() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let tuple_driver = workspace.path().join("tuple-driver");
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::create_dir_all(tuple_driver.join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n\n[dependencies]\ntuple-driver = { path = \"tuple-driver\" }\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            tuple_driver.join("Cargo.toml"),
            "[package]\nname = \"tuple-driver\"\nversion = \"0.1.0\"\n\n[lib]\nproc-macro = true\n",
        )?;
        fs::write(tuple_driver.join("src/lib.rs"), "extern crate proc_macro;\n")?;
        write_oven_inspection_source_authority(workspace.path(), Vec::new())?;

        let payload = RustWorkspace::oven_project_json_payload(workspace.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        let crates = graph["crates"].as_array().ok_or("direct Oven graph omitted crates")?;
        let root = crates
            .iter()
            .find(|candidate| candidate["display_name"] == "root")
            .ok_or("direct Oven graph omitted root")?;
        let proc_macro = crates
            .iter()
            .find(|candidate| candidate["display_name"] == "tuple_driver")
            .ok_or("direct Oven graph omitted proc-macro dependency")?;
        assert_eq!(root["is_proc_macro"], false);
        assert_eq!(proc_macro["is_proc_macro"], true);
        Ok(())
    }

    #[test]
    fn direct_oven_project_rejects_non_boolean_proc_macro_declarations() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n\n[lib]\nproc-macro = \"yes\"\n",
        )?;
        fs::write(workspace.path().join("src/lib.rs"), "pub fn api() {}\n")?;

        let error = RustWorkspace::oven_project_json_payload(workspace.path())
            .expect_err("a malformed proc-macro declaration must not be silently treated as a normal crate");
        assert!(error.to_string().contains("lib.proc-macro to be a boolean"));
        Ok(())
    }

    #[test]
    fn direct_oven_project_unifies_local_features_reached_through_multiple_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let shared = workspace.path().join("shared");
        let bridge = workspace.path().join("bridge");
        for root in [workspace.path(), shared.as_path(), bridge.as_path()] {
            fs::create_dir_all(root.join("src"))?;
        }
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n\n[dependencies]\nshared = { path = \"shared\", features = [\"left\"] }\nbridge = { path = \"bridge\" }\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            shared.join("Cargo.toml"),
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\n\n[features]\nleft = []\nright = []\n",
        )?;
        fs::write(shared.join("src/lib.rs"), "pub fn shared() {}\n")?;
        fs::write(
            bridge.join("Cargo.toml"),
            "[package]\nname = \"bridge\"\nversion = \"0.1.0\"\n\n[dependencies]\nshared = { path = \"../shared\", features = [\"right\"] }\n",
        )?;
        fs::write(bridge.join("src/lib.rs"), "pub fn bridge() {}\n")?;
        write_oven_inspection_source_authority(workspace.path(), Vec::new())?;

        let payload = RustWorkspace::oven_project_json_payload(workspace.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        let crates = graph["crates"].as_array().ok_or("direct Oven graph omitted crates")?;
        let shared_crate = crates
            .iter()
            .find(|candidate| candidate["display_name"] == "shared")
            .ok_or("direct Oven graph omitted the shared local crate")?;
        let cfg = shared_crate["cfg"].as_array().ok_or("shared local crate omitted cfg")?;
        assert!(cfg.iter().any(|value| value == "feature=\"left\""));
        assert!(cfg.iter().any(|value| value == "feature=\"right\""));
        Ok(())
    }

    #[test]
    fn direct_oven_project_uses_only_locked_sealed_registry_sources() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let sealed = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"oven-inspect-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ndemo = { version = \"1\", features = [\"selected\"] }\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            workspace.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"oven-inspect-fixture\"\nversion = \"0.1.0\"\ndependencies = [\"demo 1.0.0 (registry+https://example.invalid/index)\"]\n\n[[package]]\nname = \"demo\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\nchecksum = \"demo-checksum\"\ndependencies = [\"leaf\"]\n\n[[package]]\nname = \"leaf\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\nchecksum = \"leaf-checksum\"\n",
        )?;
        let mut authority = Vec::new();
        for (name, checksum, features, source) in [
            (
                "demo",
                "demo-checksum",
                vec!["selected".to_string()],
                "pub fn demo() {}\n",
            ),
            ("leaf", "leaf-checksum", Vec::new(), "pub fn leaf() {}\n"),
        ] {
            let package = sealed.path().join(format!("{name}-1.0.0"));
            fs::create_dir_all(package.join("src"))?;
            let dependencies = if name == "demo" {
                "\n[dependencies]\nleaf = \"1\"\n"
            } else {
                ""
            };
            let feature_table = if name == "demo" {
                "\n[features]\nselected = []\nhidden = []\n"
            } else {
                ""
            };
            fs::write(
                package.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n{dependencies}{feature_table}"
                ),
            )?;
            fs::write(package.join("src/lib.rs"), source)?;
            authority.push(OvenInspectionRegistrySource {
                package: name.to_string(),
                version: "1.0.0".to_string(),
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: checksum.to_string(),
                features,
                source_digest: digest_oven_source_tree(&package)?,
                source_root: package,
            });
        }
        write_oven_inspection_source_authority(workspace.path(), authority)?;

        let payload = RustWorkspace::oven_project_json_payload(workspace.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        let crates = graph["crates"].as_array().ok_or("direct Oven graph omitted crates")?;
        assert_eq!(
            crates.len(),
            3,
            "the root and both locked registry sources must be present"
        );
        assert_eq!(crates[0]["deps"][0]["name"], "demo");
        assert_eq!(crates[1]["display_name"], "demo");
        assert_eq!(crates[1]["deps"][0]["name"], "leaf");
        assert_eq!(crates[2]["display_name"], "leaf");
        let demo_cfg = crates[1]["cfg"].as_array().ok_or("demo crate omitted cfg")?;
        assert!(demo_cfg.iter().any(|cfg| cfg == "feature=\"selected\""));
        assert!(!demo_cfg.iter().any(|cfg| cfg == "feature=\"hidden\""));
        let loaded = RustWorkspace::load_oven_project(
            workspace.path(),
            &workspace.path().join("inspection-target"),
            &|_| {},
            false,
        )?;
        assert!(
            loaded.crate_by_name("demo").is_some(),
            "rust-analyzer must expose the sealed registry crate without asking Cargo to build the graph"
        );
        Ok(())
    }

    #[test]
    fn unlocked_baker_authority_does_not_infer_a_transitive_registry_version() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempdir()?;
        let sealed = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"oven-unlocked-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ndirect = \"1\"\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;

        let mut authority = Vec::new();
        for (name, version, dependencies) in [
            ("direct", "1.0.0", "\n[dependencies]\ngetrandom = \">=0.3, <0.5\"\n"),
            ("getrandom", "0.3.0", ""),
            ("getrandom", "0.4.0", ""),
        ] {
            let package = sealed.path().join(format!("{name}-{version}"));
            fs::create_dir_all(package.join("src"))?;
            fs::write(
                package.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n{dependencies}"),
            )?;
            fs::write(package.join("src/lib.rs"), format!("pub fn {name}_api() {{}}\n"))?;
            authority.push(OvenInspectionRegistrySource {
                package: name.to_string(),
                version: version.to_string(),
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: format!("{name}-{version}-checksum"),
                features: Vec::new(),
                source_digest: digest_oven_source_tree(&package)?,
                source_root: package,
            });
        }
        write_oven_inspection_source_authority(workspace.path(), authority)?;

        let payload = RustWorkspace::oven_project_json_payload(workspace.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        let crates = graph["crates"].as_array().ok_or("direct Oven graph omitted crates")?;
        assert_eq!(
            crates.len(),
            2,
            "the root and its one directly authorized registry source must be present"
        );
        assert_eq!(crates[1]["display_name"], "direct");
        Ok(())
    }

    #[test]
    fn direct_oven_project_rejects_source_digest_and_lock_checksum_mismatches() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempdir()?;
        let source = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\n\n[dependencies]\ndemo = \"1\"\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            workspace.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"root\"\nversion = \"0.1.0\"\ndependencies = [\"demo\"]\n\n[[package]]\nname = \"demo\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\nchecksum = \"locked-checksum\"\n",
        )?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn demo() {}\n")?;
        let source_digest = digest_oven_source_tree(source.path())?;
        write_oven_inspection_source_authority(
            workspace.path(),
            vec![OvenInspectionRegistrySource {
                package: "demo".to_string(),
                version: "1.0.0".to_string(),
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "wrong-checksum".to_string(),
                features: Vec::new(),
                source_root: source.path().to_path_buf(),
                source_digest,
            }],
        )?;
        let checksum_error = match RustWorkspace::oven_project_json_payload(workspace.path()) {
            Ok(_) => return Err("a mismatched lock checksum must not resolve a sealed source".into()),
            Err(error) => error,
        };
        assert!(checksum_error.to_string().contains("no sealed Oven source matches"));

        let digest = digest_oven_source_tree(source.path())?;
        write_oven_inspection_source_authority(
            workspace.path(),
            vec![OvenInspectionRegistrySource {
                package: "demo".to_string(),
                version: "1.0.0".to_string(),
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "locked-checksum".to_string(),
                features: Vec::new(),
                source_root: source.path().to_path_buf(),
                source_digest: digest,
            }],
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn changed() {}\n")?;
        let digest_error = match RustWorkspace::oven_project_json_payload(workspace.path()) {
            Ok(_) => return Err("a changed sealed source must not pass its recorded digest".into()),
            Err(error) => error,
        };
        assert!(digest_error.to_string().contains("expected"));
        Ok(())
    }

    #[test]
    fn selected_oven_authority_uses_the_existing_selection_boundary_without_rehashing_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let source = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\n\n[dependencies]\ndemo = \"1\"\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            workspace.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"root\"\nversion = \"1.0.0\"\ndependencies = [\"demo\"]\n\n[[package]]\nname = \"demo\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\nchecksum = \"locked-checksum\"\n",
        )?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn original() {}\n")?;
        let source_digest = digest_oven_source_tree(source.path())?;
        write_sealed_oven_inspection_source_authority(
            workspace.path(),
            vec![OvenInspectionRegistrySource {
                package: "demo".to_string(),
                version: "1.0.0".to_string(),
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "locked-checksum".to_string(),
                features: Vec::new(),
                source_root: source.path().to_path_buf(),
                source_digest,
            }],
        )?;

        fs::write(
            source.path().join("src/lib.rs"),
            "pub fn changed_after_selection() {}\n",
        )?;
        let payload = RustWorkspace::oven_project_json_payload(workspace.path())?;
        let graph: serde_json::Value = serde_json::from_slice(&payload)?;
        let crates = graph["crates"].as_array().ok_or("direct Oven graph omitted crates")?;
        assert_eq!(
            crates.len(),
            2,
            "the selected source remains available through its locked identity"
        );
        assert_eq!(crates[1]["display_name"], "demo");
        Ok(())
    }
}
