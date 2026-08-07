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

    for pkg in lock.package {
        if normalize_crate_name(pkg.name.as_str()) != normalized {
            continue;
        }
        if !pkg
            .source
            .as_deref()
            .is_some_and(|source| source.starts_with("registry+"))
        {
            continue;
        }
        let dir_name = format!("{}-{}", pkg.name, pkg.version);
        for root in registry_src_roots {
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
