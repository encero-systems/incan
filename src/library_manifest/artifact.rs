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
    /// A checked delivery coordinate could not be normalized into the semantic identity projection.
    #[error("failed to normalize provider artifact path {path}: {message}")]
    Normalization { path: PathBuf, message: String },
}

/// Hash every immutable manifest, generated source, and generated-project input in one provider artifact tree.
///
/// A nested Cargo `target/` directory is deliberately excluded because it is a mutable build cache rather than
/// provider content. Generated providers normally use an external shared target directory, but this exclusion keeps
/// integrity stable if a backend tool creates the conventional directory later.
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

/// Hash the Cargo-semantic closure of one compiler-owned support crate.
///
/// Only package inputs that can affect compiled code are included: the normalized package manifest, inherited
/// workspace package/dependency values, Rust sources, a build script, and recursive normal/build/target path
/// dependencies. Repository noise such as README files, tests, editor files, and target caches is intentionally absent.
#[cfg(test)]
fn digest_toolchain_source_tree(root: &Path) -> Result<String, ProviderArtifactDigestError> {
    digest_toolchain_source_tree_with_cache(root, &mut BTreeMap::new())
}

/// Hash a support package while sharing path-dependency results across one lock snapshot.
pub(crate) fn digest_toolchain_source_tree_with_cache(
    root: &Path,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
) -> Result<String, ProviderArtifactDigestError> {
    digest_toolchain_package_inner(root, &mut BTreeSet::new(), resolved_packages)
}

/// Resolve one support package plus its recursive Cargo path dependencies into a path-independent digest.
fn digest_toolchain_package_inner(
    root: &Path,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
) -> Result<String, ProviderArtifactDigestError> {
    if !root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: root.to_path_buf(),
        });
    }
    let normalized_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if let Some(digest) = resolved_packages.get(&normalized_root) {
        return Ok(digest.clone());
    }
    if !visiting.insert(normalized_root.clone()) {
        return Err(ProviderArtifactDigestError::Normalization {
            path: root.to_path_buf(),
            message: "toolchain Cargo path-dependency graph contains a cycle".to_string(),
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
    normalize_toolchain_manifest_path_dependencies(&mut manifest, root, visiting, resolved_packages)?;
    let workspace_context = inherited_workspace_context(root, &manifest, visiting, resolved_packages)?;
    let normalized_manifest =
        toml::to_string(&manifest).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;

    let mut hasher = Sha256::new();
    hasher.update(b"incan-toolchain-cargo-package-v1\0");
    hash_named_bytes(&mut hasher, "Cargo.toml", normalized_manifest.as_bytes());
    if let Some(context) = workspace_context {
        let context = toml::to_string(&context).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;
        hash_named_bytes(&mut hasher, "workspace-inherited.toml", context.as_bytes());
    }
    hash_rust_source_inputs(root, &root.join("src"), &mut hasher)?;
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
    visiting.remove(&normalized_root);
    let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
    resolved_packages.insert(normalized_root, digest.clone());
    Ok(digest)
}

/// Replace path dependencies in one Cargo manifest with the semantic digests of their target packages.
fn normalize_toolchain_manifest_path_dependencies(
    manifest: &mut toml::Value,
    base: &Path,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
) -> Result<(), ProviderArtifactDigestError> {
    let Some(root) = manifest.as_table_mut() else {
        return Ok(());
    };
    for section in ["dependencies", "build-dependencies"] {
        if let Some(dependencies) = root.get_mut(section) {
            normalize_toolchain_dependency_table(dependencies, base, visiting, resolved_packages)?;
        }
    }
    if let Some(targets) = root.get_mut("target").and_then(toml::Value::as_table_mut) {
        for (_, target_value) in targets.iter_mut() {
            let Some(target) = target_value.as_table_mut() else {
                continue;
            };
            for section in ["dependencies", "build-dependencies"] {
                if let Some(dependencies) = target.get_mut(section) {
                    normalize_toolchain_dependency_table(dependencies, base, visiting, resolved_packages)?;
                }
            }
        }
    }
    Ok(())
}

/// Normalize every direct path dependency in one Cargo dependency table.
fn normalize_toolchain_dependency_table(
    dependencies: &mut toml::Value,
    base: &Path,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
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
        let digest = digest_toolchain_package_inner(&dependency_root, visiting, resolved_packages)?;
        let package = dependency
            .get("package")
            .and_then(toml::Value::as_str)
            .unwrap_or(dependency_key);
        dependency.insert(
            "path".to_string(),
            toml::Value::String(format!("incan-toolchain-package://{package}#{digest}")),
        );
    }
    Ok(())
}

/// Materialize only the workspace package and dependency values inherited by one support package.
fn inherited_workspace_context(
    package_root: &Path,
    package_manifest: &toml::Value,
    visiting: &mut BTreeSet<PathBuf>,
    resolved_packages: &mut BTreeMap<PathBuf, String>,
) -> Result<Option<toml::Value>, ProviderArtifactDigestError> {
    let Some((workspace_root, workspace_manifest)) = find_workspace_manifest(package_root)? else {
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
            normalize_toolchain_dependency_table(&mut dependencies, &workspace_root, visiting, resolved_packages)?;
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
    }
}

/// Locate and parse the nearest containing Cargo workspace manifest.
fn find_workspace_manifest(root: &Path) -> Result<Option<(PathBuf, toml::Value)>, ProviderArtifactDigestError> {
    for ancestor in root.ancestors().skip(1) {
        let path = ancestor.join("Cargo.toml");
        if !path.is_file() {
            continue;
        }
        let bytes = fs::read(&path).map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        let value =
            toml::from_slice::<toml::Value>(&bytes).map_err(|error| ProviderArtifactDigestError::Normalization {
                path: path.clone(),
                message: error.to_string(),
            })?;
        if value.get("workspace").is_some() {
            return Ok(Some((ancestor.to_path_buf(), value)));
        }
    }
    Ok(None)
}

/// Hash Rust source files below `directory` while ignoring non-compiled repository noise.
fn hash_rust_source_inputs(
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
            hash_rust_source_inputs(package_root, &path, hasher)?;
        } else if file_type.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            hash_compiled_file(package_root, &path, hasher)?;
        } else if file_type.is_symlink() && path.extension().is_some_and(|extension| extension == "rs") {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
    }
    Ok(())
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

/// Hash one generated provider artifact while excluding checked provider delivery coordinates from its lock identity.
///
/// The ordinary artifact digest remains the byte-exact integrity contract frozen into compiled provider edges. This
/// projection is narrower: it is used only for semantic lock identity, where an equivalent provider graph rebuilt
/// beneath another compiler-owned cache root must remain the same provider. Only paths already paired with an exact
/// provider name, version, artifact digest, and feature projection in the checked `.incnlib` metadata are normalized.
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

/// Recursive implementation that resolves present transitive artifacts while bounding malformed cycles.
fn digest_provider_semantic_artifact_inner(
    root: &Path,
    manifest_path: &Path,
    cargo_toml_path: &Path,
    manifest: &LibraryManifest,
    context: &mut ProviderSemanticDigestContext<'_>,
) -> Result<String, ProviderArtifactDigestError> {
    if !root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: root.to_path_buf(),
        });
    }
    let normalized_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    if let Some(digest) = context.resolved_artifacts.get(&normalized_root) {
        return Ok(digest.clone());
    }
    if !context.visiting.insert(normalized_root.clone()) {
        return Err(ProviderArtifactDigestError::Normalization {
            path: root.to_path_buf(),
            message: "compiled provider dependency graph contains a cycle".to_string(),
        });
    }

    let mut normalized_manifest = manifest.clone();
    let mut delivery_coordinates = BTreeMap::<String, BTreeSet<String>>::new();
    for dependency in &mut normalized_manifest.contract_metadata.provider.provider_dependencies {
        let physical_digest = dependency.artifact_digest.clone();
        let dependency_root = root.join(&dependency.relative_artifact_path);
        let semantic_digest = if let Some(digest) = context.dependency_semantic_digests.get(&physical_digest) {
            Some(digest.clone())
        } else if dependency_root.is_dir() {
            let dependency_manifest_path = dependency_root.join(format!("{}.incnlib", dependency.provider_name));
            let dependency_cargo_toml_path = dependency_root.join("Cargo.toml");
            let dependency_manifest = LibraryManifest::read_from_path(&dependency_manifest_path).map_err(|error| {
                ProviderArtifactDigestError::Normalization {
                    path: dependency_manifest_path.clone(),
                    message: error.to_string(),
                }
            })?;
            Some(digest_provider_semantic_artifact_inner(
                &dependency_root,
                &dependency_manifest_path,
                &dependency_cargo_toml_path,
                &dependency_manifest,
                context,
            )?)
        } else {
            None
        };
        if let Some(semantic_digest) = semantic_digest {
            dependency.artifact_digest = semantic_digest;
        }
        let coordinate = provider_dependency_semantic_coordinate(dependency);
        delivery_coordinates
            .entry(dependency.relative_artifact_path.clone())
            .or_default()
            .insert(coordinate.clone());
        dependency.relative_artifact_path = coordinate;
    }
    let mut toolchain_dependencies = normalized_manifest
        .contract_metadata
        .provider
        .implementation_facets
        .iter()
        .flat_map(|facet| facet.cargo_dependencies.iter())
        .filter(|dependency| matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }))
        .map(|dependency| {
            (
                dependency.crate_name.clone(),
                toolchain_dependency_coordinate(dependency),
            )
        })
        .fold(
            BTreeMap::<String, BTreeSet<ToolchainDependencyCoordinate>>::new(),
            |mut dependencies, (crate_name, coordinate)| {
                dependencies.entry(crate_name).or_default().insert(coordinate);
                dependencies
            },
        );
    for dependency in context.sdk_toolchain_dependencies {
        let candidates = toolchain_dependencies.entry(dependency.crate_name.clone()).or_default();
        // Exact catalog provenance supersedes the manifest-relative fallback for the same Cargo package. Retain
        // multiple typed roots so aliases remain safe: the generated Cargo path must still match exactly one of them.
        candidates.retain(|candidate| {
            candidate.package_name != dependency.package_name || candidate.expected_artifact_root.is_some()
        });
        candidates.insert(ToolchainDependencyCoordinate {
            package_name: dependency.package_name.clone(),
            semantic_coordinate: format!(
                "incan-sdk-toolchain://{}?package={}#{}",
                dependency.crate_name, dependency.package_name, dependency.content_digest
            ),
            expected_artifact_root: Some(dependency.artifact_root.clone()),
        });
    }
    let normalized_manifest_bytes = serde_json::to_vec(&RawLibraryManifest::from_semantic(&normalized_manifest))
        .map_err(|error| ProviderArtifactDigestError::Normalization {
            path: manifest_path.to_path_buf(),
            message: error.to_string(),
        })?;
    let cargo_bytes = fs::read(cargo_toml_path).map_err(|source| ProviderArtifactDigestError::Io {
        path: cargo_toml_path.to_path_buf(),
        source,
    })?;
    let normalized_cargo_bytes = normalize_cargo_delivery_coordinates(
        cargo_toml_path,
        &cargo_bytes,
        &delivery_coordinates,
        &toolchain_dependencies,
    )?;

    let normalization = SemanticArtifactNormalization {
        manifest_path,
        cargo_toml_path,
        normalized_manifest_bytes: &normalized_manifest_bytes,
        normalized_cargo_bytes: &normalized_cargo_bytes,
    };
    let mut hasher = Sha256::new();
    hasher.update(b"incan-provider-semantic-artifact-v1\0");
    hash_directory_with_normalization(root, root, &mut hasher, Some(&normalization), false)?;
    context.visiting.remove(&normalized_root);
    let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
    context.resolved_artifacts.insert(normalized_root, digest.clone());
    Ok(digest)
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

/// Replace only Cargo `path` values that exactly match checked provider dependency coordinates.
fn normalize_cargo_delivery_coordinates(
    cargo_toml_path: &Path,
    bytes: &[u8],
    delivery_coordinates: &BTreeMap<String, BTreeSet<String>>,
    toolchain_dependencies: &BTreeMap<String, BTreeSet<ToolchainDependencyCoordinate>>,
) -> Result<Vec<u8>, ProviderArtifactDigestError> {
    let content = std::str::from_utf8(bytes).map_err(|error| ProviderArtifactDigestError::Normalization {
        path: cargo_toml_path.to_path_buf(),
        message: error.to_string(),
    })?;
    let mut cargo: toml::Value =
        toml::from_str(content).map_err(|error| ProviderArtifactDigestError::Normalization {
            path: cargo_toml_path.to_path_buf(),
            message: error.to_string(),
        })?;
    normalize_toml_paths(&mut cargo, delivery_coordinates);
    normalize_toolchain_dependency_paths(&mut cargo, cargo_toml_path, toolchain_dependencies);
    toml::to_string(&cargo)
        .map(String::into_bytes)
        .map_err(|error| ProviderArtifactDigestError::Normalization {
            path: cargo_toml_path.to_path_buf(),
            message: error.to_string(),
        })
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

/// Walk generated Cargo metadata and normalize exact provider-owned path values, including patch tables.
fn normalize_toml_paths(value: &mut toml::Value, delivery_coordinates: &BTreeMap<String, BTreeSet<String>>) {
    match value {
        toml::Value::Table(table) => {
            if let Some(toml::Value::String(path)) = table.get_mut("path")
                && let Some(coordinates) = delivery_coordinates.get(path)
            {
                *path = coordinates.iter().cloned().collect::<Vec<_>>().join("+");
            }
            for (_, nested) in table.iter_mut() {
                normalize_toml_paths(nested, delivery_coordinates);
            }
        }
        toml::Value::Array(values) => {
            for nested in values {
                normalize_toml_paths(nested, delivery_coordinates);
            }
        }
        _ => {}
    }
}

/// Precomputed path-specific content substitutions for the semantic artifact digest.
struct SemanticArtifactNormalization<'a> {
    manifest_path: &'a Path,
    cargo_toml_path: &'a Path,
    normalized_manifest_bytes: &'a [u8],
    normalized_cargo_bytes: &'a [u8],
}

/// Feed one artifact directory into the stable digest in lexical path order while excluding its mutable target tree.
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
        // The generated provider-root Cargo.lock is a projection of the canonical Incan lock, not an independent
        // provider input. Including it here creates a two-pass identity cycle: artifact-only preparation has no
        // Cargo.lock, while the first locked build materializes one from incan.lock and would otherwise change the
        // provider's semantic identity. Nested Cargo.lock files remain part of the artifact content projection.
        if normalization.is_some() && relative == Path::new("Cargo.lock") {
            continue;
        }
        let is_root_target = relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == "target");
        let is_nested_target = path.file_name().is_some_and(|name| name == "target");
        if is_root_target || (exclude_nested_targets && is_nested_target) {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
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
                } else if path == normalization.cargo_toml_path {
                    normalization.normalized_cargo_bytes.to_vec()
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
    fn digest_tracks_manifest_and_generated_source_but_ignores_build_cache() -> TestResult {
        let artifact = tempfile::tempdir()?;
        fs::create_dir_all(artifact.path().join("src"))?;
        fs::write(artifact.path().join("provider.incnlib"), "manifest")?;
        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }")?;
        let initial = digest_provider_artifact(artifact.path())?;

        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 2 }")?;
        let source_changed = digest_provider_artifact(artifact.path())?;
        assert_ne!(initial, source_changed);

        fs::create_dir_all(artifact.path().join("target/debug"))?;
        fs::write(artifact.path().join("target/debug/cache"), "mutable")?;
        assert_eq!(source_changed, digest_provider_artifact(artifact.path())?);
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
    fn toolchain_source_digest_ignores_nested_targets_and_tracks_source_inputs_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first = temp.path().join("source-a/crates/incan_stdlib");
        let second = temp.path().join("source-b/crates/incan_stdlib");
        for root in [&first, &second] {
            fs::create_dir_all(root.join("src"))?;
            fs::create_dir_all(root.join("tests"))?;
            fs::create_dir_all(root.join("stdlib/components/data/target/debug"))?;
            fs::create_dir_all(
                root.parent()
                    .ok_or("support root has no parent")?
                    .join("incan_core/src"),
            )?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.0\"\n\n[dependencies]\nincan_core = { path = \"../incan_core\" }\n",
            )?;
            fs::write(root.join("src/lib.rs"), "pub fn support() {}\n")?;
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

        fs::write(second.join("src/lib.rs"), "pub fn support() { changed(); }\n")?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        fs::write(second.join("src/lib.rs"), "pub fn support() {}\n")?;

        fs::write(
            second.join("Cargo.toml"),
            "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.1\"\n\n[dependencies]\nincan_core = { path = \"../incan_core\" }\n",
        )?;
        assert_ne!(stable, digest_toolchain_source_tree(&second)?);
        fs::write(
            second.join("Cargo.toml"),
            "[package]\nname = \"incan_stdlib\"\nversion = \"0.5.0\"\n\n[dependencies]\nincan_core = { path = \"../incan_core\" }\n",
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
}
