//! Oven's content-address rendering — SHA-256 over canonical bytes, written `sha256:<hex>` everywhere an identity is
//! stored or compared — and the source-tree digests built on it: a Cargo package with its recursive path
//! dependencies, under the conservative or the toolchain-semantic file-coverage policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Hash canonical text with Oven's stable `sha256:` rendering.
pub fn digest_content(content: &str) -> String {
    digest_bytes(content.as_bytes())
}

/// Hash arbitrary canonical identity bytes with Oven's stable `sha256:` rendering.
pub fn digest_bytes(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Failure while hashing a complete generated provider artifact.
#[derive(Debug, thiserror::Error)]
pub enum ProviderArtifactDigestError {
    /// The advertised artifact root is absent or not a directory.
    #[error("provider artifact root {path} is not a directory")]
    InvalidRoot { path: PathBuf },
    /// Reading or inspecting one artifact entry failed.
    #[error("failed to inspect provider artifact path {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// An entry could not be represented relative to its provider root.
    #[error("provider artifact path {path} is outside root {root}")]
    OutsideRoot { path: PathBuf, root: PathBuf },
    /// Published provider artifacts may not depend on symlinks or other special filesystem entries.
    #[error("provider artifact path {path} is not a regular file or directory")]
    UnsupportedEntry { path: PathBuf },
    /// An authored provider input is missing or is not a regular file.
    #[error("provider source input {path} is not a regular file")]
    InvalidSourceInput { path: PathBuf },
    /// An authored source resolved outside both its package and compiler-owned shared source roots.
    #[error("provider source input {path} is outside its package and trusted toolchain source roots")]
    OutsideSourceRoots { path: PathBuf },
    /// A checked delivery coordinate could not be normalized into the semantic identity projection.
    #[error("failed to normalize provider artifact path {path}: {message}")]
    Normalization { path: PathBuf, message: String },
}

/// Digest one Cargo package tree the toolchain ships, with a fresh per-call cache; see
/// [`digest_toolchain_source_tree_with_cache`] for what enters the digest.
pub fn digest_toolchain_source_tree(root: &Path) -> Result<String, ProviderArtifactDigestError> {
    digest_toolchain_source_tree_with_cache(root, &mut BTreeMap::new())
}

/// Hash a support package while sharing path-dependency results across one lock snapshot.
pub fn digest_toolchain_source_tree_with_cache(
    root: &Path,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
) -> Result<String, ProviderArtifactDigestError> {
    digest_cargo_package_inner(
        root,
        &mut BTreeSet::new(),
        resolved_packages,
        CargoSourceCoverage::ToolchainSemantic,
    )
}

/// Hash every non-output file in a local Rust package plus its recursive Cargo path dependencies.
///
/// Arbitrary third-party build scripts and `include_*` macros may consume inputs that cannot be inferred from the
/// manifest. This conservative authority therefore retains every regular package file except known mutable output
/// directories. Cargo manifests contribute normalized dependency edges so sibling packages remain relocation-stable.
pub fn digest_cargo_path_source_tree_with_cache(
    root: &Path,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
) -> Result<String, ProviderArtifactDigestError> {
    digest_cargo_package_inner(
        root,
        &mut BTreeSet::new(),
        resolved_packages,
        CargoSourceCoverage::ConservativePathCrate,
    )
}

/// File-coverage policy for one recursive Cargo package digest.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CargoSourceCoverage {
    /// Compiler-owned crates declare exceptional semantic inputs, allowing unrelated repository files to be ignored.
    ToolchainSemantic,
    /// Caller-owned crates receive conservative coverage because their build-time file reads are not constrained.
    ConservativePathCrate,
}

/// Resolve one package plus its recursive Cargo path dependencies into a path-independent digest.
fn digest_cargo_package_inner(
    root: &Path,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
    coverage: CargoSourceCoverage,
) -> Result<String, ProviderArtifactDigestError> {
    if !root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: root.to_path_buf(),
        });
    }
    if coverage == CargoSourceCoverage::ConservativePathCrate {
        let metadata = fs::symlink_metadata(root).map_err(|source| ProviderArtifactDigestError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ProviderArtifactDigestError::UnsupportedEntry {
                path: root.to_path_buf(),
            });
        }
    }
    let normalized_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if let Some(digest) = resolved_packages.get(&normalized_root) {
        return Ok(digest.clone());
    }
    if !visiting.insert(normalized_root.clone()) {
        return Err(ProviderArtifactDigestError::Normalization {
            path: root.to_path_buf(),
            message: match coverage {
                CargoSourceCoverage::ToolchainSemantic => {
                    "toolchain Cargo path-dependency graph contains a cycle".to_string()
                }
                CargoSourceCoverage::ConservativePathCrate => {
                    "Cargo path-dependency source graph contains a cycle".to_string()
                }
            },
        });
    }

    let manifest_path = root.join("Cargo.toml");
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| ProviderArtifactDigestError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest_text =
        std::str::from_utf8(&manifest_bytes).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;
    let mut manifest: toml::Value =
        toml::from_str(manifest_text).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;
    let mut direct_path_roots = BTreeSet::new();
    normalize_cargo_manifest_path_dependencies(
        &mut manifest,
        root,
        visiting,
        resolved_packages,
        coverage,
        &mut direct_path_roots,
    )?;
    let workspace_context = inherited_workspace_context(
        root,
        &manifest,
        visiting,
        resolved_packages,
        coverage,
        &mut direct_path_roots,
    )?;
    let normalized_manifest =
        toml::to_string(&manifest).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;

    let mut hasher = Sha256::new();
    hasher.update(match coverage {
        CargoSourceCoverage::ToolchainSemantic => b"incan-toolchain-cargo-package-v1\0".as_slice(),
        CargoSourceCoverage::ConservativePathCrate => b"incan-cargo-path-source-closure-v1\0".as_slice(),
    });
    hash_named_bytes(&mut hasher, "Cargo.toml", normalized_manifest.as_bytes());
    if let Some(context) = workspace_context {
        let context = toml::to_string(&context).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;
        hash_named_bytes(&mut hasher, "workspace-inherited.toml", context.as_bytes());
    }
    match coverage {
        CargoSourceCoverage::ToolchainSemantic => {
            hash_compiled_source_inputs(root, &root.join("src"), &mut hasher)?;
            let build_script = manifest
                .get("package")
                .and_then(toml::Value::as_table)
                .and_then(|package| package.get("build"))
                .and_then(toml::Value::as_str)
                .map(|path| root.join(path))
                .unwrap_or_else(|| root.join("build.rs"));
            if build_script.is_file() {
                hash_compiled_file(root, &build_script, &mut hasher)?;
            }
            hash_declared_semantic_inputs(root, &manifest, &mut hasher)?;
        }
        CargoSourceCoverage::ConservativePathCrate => {
            hash_conservative_package_inputs(root, root, &manifest_path, &direct_path_roots, &mut hasher)?;
        }
    }
    visiting.remove(&normalized_root);
    let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
    resolved_packages.insert(normalized_root, digest.clone());
    Ok(digest)
}

/// Replace path dependencies in one Cargo manifest with the source digests of their target packages.
fn normalize_cargo_manifest_path_dependencies(
    manifest: &mut toml::Value,
    base: &Path,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
    coverage: CargoSourceCoverage,
    direct_path_roots: &mut BTreeSet<PathBuf>,
) -> Result<(), ProviderArtifactDigestError> {
    let Some(root) = manifest.as_table_mut() else {
        return Ok(());
    };
    for section in ["dependencies", "build-dependencies"] {
        if let Some(dependencies) = root.get_mut(section) {
            normalize_cargo_dependency_table(
                dependencies,
                base,
                visiting,
                resolved_packages,
                coverage,
                direct_path_roots,
            )?;
        }
    }
    if let Some(targets) = root.get_mut("target").and_then(toml::Value::as_table_mut) {
        for (_, target_value) in targets.iter_mut() {
            let Some(target) = target_value.as_table_mut() else {
                continue;
            };
            for section in ["dependencies", "build-dependencies"] {
                if let Some(dependencies) = target.get_mut(section) {
                    normalize_cargo_dependency_table(
                        dependencies,
                        base,
                        visiting,
                        resolved_packages,
                        coverage,
                        direct_path_roots,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Normalize every direct path dependency in one Cargo dependency table.
fn normalize_cargo_dependency_table(
    dependencies: &mut toml::Value,
    base: &Path,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
    coverage: CargoSourceCoverage,
    direct_path_roots: &mut BTreeSet<PathBuf>,
) -> Result<(), ProviderArtifactDigestError> {
    let Some(dependencies) = dependencies.as_table_mut() else {
        return Ok(());
    };
    for (dependency_key, dependency) in dependencies {
        let Some(dependency) = dependency.as_table_mut() else {
            continue;
        };
        let Some(path) = dependency.get("path").and_then(toml::Value::as_str).map(str::to_string) else {
            continue;
        };
        let dependency_root = base.join(&path);
        let digest = digest_cargo_package_inner(&dependency_root, visiting, resolved_packages, coverage)?;
        if coverage == CargoSourceCoverage::ConservativePathCrate {
            let normalized_dependency_root =
                fs::canonicalize(&dependency_root).map_err(|source| ProviderArtifactDigestError::Io {
                    path: dependency_root.clone(),
                    source,
                })?;
            direct_path_roots.insert(normalized_dependency_root);
        }
        let package = dependency
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(dependency_key);
        dependency.insert(
            "path".to_string(),
            toml::Value::String(format!(
                "{}://{package}#{digest}",
                match coverage {
                    CargoSourceCoverage::ToolchainSemantic => "incan-toolchain-package",
                    CargoSourceCoverage::ConservativePathCrate => "incan-cargo-path-package",
                }
            )),
        );
    }
    Ok(())
}

/// Materialize only the workspace package and dependency values inherited by one Cargo package.
fn inherited_workspace_context(
    package_root: &Path,
    package_manifest: &toml::Value,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
    coverage: CargoSourceCoverage,
    direct_path_roots: &mut BTreeSet<PathBuf>,
) -> Result<Option<toml::Value>, ProviderArtifactDigestError> {
    let Some((workspace_root, workspace_manifest)) = find_workspace_manifest(package_root, package_manifest)? else {
        return Ok(None);
    };
    let mut context = toml::map::Map::new();
    if let (Some(package), Some(workspace_package)) = (
        package_manifest.get("package").and_then(toml::Value::as_table),
        workspace_manifest
            .get("workspace")
            .and_then(toml::Value::as_table)
            .and_then(|workspace| workspace.get("package"))
            .and_then(toml::Value::as_table),
    ) {
        let inherited = package
            .iter()
            .filter(|(_, value)| {
                value
                    .as_table()
                    .and_then(|table| table.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    == Some(true)
            })
            .filter_map(|(key, _)| workspace_package.get(key).cloned().map(|value| (key.clone(), value)))
            .collect::<toml::map::Map<_, _>>();
        if !inherited.is_empty() {
            context.insert("package".to_string(), toml::Value::Table(inherited));
        }
    }
    let workspace_dependencies = workspace_manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(toml::Value::as_table);
    if let Some(workspace_dependencies) = workspace_dependencies {
        let mut inherited = toml::map::Map::new();
        collect_inherited_workspace_dependencies(package_manifest, workspace_dependencies, &mut inherited);
        if !inherited.is_empty() {
            let mut dependencies = toml::Value::Table(inherited);
            normalize_cargo_dependency_table(
                &mut dependencies,
                &workspace_root,
                visiting,
                resolved_packages,
                coverage,
                direct_path_roots,
            )?;
            context.insert("dependencies".to_string(), dependencies);
        }
    }
    Ok((!context.is_empty()).then_some(toml::Value::Table(context)))
}

/// Copy workspace dependency entries selected with `{ workspace = true }` into the semantic context.
fn collect_inherited_workspace_dependencies(
    manifest: &toml::Value,
    workspace_dependencies: &toml::map::Map<String, toml::Value>,
    inherited: &mut toml::map::Map<String, toml::Value>,
) {
    let Some(root) = manifest.as_table() else {
        return;
    };
    for section in ["dependencies", "build-dependencies"] {
        if let Some(dependencies) = root.get(section).and_then(toml::Value::as_table) {
            collect_inherited_workspace_dependency_table(dependencies, workspace_dependencies, inherited);
        }
    }
    if let Some(targets) = root.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            for section in ["dependencies", "build-dependencies"] {
                if let Some(dependencies) = target.get(section).and_then(toml::Value::as_table) {
                    collect_inherited_workspace_dependency_table(dependencies, workspace_dependencies, inherited);
                }
            }
        }
    }
}

/// Copy selected workspace values from one normal, build, or target-specific dependency table.
fn collect_inherited_workspace_dependency_table(
    dependencies: &toml::map::Map<String, toml::Value>,
    workspace_dependencies: &toml::map::Map<String, toml::Value>,
    inherited: &mut toml::map::Map<String, toml::Value>,
) {
    for (key, value) in dependencies {
        if value
            .as_table()
            .and_then(|table| table.get("workspace"))
            .and_then(toml::Value::as_bool)
            == Some(true)
            && let Some(value) = workspace_dependencies.get(key)
        {
            inherited.insert(key.clone(), value.clone());
        }
    }
}

/// Locate the explicitly declared Cargo workspace, otherwise the nearest containing workspace manifest.
fn find_workspace_manifest(
    root: &Path,
    package_manifest: &toml::Value,
) -> Result<Option<(PathBuf, toml::Value)>, ProviderArtifactDigestError> {
    if let Some(workspace) = package_manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("workspace"))
    {
        let workspace = workspace
            .as_str()
            .ok_or_else(|| ProviderArtifactDigestError::Normalization {
                path: root.join("Cargo.toml"),
                message: "package.workspace must be a path string".to_string(),
            })?;
        let workspace_root = root.join(workspace);
        let path = workspace_root.join("Cargo.toml");
        let value = read_workspace_manifest(&path)?;
        if value.get("workspace").is_none() {
            return Err(ProviderArtifactDigestError::Normalization {
                path,
                message: "explicit package.workspace manifest has no [workspace] table".to_string(),
            });
        }
        return Ok(Some((workspace_root, value)));
    }
    for ancestor in root.ancestors().skip(1) {
        let path = ancestor.join("Cargo.toml");
        if !path.is_file() {
            continue;
        }
        let value = read_workspace_manifest(&path)?;
        if value.get("workspace").is_some() {
            return Ok(Some((ancestor.to_path_buf(), value)));
        }
    }
    Ok(None)
}

/// Parse one candidate workspace manifest with a path-rich failure.
fn read_workspace_manifest(path: &Path) -> Result<toml::Value, ProviderArtifactDigestError> {
    let bytes = fs::read(path).map_err(|source| ProviderArtifactDigestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_slice::<toml::Value>(&bytes).map_err(|error| ProviderArtifactDigestError::Normalization {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

/// Hash every conservatively authored package file without descending into outputs or nested path packages.
fn hash_conservative_package_inputs(
    package_root: &Path,
    directory: &Path,
    manifest_path: &Path,
    direct_path_roots: &BTreeSet<PathBuf>,
    hasher: &mut Sha256,
) -> Result<(), ProviderArtifactDigestError> {
    let generated_incan_root = fs::read_to_string(manifest_path)
        .is_ok_and(|manifest| manifest.starts_with("# Generated by the Incan compiler v"));
    let mut entries = fs::read_dir(directory)
        .map_err(|source| ProviderArtifactDigestError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ProviderArtifactDigestError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.is_dir() {
            // The generated crate's `oven/` directory holds profile-specific direct-rustc outputs and their
            // receipt indexes. It is checked independently when a public package Loaf is consumed, but normal
            // consumer commands may add a compatible debug profile after an explicit bake. It is not Cargo source.
            if generated_incan_root
                && directory == package_root
                && path.file_name().and_then(|name| name.to_str()) == Some("oven")
            {
                continue;
            }
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".git" | ".incan" | ".ralph-cache" | "target")
            ) {
                continue;
            }
            let normalized = fs::canonicalize(&path).map_err(|source| ProviderArtifactDigestError::Io {
                path: path.clone(),
                source,
            })?;
            if direct_path_roots.contains(&normalized) {
                continue;
            }
            hash_conservative_package_inputs(package_root, &path, manifest_path, direct_path_roots, hasher)?;
        } else if metadata.is_file() {
            // A generated Incan crate writes its lock and witness while the explicit publisher prepares it. Those
            // files are compiler output, not path-package source: the receiving project is bound to the canonical
            // Incan lock and the sealed direct-rustc plan instead. Keep ordinary Cargo path packages conservative,
            // including their root Cargo.lock, because they have no generated-root contract.
            if generated_incan_root
                && directory == package_root
                && matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("Cargo.lock" | ".incan-cargo-lock-manifest")
                )
            {
                continue;
            }
            if path != manifest_path {
                hash_compiled_file(package_root, &path, hasher)?;
            }
        } else {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
    }
    Ok(())
}

/// Hash every source-tree input below `directory`, including non-Rust files consumed by `include_*` macros.
fn hash_compiled_source_inputs(
    package_root: &Path,
    directory: &Path,
    hasher: &mut Sha256,
) -> Result<(), ProviderArtifactDigestError> {
    if !directory.is_dir() {
        return Ok(());
    }
    let mut entries = fs::read_dir(directory)
        .map_err(|source| ProviderArtifactDigestError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ProviderArtifactDigestError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            hash_compiled_source_inputs(package_root, &path, hasher)?;
        } else if file_type.is_file() {
            hash_compiled_file(package_root, &path, hasher)?;
        } else {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
    }
    Ok(())
}

/// Hash package-relative compiled inputs explicitly declared outside `src/`.
fn hash_declared_semantic_inputs(
    package_root: &Path,
    manifest: &toml::Value,
    hasher: &mut Sha256,
) -> Result<(), ProviderArtifactDigestError> {
    let manifest_path = package_root.join("Cargo.toml");
    let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    let Some(metadata_value) = package.get("metadata") else {
        return Ok(());
    };
    let metadata = metadata_value
        .as_table()
        .ok_or_else(|| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: "package.metadata must be a table".to_string(),
        })?;
    let Some(incan_value) = metadata.get("incan") else {
        return Ok(());
    };
    let incan_metadata = incan_value
        .as_table()
        .ok_or_else(|| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: "package.metadata.incan must be a table".to_string(),
        })?;
    let Some(inputs_value) = incan_metadata.get("semantic-inputs") else {
        return Ok(());
    };
    let inputs = inputs_value
        .as_array()
        .ok_or_else(|| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: "package.metadata.incan.semantic-inputs must be an array of paths".to_string(),
        })?;
    let mut inputs = inputs
        .iter()
        .map(|input| {
            input
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| ProviderArtifactDigestError::Normalization {
                    path: manifest_path.clone(),
                    message: "package.metadata.incan.semantic-inputs entries must be strings".to_string(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    inputs.sort();
    inputs.dedup();
    for input in inputs {
        let relative = Path::new(&input);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
        {
            return Err(ProviderArtifactDigestError::Normalization {
                path: manifest_path.clone(),
                message: format!("semantic input `{input}` must be a non-empty relative path within the package"),
            });
        }
        let path = semantic_input_path(package_root, relative)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.is_file() {
            hash_compiled_file(package_root, &path, hasher)?;
        } else if metadata.is_dir() {
            hash_compiled_source_inputs(package_root, &path, hasher)?;
        } else {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
    }
    Ok(())
}

/// Resolve a declared input while rejecting symlinks in every path component.
fn semantic_input_path(package_root: &Path, relative: &Path) -> Result<PathBuf, ProviderArtifactDigestError> {
    let mut path = package_root.to_path_buf();
    for component in relative.components() {
        path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&path).map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
    }
    Ok(path)
}

/// Hash one compiled input using its package-relative path and exact bytes.
fn hash_compiled_file(
    package_root: &Path,
    path: &Path,
    hasher: &mut Sha256,
) -> Result<(), ProviderArtifactDigestError> {
    let relative = path
        .strip_prefix(package_root)
        .map_err(|_| ProviderArtifactDigestError::OutsideRoot {
            path: path.to_path_buf(),
            root: package_root.to_path_buf(),
        })?;
    let bytes = fs::read(path).map_err(|source| ProviderArtifactDigestError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    hash_named_bytes(hasher, &relative.to_string_lossy().replace('\\', "/"), &bytes);
    Ok(())
}

/// Feed one named byte payload into a delimiter-safe semantic digest stream.
pub fn hash_named_bytes(hasher: &mut Sha256, name: &str, bytes: &[u8]) {
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    hasher.update([0xff]);
}
