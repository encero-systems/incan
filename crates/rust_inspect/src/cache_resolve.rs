//! Dependency-source resolution helpers for `RustMetadataCache`.
//!
//! These functions map a Rust import crate segment (which may use `_`) back to a concrete Cargo package source
//! directory (which may use `-`) so extraction can fall back to dependency workspaces when needed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
}

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

/// Resolve the manifest directory for a dependency crate reachable from `root` via `cargo metadata`.
///
/// Cargo package names may use `-` while Rust import paths use `_`, so lookup normalizes both the package name and
/// target/lib names before matching.
fn dependency_manifest_dir_from_cargo_metadata(root: &Path, crate_name: &str) -> Option<PathBuf> {
    let manifest_path = root.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(manifest_path.as_os_str())
        .arg("--format-version")
        .arg("1")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: CargoMetadata = serde_json::from_slice(&output.stdout).ok()?;
    let normalized = normalize_crate_name(crate_name);
    parsed
        .packages
        .into_iter()
        .find(|pkg| {
            normalize_crate_name(pkg.name.as_str()) == normalized
                || pkg
                    .targets
                    .iter()
                    .any(|target| normalize_crate_name(target.name.as_str()) == normalized)
        })
        .and_then(|pkg| pkg.manifest_path.parent().map(Path::to_path_buf))
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

/// Resolve the best-known dependency manifest directory for `crate_name` from the generated lock workspace.
pub(crate) fn dependency_manifest_dir_for_crate(
    root: &Path,
    crate_name: &str,
    registry_src_roots: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    dependency_manifest_dir_from_cargo_metadata(root, crate_name)
        .or_else(|| dependency_manifest_dir_from_lock(root, crate_name, registry_src_roots))
}
