//! Deterministic integrity identity for one relocatable compiled-provider artifact tree.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::wire::RawLibraryManifest;
use super::{LibraryManifest, ProviderCargoDependency, ProviderCargoDependencySource, ProviderDependencyMetadata};

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

/// Hash every immutable manifest, generated source, and generated-project input in one provider artifact tree.
///
/// Compiler, VCS, and test-runner output directories are deliberately excluded because they are mutable caches
/// rather than provider content. Generated providers normally use an external shared target directory, but these
/// exclusions keep integrity stable if a backend tool creates conventional local output later.
pub fn digest_provider_artifact(root: &Path) -> Result<String, ProviderArtifactDigestError> {
    if !root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: root.to_path_buf(),
        });
    }
    let mut hasher = Sha256::new();
    hasher.update(b"incan-provider-artifact-v1\0");
    hash_directory(root, root, &mut hasher)?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Hash the authored manifest and logically named Incan source modules that define one compiled provider's semantics.
///
/// The resulting identity deliberately precedes generated Rust and host-derived ABI extraction. Canonical locks use
/// it to compare equivalent native builds, while [`digest_provider_artifact`] remains the byte-exact integrity check
/// for each physical artifact. Logical module labels keep shared toolchain source outside the package directory
/// relocation-stable without excluding it from the semantic identity.
pub(crate) fn digest_provider_source_inputs(
    project_root: &Path,
    manifest_path: &Path,
    source_inputs: &[(String, PathBuf)],
    trusted_source_roots: &[PathBuf],
) -> Result<String, ProviderArtifactDigestError> {
    if !project_root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: project_root.to_path_buf(),
        });
    }
    let resolve = |path: &Path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            project_root.join(path)
        }
    };
    let canonical_project_root = fs::canonicalize(project_root).map_err(|source| ProviderArtifactDigestError::Io {
        path: project_root.to_path_buf(),
        source,
    })?;
    let mut allowed_source_roots = vec![canonical_project_root.clone()];
    for root in trusted_source_roots {
        if !root.is_dir() {
            return Err(ProviderArtifactDigestError::InvalidRoot { path: root.clone() });
        }
        let canonical = fs::canonicalize(root).map_err(|source| ProviderArtifactDigestError::Io {
            path: root.clone(),
            source,
        })?;
        if !allowed_source_roots.contains(&canonical) {
            allowed_source_roots.push(canonical);
        }
    }
    let canonical_source_file = |path: PathBuf| {
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ProviderArtifactDigestError::InvalidSourceInput { path: path.clone() }
            } else {
                ProviderArtifactDigestError::Io {
                    path: path.clone(),
                    source,
                }
            }
        })?;
        if !metadata.file_type().is_file() {
            return Err(ProviderArtifactDigestError::InvalidSourceInput { path });
        }
        fs::canonicalize(&path).map_err(|source| ProviderArtifactDigestError::Io { path, source })
    };
    let manifest_path = canonical_source_file(resolve(manifest_path))?;
    let manifest_label = manifest_path
        .strip_prefix(&canonical_project_root)
        .map_err(|_| ProviderArtifactDigestError::OutsideRoot {
            path: manifest_path.clone(),
            root: canonical_project_root.clone(),
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let mut inputs = BTreeMap::from([(format!("manifest:{manifest_label}"), manifest_path)]);
    for (module_label, path) in source_inputs {
        let path = canonical_source_file(resolve(path))?;
        if !allowed_source_roots.iter().any(|root| path.starts_with(root)) {
            return Err(ProviderArtifactDigestError::OutsideSourceRoots { path });
        }
        let label = format!("module:{module_label}");
        if module_label.is_empty() || inputs.insert(label.clone(), path.clone()).is_some() {
            return Err(ProviderArtifactDigestError::Normalization {
                path,
                message: format!("provider source input has invalid or duplicate logical label `{label}`"),
            });
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(b"incan-provider-source-inputs-v1\0");
    for (relative, path) in inputs {
        let bytes = fs::read(&path).map_err(|source| ProviderArtifactDigestError::Io { path, source })?;
        hash_named_bytes(&mut hasher, &relative, &bytes);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Hash the Cargo-semantic closure of one compiler-owned support crate.
///
/// Only package inputs that can affect compiled code are included: the normalized package manifest, inherited
/// workspace package/dependency values, every regular file under `src/`, a build script, explicitly declared
/// `package.metadata.incan.semantic-inputs` outside `src/`, and recursive normal/build/target path dependencies.
/// Undeclared repository noise such as README files, tests, editor files, and target caches is intentionally absent.
#[cfg(test)]
fn digest_toolchain_source_tree(root: &Path) -> Result<String, ProviderArtifactDigestError> {
    digest_toolchain_source_tree_with_cache(root, &mut BTreeMap::new())
}

/// Hash a support package while sharing path-dependency results across one lock snapshot.
pub(crate) fn digest_toolchain_source_tree_with_cache(
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
pub(crate) fn digest_cargo_path_source_tree_with_cache(
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
fn hash_named_bytes(hasher: &mut Sha256, name: &str, bytes: &[u8]) {
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    hasher.update([0xff]);
}

/// Hash one generated provider artifact through its source-owned semantic lock projection.
///
/// The ordinary artifact digest remains the byte-exact integrity contract frozen into compiled provider edges. This
/// projection is narrower: new artifacts bind authored source inputs to their normalized checked contract and Cargo
/// requirements, so equivalent providers rebuilt beneath another cache root or on another host remain the same.
/// Legacy artifacts without an authored source digest retain the v1 byte-derived projection for compatibility.
#[cfg(test)]
fn digest_provider_semantic_artifact(
    root: &Path,
    manifest_path: &Path,
    cargo_toml_path: &Path,
    manifest: &LibraryManifest,
) -> Result<String, ProviderArtifactDigestError> {
    digest_provider_semantic_artifact_with_dependencies(
        root,
        manifest_path,
        cargo_toml_path,
        manifest,
        &BTreeMap::new(),
    )
}

/// Hash a provider semantic projection using known path-independent identities for relocated dependency artifacts.
#[cfg(test)]
fn digest_provider_semantic_artifact_with_dependencies(
    root: &Path,
    manifest_path: &Path,
    cargo_toml_path: &Path,
    manifest: &LibraryManifest,
    dependency_semantic_digests: &BTreeMap<String, String>,
) -> Result<String, ProviderArtifactDigestError> {
    digest_provider_semantic_artifact_with_context(
        root,
        manifest_path,
        cargo_toml_path,
        manifest,
        dependency_semantic_digests,
        &[],
    )
}

/// One exact compiler-owned support crate permitted to replace a physical generated-Cargo path in semantic identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderSemanticToolchainDependency {
    /// Generated Cargo dependency key.
    pub(crate) crate_name: String,
    /// Cargo package name after alias resolution.
    pub(crate) package_name: String,
    /// Exact active support-crate root proven by the SDK dependency catalog.
    pub(crate) artifact_root: PathBuf,
    /// Path-independent content identity of that support crate.
    pub(crate) content_digest: String,
}

/// Hash a semantic provider artifact with both provider-edge and exact SDK toolchain-path identities.
#[cfg(test)]
fn digest_provider_semantic_artifact_with_context(
    root: &Path,
    manifest_path: &Path,
    cargo_toml_path: &Path,
    manifest: &LibraryManifest,
    dependency_semantic_digests: &BTreeMap<String, String>,
    sdk_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
) -> Result<String, ProviderArtifactDigestError> {
    digest_provider_semantic_artifact_with_context_and_cache(
        root,
        manifest_path,
        cargo_toml_path,
        manifest,
        dependency_semantic_digests,
        sdk_toolchain_dependencies,
        &mut BTreeMap::new(),
    )
}

/// Hash a semantic provider graph while memoizing subtrees resolved under the same semantic context.
pub(crate) fn digest_provider_semantic_artifact_with_context_and_cache(
    root: &Path,
    manifest_path: &Path,
    cargo_toml_path: &Path,
    manifest: &LibraryManifest,
    dependency_semantic_digests: &BTreeMap<String, String>,
    sdk_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    resolved_artifacts: &mut BTreeMap<PathBuf, String>,
) -> Result<String, ProviderArtifactDigestError> {
    let mut context = ProviderSemanticDigestContext {
        dependency_semantic_digests,
        sdk_toolchain_dependencies,
        visiting: BTreeSet::new(),
        resolved_artifacts,
    };
    digest_provider_semantic_artifact_inner(root, manifest_path, cargo_toml_path, manifest, &mut context)
}

/// Shared state for one recursive semantic provider digest traversal.
struct ProviderSemanticDigestContext<'a> {
    dependency_semantic_digests: &'a BTreeMap<String, String>,
    sdk_toolchain_dependencies: &'a [ProviderSemanticToolchainDependency],
    visiting: BTreeSet<PathBuf>,
    resolved_artifacts: &'a mut BTreeMap<PathBuf, String>,
}

/// Render every semantic dimension that authorizes replacing one physical provider path.
fn provider_dependency_semantic_coordinate(dependency: &ProviderDependencyMetadata) -> String {
    let features = dependency
        .requested_features
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "incan-provider://{:?}/{}/{}@{}#{}[{}];default={};optional={}",
        dependency.kind,
        dependency.dependency_key,
        dependency.provider_name,
        dependency.provider_version,
        dependency.artifact_digest,
        features,
        dependency.default_features,
        dependency.optional,
    )
}

/// Stable checked coordinate for one compiler-owned toolchain dependency used by a provider facet.
fn toolchain_dependency_coordinate(dependency: &ProviderCargoDependency) -> ToolchainDependencyCoordinate {
    let ProviderCargoDependencySource::Toolchain { relative_path } = &dependency.source else {
        unreachable!("caller filters registry-backed provider dependencies")
    };
    let features = dependency.features.iter().cloned().collect::<Vec<_>>().join(",");
    let package_name = dependency
        .package
        .as_deref()
        .unwrap_or(&dependency.crate_name)
        .to_string();
    ToolchainDependencyCoordinate {
        package_name,
        semantic_coordinate: format!(
            "incan-toolchain://{}?version={}&features={features}&default={}",
            relative_path,
            dependency.version.as_deref().unwrap_or(""),
            dependency.default_features,
        ),
        expected_artifact_root: None,
    }
}

/// Checked package identity and stable toolchain-relative source for one generated Cargo dependency.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ToolchainDependencyCoordinate {
    package_name: String,
    semantic_coordinate: String,
    expected_artifact_root: Option<PathBuf>,
}

/// Normalize paths only for unambiguous Cargo entries backed by checked provider toolchain metadata.
fn normalize_toolchain_dependency_paths(
    cargo: &mut toml::Value,
    cargo_toml_path: &Path,
    toolchain_dependencies: &BTreeMap<String, BTreeSet<ToolchainDependencyCoordinate>>,
) {
    let Some(root) = cargo.as_table_mut() else {
        return;
    };
    for table_name in ["dependencies", "dev-dependencies"] {
        let Some(dependencies) = root.get_mut(table_name).and_then(toml::Value::as_table_mut) else {
            continue;
        };
        for (crate_name, candidates) in toolchain_dependencies {
            let Some(dependency) = dependencies.get_mut(crate_name).and_then(toml::Value::as_table_mut) else {
                continue;
            };
            let package_name = dependency
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(crate_name);
            let Some(authored_path) = dependency.get("path").and_then(toml::Value::as_str) else {
                continue;
            };
            let cargo_dir = cargo_toml_path.parent().unwrap_or_else(|| Path::new("."));
            let resolved_path = if Path::new(authored_path).is_absolute() {
                PathBuf::from(authored_path)
            } else {
                cargo_dir.join(authored_path)
            };
            let matching = candidates
                .iter()
                .filter(|coordinate| {
                    package_name == coordinate.package_name
                        && coordinate
                            .expected_artifact_root
                            .as_ref()
                            .is_none_or(|expected| dependency_paths_match(&resolved_path, expected))
                })
                .collect::<Vec<_>>();
            if let [coordinate] = matching.as_slice()
                && let Some(toml::Value::String(path)) = dependency.get_mut("path")
            {
                *path = coordinate.semantic_coordinate.clone();
            }
        }
    }
}

/// Compare a generated Cargo path with an exact compiler-owned dependency root.
fn dependency_paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Feed one artifact directory into the stable digest in lexical path order while excluding mutable output trees.
fn hash_directory(root: &Path, directory: &Path, hasher: &mut Sha256) -> Result<(), ProviderArtifactDigestError> {
    hash_directory_with_normalization(root, directory, hasher, None, false)
}

/// Feed one artifact directory into either the physical or semantic stable digest projection.
fn hash_directory_with_normalization(
    root: &Path,
    directory: &Path,
    hasher: &mut Sha256,
    normalization: Option<&SemanticArtifactNormalization<'_>>,
    exclude_nested_targets: bool,
) -> Result<(), ProviderArtifactDigestError> {
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
        let relative = path
            .strip_prefix(root)
            .map_err(|_| ProviderArtifactDigestError::OutsideRoot {
                path: path.clone(),
                root: root.to_path_buf(),
            })?;
        let file_name = path.file_name().and_then(|name| name.to_str());
        let file_type = entry.file_type().map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            let is_mutable_output = matches!(file_name, Some(".git" | ".incan" | ".ralph-cache" | "target"));
            // v0.5 providers briefly placed the compiler-owned Rust-inspection Cargo target below the published
            // `oven/` directory. It is mutable preparation state, not provider content. Exclude the legacy location
            // so an existing generated provider remains loadable while current builders place it under `target/`.
            let is_legacy_rust_inspect_output = relative == Path::new("oven/rust-inspect");
            let is_nested_target = file_name == Some("target");
            if is_mutable_output || is_legacy_rust_inspect_output || (exclude_nested_targets && is_nested_target) {
                continue;
            }
        }
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        hasher.update([0]);
        if file_type.is_dir() {
            hasher.update(b"directory\0");
            hash_directory_with_normalization(root, &path, hasher, normalization, exclude_nested_targets)?;
        } else if file_type.is_file() {
            hasher.update(b"file\0");
            let bytes = if let Some(normalization) = normalization {
                if path == normalization.manifest_path {
                    normalization.normalized_manifest_bytes.to_vec()
                } else {
                    fs::read(&path).map_err(|source| ProviderArtifactDigestError::Io {
                        path: path.clone(),
                        source,
                    })?
                }
            } else {
                fs::read(&path).map_err(|source| ProviderArtifactDigestError::Io {
                    path: path.clone(),
                    source,
                })?
            };
            hasher.update(bytes);
        } else {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
        hasher.update([0xff]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn digest_tracks_manifest_and_generated_source_but_ignores_mutable_output() -> TestResult {
        let artifact = tempfile::tempdir()?;
        fs::create_dir_all(artifact.path().join("src"))?;
        fs::write(artifact.path().join("provider.incnlib"), "manifest")?;
        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }")?;
        let initial = digest_provider_artifact(artifact.path())?;

        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 2 }")?;
        let source_changed = digest_provider_artifact(artifact.path())?;
        assert_ne!(initial, source_changed);

        fs::write(artifact.path().join("target"), "authored provider content")?;
        assert_ne!(source_changed, digest_provider_artifact(artifact.path())?);
        fs::remove_file(artifact.path().join("target"))?;

        for directory in [".git", ".incan/oven", ".ralph-cache/loafs", "target/debug"] {
            fs::create_dir_all(artifact.path().join(directory))?;
            fs::write(artifact.path().join(directory).join("mutable"), "not provider content")?;
        }
        assert_eq!(source_changed, digest_provider_artifact(artifact.path())?);
        Ok(())
    }

    #[test]
    fn authored_provider_source_digest_is_relocation_stable_and_tracks_inputs_issue931() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for workspace in [first.path(), second.path()] {
            let root = workspace.join("component");
            fs::create_dir_all(root.join("src"))?;
            fs::create_dir_all(workspace.join("shared"))?;
            fs::write(
                root.join("loaf.toml"),
                "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
            )?;
            fs::write(root.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
            fs::write(
                workspace.join("shared/item.incn"),
                "pub const LABEL: str = \"stable\"\n",
            )?;
        }
        let source_inputs = |workspace: &Path| {
            vec![
                ("<root>".to_string(), workspace.join("component/src/lib.incn")),
                ("shared.item".to_string(), workspace.join("shared/item.incn")),
            ]
        };
        let first_root = first.path().join("component");
        let second_root = second.path().join("component");
        let first_digest = digest_provider_source_inputs(
            &first_root,
            &first_root.join("loaf.toml"),
            &source_inputs(first.path()),
            &[first.path().join("shared")],
        )?;
        let second_digest = digest_provider_source_inputs(
            &second_root,
            &second_root.join("loaf.toml"),
            &source_inputs(second.path()),
            &[second.path().join("shared")],
        )?;
        assert_eq!(first_digest, second_digest);

        fs::write(
            second.path().join("shared/item.incn"),
            "pub const LABEL: str = \"changed\"\n",
        )?;
        assert_ne!(
            first_digest,
            digest_provider_source_inputs(
                &second_root,
                &second_root.join("loaf.toml"),
                &source_inputs(second.path()),
                &[second.path().join("shared")],
            )?
        );

        let outside = second.path().join("untrusted.incn");
        fs::write(&outside, "pub const LABEL: str = \"outside\"\n")?;
        let error = digest_provider_source_inputs(
            &second_root,
            &second_root.join("loaf.toml"),
            &[("outside".to_string(), outside)],
            &[],
        )
        .err()
        .ok_or("expected untrusted outside-root source input to fail")?;
        assert!(matches!(error, ProviderArtifactDigestError::OutsideSourceRoots { .. }));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn authored_provider_source_digest_rejects_symlink_inputs_issue931() -> TestResult {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir()?;
        let root = workspace.path().join("component");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("loaf.toml"), "[project]\nname = \"provider\"\n")?;
        fs::write(root.join("src/real.incn"), "pub const LABEL: str = \"real\"\n")?;
        symlink(root.join("src/real.incn"), root.join("src/link.incn"))?;

        let error = digest_provider_source_inputs(
            &root,
            &root.join("loaf.toml"),
            &[("link".to_string(), root.join("src/link.incn"))],
            &[],
        )
        .err()
        .ok_or("expected symlinked provider source input to fail")?;
        assert!(matches!(error, ProviderArtifactDigestError::InvalidSourceInput { .. }));
        Ok(())
    }

    #[test]
    fn semantic_digest_uses_authored_inputs_not_host_generated_outputs_issue931() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let make_artifact = |root: &Path, generated: &str, host_abi: bool, user_path: &str| -> TestResult {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), generated)?;
            fs::write(
                root.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"provider\"\nversion = \"0.1.0\"\n\n[dependencies.user_path]\npath = \"{user_path}\"\n"
                ),
            )?;
            let mut manifest = LibraryManifest::new("provider", "0.1.0");
            manifest.contract_metadata.provider.semantic_source_digest = Some(format!("sha256:{}", "a".repeat(64)));
            if host_abi {
                manifest.rust_abi = Some(super::super::LibraryRustAbi {
                    schema_version: super::super::RUST_ABI_SCHEMA_VERSION,
                    items: Vec::new(),
                });
            }
            manifest.write_to_path(&root.join("provider.incnlib"))?;
            Ok(())
        };
        make_artifact(
            first.path(),
            "pub fn platform_value() -> &'static str { \"macos\" }\n",
            false,
            "../user-dependency",
        )?;
        make_artifact(
            second.path(),
            "pub fn platform_value() -> &'static str { \"linux\" }\n",
            true,
            "../user-dependency",
        )?;
        let semantic = |root: &Path| -> Result<String, Box<dyn std::error::Error>> {
            let manifest = LibraryManifest::read_from_path(&root.join("provider.incnlib"))?;
            Ok(digest_provider_semantic_artifact(
                root,
                &root.join("provider.incnlib"),
                &root.join("Cargo.toml"),
                &manifest,
            )?)
        };

        assert_ne!(
            digest_provider_artifact(first.path())?,
            digest_provider_artifact(second.path())?
        );
        let stable = semantic(first.path())?;
        assert_eq!(stable, semantic(second.path())?);

        let mut changed_source = LibraryManifest::read_from_path(&second.path().join("provider.incnlib"))?;
        changed_source.contract_metadata.provider.semantic_source_digest = Some(format!("sha256:{}", "b".repeat(64)));
        changed_source.write_to_path(&second.path().join("provider.incnlib"))?;
        assert_ne!(stable, semantic(second.path())?);

        changed_source.contract_metadata.provider.semantic_source_digest = Some(format!("sha256:{}", "a".repeat(64)));
        changed_source
            .contract_metadata
            .provider
            .namespace_claims
            .push(super::super::ProviderModuleClaim {
                module_path: vec!["changed_contract".to_string()],
                required_features: BTreeSet::new(),
            });
        changed_source.write_to_path(&second.path().join("provider.incnlib"))?;
        assert_ne!(stable, semantic(second.path())?);

        changed_source.contract_metadata.provider.namespace_claims.clear();
        changed_source.contract_metadata.provider.public_features.insert(
            "changed_feature".to_string(),
            super::super::ProviderFeatureMetadata::default(),
        );
        changed_source.write_to_path(&second.path().join("provider.incnlib"))?;
        assert_ne!(stable, semantic(second.path())?);

        changed_source.contract_metadata.provider.public_features.clear();
        changed_source.write_to_path(&second.path().join("provider.incnlib"))?;
        fs::write(
            second.path().join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\n\n[dependencies.user_path]\npath = \"../different-user-dependency\"\n",
        )?;
        assert_ne!(stable, semantic(second.path())?);
        Ok(())
    }

    #[test]
    fn semantic_digest_excludes_only_generated_provider_root_cargo_lock() -> TestResult {
        let artifact = tempfile::tempdir()?;
        fs::create_dir_all(artifact.path().join("src"))?;
        fs::create_dir_all(artifact.path().join("nested_dependency"))?;
        fs::write(
            artifact.path().join("Cargo.toml"),
            "[package]\nname = \"root_lib\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n")?;
        fs::write(
            artifact.path().join("nested_dependency/Cargo.lock"),
            "version = 4\n# nested-v1\n",
        )?;
        let manifest = LibraryManifest::new("root_lib", "0.1.0");
        let manifest_path = artifact.path().join("root_lib.incnlib");
        manifest.write_to_path(&manifest_path)?;
        let semantic_digest = || {
            digest_provider_semantic_artifact(
                artifact.path(),
                &manifest_path,
                &artifact.path().join("Cargo.toml"),
                &manifest,
            )
        };

        let initial_physical = digest_provider_artifact(artifact.path())?;
        let initial_semantic = semantic_digest()?;
        fs::write(artifact.path().join("Cargo.lock"), "version = 4\n# root-v1\n")?;
        let root_lock_physical = digest_provider_artifact(artifact.path())?;
        assert_ne!(initial_physical, root_lock_physical);
        assert_eq!(initial_semantic, semantic_digest()?);

        fs::write(artifact.path().join("Cargo.lock"), "version = 4\n# root-v2\n")?;
        assert_ne!(root_lock_physical, digest_provider_artifact(artifact.path())?);
        assert_eq!(initial_semantic, semantic_digest()?);

        fs::write(
            artifact.path().join("nested_dependency/Cargo.lock"),
            "version = 4\n# nested-v2\n",
        )?;
        assert_ne!(initial_semantic, semantic_digest()?);
        Ok(())
    }

    #[test]
    fn path_dependency_digest_ignores_generated_root_output_state() -> TestResult {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src"))?;
        fs::write(
            generated.path().join("Cargo.toml"),
            "# Generated by the Incan compiler v0.5.1-dev.7\n\n[package]\nname = \"generated_provider\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(generated.path().join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
        let generated_digest = || digest_cargo_path_source_tree_with_cache(generated.path(), &mut BTreeMap::new());
        let initial = generated_digest()?;
        fs::write(generated.path().join("Cargo.lock"), "version = 4\n# generated lock\n")?;
        fs::write(
            generated.path().join(".incan-cargo-lock-manifest"),
            "sha256:generated-lock-witness\n",
        )?;
        fs::create_dir_all(generated.path().join("oven/release"))?;
        fs::write(
            generated.path().join("oven/package-loafs.json"),
            "{\"schema_version\": 6}\n",
        )?;
        fs::write(
            generated.path().join("oven/release/libgenerated_provider.rlib"),
            "release-v1\n",
        )?;
        assert_eq!(initial, generated_digest()?);
        fs::write(
            generated.path().join("oven/package-loafs.json"),
            "{\"schema_version\": 6, \"debug\": true}\n",
        )?;
        fs::write(generated.path().join("oven/debug-plan.json"), "debug-v1\n")?;
        assert_eq!(initial, generated_digest()?);
        fs::write(generated.path().join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")?;
        assert_ne!(initial, generated_digest()?);

        let authored = tempfile::tempdir()?;
        fs::create_dir_all(authored.path().join("src"))?;
        fs::write(
            authored.path().join("Cargo.toml"),
            "[package]\nname = \"authored_provider\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(authored.path().join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
        let authored_digest = || digest_cargo_path_source_tree_with_cache(authored.path(), &mut BTreeMap::new());
        let initial = authored_digest()?;
        fs::write(authored.path().join("Cargo.lock"), "version = 4\n# authored lock\n")?;
        assert_ne!(initial, authored_digest()?);
        Ok(())
    }

    #[test]
    fn toolchain_source_digest_ignores_nested_targets_and_tracks_source_inputs_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first = temp.path().join("source-a/crates/incan_stdlib");
        let second = temp.path().join("source-b/crates/incan_stdlib");
        for root in [&first, &second] {
            fs::create_dir_all(root.join("src"))?;
            fs::create_dir_all(root.join("semantic-inputs"))?;
            fs::create_dir_all(root.join("tests"))?;
            fs::create_dir_all(root.join("stdlib/components/data/target/debug"))?;
            fs::create_dir_all(
                root.parent()
                    .ok_or("support root has no parent")?
                    .join("incan_core/src"),
            )?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.0\"\n\n[package.metadata.incan]\nsemantic-inputs = [\"semantic-inputs/schema.json\"]\n\n[dependencies]\nincan_core = { path = \"../incan_core\" }\n",
            )?;
            fs::write(root.join("src/lib.rs"), "pub fn support() {}\n")?;
            fs::write(root.join("src/embedded.txt"), "compiled include input\n")?;
            fs::write(root.join("semantic-inputs/schema.json"), "{\"version\": 1}\n")?;
            fs::write(root.join("build.rs"), "fn main() {}\n")?;
            fs::write(root.join("README.md"), "checkout-specific documentation\n")?;
            fs::write(root.join("tests/not_compiled.rs"), "checkout-specific test\n")?;
            fs::write(root.join(".DS_Store"), "checkout-specific editor state\n")?;
            let core = root.parent().ok_or("support root has no parent")?.join("incan_core");
            fs::write(
                core.join("Cargo.toml"),
                "[package]\nname = \"incan_core\"\nversion = \"0.5.0\"\n",
            )?;
            fs::write(core.join("src/lib.rs"), "pub fn core() {}\n")?;
        }
        fs::write(
            first.join("stdlib/components/data/target/debug/cache"),
            "source-a cache",
        )?;
        fs::write(
            second.join("stdlib/components/data/target/debug/cache"),
            "source-b cache",
        )?;
        let stable = digest_toolchain_source_tree(&first)?;
        assert_eq!(stable, digest_toolchain_source_tree(&second)?);

        fs::write(second.join("README.md"), "changed docs only\n")?;
        fs::write(second.join("tests/not_compiled.rs"), "changed tests only\n")?;
        fs::write(second.join(".DS_Store"), "changed editor state\n")?;
        assert_eq!(stable, digest_toolchain_source_tree(&second)?);

        fs::write(second.join("src/embedded.txt"), "changed compiled include input\n")?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        fs::write(second.join("src/embedded.txt"), "compiled include input\n")?;

        fs::write(second.join("semantic-inputs/schema.json"), "{\"version\": 2}\n")?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        fs::write(second.join("semantic-inputs/schema.json"), "{\"version\": 1}\n")?;

        fs::write(second.join("src/lib.rs"), "pub fn support() { changed(); }\n")?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        fs::write(second.join("src/lib.rs"), "pub fn support() {}\n")?;

        fs::write(
            second.join("Cargo.toml"),
            "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.1\"\n\n[package.metadata.incan]\nsemantic-inputs = [\"semantic-inputs/schema.json\"]\n\n[dependencies]\nincan_core = { path = \"../incan_core\" }\n",
        )?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        fs::write(
            second.join("Cargo.toml"),
            "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.0\"\n\n[package.metadata.incan]\nsemantic-inputs = [\"semantic-inputs/schema.json\"]\n\n[dependencies]\nincan_core = { path = \"../incan_core\" }\n",
        )?;

        fs::write(
            second
                .parent()
                .ok_or("support root has no parent")?
                .join("incan_core/src/lib.rs"),
            "pub fn core() { changed(); }\n",
        )?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn toolchain_semantic_inputs_fail_closed_for_invalid_entries_issue921() -> TestResult {
        let temp = tempfile::Builder::new()
            // Keep the Unix-domain socket fixture beneath macOS's short sockaddr_un path limit. `/tmp` is also
            // present on Linux CI; spelling `/private/tmp` here consumes most of the portable path budget.
            .prefix("incan-si-")
            .tempdir_in("/tmp")?;
        let package = temp.path().join("support");
        fs::create_dir_all(package.join("src"))?;
        fs::write(package.join("src/lib.rs"), "pub fn support() {}\n")?;
        fs::write(package.join("present.txt"), "present\n")?;
        fs::write(temp.path().join("outside.txt"), "outside\n")?;
        std::os::unix::fs::symlink(temp.path().join("outside.txt"), package.join("linked.txt"))?;
        let special = package.join("special-entry");
        // A FIFO exercises the same unsupported filesystem-entry branch without opening a Unix-domain socket.
        // Some otherwise valid macOS execution sandboxes refuse socket creation, which made this filesystem test
        // depend on an unrelated process-network capability.
        let status = std::process::Command::new("mkfifo").arg(&special).status()?;
        assert!(status.success(), "mkfifo failed with {status}");

        let invalid_values = [
            ("\"present.txt\"", "must be an array of paths"),
            ("[1]", "entries must be strings"),
            ("[\"\"]", "must be a non-empty relative path"),
            ("[\"/tmp/outside.txt\"]", "must be a non-empty relative path"),
            ("[\"../outside.txt\"]", "must be a non-empty relative path"),
            ("[\"missing.txt\"]", "failed to inspect provider artifact path"),
            ("[\"linked.txt\"]", "is not a regular file or directory"),
            ("[\"special-entry\"]", "is not a regular file or directory"),
        ];
        for (manifest, expected) in [
            (
                "[package]\nname = \"support\"\nversion = \"0.5.0\"\nmetadata = \"invalid\"\n",
                "package.metadata must be a table",
            ),
            (
                "[package]\nname = \"support\"\nversion = \"0.5.0\"\n\n[package.metadata]\nincan = \"invalid\"\n",
                "package.metadata.incan must be a table",
            ),
        ] {
            fs::write(package.join("Cargo.toml"), manifest)?;
            let error =
                digest_toolchain_source_tree(&package).expect_err("wrong package metadata value type must fail closed");
            assert!(
                matches!(&error, ProviderArtifactDigestError::Normalization { .. }),
                "wrong metadata type should be a normalization error: {error}"
            );
            assert!(error.to_string().contains(expected), "unexpected error: {error}");
        }
        for (value, expected) in invalid_values {
            fs::write(
                package.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"support\"\nversion = \"0.5.0\"\n\n[package.metadata.incan]\nsemantic-inputs = {value}\n"
                ),
            )?;
            let error =
                digest_toolchain_source_tree(&package).expect_err("invalid semantic-inputs entry must fail closed");
            assert!(
                error.to_string().contains(expected),
                "semantic-inputs value `{value}` produced unexpected error: {error}"
            );
        }
        Ok(())
    }

    #[test]
    fn toolchain_source_digest_tracks_target_specific_workspace_dependencies_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_workspace = temp.path().join("source-a");
        let second_workspace = temp.path().join("source-b");
        for workspace in [&first_workspace, &second_workspace] {
            let support = workspace.join("crates/support");
            let helper = workspace.join("crates/target_helper");
            fs::create_dir_all(support.join("src"))?;
            fs::create_dir_all(helper.join("src"))?;
            fs::write(
                workspace.join("Cargo.toml"),
                "[workspace]\nmembers = [\"crates/support\", \"crates/target_helper\"]\n\n[workspace.dependencies]\ntarget_helper = { path = \"crates/target_helper\", version = \"0.1.0\" }\n",
            )?;
            fs::write(
                support.join("Cargo.toml"),
                "[package]\nname = \"support\"\nversion = \"0.5.0\"\n\n[target.'cfg(unix)'.dependencies]\ntarget_helper = { workspace = true }\n",
            )?;
            fs::write(support.join("src/lib.rs"), "pub fn support() {}\n")?;
            fs::write(
                helper.join("Cargo.toml"),
                "[package]\nname = \"target_helper\"\nversion = \"0.1.0\"\n",
            )?;
            fs::write(helper.join("src/lib.rs"), "pub fn helper() {}\n")?;
        }

        let first = first_workspace.join("crates/support");
        let second = second_workspace.join("crates/support");
        let stable = digest_toolchain_source_tree(&first)?;
        assert_eq!(stable, digest_toolchain_source_tree(&second)?);

        fs::write(
            second_workspace.join("crates/target_helper/src/lib.rs"),
            "pub fn helper() { changed(); }\n",
        )?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        Ok(())
    }

    #[test]
    fn explicit_package_workspace_precedes_nearest_ancestor_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let package = root.join("packages/support");
        let explicit = root.join("selected-workspace");
        let selected_helper = explicit.join("helper");
        let ancestor_helper = root.join("ancestor-helper");
        for path in [&package, &selected_helper, &ancestor_helper] {
            fs::create_dir_all(path.join("src"))?;
        }
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\n\n[workspace.dependencies]\ntarget_helper = { path = \"ancestor-helper\", version = \"9.0.0\" }\n",
        )?;
        fs::write(
            explicit.join("Cargo.toml"),
            "[workspace]\nmembers = [\"helper\"]\n\n[workspace.dependencies]\ntarget_helper = { path = \"helper\", version = \"1.0.0\" }\n",
        )?;
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = \"support\"\nversion = \"0.5.0\"\nworkspace = \"../../selected-workspace\"\n\n[target.'cfg(unix)'.dependencies]\ntarget_helper = { workspace = true }\n",
        )?;
        fs::write(package.join("src/lib.rs"), "pub fn support() {}\n")?;
        for (helper, version) in [(&selected_helper, "1.0.0"), (&ancestor_helper, "9.0.0")] {
            fs::write(
                helper.join("Cargo.toml"),
                format!("[package]\nname = \"target_helper\"\nversion = \"{version}\"\n"),
            )?;
            fs::write(helper.join("src/lib.rs"), "pub fn helper() {}\n")?;
        }

        let stable = digest_toolchain_source_tree(&package)?;
        fs::write(ancestor_helper.join("src/lib.rs"), "pub fn helper() { ignored(); }\n")?;
        assert_eq!(stable, digest_toolchain_source_tree(&package)?);
        fs::write(selected_helper.join("src/lib.rs"), "pub fn helper() { changed(); }\n")?;
        assert_ne!(stable, digest_toolchain_source_tree(&package)?);
        Ok(())
    }

    #[test]
    fn semantic_digest_normalizes_only_checked_provider_delivery_coordinates_issue921() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let first_path = "../../provider-home-a/stdlib-core";
        let second_path = "../../provider-home-b/stdlib-core";
        let first_toolchain_path = "../../source-a/crates/incan_stdlib";
        let second_toolchain_path = "../../source-b/crates/incan_stdlib";
        let make_artifact = |root: &Path,
                             provider_path: &str,
                             toolchain_path: &str,
                             unrelated_path: &str|
         -> TestResult {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), "pub fn value() -> i32 { 1 }")?;
            fs::write(root.join("root_lib.incnlib"), format!("delivery={provider_path}"))?;
            fs::write(
                root.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"root_lib\"\nversion = \"0.1.0\"\n\n[dependencies.incan_stdlib_core]\npath = \"{provider_path}\"\n\n[dependencies.incan_stdlib]\npath = \"{toolchain_path}\"\n\n[dependencies.user_path]\npath = \"{unrelated_path}\"\n"
                ),
            )?;
            Ok(())
        };
        make_artifact(first.path(), first_path, first_toolchain_path, "../user-dependency")?;
        make_artifact(second.path(), second_path, second_toolchain_path, "../user-dependency")?;

        let manifest_with_path =
            |provider_path: &str, provider_digest: &str| {
                let mut manifest = LibraryManifest::new("root_lib", "0.1.0");
                manifest
                    .contract_metadata
                    .provider
                    .provider_dependencies
                    .push(ProviderDependencyMetadata {
                        kind: super::super::ProviderDependencyKind::PrivateImplementation,
                        dependency_key: "incan_stdlib_core".to_string(),
                        provider_name: "incan_stdlib_core".to_string(),
                        provider_version: "0.5.0".to_string(),
                        artifact_digest: provider_digest.to_string(),
                        relative_artifact_path: provider_path.to_string(),
                        requested_features: BTreeSet::new(),
                        default_features: false,
                        optional: false,
                    });
                manifest.contract_metadata.provider.implementation_facets.push(
                    super::super::ProviderImplementationFacet {
                        id: "stdlib-runtime".to_string(),
                        required_modules: BTreeSet::new(),
                        required_features: BTreeSet::new(),
                        cargo_features: BTreeMap::new(),
                        cargo_dependencies: vec![ProviderCargoDependency {
                            crate_name: "incan_stdlib".to_string(),
                            package: None,
                            version: None,
                            features: BTreeSet::new(),
                            default_features: false,
                            source: ProviderCargoDependencySource::Toolchain {
                                relative_path: "crates/incan_stdlib".to_string(),
                            },
                        }],
                    },
                );
                manifest
            };
        let first_manifest = manifest_with_path(first_path, "sha256:provider");
        let second_manifest = manifest_with_path(second_path, "sha256:provider");
        let semantic_digest = |root: &Path, manifest: &LibraryManifest| {
            digest_provider_semantic_artifact(root, &root.join("root_lib.incnlib"), &root.join("Cargo.toml"), manifest)
        };

        assert_ne!(
            digest_provider_artifact(first.path())?,
            digest_provider_artifact(second.path())?
        );
        assert_eq!(
            semantic_digest(first.path(), &first_manifest)?,
            semantic_digest(second.path(), &second_manifest)?
        );

        let changed_provider = manifest_with_path(second_path, "sha256:changed-provider");
        assert_ne!(
            semantic_digest(second.path(), &second_manifest)?,
            semantic_digest(second.path(), &changed_provider)?
        );

        let mut changed_toolchain = second_manifest.clone();
        changed_toolchain.contract_metadata.provider.implementation_facets[0].cargo_dependencies[0].source =
            ProviderCargoDependencySource::Toolchain {
                relative_path: "crates/changed_stdlib".to_string(),
            };
        assert_ne!(
            semantic_digest(second.path(), &second_manifest)?,
            semantic_digest(second.path(), &changed_toolchain)?
        );

        fs::write(
            second.path().join("Cargo.toml"),
            format!(
                "[package]\nname = \"root_lib\"\nversion = \"0.1.0\"\n\n[dependencies.incan_stdlib_core]\npath = \"{second_path}\"\n\n[dependencies.incan_stdlib]\npath = \"{second_toolchain_path}\"\n\n[dependencies.user_path]\npath = \"../different-user-dependency\"\n"
            ),
        )?;
        assert_ne!(
            semantic_digest(first.path(), &first_manifest)?,
            semantic_digest(second.path(), &second_manifest)?
        );
        Ok(())
    }

    #[test]
    fn selected_stdlib_web_uses_exact_macro_content_identity_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_provider = temp.path().join("source-a/provider/stdlib-web");
        let second_provider = temp.path().join("source-b/provider/stdlib-web");
        let first_macros = temp.path().join("source-a/crates/incan_web_macros");
        let second_macros = temp.path().join("source-b/crates/incan_web_macros");
        for macros in [&first_macros, &second_macros] {
            fs::create_dir_all(macros.join("src"))?;
            fs::write(
                macros.join("Cargo.toml"),
                "[package]\nname = \"incan_web_macros\"\nversion = \"0.5.0\"\n",
            )?;
            fs::write(macros.join("src/lib.rs"), "pub fn route() {}\n")?;
        }
        let make_provider = |root: &Path, macros: &Path| -> TestResult {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), "pub fn web() {}\n")?;
            fs::write(
                root.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"incan_stdlib_web\"\nversion = \"0.5.0\"\n\n[dependencies.incan_web_macros]\npath = {:?}\n",
                    macros.to_string_lossy()
                ),
            )?;
            let mut manifest = LibraryManifest::new("incan_stdlib_web", "0.5.0");
            manifest
                .contract_metadata
                .provider
                .implementation_facets
                .push(super::super::ProviderImplementationFacet {
                    id: "web-macros".to_string(),
                    required_modules: BTreeSet::new(),
                    required_features: BTreeSet::new(),
                    cargo_features: BTreeMap::new(),
                    cargo_dependencies: vec![ProviderCargoDependency {
                        crate_name: "incan_web_macros".to_string(),
                        package: None,
                        version: None,
                        features: BTreeSet::new(),
                        default_features: false,
                        source: ProviderCargoDependencySource::Toolchain {
                            relative_path: "crates/incan_web_macros".to_string(),
                        },
                    }],
                });
            manifest.write_to_path(&root.join("incan_stdlib_web.incnlib"))?;
            Ok(())
        };
        make_provider(&first_provider, &first_macros)?;
        make_provider(&second_provider, &second_macros)?;
        let first_manifest = LibraryManifest::read_from_path(&first_provider.join("incan_stdlib_web.incnlib"))?;
        let second_manifest = LibraryManifest::read_from_path(&second_provider.join("incan_stdlib_web.incnlib"))?;
        let dependency = |root: &Path| -> Result<ProviderSemanticToolchainDependency, ProviderArtifactDigestError> {
            Ok(ProviderSemanticToolchainDependency {
                crate_name: "incan_web_macros".to_string(),
                package_name: "incan_web_macros".to_string(),
                artifact_root: root.to_path_buf(),
                content_digest: digest_toolchain_source_tree(root)?,
            })
        };
        let semantic = |root: &Path, manifest: &LibraryManifest, toolchain: ProviderSemanticToolchainDependency| {
            digest_provider_semantic_artifact_with_context(
                root,
                &root.join("incan_stdlib_web.incnlib"),
                &root.join("Cargo.toml"),
                manifest,
                &BTreeMap::new(),
                &[toolchain],
            )
        };

        assert_ne!(
            digest_provider_artifact(&first_provider)?,
            digest_provider_artifact(&second_provider)?
        );
        let stable = semantic(&first_provider, &first_manifest, dependency(&first_macros)?)?;
        assert_eq!(
            stable,
            semantic(&second_provider, &second_manifest, dependency(&second_macros)?)?
        );

        fs::write(second_macros.join("src/lib.rs"), "pub fn route() { changed(); }\n")?;
        assert_ne!(
            stable,
            semantic(&second_provider, &second_manifest, dependency(&second_macros)?)?
        );
        Ok(())
    }

    #[test]
    fn semantic_digest_recurses_through_transitive_provider_content_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let build_graph = |graph_root: &Path,
                           source_root: &str|
         -> Result<LibraryManifest, Box<dyn std::error::Error>> {
            let leaf_root = graph_root.join("leaf");
            fs::create_dir_all(leaf_root.join("src"))?;
            fs::write(leaf_root.join("src/lib.rs"), "pub fn leaf() -> u8 { 1 }\n")?;
            fs::write(
                leaf_root.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\n\n[dependencies.incan_stdlib]\npath = \"{source_root}/crates/incan_stdlib\"\n"
                ),
            )?;
            let mut leaf_manifest = LibraryManifest::new("leaf", "0.1.0");
            leaf_manifest.contract_metadata.provider.implementation_facets.push(
                super::super::ProviderImplementationFacet {
                    id: "runtime".to_string(),
                    required_modules: BTreeSet::new(),
                    required_features: BTreeSet::new(),
                    cargo_features: BTreeMap::new(),
                    cargo_dependencies: vec![ProviderCargoDependency {
                        crate_name: "incan_stdlib".to_string(),
                        package: None,
                        version: None,
                        features: BTreeSet::new(),
                        default_features: false,
                        source: ProviderCargoDependencySource::Toolchain {
                            relative_path: "crates/incan_stdlib".to_string(),
                        },
                    }],
                },
            );
            leaf_manifest.write_to_path(&leaf_root.join("leaf.incnlib"))?;

            let child_root = graph_root.join("child");
            fs::create_dir_all(child_root.join("src"))?;
            fs::write(child_root.join("src/lib.rs"), "pub fn child() -> u8 { leaf::leaf() }\n")?;
            fs::write(
                child_root.join("Cargo.toml"),
                "[package]\nname = \"child\"\nversion = \"0.1.0\"\n\n[dependencies.leaf]\npath = \"../leaf\"\n",
            )?;
            let mut child_manifest = LibraryManifest::new("child", "0.1.0");
            child_manifest
                .contract_metadata
                .provider
                .provider_dependencies
                .push(ProviderDependencyMetadata {
                    kind: super::super::ProviderDependencyKind::PublicPackage,
                    dependency_key: "leaf".to_string(),
                    provider_name: "leaf".to_string(),
                    provider_version: "0.1.0".to_string(),
                    artifact_digest: digest_provider_artifact(&leaf_root)?,
                    relative_artifact_path: "../leaf".to_string(),
                    requested_features: BTreeSet::new(),
                    default_features: false,
                    optional: false,
                });
            child_manifest.write_to_path(&child_root.join("child.incnlib"))?;

            let root = graph_root.join("root");
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/lib.rs"), "pub fn root() -> u8 { child::child() }\n")?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"root\"\nversion = \"0.1.0\"\n\n[dependencies.child]\npath = \"../child\"\n",
            )?;
            let mut root_manifest = LibraryManifest::new("root", "0.1.0");
            root_manifest
                .contract_metadata
                .provider
                .provider_dependencies
                .push(ProviderDependencyMetadata {
                    kind: super::super::ProviderDependencyKind::PublicPackage,
                    dependency_key: "child".to_string(),
                    provider_name: "child".to_string(),
                    provider_version: "0.1.0".to_string(),
                    artifact_digest: digest_provider_artifact(&child_root)?,
                    relative_artifact_path: "../child".to_string(),
                    requested_features: BTreeSet::new(),
                    default_features: false,
                    optional: false,
                });
            root_manifest.write_to_path(&root.join("root.incnlib"))?;
            Ok(root_manifest)
        };

        let first_graph = temp.path().join("source-a/provider-graph");
        let second_graph = temp.path().join("source-b/provider-graph");
        let first_manifest = build_graph(&first_graph, "/source-a")?;
        let second_manifest = build_graph(&second_graph, "/source-b")?;
        let first_root = first_graph.join("root");
        let second_root = second_graph.join("root");
        let semantic = |root: &Path, manifest: &LibraryManifest| {
            digest_provider_semantic_artifact(root, &root.join("root.incnlib"), &root.join("Cargo.toml"), manifest)
        };

        assert_ne!(
            digest_provider_artifact(&first_root)?,
            digest_provider_artifact(&second_root)?
        );
        assert_eq!(
            semantic(&first_root, &first_manifest)?,
            semantic(&second_root, &second_manifest)?
        );

        fs::write(second_graph.join("leaf/src/lib.rs"), "pub fn leaf() -> u8 { 2 }\n")?;
        assert_ne!(
            semantic(&first_root, &first_manifest)?,
            semantic(&second_root, &second_manifest)?
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn digest_rejects_symlinks() -> TestResult {
        use std::os::unix::fs::symlink;

        let artifact = tempfile::tempdir()?;
        fs::write(artifact.path().join("outside"), "content")?;
        symlink("outside", artifact.path().join("linked"))?;

        assert!(matches!(
            digest_provider_artifact(artifact.path()),
            Err(ProviderArtifactDigestError::UnsupportedEntry { .. })
        ));
        fs::write(
            artifact.path().join("Cargo.toml"),
            "[package]\nname = \"support\"\nversion = \"0.1.0\"\n",
        )?;
        fs::create_dir_all(artifact.path().join("src"))?;
        symlink("../outside", artifact.path().join("src/linked.rs"))?;
        assert!(matches!(
            digest_toolchain_source_tree(artifact.path()),
            Err(ProviderArtifactDigestError::UnsupportedEntry { .. })
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn digest_ignores_legacy_compiler_owned_rust_inspect_output() -> TestResult {
        use std::os::unix::fs::symlink;

        let artifact = tempfile::tempdir()?;
        fs::write(artifact.path().join("provider.incnlib"), "manifest")?;
        fs::create_dir_all(artifact.path().join("oven/debug"))?;
        fs::write(artifact.path().join("oven/debug/libprovider.rlib"), "provider")?;
        let initial = digest_provider_artifact(artifact.path())?;

        let inspection_output = artifact.path().join("oven/rust-inspect/debug/build/tool/out/bin");
        fs::create_dir_all(&inspection_output)?;
        fs::write(inspection_output.join("tool-1"), "compiler-owned output")?;
        symlink("tool-1", inspection_output.join("tool"))?;

        assert_eq!(initial, digest_provider_artifact(artifact.path())?);
        Ok(())
    }
}
