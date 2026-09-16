//! Dependency-source resolution helpers for `RustMetadataCache`.
//!
//! These functions map a Rust import crate segment (which may use `_`) back to a concrete Cargo package source
//! directory (which may use `-`) so extraction can fall back to dependency workspaces when needed.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct CargoLock {
    package: Vec<CargoLockPackage>,
}

#[derive(Deserialize)]
struct CargoLockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Return whether one exact source directory is the registry package selected by a lock entry.
///
/// Cargo's conventional registry layout nests package directories below an index directory. Oven keeps the same
/// sealed sources as individually addressed roots instead, so support both layouts without ever searching ambient
/// sources when a caller supplies an explicit authority list.
fn exact_registry_package_dir(root: &Path, package: &str, version: &str) -> Option<PathBuf> {
    let manifest_path = root.join("Cargo.toml");
    let manifest = toml::from_str::<toml::Value>(fs::read_to_string(manifest_path).ok()?.as_str()).ok()?;
    let manifest_package = manifest.get("package")?.get("name")?.as_str()?;
    let manifest_version = manifest.get("package")?.get("version")?.as_str()?;
    (manifest_package == package && manifest_version == version).then(|| root.to_path_buf())
}

/// Return the first path segment that identifies the crate for a canonical path.
pub(crate) fn crate_name_for_path(canonical_path: &str) -> &str {
    canonical_path.split("::").next().unwrap_or(canonical_path)
}

/// Resolve a path dependency directly from the generated manifest.
///
/// The compiler writes this manifest itself, so its dependency declarations are already the authoritative input to
/// the legacy publisher. Reading them here avoids turning a consumer metadata lookup into a `cargo metadata`
/// subprocess merely to recover a path Cargo was just given. Package aliases use Cargo's `package = "..."` spelling
/// while Rust imports use underscores, so both dependency keys and declared package names are normalized.
pub(crate) fn dependency_manifest_dir_from_manifest(root: &Path, crate_name: &str) -> Option<PathBuf> {
    let manifest_path = root.join("Cargo.toml");
    let manifest = toml::from_str::<toml::Value>(&fs::read_to_string(manifest_path).ok()?).ok()?;
    let normalized = normalize_crate_name(crate_name);

    let direct = ["dependencies", "dev-dependencies", "build-dependencies"]
        .into_iter()
        .find_map(|section| dependency_path_in_table(manifest.get(section), normalized.as_str(), root));
    direct
        .or_else(|| {
            manifest
                .get("target")
                .and_then(toml::Value::as_table)
                .and_then(|targets| {
                    targets.values().find_map(|target| {
                        ["dependencies", "dev-dependencies", "build-dependencies"]
                            .into_iter()
                            .find_map(|section| {
                                dependency_path_in_table(target.get(section), normalized.as_str(), root)
                            })
                    })
                })
        })
        .or_else(|| {
            manifest
                .get("workspace")
                .and_then(|workspace| workspace.get("dependencies"))
                .and_then(|dependencies| dependency_path_in_table(Some(dependencies), normalized.as_str(), root))
        })
}

/// Compiler-injected support crates share this package-name prefix in every generated Cargo manifest (`incan_derive`,
/// the `incan_std_<facet>` runtime facets, and each `incan_stdlib_<component>` SDK component). They are never a project
/// author's own interop dependency, so [`path_dependency_dirs_from_manifest`] excludes them; see that function's docs
/// for why.
const COMPILER_OWNED_CRATE_NAME_PREFIX: &str = "incan_";

/// Return every local path-dependency directory declared directly in the workspace root manifest, excluding the
/// compiler's own injected support crates.
///
/// `Cargo.lock` records a locked version for a `path = "..."` dependency but no content checksum, so it cannot
/// detect an edit to the dependency's own source. Callers use this to fold path-dependency contents into a cache
/// fingerprint that would otherwise treat an edited local crate as unchanged.
///
/// Every generated project's manifest also carries `path = "..."` dependencies the compiler injects for its own
/// runtime support (`incan_derive`, the `incan_std_<facet>` runtime facets, and the `incan_stdlib_<component>` SDK
/// components) rather than anything the project author wrote. Those are excluded by package-name prefix: they are not
/// editable through ordinary project use, their own staleness is already covered by the toolchain/SDK identity that
/// selects their path in the first place, and walking them (the stdlib alone spans well over a hundred files) on every
/// cache load for every generated project turns an occasional-edit staleness check into the dominant cost of the whole
/// test suite. The prefix is a deliberate, environment-agnostic proxy for "compiler-owned": unlike an env-var-based
/// check, it holds the same in a development checkout, a CI test harness, and an installed toolchain alike.
///
/// Mirrors [`dependency_manifest_dir_from_manifest`]'s table coverage (ordinary, target-specific, and `{ workspace =
/// true }`-inherited dependency tables) so a path dependency declared through any of those shapes still invalidates the
/// fingerprint.
pub(crate) fn path_dependency_dirs_from_manifest(root: &Path) -> Vec<PathBuf> {
    let Some(manifest_text) = fs::read_to_string(root.join("Cargo.toml")).ok() else {
        return Vec::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(&manifest_text) else {
        return Vec::new();
    };

    let workspace_dependencies = manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table);

    let mut tables = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(section).and_then(toml::Value::as_table) {
            tables.push(table);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(table) = target.get(section).and_then(toml::Value::as_table) {
                    tables.push(table);
                }
            }
        }
    }

    tables
        .into_iter()
        .flat_map(|table| table.iter())
        .filter_map(|(key, declaration)| {
            let declaration = declaration.as_table()?;
            let resolved = if declaration.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                workspace_dependencies?.get(key)?.as_table()?
            } else {
                declaration
            };
            let package_name = resolved.get("package").and_then(toml::Value::as_str).unwrap_or(key);
            if package_name.starts_with(COMPILER_OWNED_CRATE_NAME_PREFIX) {
                return None;
            }
            let path = resolved.get("path")?.as_str()?;
            root.join(path).canonicalize().ok()
        })
        .collect()
}

/// Resolve one path dependency from a Cargo dependency table without invoking Cargo.
fn dependency_path_in_table(table: Option<&toml::Value>, normalized_crate_name: &str, root: &Path) -> Option<PathBuf> {
    let dependencies = table?.as_table()?;
    dependencies.iter().find_map(|(key, value)| {
        let declaration = value.as_table()?;
        let package_name = declaration
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(key.as_str());
        let path = declaration.get("path")?.as_str()?;
        let candidate = root.join(path);
        let manifest_path = candidate.join("Cargo.toml");
        if !manifest_path.is_file() {
            return None;
        }
        let manifest_crate_names = fs::read_to_string(manifest_path)
            .ok()
            .and_then(|payload| toml::from_str::<toml::Value>(payload.as_str()).ok())
            .map(|manifest| {
                [
                    manifest
                        .get("package")
                        .and_then(|package| package.get("name"))
                        .and_then(toml::Value::as_str),
                    manifest
                        .get("lib")
                        .and_then(|library| library.get("name"))
                        .and_then(toml::Value::as_str),
                ]
                .into_iter()
                .flatten()
                .map(normalize_crate_name)
                .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (normalize_crate_name(key) == normalized_crate_name
            || normalize_crate_name(package_name) == normalized_crate_name
            || manifest_crate_names.iter().any(|name| name == normalized_crate_name))
        .then(|| candidate.canonicalize().ok())
        .flatten()
    })
}

fn cargo_registry_src_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        roots.push(PathBuf::from(cargo_home).join("registry").join("src"));
    }

    if let Some(home) = std::env::var_os("HOME") {
        let default_root = PathBuf::from(home).join(".cargo").join("registry").join("src");
        if !roots.contains(&default_root) {
            roots.push(default_root);
        }
    }

    roots
}

/// Return the version requirement the root manifest declares for `normalized_crate_name`, if it declares the crate.
///
/// Looks through every dependency table, including target-specific ones, matching the declaration key or its
/// `package` rename. A requirement spelled as a bare string or as a `version` field both count; a path or workspace
/// declaration without a version yields `None`.
fn root_manifest_version_requirement(root: &Path, normalized_crate_name: &str) -> Option<semver::VersionReq> {
    let manifest = toml::from_str::<toml::Value>(fs::read_to_string(root.join("Cargo.toml")).ok()?.as_str()).ok()?;
    let mut tables = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = manifest.get(section).and_then(toml::Value::as_table) {
            tables.push(table);
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(table) = target.get(section).and_then(toml::Value::as_table) {
                    tables.push(table);
                }
            }
        }
    }
    tables
        .into_iter()
        .flat_map(|table| table.iter())
        .find_map(|(key, declaration)| {
            let package_name = declaration
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(key.as_str());
            if normalize_crate_name(key) != normalized_crate_name
                && normalize_crate_name(package_name) != normalized_crate_name
            {
                return None;
            }
            let requirement = match declaration {
                toml::Value::String(requirement) => requirement.as_str(),
                toml::Value::Table(table) => table.get("version")?.as_str()?,
                _ => return None,
            };
            semver::VersionReq::parse(requirement).ok()
        })
}

/// Resolve a registry package source directory from a `Cargo.lock` entry.
///
/// Generated rust-inspect lock workspaces may know the exact locked package version without having a fully populated
/// rust-analyzer crate graph. This fallback bridges from Cargo's package identity (`foo-bar`) back to the local
/// downloaded crate source so extractor queries can still load metadata for Rust import paths like `foo_bar::...`.
pub(crate) fn dependency_manifest_dir_from_lock_with_search_roots(
    root: &Path,
    crate_name: &str,
    registry_src_roots: &[PathBuf],
) -> Option<PathBuf> {
    let lock_path = root.join("Cargo.lock");
    let lock: CargoLock = toml::from_str(fs::read_to_string(lock_path).ok()?.as_str()).ok()?;
    let normalized = normalize_crate_name(crate_name);
    // A lock can hold several versions of one package: the root's own `substrait` beside the one a DataFusion
    // adapter pins. rustc links the root against exactly the version its manifest requirement selects, so when the
    // root declares the crate, only lock entries satisfying that requirement are candidates. Entries stay in lock
    // order otherwise, which keeps today's choice for a crate the root reaches only transitively.
    let requirement = root_manifest_version_requirement(root, &normalized);
    let mut candidates = lock
        .package
        .into_iter()
        .filter(|pkg| normalize_crate_name(pkg.name.as_str()) == normalized)
        .filter(|pkg| {
            pkg.source
                .as_deref()
                .is_some_and(|source| source.starts_with("registry+"))
        })
        .collect::<Vec<_>>();
    if let Some(requirement) = requirement
        && candidates.len() > 1
    {
        candidates.retain(|pkg| {
            semver::Version::parse(pkg.version.as_str()).is_ok_and(|version| requirement.matches(&version))
        });
    }

    for pkg in candidates {
        let dir_name = format!("{}-{}", pkg.name, pkg.version);
        for root in registry_src_roots {
            if let Some(candidate) = exact_registry_package_dir(root, pkg.name.as_str(), pkg.version.as_str()) {
                return Some(candidate);
            }
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            for entry in entries.flatten() {
                let candidate = entry.path().join(dir_name.as_str());
                if candidate.join("Cargo.toml").is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn dependency_manifest_dir_from_lock(
    root: &Path,
    crate_name: &str,
    registry_src_roots: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    let owned_roots;
    let search_roots = if let Some(roots) = registry_src_roots {
        roots
    } else {
        owned_roots = cargo_registry_src_roots();
        &owned_roots
    };
    dependency_manifest_dir_from_lock_with_search_roots(root, crate_name, search_roots)
}

/// Resolve the best-known dependency manifest directory for `crate_name` from compiler-authored workspace inputs.
pub(crate) fn dependency_manifest_dir_for_crate(
    root: &Path,
    crate_name: &str,
    registry_src_roots: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    dependency_manifest_dir_from_manifest(root, crate_name)
        .or_else(|| dependency_manifest_dir_from_lock(root, crate_name, registry_src_roots))
}
