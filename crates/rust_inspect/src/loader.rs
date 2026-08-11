//! Load a Cargo tree into rust-analyzer's `RootDatabase`.
//!
//! This module is intentionally behind the rust-inspect preparation/cache boundary. It owns the unstable rust-analyzer
//! embedding details so parser/typechecker/codegen code does not load Cargo workspaces directly.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
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
pub const OVEN_DIRECT_INSPECTION_MARKER: &str = ".incan_oven_direct_rust_project";
/// Compiler-authored source authority consumed by the direct Oven inspection loader.
pub const OVEN_DIRECT_INSPECTION_AUTHORITY_FILE: &str = ".incan_oven_rust_sources.json";
const OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION: u32 = 1;

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
    sources: Vec<OvenInspectionRegistrySource>,
}

/// Write the exact source authority beside one compiler-authored direct inspection projection.
pub fn write_oven_inspection_source_authority(
    manifest_dir: &Path,
    mut sources: Vec<OvenInspectionRegistrySource>,
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

    /// Build the direct source graph from the exact lockfile and compiler-authored Oven source authority.
    fn oven_project_json_payload_with_source_authority(manifest_dir: &Path) -> Result<Vec<u8>, RustMetadataError> {
        /// Read the non-build dependency declarations that can contribute source crates to the direct graph.
        fn dependency_declarations(manifest: &toml::Value, manifest_dir: &Path) -> Vec<OvenDependencyDeclaration> {
            /// Normalize one Cargo dependency table into direct-graph source candidates.
            fn collect(
                table: Option<&toml::Value>,
                manifest_dir: &Path,
                dependencies: &mut Vec<OvenDependencyDeclaration>,
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
                    let mut features = declaration
                        .get("features")
                        .and_then(toml::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(toml::Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    if declaration
                        .get("default-features")
                        .and_then(toml::Value::as_bool)
                        .unwrap_or(true)
                    {
                        features.push("default".to_string());
                    }
                    features.sort();
                    features.dedup();
                    dependencies.push(OvenDependencyDeclaration {
                        name: dependency_name.replace('-', "_"),
                        package: package_name,
                        path,
                        version,
                        features,
                    });
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
            dependencies.sort_by(|left, right| {
                (&left.name, &left.package, &left.path, &left.version, &left.features).cmp(&(
                    &right.name,
                    &right.package,
                    &right.path,
                    &right.version,
                    &right.features,
                ))
            });
            dependencies.dedup_by(|left, right| {
                left.name == right.name
                    && left.package == right.package
                    && left.path == right.path
                    && left.version == right.version
                    && left.features == right.features
            });
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
                    let checksum = package
                        .get("checksum")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string);
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
                        checksum,
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

        /// Load and validate the compiler-authored registry-source authority for this projection.
        fn load_source_authority(manifest_dir: &Path) -> Result<OvenInspectionSourceAuthority, RustMetadataError> {
            let path = manifest_dir.join(OVEN_DIRECT_INSPECTION_AUTHORITY_FILE);
            if !path.is_file() {
                return Ok(OvenInspectionSourceAuthority {
                    schema_version: OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
                    sources: Vec::new(),
                });
            }
            let authority =
                serde_json::from_slice::<OvenInspectionSourceAuthority>(&fs::read(&path)?).map_err(|error| {
                    RustMetadataError::LoadWorkspace {
                        path: path.clone(),
                        message: format!("invalid Oven Rust source authority: {error}"),
                    }
                })?;
            if authority.schema_version != OVEN_DIRECT_INSPECTION_AUTHORITY_SCHEMA_VERSION {
                return Err(RustMetadataError::LoadWorkspace {
                    path,
                    message: format!(
                        "unsupported Oven Rust source authority schema {}",
                        authority.schema_version
                    ),
                });
            }
            let mut identities = BTreeMap::new();
            for source in &authority.sources {
                if source.package.trim().is_empty()
                    || Version::parse(&source.version).is_err()
                    || !source.registry.starts_with("registry+")
                    || source.checksum.trim().is_empty()
                    || source.source_digest.trim().is_empty()
                {
                    return Err(RustMetadataError::LoadWorkspace {
                        path: source.source_root.clone(),
                        message: "Oven Rust source authority contains an incomplete registry identity".to_string(),
                    });
                }
                let root = source.source_root.canonicalize()?;
                if !root.join("Cargo.toml").is_file() {
                    return Err(RustMetadataError::LoadWorkspace {
                        path: root,
                        message: "sealed Oven registry source has no Cargo.toml".to_string(),
                    });
                }
                let actual_digest = digest_oven_source_tree(&root)?;
                if actual_digest != source.source_digest {
                    return Err(RustMetadataError::LoadWorkspace {
                        path: root,
                        message: format!(
                            "sealed Oven registry source digest is {actual_digest}, expected {}",
                            source.source_digest
                        ),
                    });
                }
                let key = (
                    source.package.clone(),
                    source.version.clone(),
                    source.registry.clone(),
                    source.checksum.clone(),
                );
                if identities.insert(key.clone(), root).is_some() {
                    return Err(RustMetadataError::LoadWorkspace {
                        path: source.source_root.clone(),
                        message: format!(
                            "Oven Rust source authority declares `{}` {} from `{}` more than once",
                            key.0, key.1, key.2
                        ),
                    });
                }
            }
            Ok(authority)
        }

        /// Locate the source with the same package, registry, checksum, and version as one locked package.
        fn registry_source(
            authority: &OvenInspectionSourceAuthority,
            package: &OvenLockedPackage,
        ) -> Result<Option<(PathBuf, Vec<String>)>, RustMetadataError> {
            let Some(registry) = package
                .source
                .as_deref()
                .filter(|source| source.starts_with("registry+"))
            else {
                return Ok(None);
            };
            let checksum = package
                .checksum
                .as_deref()
                .ok_or_else(|| RustMetadataError::LoadWorkspace {
                    path: PathBuf::from("Cargo.lock"),
                    message: format!(
                        "locked registry package `{}` {} has no checksum",
                        package.name, package.version
                    ),
                })?;
            let mut matches = authority.sources.iter().filter(|source| {
                source.package == package.name
                    && source.version == package.version
                    && source.registry == registry
                    && source.checksum == checksum
            });
            let Some(source) = matches.next() else {
                return Err(RustMetadataError::LoadWorkspace {
                    path: PathBuf::from(OVEN_DIRECT_INSPECTION_AUTHORITY_FILE),
                    message: format!(
                        "no sealed Oven source matches locked registry package `{}` {} checksum `{checksum}`",
                        package.name, package.version
                    ),
                });
            };
            if matches.next().is_some() {
                return Err(RustMetadataError::LoadWorkspace {
                    path: PathBuf::from(OVEN_DIRECT_INSPECTION_AUTHORITY_FILE),
                    message: format!(
                        "sealed Oven source authority is ambiguous for `{}` {}",
                        package.name, package.version
                    ),
                });
            }
            Ok(Some((source.source_root.clone(), source.features.clone())))
        }

        /// Match an unlocked baker fixture only against one unambiguous version in its sealed authority.
        fn registry_source_for_requirement(
            authority: &OvenInspectionSourceAuthority,
            package_name: &str,
            version_requirement: Option<&str>,
        ) -> Result<Option<(PathBuf, Vec<String>)>, RustMetadataError> {
            let Some(version_requirement) = version_requirement else {
                return Ok(None);
            };
            let requirement =
                VersionReq::parse(version_requirement).map_err(|error| RustMetadataError::LoadWorkspace {
                    path: PathBuf::from("Cargo.toml"),
                    message: format!("invalid direct Oven dependency requirement `{version_requirement}`: {error}"),
                })?;
            let mut matches = authority.sources.iter().filter(|source| {
                source.package == package_name
                    && Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
            });
            let Some(source) = matches.next() else {
                return Ok(None);
            };
            if matches.next().is_some() {
                return Err(RustMetadataError::LoadWorkspace {
                    path: PathBuf::from(OVEN_DIRECT_INSPECTION_AUTHORITY_FILE),
                    message: format!(
                        "sealed Oven source authority contains multiple `{package_name}` versions matching `{version_requirement}`"
                    ),
                });
            }
            Ok(Some((source.source_root.clone(), source.features.clone())))
        }

        /// Translate one manifest and its lock-authorized dependencies into a rust-project crate record.
        fn crate_from_manifest(
            manifest_dir: &Path,
            lock: Option<&OvenProjectLock>,
            authority: &OvenInspectionSourceAuthority,
            selected_features: &[String],
            allow_unlocked_registry_dependencies: bool,
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
            // The selected Loaf records Cargo's already-unified feature set for registry crates. Local path edges pass
            // only their explicit feature request. Never make cfg-gated APIs visible merely because a feature happens
            // to be declared in a mutable manifest.
            let declared_features = manifest.get("features").and_then(toml::Value::as_table);
            let mut cfg = selected_features
                .iter()
                .filter(|feature| declared_features.is_some_and(|declared| declared.contains_key(feature.as_str())))
                .map(|feature| format!("feature=\"{feature}\""))
                .collect::<Vec<_>>();
            cfg.sort();
            cfg.dedup();
            let declarations = dependency_declarations(&manifest, manifest_dir);
            let locked_root = lock.and_then(|lock| {
                package_version
                    .and_then(|version| locked_package(lock, package_name, version).map(|package| (lock, package)))
            });
            let mut dependencies = Vec::new();
            if let Some((lock, package)) = locked_root {
                for reference in &package.dependencies {
                    let Some(dependency) = locked_dependency(lock, reference) else {
                        continue;
                    };
                    // Cargo.lock retains dev and build dependencies too. The direct no-Cargo graph contains only
                    // normal target dependencies declared by this crate.
                    let Some(declaration) = declarations
                        .iter()
                        .find(|declaration| declaration.package == dependency.name)
                    else {
                        continue;
                    };
                    if let Some(source_dir) = declaration.path.clone() {
                        dependencies.push(OvenProjectDependency {
                            name: declaration.name.clone(),
                            source_dir,
                            is_local_path: true,
                            features: declaration.features.clone(),
                        });
                    } else if let Some((source_dir, features)) = registry_source(authority, dependency)? {
                        dependencies.push(OvenProjectDependency {
                            name: declaration.name.clone(),
                            source_dir,
                            is_local_path: false,
                            features,
                        });
                    }
                }
            } else {
                for declaration in declarations {
                    if let Some(source_dir) = declaration.path {
                        dependencies.push(OvenProjectDependency {
                            name: declaration.name,
                            source_dir,
                            is_local_path: true,
                            features: declaration.features,
                        });
                    } else if allow_unlocked_registry_dependencies
                        && let Some((source_dir, features)) = registry_source_for_requirement(
                            authority,
                            &declaration.package,
                            declaration.version.as_deref(),
                        )?
                    {
                        dependencies.push(OvenProjectDependency {
                            name: declaration.name,
                            source_dir,
                            is_local_path: false,
                            features,
                        });
                    }
                }
            }
            dependencies.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(OvenProjectCrate {
                display_name,
                root_module,
                edition,
                dependencies,
                cfg,
            })
        }

        /// Single authority and accumulation state for one sealed rust-project graph.
        struct OvenProjectGraphBuilder<'a> {
            crates: Vec<OvenProjectCrate>,
            indices: HashMap<PathBuf, usize>,
            feature_selections: HashMap<PathBuf, Vec<String>>,
            lock: Option<&'a OvenProjectLock>,
            authority: &'a OvenInspectionSourceAuthority,
        }

        impl OvenProjectGraphBuilder<'_> {
            /// Add one source crate or widen its local feature union, returning its stable rust-project index.
            fn visit(
                &mut self,
                manifest_dir: &Path,
                selected_features: &[String],
                dependency_depth: usize,
            ) -> Result<usize, RustMetadataError> {
                let manifest_dir = manifest_dir.canonicalize()?;
                let mut unified_features = self.feature_selections.get(&manifest_dir).cloned().unwrap_or_default();
                unified_features.extend_from_slice(selected_features);
                unified_features.sort();
                unified_features.dedup();
                if let Some(index) = self.indices.get(&manifest_dir).copied() {
                    if self.feature_selections.get(&manifest_dir) == Some(&unified_features) {
                        return Ok(index);
                    }
                    let direct_crate = crate_from_manifest(
                        &manifest_dir,
                        self.lock,
                        self.authority,
                        &unified_features,
                        self.lock.is_some() || dependency_depth > 0,
                    )?;
                    let dependencies = direct_crate.dependencies.clone();
                    self.crates[index] = direct_crate;
                    self.feature_selections.insert(manifest_dir, unified_features);
                    for dependency in dependencies {
                        if dependency_depth > 0 || dependency.is_local_path {
                            self.visit(
                                &dependency.source_dir,
                                &dependency.features,
                                dependency_depth.saturating_sub(1),
                            )?;
                        }
                    }
                    return Ok(index);
                }
                let index = self.crates.len();
                self.indices.insert(manifest_dir.clone(), index);
                self.feature_selections
                    .insert(manifest_dir.clone(), unified_features.clone());
                let direct_crate = crate_from_manifest(
                    &manifest_dir,
                    self.lock,
                    self.authority,
                    &unified_features,
                    self.lock.is_some() || dependency_depth > 0,
                )?;
                let dependencies = direct_crate.dependencies.clone();
                self.crates.push(direct_crate);
                for dependency in dependencies {
                    // Without a lock, local workspace crates remain authoritative, while a registry edge is only a
                    // best-effort direct source lookup. Do not let that fallback manufacture a transitive graph.
                    if dependency_depth > 0 || dependency.is_local_path {
                        self.visit(
                            &dependency.source_dir,
                            &dependency.features,
                            dependency_depth.saturating_sub(1),
                        )?;
                    }
                }
                Ok(index)
            }
        }

        let lock = load_lock(manifest_dir)?;
        let authority = load_source_authority(manifest_dir)?;
        let mut graph = OvenProjectGraphBuilder {
            crates: Vec::new(),
            indices: HashMap::new(),
            feature_selections: HashMap::new(),
            lock: lock.as_ref(),
            authority: &authority,
        };
        graph.visit(
            manifest_dir,
            &[],
            // An exact lock plus sealed source authority identifies the complete runtime closure. An unlocked explicit
            // baker fixture may inspect its direct registry roots but must not infer an unpinned transitive registry
            // graph from broad Cargo requirements in those roots.
            if lock.is_some() { usize::MAX } else { 1 },
        )?;
        let crates = graph
            .crates
            .into_iter()
            .map(|direct_crate| {
                let dependencies = direct_crate
                    .dependencies
                    .into_iter()
                    .filter_map(|dependency| {
                        graph
                            .indices
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

    use super::{
        OVEN_DIRECT_INSPECTION_MARKER, OvenInspectionRegistrySource, RustWorkspace, digest_oven_source_tree,
        write_oven_inspection_source_authority,
    };

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
}
