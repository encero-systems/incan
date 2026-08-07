//! Load a Cargo tree into rust-analyzer's `RootDatabase`.
//!
//! This module is intentionally behind the rust-inspect preparation/cache boundary. It owns the unstable rust-analyzer
//! embedding details so parser/typechecker/codegen code does not load Cargo workspaces directly.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::Vfs;
use semver::{Version, VersionReq};

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

/// A sequence scoped to this process keeps generated direct-project descriptions independent when libtests run in
/// parallel. The descriptions live under the caller-managed inspection output, never beside an inspected source
/// tree or in Cargo's cache.
static OVEN_PROJECT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Compiler-authored marker for an inspection projection that must use the direct rust-project loader.
///
/// The marker lives only in a generated inspection directory. It keeps the selection local to the prepared Oven
/// invocation instead of relying on an ambient environment variable that could accidentally make a legacy Cargo
/// inspection session lose build-script support.
const OVEN_DIRECT_INSPECTION_MARKER: &str = ".incan_oven_direct_rust_project";

/// One local source crate in the direct rust-analyzer graph used by a sealed Oven consumer.
struct OvenProjectCrate {
    display_name: String,
    root_module: PathBuf,
    edition: String,
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
}

/// A package entry from the already-resolved lockfile for a direct Oven project.
///
/// This is deliberately much smaller than Cargo's resolver model: direct inspection consumes the exact locked
/// graph and locally-present source trees; it does not resolve versions, update a lockfile, or contact a registry.
struct OvenLockedPackage {
    name: String,
    version: String,
    source: Option<String>,
    dependencies: Vec<String>,
}

struct OvenProjectLock {
    packages: Vec<OvenLockedPackage>,
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

    /// Whether this inspection workspace belongs to a receipt-bound direct-Rustc Oven consumer.
    ///
    /// The normal compiler has no reason to ask rust-analyzer to rediscover a Cargo graph: Oven either supplies a
    /// sealed provider ABI or rejects the unsupported dynamic request. The legacy publisher retains the historical
    /// Cargo loader. Compiler-suite children use the environment capability, while ordinary Oven commands write the
    /// generated-workspace marker so concurrent legacy calls remain on their explicitly selected route.
    pub(crate) fn oven_direct_inspection_active(manifest_dir: &Path) -> bool {
        std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC").is_some()
            || manifest_dir.join(OVEN_DIRECT_INSPECTION_MARKER).is_file()
    }

    /// Materialize a minimal rust-analyzer project description for one compiler-authored manifest without invoking
    /// Cargo. `rust-project.json` is rust-analyzer's documented build-system interface; absolute source paths keep
    /// the descriptor independent from the caller's working directory.
    #[cfg(test)]
    fn oven_project_json_payload(manifest_dir: &Path) -> Result<Vec<u8>, RustMetadataError> {
        Self::oven_project_json_payload_with_cargo_home(manifest_dir, None)
    }

    /// Build the direct source graph from an exact lockfile and an already-present Cargo source cache.
    ///
    /// `cargo_home` exists primarily to make the source-only resolution contract testable without touching process
    /// environment. Production calls discover Cargo's conventional cache directory, but never execute Cargo.
    fn oven_project_json_payload_with_cargo_home(
        manifest_dir: &Path,
        cargo_home: Option<&Path>,
    ) -> Result<Vec<u8>, RustMetadataError> {
        /// Read the non-build dependency declarations that can contribute source crates to the direct graph.
        fn dependency_declarations(
            manifest: &toml::Value,
            manifest_dir: &Path,
        ) -> Vec<(String, String, Option<PathBuf>, Option<String>)> {
            /// Normalize one Cargo dependency table into direct-graph source candidates.
            fn collect(
                table: Option<&toml::Value>,
                manifest_dir: &Path,
                dependencies: &mut Vec<(String, String, Option<PathBuf>, Option<String>)>,
            ) {
                let Some(table) = table.and_then(toml::Value::as_table) else {
                    return;
                };
                for (dependency_name, declaration) in table {
                    let package_name = declaration
                        .get("package")
                        .and_then(toml::Value::as_str)
                        .unwrap_or(dependency_name)
                        .to_string();
                    let path = declaration
                        .get("path")
                        .and_then(toml::Value::as_str)
                        .map(|path| manifest_dir.join(path))
                        .filter(|candidate| candidate.join("Cargo.toml").is_file());
                    let version = declaration.as_str().map(str::to_string).or_else(|| {
                        declaration
                            .get("version")
                            .and_then(toml::Value::as_str)
                            .map(str::to_string)
                    });
                    dependencies.push((dependency_name.replace('-', "_"), package_name, path, version));
                }
            }

            let mut dependencies = Vec::new();
            // Direct inspection needs the runtime crate graph. Dev-only and build-script dependencies are neither
            // linked into the sealed Loaf nor available through this no-Cargo projection.
            collect(manifest.get("dependencies"), manifest_dir, &mut dependencies);
            if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
                for target in targets.values() {
                    collect(target.get("dependencies"), manifest_dir, &mut dependencies);
                }
            }
            dependencies.sort();
            dependencies.dedup();
            dependencies
        }

        /// Load the package lockfile when it is present, retaining only data needed for source resolution.
        fn load_lock(manifest_dir: &Path) -> Result<Option<OvenProjectLock>, RustMetadataError> {
            let lock_path = manifest_dir.join("Cargo.lock");
            if !lock_path.is_file() {
                return Ok(None);
            }
            let lock = toml::from_str::<toml::Value>(&fs::read_to_string(&lock_path)?).map_err(|error| {
                RustMetadataError::LoadWorkspace {
                    path: lock_path.clone(),
                    message: format!("failed to parse compiler-authored lockfile for direct Oven inspection: {error}"),
                }
            })?;
            let packages = lock
                .get("package")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|package| {
                    let name = package.get("name")?.as_str()?.to_string();
                    let version = package.get("version")?.as_str()?.to_string();
                    let source = package.get("source").and_then(toml::Value::as_str).map(str::to_string);
                    let dependencies = package
                        .get("dependencies")
                        .and_then(toml::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_string)
                        .collect();
                    Some(OvenLockedPackage {
                        name,
                        version,
                        source,
                        dependencies,
                    })
                })
                .collect();
            Ok(Some(OvenProjectLock { packages }))
        }

        /// Find the exact package identity recorded in the sealed lockfile.
        fn locked_package<'a>(lock: &'a OvenProjectLock, name: &str, version: &str) -> Option<&'a OvenLockedPackage> {
            lock.packages
                .iter()
                .find(|package| package.name == name && package.version == version)
        }

        /// Resolve Cargo's lockfile dependency spelling to one unambiguous locked package.
        fn locked_dependency<'a>(lock: &'a OvenProjectLock, reference: &str) -> Option<&'a OvenLockedPackage> {
            let mut segments = reference.split_whitespace();
            let name = segments.next()?;
            let version = segments.next();
            if let Some(version) =
                version.filter(|version| version.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
            {
                return locked_package(lock, name, version);
            }
            let mut candidates = lock.packages.iter().filter(|package| package.name == name);
            let package = candidates.next()?;
            candidates.next().is_none().then_some(package)
        }

        /// Select the source-cache root without invoking Cargo or consulting a Cargo build target.
        fn discovered_cargo_home(explicit: Option<&Path>) -> Option<PathBuf> {
            explicit.map(Path::to_path_buf).or_else(|| {
                std::env::var_os("CARGO_HOME")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
            })
        }

        /// Locate a lock-authorized registry package source below the already-present Cargo source cache.
        fn registry_source_dir(cargo_home: Option<&Path>, package: &OvenLockedPackage) -> Option<PathBuf> {
            if !package
                .source
                .as_deref()
                .is_some_and(|source| source.starts_with("registry+"))
            {
                return None;
            }
            let source_root = cargo_home?.join("registry").join("src");
            let package_dir = format!("{}-{}", package.name, package.version);
            fs::read_dir(source_root)
                .ok()?
                .flatten()
                .map(|registry| registry.path().join(&package_dir))
                .find(|candidate| candidate.join("Cargo.toml").is_file())
        }

        /// Locate the best locally-cached source for an unlocked dependency declaration.
        ///
        /// An explicit Loaf preparation fixture is generated before the named publisher creates its final Cargo.lock.
        /// This source-only fallback is therefore limited to a version requirement already present in the
        /// compiler-authored manifest and never reaches a registry. The publisher remains responsible for
        /// sealing the exact resulting lock and native artifact together.
        fn registry_source_dir_for_requirement(
            cargo_home: Option<&Path>,
            package_name: &str,
            version_requirement: Option<&str>,
        ) -> Option<PathBuf> {
            let requirement = VersionReq::parse(version_requirement?).ok()?;
            let source_root = cargo_home?.join("registry").join("src");
            let prefix = format!("{package_name}-");
            fs::read_dir(source_root)
                .ok()?
                .flatten()
                .filter_map(|registry| fs::read_dir(registry.path()).ok())
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    let name = path.file_name()?.to_str()?;
                    let version = Version::parse(name.strip_prefix(&prefix)?).ok()?;
                    (requirement.matches(&version) && path.join("Cargo.toml").is_file()).then_some((version, path))
                })
                .max_by(|left, right| left.0.cmp(&right.0))
                .map(|(_, path)| path)
        }

        /// Translate one manifest and its lock-authorized dependencies into a rust-project crate record.
        fn crate_from_manifest(
            manifest_dir: &Path,
            lock: Option<&OvenProjectLock>,
            cargo_home: Option<&Path>,
        ) -> Result<OvenProjectCrate, RustMetadataError> {
            let manifest_path = manifest_dir.join("Cargo.toml");
            let manifest = toml::from_str::<toml::Value>(&fs::read_to_string(&manifest_path)?).map_err(|error| {
                RustMetadataError::LoadWorkspace {
                    path: manifest_path.clone(),
                    message: format!("failed to parse compiler-authored manifest for direct Oven inspection: {error}"),
                }
            })?;
            let package = manifest.get("package").and_then(toml::Value::as_table).ok_or_else(|| {
                RustMetadataError::LoadWorkspace {
                    path: manifest_path.clone(),
                    message: "direct Oven inspection requires a package manifest".to_string(),
                }
            })?;
            let package_name =
                package
                    .get("name")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| RustMetadataError::LoadWorkspace {
                        path: manifest_path.clone(),
                        message: "direct Oven inspection requires package.name".to_string(),
                    })?;
            // Workspace crates may inherit `package.version`; a direct source graph can still follow their explicit
            // path dependencies without a literal version. A version is only needed for an exact lockfile lookup.
            let package_version = package.get("version").and_then(toml::Value::as_str);
            let display_name = manifest
                .get("lib")
                .and_then(|library| library.get("name"))
                .and_then(toml::Value::as_str)
                .unwrap_or(package_name)
                .replace('-', "_");
            let edition = package
                .get("edition")
                .and_then(toml::Value::as_str)
                .filter(|edition| matches!(*edition, "2015" | "2018" | "2021" | "2024"))
                .unwrap_or("2021")
                .to_string();
            let root = manifest
                .get("lib")
                .and_then(|library| library.get("path"))
                .and_then(toml::Value::as_str)
                .map(|path| manifest_dir.join(path))
                .unwrap_or_else(|| {
                    let library_root = manifest_dir.join("src/lib.rs");
                    if library_root.is_file() {
                        library_root
                    } else {
                        manifest
                            .get("bin")
                            .and_then(toml::Value::as_array)
                            .and_then(|bins| bins.first())
                            .and_then(toml::Value::as_table)
                            .and_then(|bin| bin.get("path"))
                            .and_then(toml::Value::as_str)
                            .map(|path| manifest_dir.join(path))
                            .unwrap_or_else(|| manifest_dir.join("src/main.rs"))
                    }
                });
            let root_module = root.canonicalize().map_err(|error| RustMetadataError::LoadWorkspace {
                path: root,
                message: format!("direct Oven inspection requires a readable library root: {error}"),
            })?;
            // The full compiler-owned Loaf retains the compiler's complete provider envelope, including imports behind
            // optional Rust-provider features. rust-analyzer receives this source graph only for metadata; enabling
            // declared local features makes those conditional public items visible without asking Cargo to solve or
            // build anything. The named publisher later seals the exact activated feature set with its Loaf.
            let mut cfg = manifest
                .get("features")
                .and_then(toml::Value::as_table)
                .map(|features| {
                    features
                        .keys()
                        .map(|feature| format!("feature=\"{feature}\""))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            cfg.sort();
            let declarations = dependency_declarations(&manifest, manifest_dir);
            let dependencies = lock
                .and_then(|lock| {
                    package_version
                        .and_then(|version| locked_package(lock, package_name, version).map(|package| (lock, package)))
                })
                .map(|(lock, package)| {
                    package
                        .dependencies
                        .iter()
                        .filter_map(|reference| {
                            let dependency = locked_dependency(lock, reference)?;
                            // Cargo.lock retains dev and build dependencies too. The direct no-Cargo graph contains
                            // only normal target dependencies declared by this crate, which prevents test-only cycles
                            // from changing provider metadata resolution.
                            let declaration = declarations
                                .iter()
                                .find(|(_, package_name, _, _)| package_name == &dependency.name)?;
                            let dependency_name = declaration.0.clone();
                            let is_local_path = declaration.2.is_some();
                            let source_dir = declaration
                                .2
                                .clone()
                                .or_else(|| registry_source_dir(cargo_home, dependency));
                            source_dir.map(|source_dir| OvenProjectDependency {
                                name: dependency_name,
                                source_dir,
                                is_local_path,
                            })
                        })
                        .collect()
                })
                .unwrap_or_else(|| {
                    declarations
                        .into_iter()
                        .filter_map(|(name, package_name, path, version)| {
                            path.map(|source_dir| OvenProjectDependency {
                                name: name.clone(),
                                source_dir,
                                is_local_path: true,
                            })
                            .or_else(|| {
                                registry_source_dir_for_requirement(cargo_home, &package_name, version.as_deref()).map(
                                    |source_dir| OvenProjectDependency {
                                        name,
                                        source_dir,
                                        is_local_path: false,
                                    },
                                )
                            })
                        })
                        .collect()
                });
            Ok(OvenProjectCrate {
                display_name,
                root_module,
                edition,
                dependencies,
                cfg,
            })
        }

        /// Add one source crate and its direct source dependencies once, returning its rust-project index.
        fn visit(
            manifest_dir: &Path,
            crates: &mut Vec<OvenProjectCrate>,
            indices: &mut HashMap<PathBuf, usize>,
            lock: Option<&OvenProjectLock>,
            cargo_home: Option<&Path>,
            dependency_depth: usize,
        ) -> Result<usize, RustMetadataError> {
            let manifest_dir = manifest_dir.canonicalize()?;
            if let Some(index) = indices.get(&manifest_dir) {
                return Ok(*index);
            }
            let index = crates.len();
            indices.insert(manifest_dir.clone(), index);
            let direct_crate = crate_from_manifest(&manifest_dir, lock, cargo_home)?;
            let dependencies = direct_crate.dependencies.clone();
            crates.push(direct_crate);
            for dependency in dependencies {
                // Without a lock, local workspace crates remain authoritative, while a registry edge is only a
                // best-effort direct source lookup. Do not let that fallback manufacture a transitive graph.
                if dependency_depth > 0 || dependency.is_local_path {
                    visit(
                        &dependency.source_dir,
                        crates,
                        indices,
                        lock,
                        cargo_home,
                        dependency_depth.saturating_sub(1),
                    )?;
                }
            }
            Ok(index)
        }

        let lock = load_lock(manifest_dir)?;
        let cargo_home = discovered_cargo_home(cargo_home);
        let mut crates = Vec::new();
        let mut indices = HashMap::new();
        visit(
            manifest_dir,
            &mut crates,
            &mut indices,
            lock.as_ref(),
            cargo_home.as_deref(),
            // An exact lock safely identifies the complete runtime closure. Before the named publisher has created
            // that lock, inspect only the manifest's direct, locally-cached dependencies; recurse no further.
            if lock.is_some() { usize::MAX } else { 1 },
        )?;
        let crates = crates
            .into_iter()
            .map(|direct_crate| {
                let dependencies = direct_crate
                    .dependencies
                    .into_iter()
                    .filter_map(|dependency| {
                        indices
                            .get(&dependency.source_dir.canonicalize().ok()?)
                            .map(|index| serde_json::json!({ "crate": index, "name": dependency.name }))
                    })
                    .collect::<Vec<_>>();
                serde_json::json!({
                    "display_name": direct_crate.display_name,
                    "root_module": direct_crate.root_module,
                    "edition": direct_crate.edition,
                    "deps": dependencies,
                    "cfg": direct_crate.cfg,
                    "env": {},
                    "is_workspace_member": true,
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&serde_json::json!({
            "crates": crates,
        }))
        .map_err(|error| RustMetadataError::LoadWorkspace {
            path: manifest_dir.to_path_buf(),
            message: format!("failed to encode direct Oven rust-project graph: {error}"),
        })
    }

    /// Load the sealed compiler-suite source graph with rust-analyzer's build-system-neutral interface.
    fn load_oven_project(
        manifest_dir: &Path,
        target_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        _load_out_dirs_from_check: bool,
    ) -> Result<Self, RustMetadataError> {
        Self::load_oven_project_with_cargo_home(manifest_dir, target_dir, progress, None)
    }

    /// Load a direct Oven project while allowing tests to select the read-only registry source cache explicitly.
    fn load_oven_project_with_cargo_home(
        manifest_dir: &Path,
        target_dir: &Path,
        progress: &(dyn Fn(String) + Sync),
        cargo_home: Option<&Path>,
    ) -> Result<Self, RustMetadataError> {
        let manifest_dir = manifest_dir.canonicalize()?;
        let payload = Self::oven_project_json_payload_with_cargo_home(&manifest_dir, cargo_home)?;
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
        Ok(RustWorkspace { db, crate_index, vfs })
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
        if Self::oven_direct_inspection_active(manifest_dir) {
            return Self::load_oven_project(manifest_dir, target_dir, progress, load_out_dirs_from_check);
        }
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

    use super::{OVEN_DIRECT_INSPECTION_MARKER, RustWorkspace};

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

    #[test]
    fn direct_oven_inspection_uses_workspace_marker_or_explicit_suite_capability()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let suite_capability = std::env::var_os("INCAN_OVEN_COMPILER_SUITE_RUSTC").is_some();
        assert_eq!(
            RustWorkspace::oven_direct_inspection_active(workspace.path()),
            suite_capability,
            "a legacy workspace must retain its Cargo inspection route unless the scheduler explicitly authorizes its direct route"
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
    fn direct_oven_project_follows_locked_registry_sources_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempdir()?;
        let cargo_home = tempdir()?;
        fs::create_dir_all(workspace.path().join("src"))?;
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"oven-inspect-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ndemo = \"1\"\n",
        )?;
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            workspace.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"oven-inspect-fixture\"\nversion = \"0.1.0\"\ndependencies = [\"demo 1.0.0 (registry+https://example.invalid/index)\"]\n\n[[package]]\nname = \"demo\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\ndependencies = [\"leaf\"]\n\n[[package]]\nname = \"leaf\"\nversion = \"1.0.0\"\nsource = \"registry+https://example.invalid/index\"\n",
        )?;
        let registry = cargo_home.path().join("registry/src/test-index");
        for (name, source) in [
            ("demo-1.0.0", "pub fn demo() {}\n"),
            ("leaf-1.0.0", "pub fn leaf() {}\n"),
        ] {
            let package = registry.join(name);
            fs::create_dir_all(package.join("src"))?;
            let package_name = name.split('-').next().ok_or("registry fixture package name missing")?;
            let dependencies = if package_name == "demo" {
                "\n[dependencies]\nleaf = \"1\"\n"
            } else {
                ""
            };
            fs::write(
                package.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"{package_name}\"\nversion = \"1.0.0\"\nedition = \"2021\"\n{dependencies}"
                ),
            )?;
            fs::write(package.join("src/lib.rs"), source)?;
        }

        let payload =
            RustWorkspace::oven_project_json_payload_with_cargo_home(workspace.path(), Some(cargo_home.path()))?;
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

        fs::remove_file(workspace.path().join("Cargo.lock"))?;
        let unlocked_payload =
            RustWorkspace::oven_project_json_payload_with_cargo_home(workspace.path(), Some(cargo_home.path()))?;
        let unlocked_graph: serde_json::Value = serde_json::from_slice(&unlocked_payload)?;
        let unlocked_crates = unlocked_graph["crates"]
            .as_array()
            .ok_or("unlocked direct Oven graph omitted crates")?;
        assert_eq!(
            unlocked_crates.len(),
            2,
            "an unlocked Loaf fixture may inspect direct cached dependencies but must not guess a transitive closure"
        );
        assert_eq!(unlocked_crates[0]["deps"][0]["name"], "demo");
        assert_eq!(unlocked_crates[1]["display_name"], "demo");
        let loaded = RustWorkspace::load_oven_project_with_cargo_home(
            workspace.path(),
            &workspace.path().join("inspection-target"),
            &|_| {},
            Some(cargo_home.path()),
        )?;
        assert!(
            loaded.crate_by_name("demo").is_some(),
            "rust-analyzer must expose a direct cached registry crate without asking Cargo to build the graph"
        );
        Ok(())
    }
}
