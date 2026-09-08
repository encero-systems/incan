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

/// Compiler-injected support crates share this package-name prefix in every generated Cargo manifest (`incan_derive`,
/// `incan_stdlib`, and each `incan_stdlib_<component>` SDK component). They are never a project author's own
/// interop dependency, so [`path_dependency_dirs_from_manifest`] excludes them; see that function's docs for why.
const COMPILER_OWNED_CRATE_NAME_PREFIX: &str = "incan_";
