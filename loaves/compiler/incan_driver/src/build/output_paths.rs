//! Where a project's outputs go: entrypoints, artifact paths, bake files, inspection roots and generated out-dirs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::backend::ProjectGenerator;
use crate::build::output_materialization::digest_project_output_projection_file;
use crate::build::{
    BakeGeneratedOutDir, LibraryInspectionConstituent, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH,
    OvenPackagedLibraryLoafManifest, OvenPackagedLibraryMetadataFile, OvenPreparedProject, OvenProjectOutputBakeFile,
};
use crate::cargo_policy::enforce_project_toolchain_constraint;
use crate::error::{CliError, CliResult};
use crate::project::{discover_effective_project_manifest, resolve_project_root};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest_index::LibraryArtifactMetadata;
use oven_cargo_compat::OvenProjectRegistrySourceDependency;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::rustc::{
    OvenProjectInspectionRootDependency, OvenProjectInspectionTestDependencyRoot, OvenRustcRegistrySourcePackage,
};

/// Return the caller-owned Oven binary destination, intentionally outside generated-Cargo target layout.
pub fn oven_binary_path(prepared: &OvenPreparedProject, profile: &str) -> PathBuf {
    prepared
        .generator
        .output_dir()
        .join("oven")
        .join(profile)
        .join(&prepared.crate_name)
}

/// Return the native host target without asking an ambient Rust installation to decide whether a completed Loaf may
/// run. A completed project output is already linked; only a fresh compile needs to resolve `rustc`.
pub fn native_project_output_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// Normalize one entrypoint exactly as project preparation does, without discovering modules or constructing a compiler
/// session.
pub fn normalized_project_entrypoint(file_path: &str) -> CliResult<PathBuf> {
    if Path::new(file_path).is_absolute() {
        return Ok(PathBuf::from(file_path));
    }
    env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))
        .map(|current_dir| current_dir.join(file_path))
}

/// Return a portable project-relative entrypoint path or decline the optional output fast path. Non-project invocations
/// remain supported by normal Oven preparation; only explicit project bakes may have published this form.
pub fn project_relative_entrypoint(project_root: &Path, entrypoint: &Path) -> Option<String> {
    entrypoint
        .strip_prefix(project_root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .filter(|relative| !relative.is_empty())
}

/// Discover the manifest-owning project root for a completed-output lookup.
///
/// This avoids assuming a conventional `src/` layout: a custom source root is still an exact project bake, while a
/// standalone file intentionally remains on the normal explicit-preparation path.
pub fn project_root_for_completed_output(entrypoint: &Path) -> CliResult<Option<PathBuf>> {
    let inferred_root = resolve_project_root(entrypoint);
    let Some(manifest) = discover_effective_project_manifest(&inferred_root)? else {
        return Ok(None);
    };
    enforce_project_toolchain_constraint(&manifest)?;
    Ok(Some(manifest.project_root().to_path_buf()))
}

/// Reject a store- or caller-relative path that could escape its declared root.
pub fn validated_project_output_relative_path(relative_path: &str, role: &str) -> CliResult<PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf has an unsafe {role} path"
        )));
    }
    Ok(relative.to_path_buf())
}

/// Add one regular file physically below the project root to the completed-Loaf publication set.
///
/// Resolve ancestor aliases on both sides before checking containment, while refusing a symlink at the file itself.
fn append_project_output_bake_file(
    project_root: &Path,
    source_path: &Path,
    output_relative_path: String,
    files: &mut Vec<OvenProjectOutputBakeFile>,
) -> CliResult<()> {
    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        CliError::failure(format!(
            "cannot retain generated Oven project output {}: {error}",
            source_path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "completed Oven project output must be a regular non-symlink file: {}",
            source_path.display()
        )));
    }
    let canonical_root = fs::canonicalize(project_root).map_err(|error| {
        CliError::failure(format!(
            "cannot resolve Oven project root {}: {error}",
            project_root.display()
        ))
    })?;
    let canonical_source = fs::canonicalize(source_path).map_err(|error| {
        CliError::failure(format!(
            "cannot resolve generated Oven project output {}: {error}",
            source_path.display()
        ))
    })?;
    let caller_relative_path = canonical_source
        .strip_prefix(&canonical_root)
        .map_err(|_| {
            CliError::failure(format!(
                "completed Oven project output {} escaped project root {}",
                source_path.display(),
                project_root.display()
            ))
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let output_relative_path = validated_project_output_relative_path(&output_relative_path, "stored output")?
        .to_string_lossy()
        .replace('\\', "/");
    let _ = validated_project_output_relative_path(&caller_relative_path, "caller output")?;
    files.push(OvenProjectOutputBakeFile {
        source_path: canonical_source,
        caller_relative_path,
        output_relative_path,
    });
    Ok(())
}

/// Recursively collect generated source files in stable order without treating unrelated worktree output or a mutable
/// inspection cache as completed output.
fn append_project_output_tree(
    project_root: &Path,
    source_root: &Path,
    output_prefix: &str,
    files: &mut Vec<OvenProjectOutputBakeFile>,
) -> CliResult<()> {
    let mut entries = fs::read_dir(source_root)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to read generated Oven source {}: {error}",
                source_root.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CliError::failure(format!(
                "failed to enumerate generated Oven source {}: {error}",
                source_root.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CliError::failure(format!(
                "failed to inspect generated Oven source {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            let child_prefix = format!(
                "{}/{}",
                output_prefix.trim_end_matches('/'),
                entry.file_name().to_string_lossy()
            );
            append_project_output_tree(project_root, &path, &child_prefix, files)?;
        } else if file_type.is_file() {
            let output_relative_path = format!(
                "{}/{}",
                output_prefix.trim_end_matches('/'),
                entry.file_name().to_string_lossy()
            );
            append_project_output_bake_file(project_root, &path, output_relative_path, files)?;
        } else {
            return Err(CliError::failure(format!(
                "generated Oven source contains a non-regular entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Return every manifest-declared provider sidecar that a consumer must retain beside its library artifact.
pub fn library_project_output_sidecars(
    manifest: &LibraryManifest,
    artifact_root: &Path,
) -> CliResult<Vec<(PathBuf, String)>> {
    let mut relative_paths = Vec::new();
    if let Some(desugarer) = manifest
        .vocab
        .as_ref()
        .and_then(|vocab| vocab.desugarer_artifact.as_ref())
    {
        relative_paths.push(validated_project_output_relative_path(
            &desugarer.relative_path,
            "vocab desugarer artifact",
        )?);
    }
    if manifest.contract_metadata.executable_representation.is_some() {
        let path = incan_frontend::library_manifest::published_layout::executable_surface_path(
            &artifact_root.join("manifest.incnlib"),
            manifest,
        )
        .ok_or_else(|| CliError::failure("invalid executable artifact descriptor"))?;
        relative_paths.push(
            path.strip_prefix(artifact_root)
                .map_err(|error| CliError::failure(error.to_string()))?
                .to_path_buf(),
        );
    }
    relative_paths
        .into_iter()
        .map(|relative| {
            let source = artifact_root.join(&relative);
            if !source.is_file() {
                return Err(CliError::failure(format!(
                    "completed Oven library output is missing manifest-declared sidecar {}",
                    source.display()
                )));
            }
            Ok((
                source,
                format!(
                    "generated/provider-sidecars/{}",
                    relative.to_string_lossy().replace('\\', "/")
                ),
            ))
        })
        .collect()
}

/// Seal the checked provider manifest and every sidecar it authorizes as one package handoff.
pub fn packaged_library_metadata_files(
    manifest_path: &Path,
    manifest: &LibraryManifest,
    artifact_root: &Path,
) -> CliResult<Vec<OvenPackagedLibraryMetadataFile>> {
    let mut paths = vec![manifest_path.to_path_buf()];
    paths.extend(
        library_project_output_sidecars(manifest, artifact_root)?
            .into_iter()
            .map(|(path, _)| path),
    );
    let canonical_root = fs::canonicalize(artifact_root).map_err(|error| {
        CliError::failure(format!(
            "failed to resolve package artifact root {}: {error}",
            artifact_root.display()
        ))
    })?;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CliError::failure(format!(
                "failed to inspect package metadata file {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::failure(format!(
                "package metadata must be a regular file below its artifact root: {}",
                path.display()
            )));
        }
        let canonical = fs::canonicalize(&path).map_err(|error| {
            CliError::failure(format!(
                "failed to resolve package metadata file {}: {error}",
                path.display()
            ))
        })?;
        let relative = canonical.strip_prefix(&canonical_root).map_err(|_| {
            CliError::failure(format!(
                "package metadata file {} escapes artifact root {}",
                path.display(),
                artifact_root.display()
            ))
        })?;
        let relative =
            validated_project_output_relative_path(&relative.to_string_lossy().replace('\\', "/"), "package metadata")?;
        let (_, digest) = digest_project_output_projection_file(&canonical)?;
        files.push(OvenPackagedLibraryMetadataFile {
            relative_path: relative.to_string_lossy().replace('\\', "/"),
            digest,
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if files
        .windows(2)
        .any(|pair| pair[0].relative_path == pair[1].relative_path)
    {
        return Err(CliError::failure(
            "package metadata authority contains duplicate relative paths",
        ));
    }
    Ok(files)
}

/// Verify that a package handoff still describes its exact checked manifest and declared sidecars.
pub fn validate_packaged_library_metadata_files(
    artifact: &LibraryArtifactMetadata,
    manifest: &OvenPackagedLibraryLoafManifest,
) -> CliResult<()> {
    let library_manifest = LibraryManifest::read_from_path(&artifact.manifest_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read checked package metadata for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.manifest_path.display()
        ))
    })?;
    let actual = packaged_library_metadata_files(&artifact.manifest_path, &library_manifest, &artifact.crate_root)?;
    if actual != manifest.metadata_files {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses pub::{} because its checked library metadata or declared sidecars changed after the package Loaf was baked; rebake the provider",
            artifact.dependency_key
        )));
    }
    Ok(())
}

/// Describe the durable generated project result rather than a binary alone.
///
/// Rust-inspection caches, direct-rustc sidecars, and the selected dependency Loafs remain separately managed
/// authorities. The generated source, package handoff index, and final output are the caller-visible project result
/// that an exact hot path may restore without front-end work.
pub fn project_output_bake_files(
    project_root: &Path,
    generator: &ProjectGenerator,
    native_output: &Path,
    library_manifest: Option<&Path>,
    package_loaf_manifest: Option<&Path>,
    library_sidecars: &[(PathBuf, String)],
) -> CliResult<Vec<OvenProjectOutputBakeFile>> {
    let mut files = Vec::new();
    append_project_output_bake_file(
        project_root,
        &generator.cargo_manifest_path(),
        "generated/Cargo.toml".to_string(),
        &mut files,
    )?;
    append_project_output_tree(
        project_root,
        &generator.output_dir().join("src"),
        "generated/src",
        &mut files,
    )?;
    if let Some(library_manifest) = library_manifest {
        append_project_output_bake_file(
            project_root,
            library_manifest,
            "generated/library.incnlib".to_string(),
            &mut files,
        )?;
    }
    if let Some(package_loaf_manifest) = package_loaf_manifest {
        append_project_output_bake_file(
            project_root,
            package_loaf_manifest,
            "generated/oven/package-loafs.json".to_string(),
            &mut files,
        )?;
    }
    for (sidecar, output_relative_path) in library_sidecars {
        append_project_output_bake_file(project_root, sidecar, output_relative_path.clone(), &mut files)?;
    }
    append_project_output_bake_file(
        project_root,
        native_output,
        OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
        &mut files,
    )?;
    let mut caller_paths = BTreeSet::new();
    let mut output_paths = BTreeSet::new();
    for file in &files {
        if !caller_paths.insert(file.caller_relative_path.clone())
            || !output_paths.insert(file.output_relative_path.clone())
        {
            return Err(CliError::failure(
                "completed Oven project-output Loaf contains duplicate generated paths",
            ));
        }
    }
    Ok(files)
}

/// Exact registry package identity selected by a project's generated Cargo lock projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProjectLockedRegistryPackage {
    package: String,
    version: String,
    registry: String,
    checksum: String,
}

/// Read the exact registry package set selected by one or more project Cargo lock projections.
///
/// Multiple prepared targets may name the same package identity; those repeats collapse. Distinct compatible
/// identities remain distinct so root selection can refuse when the project locks do not decide one exact package.
pub(crate) fn project_locked_registry_packages<'a>(
    lock_paths: impl IntoIterator<Item = &'a Path>,
) -> CliResult<Vec<ProjectLockedRegistryPackage>> {
    let mut locked = BTreeSet::new();
    for path in lock_paths {
        let payload = fs::read_to_string(path).map_err(|error| {
            CliError::failure(format!(
                "project inspection cannot read its generated Cargo lock projection {}: {error}",
                path.display()
            ))
        })?;
        let document = toml::from_str::<toml::Value>(&payload).map_err(|error| {
            CliError::failure(format!(
                "project inspection cannot parse its generated Cargo lock projection {}: {error}",
                path.display()
            ))
        })?;
        let packages = document.get("package").and_then(toml::Value::as_array).ok_or_else(|| {
            CliError::failure(format!(
                "project inspection Cargo lock projection {} has no package array",
                path.display()
            ))
        })?;
        for package in packages {
            let Some(table) = package.as_table() else {
                return Err(CliError::failure(format!(
                    "project inspection Cargo lock projection {} contains a non-table package record",
                    path.display()
                )));
            };
            let Some(registry) = table
                .get("source")
                .and_then(toml::Value::as_str)
                .filter(|source| source.starts_with("registry+"))
            else {
                continue;
            };
            let field = |name| table.get(name).and_then(toml::Value::as_str);
            let (Some(package), Some(version), Some(checksum)) = (field("name"), field("version"), field("checksum"))
            else {
                return Err(CliError::failure(format!(
                    "project inspection Cargo lock projection {} contains an incomplete registry package record",
                    path.display()
                )));
            };
            locked.insert(ProjectLockedRegistryPackage {
                package: package.to_string(),
                version: version.to_string(),
                registry: registry.to_string(),
                checksum: checksum.to_string(),
            });
        }
    }
    Ok(locked.into_iter().collect())
}

/// Select the unique exact package identity authorized by the owner of one registry edge.
///
/// A publisher root edge or compatible project-lock record marks a project-owned edge, so the project lock must
/// select exactly one identity and agree with publisher evidence. An edge absent from both is owned by the SDK/base
/// closure; its sealed lock may select one compatible catalog identity, while a singular compatible catalog is
/// already exact. No catalog ordering or newest-compatible fallback is allowed at this boundary.
fn project_inspection_locked_identity(
    alias: &str,
    package: &str,
    requirement: &semver::VersionReq,
    catalog: &[OvenRustcRegistrySourcePackage],
    publisher_roots: Option<&[OvenProjectRegistrySourceDependency]>,
    project_locked: &[ProjectLockedRegistryPackage],
    owner_locked: &[ProjectLockedRegistryPackage],
) -> CliResult<ProjectLockedRegistryPackage> {
    let publisher_locked = if let Some(publisher_roots) = publisher_roots {
        let mut matches = publisher_roots.iter().filter(|root| root.alias == alias);
        let exact = matches.next();
        if exact.is_some_and(|exact| matches.any(|candidate| candidate != exact)) {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` has conflicting exact root-edge records in the publisher payload"
            )));
        }
        if exact.is_some_and(|exact| {
            exact.package != package
                || !semver::Version::parse(&exact.version).is_ok_and(|version| requirement.matches(&version))
        }) {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` differs from its exact publisher root edge"
            )));
        }
        exact
    } else {
        None
    };
    let project_matches = project_locked
        .iter()
        .filter(|locked| {
            locked.package == package
                && semver::Version::parse(&locked.version).is_ok_and(|version| requirement.matches(&version))
        })
        .collect::<Vec<_>>();
    if let Some(publisher) = publisher_locked {
        let [project] = project_matches.as_slice() else {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` has {} compatible exact records in the project lock",
                project_matches.len()
            )));
        };
        if !(project.package == publisher.package
            && project.version == publisher.version
            && project.registry == publisher.registry
            && project.checksum == publisher.checksum)
        {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` has conflicting publisher and project lock identities"
            )));
        }
        return Ok((*project).clone());
    }
    match project_matches.as_slice() {
        [project] => return Ok((*project).clone()),
        [] => {}
        _ => {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` has {} compatible exact records in the project lock",
                project_matches.len()
            )));
        }
    }
    let catalog_matches = catalog
        .iter()
        .filter(|source| {
            source.package == package
                && semver::Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
        })
        .collect::<Vec<_>>();
    if let [source] = catalog_matches.as_slice() {
        return Ok(ProjectLockedRegistryPackage {
            package: source.package.clone(),
            version: source.version.clone(),
            registry: source.source.registry.clone(),
            checksum: source.source.checksum.clone(),
        });
    }
    let owner_matches = owner_locked
        .iter()
        .filter(|locked| {
            locked.package == package
                && semver::Version::parse(&locked.version).is_ok_and(|version| requirement.matches(&version))
                && catalog_matches.iter().any(|source| {
                    source.package == locked.package
                        && source.version == locked.version
                        && source.source.registry == locked.registry
                        && source.source.checksum == locked.checksum
                })
        })
        .collect::<Vec<_>>();
    let [owner] = owner_matches.as_slice() else {
        return Err(CliError::failure(format!(
            "SDK-owned project inspection dependency `{alias}` has {} compatible exact records in the selected immutable source catalog and {} in the SDK/base lock",
            catalog_matches.len(),
            owner_matches.len()
        )));
    };
    Ok((*owner).clone())
}

/// Publish one singular, project-level Rust inspection authority from the preferred debug plan.
///
/// A current project extension already splits every source tree between its exact release Loaf and its bounded
/// project fragment. The authority therefore binds catalog selection to the canonical publisher roots and project
/// lock projection and names those two immutable constituents; it does not copy their source trees into a third
/// closure.
pub(crate) fn project_inspection_root_dependencies(
    dependencies: &[DependencySpec],
    catalog: &[OvenRustcRegistrySourcePackage],
    publisher_roots: Option<&[OvenProjectRegistrySourceDependency]>,
    project_locked: &[ProjectLockedRegistryPackage],
    owner_locked: &[ProjectLockedRegistryPackage],
) -> CliResult<Vec<OvenProjectInspectionRootDependency>> {
    let mut roots = Vec::new();
    for dependency in dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
    {
        let alias = dependency.crate_name.replace('-', "_");
        let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let requirement = dependency
            .version
            .as_deref()
            .and_then(|version| semver::VersionReq::parse(version).ok())
            .ok_or_else(|| {
                CliError::failure(format!(
                    "project inspection dependency `{alias}` has no valid registry version requirement"
                ))
            })?;
        let mut requested_features = dependency.features.clone();
        requested_features.sort();
        requested_features.dedup();
        let exact = project_inspection_locked_identity(
            &alias,
            package,
            &requirement,
            catalog,
            publisher_roots,
            project_locked,
            owner_locked,
        )?;
        let matches = catalog
            .iter()
            .filter(|source| {
                source.package == package
                    && semver::Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
                    && source.package == exact.package
                    && source.version == exact.version
                    && source.source.registry == exact.registry
                    && source.source.checksum == exact.checksum
                    && requested_features
                        .iter()
                        .all(|feature| source.features.contains(feature))
            })
            .collect::<Vec<_>>();
        let [source] = matches.as_slice() else {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` has {} feature-compatible exact records in the selected immutable source catalog",
                matches.len()
            )));
        };
        roots.push(OvenProjectInspectionRootDependency {
            alias,
            package: source.package.clone(),
            version: source.version.clone(),
            registry: source.source.registry.clone(),
            checksum: source.source.checksum.clone(),
            requested_features,
            default_features: dependency.default_features,
        });
    }
    roots.sort_by(|left, right| left.alias.cmp(&right.alias));
    if roots.windows(2).any(|window| window[0].alias == window[1].alias) {
        return Err(CliError::failure(
            "project inspection dependencies contain duplicate Rust-facing registry aliases",
        ));
    }
    Ok(roots)
}

/// Bind every promoted test dependency to its portable declaration/source digest and, for registry roots, Cargo's
/// exact locked package identity from the selected publisher closure.
pub fn project_inspection_test_dependency_roots(
    dependencies: &[DependencySpec],
    dependency_root_digests: &BTreeMap<String, String>,
    registry_source_dependencies: &[OvenProjectInspectionRootDependency],
    dev_registry_source_dependencies: &[OvenProjectInspectionRootDependency],
) -> CliResult<BTreeMap<String, OvenProjectInspectionTestDependencyRoot>> {
    let mut roots = BTreeMap::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        let dependency_digest = dependency_root_digests.get(&alias).cloned().ok_or_else(|| {
            CliError::failure(format!(
                "project inspection test dependency `{alias}` lost its portable root digest"
            ))
        })?;
        let root = match dependency.source {
            DependencySource::Registry => {
                let mut matching = registry_source_dependencies
                    .iter()
                    .chain(dev_registry_source_dependencies)
                    .filter(|root| root.alias == alias);
                let locked = matching.next().cloned().ok_or_else(|| {
                    CliError::failure(format!(
                        "project inspection test dependency `{alias}` has no exact locked publisher root"
                    ))
                })?;
                if matching.any(|candidate| candidate != &locked) {
                    return Err(CliError::failure(format!(
                        "project inspection test dependency `{alias}` has conflicting normal/dev publisher roots"
                    )));
                }
                let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
                let requirement = dependency
                    .version
                    .as_deref()
                    .and_then(|version| semver::VersionReq::parse(version).ok());
                let mut requested_features = dependency.features.clone();
                requested_features.sort();
                requested_features.dedup();
                if locked.package != package
                    || !requirement.is_some_and(|requirement| {
                        semver::Version::parse(&locked.version).is_ok_and(|version| requirement.matches(&version))
                    })
                    || locked.requested_features != requested_features
                    || locked.default_features != dependency.default_features
                {
                    return Err(CliError::failure(format!(
                        "project inspection test dependency `{alias}` differs from its exact locked publisher root"
                    )));
                }
                OvenProjectInspectionTestDependencyRoot::Registry {
                    dependency_digest,
                    locked,
                }
            }
            DependencySource::Path { .. } => OvenProjectInspectionTestDependencyRoot::Path { dependency_digest },
            DependencySource::Git { .. } => OvenProjectInspectionTestDependencyRoot::Git { dependency_digest },
        };
        if roots.insert(alias.clone(), root).is_some() {
            return Err(CliError::failure(format!(
                "project inspection test dependency surface repeats Rust-facing alias `{alias}`"
            )));
        }
    }
    Ok(roots)
}

/// Return every build-script output directory the explicit bake can seal for one library, versioned where known.
///
/// Units named by a package-keyed map come first: the inspection workspace loader writes one from rust-analyzer's
/// crate graph, and the compatibility build writes one from Cargo's own messages. A directory scan of the same
/// Cargo targets then adds any unit the maps did not name, without a version. Units are deduplicated by build-unit
/// path, so a directory the maps already named is not sealed twice.
pub fn bake_generated_out_dir_units(library: &LibraryInspectionConstituent) -> CliResult<Vec<BakeGeneratedOutDir>> {
    let mut units = Vec::new();
    let mut seen = BTreeSet::new();
    for map_dir in [
        library.rust_inspect_manifest_dir.as_deref(),
        library.generated_project_dir.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        for (package, version, out_dir) in generated_out_dir_map_records(map_dir) {
            let Some(unit_relative_path) = build_unit_relative_path(&out_dir) else {
                continue;
            };
            if !out_dir_holds_rust(&out_dir) || !seen.insert(unit_relative_path.clone()) {
                continue;
            }
            units.push(BakeGeneratedOutDir {
                crate_name: package,
                unit_relative_path,
                out_dir,
                version: Some(version),
            });
        }
    }
    for target_dir in bake_generated_out_dir_targets(library)? {
        for generated in bake_generated_out_dirs(&target_dir)? {
            if seen.insert(generated.unit_relative_path.clone()) {
                units.push(generated);
            }
        }
    }
    Ok(units)
}

/// Read the package-keyed build-script output map written beside `dir`, as `(package, version, OUT_DIR)`.
#[allow(unused_variables)]
fn generated_out_dir_map_records(dir: &Path) -> Vec<(String, String, PathBuf)> {
    #[cfg(feature = "rust_inspect")]
    let records = ::rust_inspect::read_generated_out_dirs_map(dir)
        .into_iter()
        .map(|record| (record.package, record.version, record.out_dir))
        .collect();
    #[cfg(not(feature = "rust_inspect"))]
    let records = Vec::new();
    records
}

/// Return the build-unit path below `build/` for one `out` directory, in either layout.
///
/// Oven's bootstrap lays build units out as `build/<crate>/<hash>/out`; a plain Cargo target uses
/// `build/<crate>-<hash>/out`. Anything else is not a build-script output directory this bake seals.
fn build_unit_relative_path(out_dir: &Path) -> Option<String> {
    if out_dir.file_name()? != "out" {
        return None;
    }
    let mut components = Vec::new();
    let mut cursor = out_dir.parent()?;
    loop {
        let name = cursor.file_name()?.to_str()?;
        if name == "build" {
            break;
        }
        if components.len() == 2 {
            return None;
        }
        components.push(name.to_string());
        cursor = cursor.parent()?;
    }
    components.reverse();
    Some(components.join("/"))
}

/// Return whether one build-script output directory holds generated Rust worth sealing.
fn out_dir_holds_rust(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("rs"))
        })
        .unwrap_or(false)
}

/// Return the Cargo target directories whose build-script output the explicit bake can seal for one library.
///
/// Generated Rust reaches the bake by two routes. A direct-rustc bake's rust-inspect workspace names its Cargo
/// target through `.cargo/config.toml`; the bounded compatibility baker builds the library through one unified Cargo
/// invocation in the generated project's own selected target and leaves no rust-inspect config behind. Either can
/// exist alone, so both are offered, in that order, and the sealer deduplicates by build unit.
fn bake_generated_out_dir_targets(library: &LibraryInspectionConstituent) -> CliResult<Vec<PathBuf>> {
    let mut targets = Vec::new();
    if let Some(manifest_dir) = library.rust_inspect_manifest_dir.as_deref()
        && let Some(target_dir) = rust_inspect_workspace_cargo_target(manifest_dir)?
    {
        targets.push(target_dir);
    }
    if let Some(target_dir) = library.cargo_target_dir.clone()
        && !targets.contains(&target_dir)
    {
        targets.push(target_dir);
    }
    Ok(targets)
}

/// Return the Cargo target a rust-inspect workspace names through its `.cargo/config.toml`, if it names one.
fn rust_inspect_workspace_cargo_target(rust_inspect_manifest_dir: &Path) -> CliResult<Option<PathBuf>> {
    let config_path = rust_inspect_manifest_dir.join(".cargo").join("config.toml");
    let Ok(config) = fs::read_to_string(&config_path) else {
        return Ok(None);
    };
    let config = toml::from_str::<toml::Value>(&config).map_err(|error| {
        CliError::failure(format!(
            "rust-inspect Cargo config {} is not valid TOML: {error}",
            config_path.display()
        ))
    })?;
    Ok(config
        .get("build")
        .and_then(|build| build.get("target-dir"))
        .and_then(toml::Value::as_str)
        .map(PathBuf::from))
}

/// Return the build-script output directories the explicit bake's Cargo bootstrap left below one Cargo target.
///
/// Oven's bootstrap lays build units out as `debug/build/<crate>/<hash>/out`; a plain Cargo target uses
/// `debug/build/<crate>-<hash>/out`. Every `out` that holds generated Rust is returned with its package name and the
/// build-unit path below `build/`, so the sealed copy keeps the layout the generated-code route already recognizes.
fn bake_generated_out_dirs(cargo_target_dir: &Path) -> CliResult<Vec<BakeGeneratedOutDir>> {
    let build_dir = cargo_target_dir.join("debug").join("build");
    let Ok(units) = fs::read_dir(&build_dir) else {
        return Ok(Vec::new());
    };
    let holds_rust = out_dir_holds_rust;
    let mut out_dirs = Vec::new();
    for unit in units.flatten() {
        let unit_name = unit.file_name().to_string_lossy().into_owned();
        let direct_out = unit.path().join("out");
        if direct_out.is_dir() {
            // Cargo layout: `<crate>-<hash>/out`.
            if let Some((crate_name, _)) = unit_name.rsplit_once('-')
                && holds_rust(&direct_out)
            {
                out_dirs.push(BakeGeneratedOutDir {
                    crate_name: crate_name.to_string(),
                    unit_relative_path: unit_name.clone(),
                    out_dir: direct_out,
                    version: None,
                });
            }
            continue;
        }
        // Oven layout: `<crate>/<hash>/out`.
        let Ok(hashes) = fs::read_dir(unit.path()) else {
            continue;
        };
        for hash in hashes.flatten() {
            let out_dir = hash.path().join("out");
            if out_dir.is_dir() && holds_rust(&out_dir) {
                out_dirs.push(BakeGeneratedOutDir {
                    crate_name: unit_name.clone(),
                    unit_relative_path: format!("{unit_name}/{}", hash.file_name().to_string_lossy()),
                    out_dir,
                    version: None,
                });
            }
        }
    }
    out_dirs.sort_by(|left, right| left.unit_relative_path.cmp(&right.unit_relative_path));
    Ok(out_dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;

    use crate::backend::ProjectGenerator;
    use crate::build::{BakeGeneratedOutDir, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH};
    use oven_cargo_compat::OvenProjectRegistrySourceDependency;
    use oven_model::manifest::{DependencySource, DependencySpec};
    use oven_rustc::rustc::OvenRustcRegistrySourcePackage;
    use oven_store::digest_bytes;

    /// Build one fictional immutable registry-source record for project-inspection selection tests.
    fn inspection_source(package: &str, version: &str, checksum: &str) -> OvenRustcRegistrySourcePackage {
        OvenRustcRegistrySourcePackage {
            package: package.to_string(),
            version: version.to_string(),
            features: vec!["portable".to_string()],
            source: oven_rustc::rustc::OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: checksum.to_string(),
                relative_root: format!("registry-sources/{package}-{version}"),
                digest: digest_bytes(format!("{package} {version} source").as_bytes()),
            },
        }
    }

    #[test]
    fn project_inspection_roots_use_the_locked_exact_catalog_record() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let lock_path = tmp.path().join("Cargo.lock");
        fs::write(
            &lock_path,
            r#"version = 4

[[package]]
name = "fictional-codec"
version = "1.2.3"
source = "registry+https://example.invalid/index"
checksum = "fictional-codec-1.2.3-checksum"
"#,
        )?;
        let project_locked = project_locked_registry_packages([lock_path.as_path()])?;
        let catalog = vec![
            inspection_source("fictional-codec", "1.2.3", "fictional-codec-1.2.3-checksum"),
            inspection_source("fictional-codec", "1.2.4", "fictional-codec-1.2.4-checksum"),
        ];
        let dependency = DependencySpec {
            crate_name: "codec".to_string(),
            version: Some("1".to_string()),
            features: vec!["portable".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: false,
            package: Some("fictional-codec".to_string()),
        };
        let locked = OvenProjectRegistrySourceDependency {
            alias: "codec".to_string(),
            package: "fictional-codec".to_string(),
            version: "1.2.3".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fictional-codec-1.2.3-checksum".to_string(),
        };

        let selected = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(std::slice::from_ref(&locked)),
            &project_locked,
            &[],
        )?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].version, "1.2.3");
        assert_eq!(selected[0].checksum, "fictional-codec-1.2.3-checksum");

        let selected_from_project_lock = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            None,
            &project_locked,
            &[],
        )?;
        assert_eq!(selected_from_project_lock, selected);

        let Err(lockless_error) = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(std::slice::from_ref(&locked)),
            &[],
            &[],
        ) else {
            return Err("lock-less project inspection selected one of two compatible versions".into());
        };
        assert!(
            lockless_error
                .to_string()
                .contains("0 compatible exact records in the project lock")
        );

        let mut conflicting = locked.clone();
        conflicting.version = "1.2.4".to_string();
        conflicting.checksum = "fictional-codec-1.2.4-checksum".to_string();
        let Err(conflict_error) = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(&[locked, conflicting]),
            &project_locked,
            &[],
        ) else {
            return Err("conflicting locked root edges selected one catalog record".into());
        };
        assert!(
            conflict_error
                .to_string()
                .contains("conflicting exact root-edge records")
        );

        let publisher_conflict = OvenProjectRegistrySourceDependency {
            alias: "codec".to_string(),
            package: "fictional-codec".to_string(),
            version: "1.2.4".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fictional-codec-1.2.4-checksum".to_string(),
        };
        let Err(authority_conflict_error) = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(std::slice::from_ref(&publisher_conflict)),
            &project_locked,
            &[],
        ) else {
            return Err("publisher root overrode a conflicting project lock".into());
        };
        assert!(
            authority_conflict_error
                .to_string()
                .contains("conflicting publisher and project lock identities")
        );
        Ok(())
    }

    #[test]
    fn project_inspection_roots_select_a_single_sdk_owned_catalog_record_without_a_project_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = vec![inspection_source(
            "fictional-runtime-codec",
            "2.4.1",
            "fictional-runtime-codec-2.4.1-checksum",
        )];
        let dependency = DependencySpec {
            crate_name: "runtime_codec".to_string(),
            version: Some("2".to_string()),
            features: vec!["portable".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: false,
            package: Some("fictional-runtime-codec".to_string()),
        };
        let unrelated_project_root = OvenProjectRegistrySourceDependency {
            alias: "project_helper".to_string(),
            package: "fictional-project-helper".to_string(),
            version: "1.0.0".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fictional-project-helper-1.0.0-checksum".to_string(),
        };

        let selected = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(std::slice::from_ref(&unrelated_project_root)),
            &[],
            &[],
        )?;

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].version, "2.4.1");
        assert_eq!(selected[0].checksum, "fictional-runtime-codec-2.4.1-checksum");

        catalog.push(inspection_source(
            "fictional-runtime-codec",
            "2.5.0",
            "fictional-runtime-codec-2.5.0-checksum",
        ));
        let Err(ambiguous_error) = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(std::slice::from_ref(&unrelated_project_root)),
            &[],
            &[],
        ) else {
            return Err("ambiguous SDK-owned dependency selected an arbitrary catalog record".into());
        };
        assert!(ambiguous_error.to_string().contains("SDK-owned"));
        assert!(ambiguous_error.to_string().contains("2 compatible exact records"));

        let sdk_locked = [ProjectLockedRegistryPackage {
            package: "fictional-runtime-codec".to_string(),
            version: "2.4.1".to_string(),
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fictional-runtime-codec-2.4.1-checksum".to_string(),
        }];
        let selected_from_sdk_lock = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            Some(std::slice::from_ref(&unrelated_project_root)),
            &[],
            &sdk_locked,
        )?;
        assert_eq!(selected_from_sdk_lock, selected);
        Ok(())
    }

    #[test]
    fn project_inspection_roots_bind_release_owned_features_without_an_extension()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalog = vec![OvenRustcRegistrySourcePackage {
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            features: vec!["preserve_order".to_string(), "std".to_string()],
            source: oven_rustc::rustc::OvenRustcRegistrySource {
                registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
                checksum: "serde-json-checksum".to_string(),
                relative_root: "registry-sources/serde_json-1.0.140".to_string(),
                digest: digest_bytes(b"serde_json source"),
            },
        }];
        let dependency = DependencySpec {
            crate_name: "serde_json".to_string(),
            version: Some("1".to_string()),
            features: vec!["preserve_order".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let project_locked = [ProjectLockedRegistryPackage {
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-json-checksum".to_string(),
        }];

        let roots = project_inspection_root_dependencies(
            std::slice::from_ref(&dependency),
            &catalog,
            None,
            &project_locked,
            &[],
        )?;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].package, "serde_json");
        assert_eq!(roots[0].requested_features, ["preserve_order"]);
        assert!(!roots[0].default_features);
        let publisher_root = OvenProjectRegistrySourceDependency {
            alias: "serde_json".to_string(),
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-json-checksum".to_string(),
        };
        let promoted_publisher_roots = [publisher_root.clone(), publisher_root];
        assert_eq!(
            project_inspection_root_dependencies(
                std::slice::from_ref(&dependency),
                &catalog,
                Some(&promoted_publisher_roots),
                &project_locked,
                &[],
            )?,
            roots
        );

        let mut unavailable = dependency;
        unavailable.features = vec!["raw_value".to_string()];
        let Err(error) = project_inspection_root_dependencies(&[unavailable], &catalog, None, &project_locked, &[])
        else {
            return Err("release-only source authority accepted a feature absent from the Loaf".into());
        };
        assert!(error.to_string().contains("0 feature-compatible"));
        Ok(())
    }

    #[test]
    fn bake_generated_out_dirs_reads_only_rust_bearing_cargo_build_units() -> Result<(), Box<dyn std::error::Error>> {
        // The bake's rust-inspect workspace names its Cargo target through `.cargo/config.toml`. Only build units
        // whose `out` holds generated Rust are worth sealing, in Oven's `<crate>/<hash>/out` layout as well as
        // Cargo's `<crate>-<hash>/out`.
        let tmp = tempfile::tempdir()?;
        let manifest_dir = tmp.path().join("inspect");
        let target_dir = tmp.path().join("inspect-target");
        fs::create_dir_all(manifest_dir.join(".cargo"))?;
        fs::write(
            manifest_dir.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = \"{}\"\n", target_dir.display()),
        )?;
        let oven_out = target_dir.join("debug/build/substrait/157348677c93f659/out");
        let cargo_out = target_dir.join("debug/build/prost-types-9741e23407182c1c/out");
        let plain_out = target_dir.join("debug/build/cc/0123456789abcdef/out");
        for dir in [&oven_out, &cargo_out, &plain_out] {
            fs::create_dir_all(dir)?;
        }
        fs::write(oven_out.join("substrait.rs"), "pub mod proto {}\n")?;
        fs::write(cargo_out.join("types.rs"), "pub struct Duration;\n")?;
        fs::write(plain_out.join("flags"), "")?;

        let named_target =
            rust_inspect_workspace_cargo_target(&manifest_dir)?.ok_or("the workspace config names a Cargo target")?;
        assert_eq!(named_target, target_dir);
        let dirs = bake_generated_out_dirs(&named_target)?;

        assert_eq!(
            dirs,
            vec![
                BakeGeneratedOutDir {
                    crate_name: "prost-types".to_string(),
                    unit_relative_path: "prost-types-9741e23407182c1c".to_string(),
                    out_dir: cargo_out.clone(),
                    version: None,
                },
                BakeGeneratedOutDir {
                    crate_name: "substrait".to_string(),
                    unit_relative_path: "substrait/157348677c93f659".to_string(),
                    out_dir: oven_out.clone(),
                    version: None,
                },
            ]
        );
        assert!(
            rust_inspect_workspace_cargo_target(tmp.path())?.is_none(),
            "a workspace without a Cargo config names no target"
        );
        assert!(
            bake_generated_out_dirs(&tmp.path().join("never-built"))?.is_empty(),
            "a Cargo target without build units seals nothing"
        );
        // The build-unit path is read from either layout and only below a `build` directory.
        assert_eq!(
            build_unit_relative_path(&cargo_out).as_deref(),
            Some("prost-types-9741e23407182c1c")
        );
        assert_eq!(
            build_unit_relative_path(&oven_out).as_deref(),
            Some("substrait/157348677c93f659")
        );
        assert_eq!(build_unit_relative_path(&tmp.path().join("deps/out")), None);
        assert_eq!(
            build_unit_relative_path(&target_dir.join("debug/build/x/y/z/out")),
            None
        );
        Ok(())
    }

    #[test]
    fn project_output_loaf_retains_durable_generated_files_but_not_inspection_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output_root = project.path().join("target/incan/fixture");
        let generator = ProjectGenerator::new(&output_root, "fixture", false);
        generator.generate("pub fn answer() -> i64 { 42 }\n")?;
        fs::write(output_root.join("Cargo.lock"), "version = 4\n")?;
        let inspection_cache = output_root.join("oven/rust-inspect/metadata.json");
        fs::create_dir_all(inspection_cache.parent().ok_or("inspection cache has no parent")?)?;
        fs::write(&inspection_cache, "mutable inspection cache")?;
        let native_output = output_root.join("oven/release/libfixture.rlib");
        fs::create_dir_all(native_output.parent().ok_or("native output has no parent")?)?;
        fs::write(&native_output, "native output")?;
        let desugarer = output_root.join("desugarers/fixture.wasm");
        fs::create_dir_all(desugarer.parent().ok_or("desugarer has no parent")?)?;
        fs::write(&desugarer, "sealed vocab desugarer")?;

        let files = project_output_bake_files(
            project.path(),
            &generator,
            &native_output,
            None,
            None,
            &[(
                desugarer,
                "generated/provider-sidecars/desugarers/fixture.wasm".to_string(),
            )],
        )?;
        let output_paths = files
            .iter()
            .map(|file| file.output_relative_path.as_str())
            .collect::<BTreeSet<_>>();
        assert!(output_paths.contains("generated/Cargo.toml"));
        assert!(
            !output_paths.contains("generated/Cargo.lock"),
            "the explicit publisher lock must not become a caller-visible completed-output artifact"
        );
        assert!(output_paths.contains("generated/src/lib.rs"));
        assert!(output_paths.contains("generated/provider-sidecars/desugarers/fixture.wasm"));
        assert!(output_paths.contains(OVEN_PROJECT_OUTPUT_ARTIFACT_PATH));
        assert!(output_paths.iter().all(|path| !path.contains("rust-inspect")));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn project_output_accepts_symlink_equivalent_roots() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let project = temporary.path().join("project");
        fs::create_dir_all(project.join("target"))?;
        fs::write(project.join("target/result"), "native artifact")?;
        let canonical_project = fs::canonicalize(&project)?;
        let alias = temporary.path().join("alias");
        std::os::unix::fs::symlink(&canonical_project, &alias)?;
        for (root, source) in [
            (alias.clone(), canonical_project.join("target/result")),
            (canonical_project.clone(), alias.join("target/result")),
        ] {
            let mut files = Vec::new();
            append_project_output_bake_file(&root, &source, "artifact/result".into(), &mut files)?;
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].caller_relative_path, "target/result");
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn project_output_rejects_physical_escape_and_symlink_files() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let project = temporary.path().join("project");
        let outside = temporary.path().join("outside");
        fs::create_dir_all(&project)?;
        fs::create_dir_all(&outside)?;
        fs::write(outside.join("result"), "outside artifact")?;
        std::os::unix::fs::symlink(&outside, project.join("escape"))?;
        std::os::unix::fs::symlink(outside.join("result"), project.join("linked-file"))?;
        for source in [
            outside.join("result"),
            project.join("escape/result"),
            project.join("linked-file"),
        ] {
            let mut files = Vec::new();
            assert!(append_project_output_bake_file(&project, &source, "artifact/result".into(), &mut files).is_err());
            assert!(files.is_empty(), "a refused file must not enter the publication set");
        }
        Ok(())
    }
}
