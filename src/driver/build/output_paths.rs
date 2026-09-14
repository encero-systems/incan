//! Where a project's outputs go: entrypoints, artifact paths, bake files, inspection roots and generated out-dirs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{env, fs};

use crate::backend::ProjectGenerator;
use crate::driver::build::output_materialization::digest_project_output_projection_file;
use crate::driver::build::{
    BakeGeneratedOutDir, LibraryInspectionConstituent, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH,
    OvenPackagedLibraryLoafManifest, OvenPackagedLibraryMetadataFile, OvenPreparedProject, OvenProjectOutputBakeFile,
};
use crate::driver::cargo_policy::enforce_project_toolchain_constraint;
use crate::driver::error::{CliError, CliResult};
use crate::driver::project::{discover_effective_project_manifest, resolve_project_root};
use crate::frontend::library_manifest_index::LibraryArtifactMetadata;
use crate::library_manifest::LibraryManifest;
use crate::manifest::{DependencySource, DependencySpec};
use crate::oven::legacy_cargo::OvenProjectRegistrySourceDependency;
use crate::oven::rustc::{
    OvenProjectInspectionRootDependency, OvenProjectInspectionTestDependencyRoot, OvenRustcRegistrySourcePackage,
};

/// Return the caller-owned Oven binary destination, intentionally outside generated-Cargo target layout.
pub(crate) fn oven_binary_path(prepared: &OvenPreparedProject, profile: &str) -> PathBuf {
    prepared
        .generator
        .output_dir()
        .join("oven")
        .join(profile)
        .join(&prepared.crate_name)
}

/// Return the native host target without asking an ambient Rust installation to decide whether a completed Loaf may
/// run. A completed project output is already linked; only a fresh compile needs to resolve `rustc`.
pub(crate) fn native_project_output_target() -> Option<&'static str> {
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
pub(crate) fn normalized_project_entrypoint(file_path: &str) -> CliResult<PathBuf> {
    if Path::new(file_path).is_absolute() {
        return Ok(PathBuf::from(file_path));
    }
    env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))
        .map(|current_dir| current_dir.join(file_path))
}

/// Return a portable project-relative entrypoint path or decline the optional output fast path. Non-project invocations
/// remain supported by normal Oven preparation; only explicit project bakes may have published this form.
pub(crate) fn project_relative_entrypoint(project_root: &Path, entrypoint: &Path) -> Option<String> {
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
pub(crate) fn project_root_for_completed_output(entrypoint: &Path) -> CliResult<Option<PathBuf>> {
    let inferred_root = resolve_project_root(entrypoint);
    let Some(manifest) = discover_effective_project_manifest(&inferred_root)? else {
        return Ok(None);
    };
    enforce_project_toolchain_constraint(&manifest)?;
    Ok(Some(manifest.project_root().to_path_buf()))
}

/// Reject a store- or caller-relative path that could escape its declared root.
pub(crate) fn validated_project_output_relative_path(relative_path: &str, role: &str) -> CliResult<PathBuf> {
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

/// Add one regular file below a generated project result to the completed-Loaf publication set.
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
    let caller_relative_path = source_path
        .strip_prefix(project_root)
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
        source_path: source_path.to_path_buf(),
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
pub(crate) fn library_project_output_sidecars(
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
        let path = crate::library_manifest::published_layout::executable_surface_path(
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
pub(crate) fn packaged_library_metadata_files(
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
pub(crate) fn validate_packaged_library_metadata_files(
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
pub(crate) fn project_output_bake_files(
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

/// Publish one singular, project-level Rust inspection authority from the preferred debug plan.
///
/// A current project extension already splits every source tree between its exact release Loaf and its bounded
/// project fragment. The authority therefore materializes only the canonical publisher lock and names those two
/// immutable constituents; it does not copy their source trees into a third closure.
pub(crate) fn project_inspection_root_dependencies(
    dependencies: &[DependencySpec],
    catalog: &[OvenRustcRegistrySourcePackage],
    publisher_roots: Option<&[OvenProjectRegistrySourceDependency]>,
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
        let matches = catalog
            .iter()
            .filter(|source| {
                source.package == package
                    && semver::Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
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
        if let Some(publisher_roots) = publisher_roots {
            let mut matches = publisher_roots.iter().filter(|root| root.alias == alias);
            let Some(exact) = matches.next() else {
                return Err(CliError::failure(format!(
                    "project inspection dependency `{alias}` has no exact root-edge record in the publisher payload"
                )));
            };
            if matches.any(|candidate| candidate != exact) {
                return Err(CliError::failure(format!(
                    "project inspection dependency `{alias}` has conflicting exact root-edge records in the publisher payload"
                )));
            }
            if exact.package != source.package
                || exact.version != source.version
                || exact.registry != source.source.registry
                || exact.checksum != source.source.checksum
            {
                return Err(CliError::failure(format!(
                    "project inspection dependency `{alias}` differs from its exact publisher root edge"
                )));
            }
        }
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
pub(crate) fn project_inspection_test_dependency_roots(
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
pub(crate) fn bake_generated_out_dir_units(
    library: &LibraryInspectionConstituent,
) -> CliResult<Vec<BakeGeneratedOutDir>> {
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
    let records = crate::rust_inspect::read_generated_out_dirs_map(dir)
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
    use crate::driver::build::{BakeGeneratedOutDir, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH};
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::oven::digest_bytes;
    use crate::oven::legacy_cargo::OvenProjectRegistrySourceDependency;
    use crate::oven::rustc::OvenRustcRegistrySourcePackage;

    #[test]
    fn project_inspection_roots_bind_release_owned_features_without_an_extension()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalog = vec![OvenRustcRegistrySourcePackage {
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            features: vec!["preserve_order".to_string(), "std".to_string()],
            source: crate::oven::rustc::OvenRustcRegistrySource {
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

        let roots = project_inspection_root_dependencies(std::slice::from_ref(&dependency), &catalog, None)?;
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
            )?,
            roots
        );

        let mut unavailable = dependency;
        unavailable.features = vec!["raw_value".to_string()];
        let Err(error) = project_inspection_root_dependencies(&[unavailable], &catalog, None) else {
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
}
