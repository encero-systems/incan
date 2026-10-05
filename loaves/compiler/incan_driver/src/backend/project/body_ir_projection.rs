//! Manifest-only closure projection for the compiler's Rust-backed Body IR caller contract.
//!
//! Cargo cannot distinguish same-version local packages at two roots in one lock. The real frontend and semantics-core
//! source trees are retained verbatim while their compiler-owned language edges select the active byte-equivalent SDK
//! copy. Only enumerated compiler roots qualify; build scripts and production location-dependent source are refused.

use std::{fs, io, path::Path};

use oven_model::manifest::{DependencySource, DependencySpec};
use oven_model::toolchain_layout::{development_root, resolve_toolchain_crate_path};
use sha2::{Digest, Sha256};

use super::generator::ProjectGenerator;

impl ProjectGenerator {
    /// Bind compiler-owned Body IR and frontend path crates to one byte-equivalent SDK language definition.
    ///
    /// Exact source bytes are retained; only workspace inheritance and compiler-owned path edges are projected.
    /// The compiler graph shares content-addressed package roots between the provider and Rust host.
    pub(super) fn project_body_ir_dependencies(
        &self,
        mut dependencies: Vec<DependencySpec>,
    ) -> io::Result<Vec<DependencySpec>> {
        let selected = resolve_toolchain_crate_path("incan_lang");
        let mut projected = std::collections::BTreeMap::new();
        let mut visiting = std::collections::BTreeSet::new();
        for dependency in &mut dependencies {
            let DependencySource::Path { path } = &mut dependency.source else {
                continue;
            };
            if compiler_projection_root(path)? {
                *path = project_compiler_library(path, &selected, &mut projected, &mut visiting)?;
            }
        }
        Ok(dependencies)
    }
}

/// Admit only source roots owned by the compiler's checked Body IR and in-process frontend closure.
///
/// Every generated project's path dependencies pass through here, so a path that does not resolve, or a toolchain with
/// no development checkout, is simply not a compiler root: it keeps its dependency exactly as declared.
fn compiler_projection_root(path: &Path) -> io::Result<bool> {
    let Ok(root) = fs::canonicalize(path) else {
        return Ok(false);
    };
    let development = development_root();
    Ok([
        "loaves/kernel/incan_semantics_core",
        "loaves/kernel/incan_syntax",
        "loaves/compiler/incan_frontend",
        "loaves/compiler/incan_format",
        "loaves/compiler/incan_semantics_stdlib",
        "loaves/compiler/rust_inspect",
    ]
    .iter()
    .filter_map(|relative| fs::canonicalize(development.join(relative)).ok())
    .any(|compiler_root| compiler_root == root))
}

/// Expand one selected compiler manifest and recursively retain its compiler-owned dependency roots.
fn project_compiler_library(
    root: &Path,
    language: &Path,
    projected: &mut std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    visiting: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> io::Result<std::path::PathBuf> {
    let root = fs::canonicalize(root)?;
    if let Some(path) = projected.get(&root) {
        return Ok(path.clone());
    }
    if !visiting.insert(root.clone()) {
        return Err(io::Error::other(
            "compiler source projection refuses cyclic path dependencies",
        ));
    }
    let mut manifest = read_effective_manifest(&root.join("Cargo.toml"))?;
    project_compiler_edges(&root, &mut manifest, language, projected, visiting)?;
    if root.join("build.rs").exists() {
        return Err(io::Error::other("compiler source projection refuses build scripts"));
    }
    let sources = source_records(&root.join("src"))?;
    validate_projection_locations(&sources)?;
    let table = manifest
        .as_table_mut()
        .ok_or_else(|| io::Error::other("compiler manifest must be a table"))?;
    table.insert("workspace".into(), toml::Value::Table(toml::Table::new()));
    table.remove("dev-dependencies");
    let result = materialize_projection(&manifest, sources)?;
    visiting.remove(&root);
    projected.insert(root, result.clone());
    Ok(result)
}

/// Rebind only proven language sources and recursively selected compiler crates, leaving other paths anchored.
fn project_compiler_edges(
    root: &Path,
    manifest: &mut toml::Value,
    language: &Path,
    projected: &mut std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    visiting: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> io::Result<()> {
    let Some(dependencies) = manifest.get_mut("dependencies").and_then(toml::Value::as_table_mut) else {
        return Ok(());
    };
    for (name, specification) in dependencies {
        let Some(table) = specification.as_table_mut() else {
            continue;
        };
        let Some(declared) = table.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let declared = root.join(declared);
        let selected = if name == "incan_lang" {
            verify_language_source(&declared, language)?;
            language.to_path_buf()
        } else if compiler_projection_root(&declared)? {
            project_compiler_library(&declared, language, projected, visiting)?
        } else {
            declared
        };
        table.insert(
            "path".into(),
            toml::Value::String(selected.to_string_lossy().into_owned()),
        );
    }
    Ok(())
}

/// Require the declaring compiler and selected SDK language package to have identical definitions and versions.
fn verify_language_source(original: &Path, selected: &Path) -> io::Result<()> {
    if fs::canonicalize(original)? == fs::canonicalize(selected)? {
        return Ok(());
    }
    if source_records(&original.join("src"))? != source_records(&selected.join("src"))? {
        return Err(io::Error::other(
            "Body IR caller requires byte-equivalent compiler and SDK incan_lang sources",
        ));
    }
    let original = read_effective_manifest(&original.join("Cargo.toml"))?;
    let selected = read_effective_manifest(&selected.join("Cargo.toml"))?;
    if original.get("package").and_then(|p| p.get("version")) != selected.get("package").and_then(|p| p.get("version"))
    {
        return Err(io::Error::other(
            "Body IR caller compiler and SDK incan_lang versions differ",
        ));
    }
    Ok(())
}

/// Refuse manifest-relative production inputs; test-only checkout probes are absent from library builds.
///
/// Relative include files remain at their exact source-relative locations in the snapshot. The unprojected
/// oven_model retains its real manifest root because its development-root contract is location-sensitive.
fn validate_projection_locations(sources: &[(String, Vec<u8>)]) -> io::Result<()> {
    for (name, bytes) in sources {
        let test = name.contains("/tests/") || name.starts_with("tests/") || name.ends_with("_tests.rs");
        if !test && String::from_utf8_lossy(bytes).contains("CARGO_MANIFEST_DIR") {
            return Err(io::Error::other(format!(
                "compiler source projection refuses manifest-relative source {name}"
            )));
        }
    }
    Ok(())
}

/// Publish content-addressed source bytes under a lock shared by provider and host Cargo graphs.
fn materialize_projection(manifest: &toml::Value, sources: Vec<(String, Vec<u8>)>) -> io::Result<std::path::PathBuf> {
    let rendered = toml::to_string(&manifest).map_err(io::Error::other)?;
    let mut digest = Sha256::new();
    digest.update(rendered.as_bytes());
    for (name, bytes) in &sources {
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update(bytes.len().to_le_bytes());
        digest.update(bytes);
    }
    let shadow = std::env::temp_dir()
        .join("incan-body-ir-source")
        .join(hex::encode(digest.finalize()));
    fs::create_dir_all(&shadow)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(shadow.join(".projection.lock"))?;
    lock.lock()?;
    fs::create_dir_all(shadow.join("src"))?;
    for (name, bytes) in sources {
        let destination = shadow.join("src").join(name);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::read(&destination).ok().as_deref() != Some(bytes.as_slice()) {
            fs::write(destination, bytes)?;
        }
    }
    let manifest_path = shadow.join("Cargo.toml");
    if fs::read_to_string(&manifest_path).ok().as_deref() != Some(rendered.as_str()) {
        fs::write(manifest_path, rendered)?;
    }
    Ok(shadow)
}

/// Resolve selected workspace fields for one manifest using the native path planner's identical projection.
fn read_effective_manifest(path: &Path) -> io::Result<toml::Value> {
    let manifest = toml::from_str(&fs::read_to_string(path)?).map_err(io::Error::other)?;
    oven_rustc::rustc::effective_path_manifest(path, manifest).map_err(io::Error::other)
}

/// Collect exact source bytes in stable relative-path order; non-regular entries are never projected.
fn source_records(root: &Path) -> io::Result<Vec<(String, Vec<u8>)>> {
    /// Retain relative names and bytes without following symbolic links.
    fn collect(root: &Path, directory: &Path, records: &mut Vec<(String, Vec<u8>)>) -> io::Result<()> {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                collect(root, &path, records)?;
            } else if entry.file_type()?.is_file() {
                let relative = path.strip_prefix(root).map_err(io::Error::other)?;
                records.push((relative.to_string_lossy().into_owned(), fs::read(path)?));
            } else {
                return Err(io::Error::other(
                    "Body IR projection refuses non-regular source entries",
                ));
            }
        }
        Ok(())
    }
    let mut records = Vec::new();
    collect(root, root, &mut records)?;
    Ok(records)
}
