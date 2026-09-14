//! The compiler-suite artifact catalog: from Cargo's publisher-only unit graph to the immutable artifact index.
//!
//! The explicit compiler-suite publisher runs Cargo once to learn what it built, then never again. These helpers
//! decode that unit graph, choose the target selections and bootstrap set, and turn the built artifacts into a
//! catalog keyed by unit so each direct-rustc target plan can name exactly the externs and search directories it
//! needs. The publisher that calls them lives in `legacy_cargo.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    CargoCompilerArtifact, CargoInvocationOutput, CargoUnitGraph, CargoUnitGraphUnit, OVEN_COMPILER_TEST_PROFILE,
    OvenArtifactMaterializedFile, OvenCompilerTestSuiteArtifactClosure, OvenCompilerWorkspaceLibraryKey,
    OvenLegacyCargoError, OvenRustcArtifactExtern, OvenRustcSupportingArtifact, canonical_directory,
    compiler_suite_cargo_build_output, compiler_suite_cargo_reported_direct_artifact, compiler_suite_target_kind,
    compiler_suite_target_runner, compiler_suite_unit_is_in_workspace, digest_bytes, is_direct_rustc_artifact,
    publisher_output_artifact_paths, regular_file_bytes, relative_path, verified_regular_file,
};
#[cfg(test)]
use super::{OvenCompilerTestSuiteTarget, OvenLegacyCargoInvocationTarget};

/// Decode the publisher-only Cargo unit graph before it can be transformed into immutable Oven target plans.
pub(crate) fn parse_compiler_suite_unit_graph(
    output: &CargoInvocationOutput,
) -> Result<CargoUnitGraph, OvenLegacyCargoError> {
    let graph = serde_json::from_slice::<CargoUnitGraph>(&output.stdout).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "the internal compatibility publisher did not emit a valid compiler-suite unit graph: {error}"
        ))
    })?;
    if graph.version != 1 {
        return Err(OvenLegacyCargoError::Plan(format!(
            "the internal compatibility publisher emitted unsupported compiler-suite unit graph version {}",
            graph.version
        )));
    }
    if graph.roots.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "the internal compatibility publisher emitted a compiler-suite unit graph without root units".to_string(),
        ));
    }
    Ok(graph)
}

/// Validate every workspace test root before compiling the transient publisher target.
///
/// This prevents a costly partial suite publication when Cargo discovers a root mode or target kind that no
/// receipt-bound Oven executor can faithfully run. Rustc libtests (including proc macros) and Rustdoc doctests are
/// both explicit supported runner classes.
pub(crate) fn validate_compiler_suite_unit_graph(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
) -> Result<(), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    for index in &graph.roots {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !compiler_suite_unit_is_in_workspace(&compiler_root, unit)? {
            continue;
        }
        if !matches!(unit.mode.as_str(), "test" | "doctest") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "full compiler-suite publication does not support Cargo root mode `{}` for {}",
                unit.mode,
                unit.target.src_path.display(),
            )));
        }
        compiler_suite_target_kind(&unit.target.kind)?;
        compiler_suite_target_runner(&unit.mode)?;
    }
    Ok(())
}

/// Convert every supported workspace root into an exact publisher command selection.
///
/// Cargo's unit graph identifies a package by an opaque implementation-specific package ID. Oven derives the
/// package name from the nearest regular `Cargo.toml` below the receipt-authorized compiler root instead of parsing
/// that opaque string. This keeps the hidden publisher command stable while preserving a hard source-root
/// boundary. The resulting selections are deliberately package-qualified: two workspace crates may legitimately
/// expose identically named integration targets.
#[cfg(test)]
pub(crate) fn compiler_suite_target_selections(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
) -> Result<Vec<OvenLegacyCargoInvocationTarget>, OvenLegacyCargoError> {
    Ok(compiler_suite_target_selection_groups(compiler_root, graph)?
        .into_iter()
        .map(|(selection, _)| selection)
        .collect())
}

/// Group every supported graph root by the exact publisher selection that materializes it.
///
/// One package doctest selection may provide more than one root, so the publisher keeps the graph indices alongside
/// the command rather than guessing from target names after Cargo returns. The target directory is still reclaimed
/// immediately after every group has been copied into independent Oven shard staging.
#[cfg(test)]
pub(crate) fn compiler_suite_target_selection_groups(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
) -> Result<Vec<(OvenLegacyCargoInvocationTarget, Vec<usize>)>, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut selections = BTreeMap::<OvenLegacyCargoInvocationTarget, Vec<usize>>::new();
    for index in &graph.roots {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !matches!(unit.mode.as_str(), "test" | "doctest")
            || !compiler_suite_unit_is_in_workspace(&compiler_root, unit)?
        {
            continue;
        }
        let selection = compiler_suite_target_selection_for_unit(&compiler_root, unit)?;
        selections.entry(selection).or_default().push(*index);
    }
    Ok(selections.into_iter().collect())
}

/// Choose the single compiler-library test root used as the bounded direct-Rustc bootstrap closure.
///
/// This is deliberately source-based rather than package-ID-based: Cargo's package identifiers are opaque, while
/// `src/lib.rs` is already receipt-authorized compiler source. A missing or ambiguous bootstrap is a planning
/// refusal; it must not trigger a scan through every package selection.
#[cfg(test)]
pub(crate) fn compiler_suite_bootstrap_selection(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
    selections: &[(OvenLegacyCargoInvocationTarget, Vec<usize>)],
) -> Result<(OvenLegacyCargoInvocationTarget, Vec<usize>), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let root_source =
        fs::canonicalize(compiler_root.join("src/lib.rs")).map_err(|source| OvenLegacyCargoError::Io {
            path: compiler_root.join("src/lib.rs"),
            source,
        })?;
    let mut candidates = Vec::new();
    for (selection, root_indices) in selections {
        let contains_bootstrap = root_indices.iter().any(|index| {
            graph.units.get(*index).is_some_and(|unit| {
                unit.mode == "test"
                    && unit.target.kind.iter().any(|kind| kind == "lib")
                    && fs::canonicalize(&unit.target.src_path).ok().as_ref() == Some(&root_source)
            })
        });
        if contains_bootstrap {
            candidates.push((selection.clone(), root_indices.clone()));
        }
    }
    match candidates.as_slice() {
        [selection] => Ok(selection.clone()),
        [] => Err(OvenLegacyCargoError::Plan(
            "compiler-suite graph has no receipt-authorized src/lib.rs test bootstrap".to_string(),
        )),
        _ => Err(OvenLegacyCargoError::Plan(
            "compiler-suite graph has multiple src/lib.rs test bootstrap selections".to_string(),
        )),
    }
}

/// Return the resolved package features for one exact publisher selection.
///
/// The suite receipt's feature list belongs to the root `incan` package. Reusing it for a selected workspace
/// package makes Cargo reject legitimate roots whose package does not define root-only features such as `cli`.
/// Cargo's unit graph already records the resolved features for every root, so preserve that package-local evidence
/// for the one narrow invocation instead of treating the root receipt features as workspace-global.
#[cfg(test)]
pub(crate) fn compiler_suite_target_selection_features(
    graph: &CargoUnitGraph,
    root_indices: &[usize],
) -> Result<Vec<String>, OvenLegacyCargoError> {
    if root_indices.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite publisher selection has no root units".to_string(),
        ));
    }
    let mut features = BTreeSet::new();
    for index in root_indices {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite publisher selection root index {index} is outside its unit list"
            ))
        })?;
        features.extend(unit.features.iter().cloned());
    }
    Ok(features.into_iter().collect())
}

/// Derive the one exact `legacy_cargo` invocation authorized for a workspace test root.
#[cfg(test)]
pub(crate) fn compiler_suite_target_selection_for_unit(
    compiler_root: &Path,
    unit: &CargoUnitGraphUnit,
) -> Result<OvenLegacyCargoInvocationTarget, OvenLegacyCargoError> {
    let package = compiler_suite_package_name_for_source(compiler_root, &unit.target.src_path)?;
    let target_kind = compiler_suite_target_kind(&unit.target.kind)?;
    let runner = compiler_suite_target_runner(&unit.mode)?;
    if runner == "rustdoc-test" {
        return Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageDoctests(package));
    }
    match target_kind.as_str() {
        "lib" | "proc-macro" => Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary(package)),
        "bin" => Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageBinary {
            package,
            target: unit.target.name.clone(),
        }),
        "test" => Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
            package,
            target: unit.target.name.clone(),
        }),
        _ => unreachable!("validated compiler-suite target kind"),
    }
}

/// Find the package manifest that owns one receipt-authorized workspace source and return its declared package name.
pub(crate) fn compiler_suite_package_name_for_source(
    compiler_root: &Path,
    source: &Path,
) -> Result<String, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = fs::canonicalize(source).map_err(|source_error| OvenLegacyCargoError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if !source.starts_with(&compiler_root) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite workspace source",
            message: format!(
                "{} escapes receipt-authorized compiler root {}",
                source.display(),
                compiler_root.display()
            ),
        });
    }
    let mut directory = source.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite workspace source",
        message: format!("{} has no parent directory", source.display()),
    })?;
    loop {
        let manifest = directory.join("Cargo.toml");
        if manifest.exists() {
            let metadata = fs::symlink_metadata(&manifest).map_err(|source_error| OvenLegacyCargoError::Io {
                path: manifest.clone(),
                source: source_error,
            })?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite package manifest",
                    message: format!("{} must be a regular non-symlink file", manifest.display()),
                });
            }
            let bytes = regular_file_bytes(&manifest)?;
            let document =
                toml::from_slice::<toml::Value>(&bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite package manifest",
                    message: format!("{} is not valid TOML: {error}", manifest.display()),
                })?;
            if let Some(package) = document
                .get("package")
                .and_then(toml::Value::as_table)
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
            {
                return Ok(package.to_string());
            }
        }
        if directory == compiler_root {
            break;
        }
        directory = directory.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite workspace source",
            message: format!("{} has no parent below compiler root", directory.display()),
        })?;
    }
    Err(OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite package manifest",
        message: format!(
            "no package name owns {} below compiler root {}",
            source.display(),
            compiler_root.display()
        ),
    })
}

/// Immutable publisher-side catalog for every direct-rustc input retained beneath one compiler-suite entry.
pub(crate) struct CompilerSuiteArtifactCatalog {
    pub(crate) closure: OvenCompilerTestSuiteArtifactClosure,
    pub(crate) materialized_files: Vec<OvenArtifactMaterializedFile>,
    pub(crate) by_source_path: BTreeMap<PathBuf, (String, String)>,
}

/// The subset of frozen suite roots that one bounded bootstrap closure can already materialize through direct Rustc.
///
/// A failed root is planning evidence, not permission to launch another Cargo selection. The caller must either
/// supply the missing dependency edge through Oven-owned materialization or refuse the suite before allocating a
/// second publisher closure.
#[cfg(test)]
pub(crate) struct CompilerSuiteTargetPlanCoverage {
    pub(crate) targets: Vec<OvenCompilerTestSuiteTarget>,
    pub(crate) failures: Vec<String>,
}

/// Stable lookup key joining Cargo's unit graph dependency edge to its JSON compiler artifact record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CargoUnitArtifactKey {
    package_id: String,
    target_name: String,
    source_path: PathBuf,
    features: Vec<String>,
    test_profile: bool,
    platform: Option<String>,
}

/// Scan the bounded publisher target once and retain every compiler/linker input as an immutable shared closure.
pub(crate) fn compiler_suite_artifact_catalog(
    staging: &Path,
    dependency_directories: &[PathBuf],
    direct_artifact_files: &[PathBuf],
) -> Result<CompilerSuiteArtifactCatalog, OvenLegacyCargoError> {
    let mut dependency_search_paths = Vec::new();
    let mut all_files = BTreeMap::new();
    let mut by_source_path = BTreeMap::new();
    for directory in dependency_directories {
        let directory = canonical_directory(directory, "compiler-suite Cargo dependency output")?;
        let directory_relative = relative_path(staging, &directory)?;
        let mut entries = fs::read_dir(&directory)
            .map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut found = false;
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite Cargo dependency output",
                    message: format!("{} must contain regular non-symlink files only", path.display()),
                });
            }
            found |=
                insert_compiler_suite_catalog_artifact(staging, &mut all_files, &mut by_source_path, &path, false)?;
        }
        if found {
            dependency_search_paths.push(directory_relative);
        }
    }
    // A package library's `.rlib` is emitted alongside Cargo's profile directory rather than under `deps/`.
    // Materialize only the explicit compiler-artifact paths recorded by the named publisher; scanning that broad
    // profile directory would accidentally admit Cargo bookkeeping and executables.
    for path in direct_artifact_files {
        // These paths already passed the Cargo-recorded build-output identity check in
        // `compiler_suite_output_artifact_paths`. A current Cargo can place a real compiler artifact below
        // `oven-test/build/<crate>/<identity>/out`; retain only that verified record, never a directory scan of
        // arbitrary build-script output.
        let cargo_reported_build_output = compiler_suite_cargo_build_output(path);
        if insert_compiler_suite_catalog_artifact(staging, &mut all_files, &mut by_source_path, path, true)?
            && cargo_reported_build_output
        {
            // A direct `--extern` points at the target library, but Rustc resolves that library's own closure via
            // `-L dependency`. Cargo 1.99 gives each such library its own `out` directory instead of one shared
            // `deps` directory, so retain only the parent directory of every already-verified JSON record.
            let source_path = verified_regular_file(path, "compiler-suite Cargo artifact")?;
            let parent = source_path.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite Cargo artifact",
                message: format!("{} has no parent directory", source_path.display()),
            })?;
            let parent_relative = relative_path(staging, parent)?;
            if !dependency_search_paths.contains(&parent_relative) {
                dependency_search_paths.push(parent_relative);
            }
        }
    }
    if all_files.is_empty() {
        return Err(OvenLegacyCargoError::MissingDirectArtifact {
            crate_name: "compiler-suite direct-rustc closure".to_string(),
            path: staging.to_path_buf(),
        });
    }
    let supporting_artifacts = all_files
        .iter()
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact {
            relative_path: relative_path.clone(),
            digest: digest.clone(),
        })
        .collect::<Vec<_>>();
    let materialized_files = by_source_path
        .iter()
        .map(|(source_path, (relative_path, _))| OvenArtifactMaterializedFile {
            source_path: source_path.clone(),
            relative_path: relative_path.clone(),
        })
        .collect::<Vec<_>>();
    Ok(CompilerSuiteArtifactCatalog {
        closure: OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths,
            native_search_paths: Vec::new(),
            supporting_artifacts,
        },
        materialized_files,
        by_source_path,
    })
}

/// Select only dependency directories Cargo actually emitted for one direct-rustc closure.
///
/// An explicit target build normally produces `target/<triple>/<profile>/deps` as well as host-side procedural
/// macro output. A foundation containing only host-built build or procedural-macro dependencies legitimately has no
/// target directory, however. Treating that absence as publisher I/O failure turns a valid host-only closure into a
/// CI-only failure. The catalog still rejects an entirely empty closure and every root still validates its exact
/// artifact records before Oven publishes it.
pub(crate) fn compiler_suite_dependency_directories(target_deps: PathBuf, host_deps: PathBuf) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for directory in [target_deps, host_deps] {
        if directory.is_dir() && !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

/// Add one exact Cargo-reported compiler artifact to the immutable suite catalog.
pub(crate) fn insert_compiler_suite_catalog_artifact(
    staging: &Path,
    all_files: &mut BTreeMap<String, String>,
    by_source_path: &mut BTreeMap<PathBuf, (String, String)>,
    path: &Path,
    permit_cargo_reported_build_output: bool,
) -> Result<bool, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!("{} must be a regular non-symlink file", path.display()),
        });
    }
    let file_name =
        path.file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite Cargo artifact",
                message: format!("{} has a non-UTF-8 file name", path.display()),
            })?;
    if !is_direct_rustc_artifact(file_name)
        || (!permit_cargo_reported_build_output && compiler_suite_cargo_build_output(path))
    {
        return Ok(false);
    }
    let source_path = verified_regular_file(path, "compiler-suite Cargo artifact")?;
    // `verified_regular_file` returns a canonical path. Canonicalize the trusted staging root before containment so
    // macOS's interchangeable `/var` and `/private/var` spellings do not turn a staged compiler artifact into a
    // false escape, while preserving the non-symlink boundary check above.
    let canonical_staging = canonical_directory(staging, "compiler-suite publisher staging")?;
    if !source_path.starts_with(&canonical_staging) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!("{} escapes the publisher staging directory", source_path.display()),
        });
    }
    let relative_path = relative_path(&canonical_staging, &source_path)?;
    let digest = digest_bytes(&regular_file_bytes(&source_path)?);
    let materialized = (relative_path.clone(), digest.clone());
    if let Some(previous) = by_source_path.get(&source_path) {
        if previous == &materialized {
            return Ok(true);
        }
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!(
                "declares one compiler artifact more than once: {}",
                source_path.display()
            ),
        });
    }
    if all_files.insert(relative_path.clone(), digest).is_some() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!("declares duplicate compiler artifact `{relative_path}`"),
        });
    }
    by_source_path.insert(source_path, materialized);
    Ok(true)
}

/// Index compiler-artifact records by the corresponding resolved Cargo unit, preserving exact emitted file paths.
pub(crate) fn compiler_suite_artifact_index(
    output: &CargoInvocationOutput,
    target_triple: &str,
) -> Result<BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>, OvenLegacyCargoError> {
    let mut index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
            continue;
        };
        if artifact.reason != "compiler-artifact" || artifact.target.src_path.as_os_str().is_empty() {
            continue;
        }
        let source_path = fs::canonicalize(&artifact.target.src_path).map_err(|source| OvenLegacyCargoError::Io {
            path: artifact.target.src_path.clone(),
            source,
        })?;
        let files = artifact
            .filenames
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_direct_rustc_artifact)
                    && compiler_suite_cargo_reported_direct_artifact(&artifact.target.name, path)
            })
            .map(|path| {
                fs::canonicalize(&path).map_err(|source| OvenLegacyCargoError::Io {
                    path: path.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Vec<_>>();
        if !files.is_empty() {
            let mut platforms = files
                .iter()
                .map(|path| compiler_artifact_platform(path, target_triple))
                .collect::<Vec<_>>();
            platforms.sort();
            platforms.dedup();
            let platform = match platforms.as_slice() {
                [platform] => platform.clone(),
                _ => {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "compiler-suite Cargo artifact",
                        message: format!(
                            "{} emitted files for multiple compilation platforms",
                            artifact.target.name
                        ),
                    });
                }
            };
            let mut features = artifact.features;
            features.sort();
            features.dedup();
            let key = CargoUnitArtifactKey {
                package_id: artifact.package_id,
                target_name: artifact.target.name,
                source_path,
                features,
                test_profile: artifact.profile.test,
                platform,
            };
            index.entry(key).or_default().extend(files);
        }
    }
    for files in index.values_mut() {
        files.sort();
        files.dedup();
    }
    Ok(index)
}

/// Read the exact compiler/linker files Cargo reported for one named publisher invocation.
///
/// Cargo can place a package's primary `.rlib` beside the profile directory while dependencies appear below `deps`.
/// Newer Cargo releases can instead report a compiler library below a per-crate `build/<crate>/<identity>/out`
/// directory. The catalog admits the latter only when it is an exact Cargo-recorded, crate-and-identity-matching
/// compiler artifact, then verifies it is a regular path below publisher staging.
pub(crate) fn compiler_suite_output_artifact_paths(
    output: &CargoInvocationOutput,
) -> Result<Vec<PathBuf>, OvenLegacyCargoError> {
    publisher_output_artifact_paths(std::slice::from_ref(output), OVEN_COMPILER_TEST_PROFILE)
}

/// Cargo's JSON artifact records do not carry a platform field; the target directory in their canonical output paths
/// is the authoritative publisher-side distinction between host build/proc-macro artifacts and target artifacts.
pub(crate) fn compiler_artifact_platform(path: &Path, target_triple: &str) -> Option<String> {
    path.components()
        .any(|component| component.as_os_str() == target_triple)
        .then(|| target_triple.to_string())
}

/// Convert a resolved Cargo unit to the key used by publisher compiler-artifact output.
pub(crate) fn cargo_unit_artifact_key(unit: &CargoUnitGraphUnit) -> Result<CargoUnitArtifactKey, OvenLegacyCargoError> {
    let source_path = fs::canonicalize(&unit.target.src_path).map_err(|source| OvenLegacyCargoError::Io {
        path: unit.target.src_path.clone(),
        source,
    })?;
    let mut features = unit.features.clone();
    features.sort();
    features.dedup();
    Ok(CargoUnitArtifactKey {
        package_id: unit.pkg_id.clone(),
        target_name: unit.target.name.clone(),
        source_path,
        features,
        test_profile: unit.mode == "test",
        platform: unit.platform.clone(),
    })
}

/// Select the exact compiler artifact Cargo emitted for one unit-graph dependency edge.
pub(crate) fn compiler_suite_dependency_artifact(
    unit: &CargoUnitGraphUnit,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
    crate_name: &str,
    direct_rustc_target: Option<&str>,
) -> Result<OvenRustcArtifactExtern, OvenLegacyCargoError> {
    let key = cargo_unit_artifact_key(unit)?;
    let wants_dynamic = unit.target.crate_types.iter().any(|kind| kind == "proc-macro");
    // The unit graph tracks Cargo's own host/target compilation placement.  Oven subsequently recompiles every
    // regular dependency consumer with the receipt target, including workspace libraries reached by a host-side
    // proc-macro root.  Selecting that host library merely because the source unit was first seen there can choose
    // a feature-incompatible rlib (for example Serde without its `derive` re-export).  Dynamic proc macros remain
    // host inputs; regular libraries must come from the receipt target and fail closed if that exact family is not
    // among the publisher's emitted artifacts.
    let files = if let Some(target) = direct_rustc_target.filter(|_| !wants_dynamic) {
        let mut target_families = artifact_index
            .iter()
            .filter(|(candidate, files)| {
                candidate.package_id == key.package_id
                    && candidate.target_name == key.target_name
                    && candidate.source_path == key.source_path
                    && candidate.platform.as_deref() == Some(target)
                    && files.iter().any(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.ends_with(".rlib"))
                    })
            })
            .collect::<Vec<_>>();
        target_families.sort_by_key(|(key, _)| *key);
        match target_families.as_slice() {
            [(_, files)] => *files,
            [] => {
                return compiler_suite_catalog_target_library(unit, catalog, crate_name, target);
            }
            _ => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite unit graph",
                    message: format!(
                        "dependency `{crate_name}` has {} receipt-target artifact families",
                        target_families.len()
                    ),
                });
            }
        }
    } else if let Some(files) = artifact_index.get(&key) {
        files
    } else {
        // Cargo's unit graph records the dependency edge's test/platform/features mode, while its stable JSON
        // artifact stream records the emitted library's own compilation mode. A normal dependency of a test root
        // may therefore be the only artifact with the same package/source but different `profile.test` or feature
        // facts. Relax those secondary keys only in ordered steps and only when one emitted artifact family remains;
        // host/target or test/non-test ambiguity is still a deterministic refusal rather than an implicit choice.
        let same_source = |candidate: &CargoUnitArtifactKey| {
            candidate.package_id == key.package_id
                && candidate.target_name == key.target_name
                && candidate.source_path == key.source_path
        };
        let mut compatible = artifact_index
            .iter()
            .filter(|(candidate, _)| {
                same_source(candidate) && candidate.features == key.features && candidate.platform == key.platform
            })
            .collect::<Vec<_>>();
        if compatible.is_empty() {
            compatible = artifact_index
                .iter()
                .filter(|(candidate, _)| same_source(candidate) && candidate.platform == key.platform)
                .collect();
        }
        if compatible.is_empty() {
            compatible = artifact_index
                .iter()
                .filter(|(candidate, _)| same_source(candidate) && candidate.test_profile == key.test_profile)
                .collect();
        }
        if compatible.is_empty() {
            compatible = artifact_index
                .iter()
                .filter(|(candidate, _)| same_source(candidate))
                .collect();
        }
        match compatible.as_slice() {
            [(_, files)] => *files,
            [] => {
                return Err(OvenLegacyCargoError::MissingDirectArtifact {
                    crate_name: crate_name.to_string(),
                    path: unit.target.src_path.clone(),
                });
            }
            _ => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite unit graph",
                    message: format!(
                        "dependency `{crate_name}` has {} emitted artifact families after test/platform reconciliation",
                        compatible.len()
                    ),
                });
            }
        }
    };
    let mut candidates = files
        .iter()
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if wants_dynamic {
                [".dylib", ".so", ".dll"]
                    .iter()
                    .any(|extension| name.ends_with(extension))
            } else {
                name.ends_with(".rlib")
            }
        })
        .filter_map(|path| catalog.by_source_path.get(path).cloned())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    let (relative_path, digest) = match candidates.as_slice() {
        [artifact] => artifact.clone(),
        [] => {
            if let Some(target) = direct_rustc_target.filter(|_| !wants_dynamic) {
                return compiler_suite_catalog_target_library(unit, catalog, crate_name, target);
            }
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: crate_name.to_string(),
                path: unit.target.src_path.clone(),
            });
        }
        _ => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                message: format!(
                    "dependency `{crate_name}` resolves to multiple immutable compiler artifacts: {}",
                    candidates
                        .iter()
                        .map(|(path, _)| path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    };
    Ok(OvenRustcArtifactExtern {
        crate_name: crate_name.replace('-', "_"),
        relative_path,
        digest,
    })
}

/// Recover one exact target-library extern from the sealed foundation catalog when Cargo's JSON artifact stream did
/// not retain an artifact family for the matching unit-graph edge.
///
/// The compiler-suite publisher scans the named foundation's target `deps` directory into this immutable catalog
/// before it creates a direct-Rustc plan. A Cargo build-script output record can be filtered because it is not a
/// linkable `--extern`, while the corresponding library record is absent or keyed differently in that JSON stream.
/// In that narrow case the verified target catalog remains sufficient only when it has exactly one target `.rlib`
/// named for the dependency unit. Multiple candidates remain a deterministic refusal; this is neither source
/// discovery nor a consumer-side Cargo fallback.
pub(crate) fn compiler_suite_catalog_target_library(
    unit: &CargoUnitGraphUnit,
    catalog: &CompilerSuiteArtifactCatalog,
    crate_name: &str,
    direct_rustc_target: &str,
) -> Result<OvenRustcArtifactExtern, OvenLegacyCargoError> {
    let crate_prefix = format!("lib{}-", unit.target.name.replace('-', "_"));
    let target_dependencies = Path::new("third-party-foundation-target")
        .join(direct_rustc_target)
        .join("oven-test/deps");
    let mut candidates = catalog
        .by_source_path
        .iter()
        .filter(|(source_path, (relative_path, _))| {
            Path::new(relative_path).starts_with(&target_dependencies)
                && source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&crate_prefix) && name.ends_with(".rlib"))
        })
        .map(|(_, artifact)| artifact.clone())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    let (relative_path, digest) = match candidates.as_slice() {
        [artifact] => artifact.clone(),
        [] => {
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: crate_name.to_string(),
                path: unit.target.src_path.clone(),
            });
        }
        _ => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                message: format!(
                    "dependency `{crate_name}` has {} sealed receipt-target library artifacts after Cargo JSON reconciliation",
                    candidates.len()
                ),
            });
        }
    };
    Ok(OvenRustcArtifactExtern {
        crate_name: crate_name.replace('-', "_"),
        relative_path,
        digest,
    })
}

/// Derive a stable direct-Rustc workspace-library key from one source unit.
pub(crate) fn compiler_suite_workspace_library_key(
    compiler_root: &Path,
    unit: &CargoUnitGraphUnit,
) -> Result<OvenCompilerWorkspaceLibraryKey, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = verified_regular_file(&unit.target.src_path, "compiler-suite workspace library source")?;
    let source_relative_path = source
        .strip_prefix(&compiler_root)
        .map_err(|_| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("workspace library source {} escapes compiler root", source.display()),
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let target_kind = compiler_suite_target_kind(&unit.target.kind)?;
    if !matches!(target_kind.as_str(), "lib" | "proc-macro") {
        return Err(OvenLegacyCargoError::Plan(format!(
            "workspace dependency `{}` has non-library target kind `{target_kind}`",
            unit.target.src_path.display()
        )));
    }
    let mut features = unit.features.clone();
    features.sort();
    features.dedup();
    Ok(OvenCompilerWorkspaceLibraryKey {
        package_name: compiler_suite_package_name_for_source(&compiler_root, &source)?,
        crate_name: unit.target.name.replace('-', "_"),
        target_kind,
        source_relative_path,
        features,
    })
}

/// Add one immutable `--extern` input, refusing a same-name artifact conflict.
pub(crate) fn compiler_suite_insert_extern(
    externs_by_name: &mut BTreeMap<String, OvenRustcArtifactExtern>,
    extern_artifact: OvenRustcArtifactExtern,
) -> Result<(), OvenLegacyCargoError> {
    match externs_by_name.get(&extern_artifact.crate_name) {
        Some(previous) if previous == &extern_artifact => {
            // Cargo can expose the same resolved dependency through more than one graph edge. `rustc` needs one
            // `--extern`; accepting this is safe only when the immutable artifact identity is exactly identical.
        }
        Some(previous) => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                message: format!(
                    "root target resolves extern `{}` to conflicting immutable artifacts {} and {}",
                    extern_artifact.crate_name, previous.relative_path, extern_artifact.relative_path
                ),
            });
        }
        None => {
            externs_by_name.insert(extern_artifact.crate_name.clone(), extern_artifact);
        }
    }
    Ok(())
}

/// Retain only the transitive external proc-macro edges required while rustc expands a direct dependency.
///
/// A Cargo library can re-export derive macros without exposing those macros as direct edges of its dependent.
/// Direct rustc nevertheless needs the host proc-macro dylib as an explicit `--extern`; traversing the publisher-only
/// unit graph preserves that edge without broadening the plan to every transitive library. Build-script and binary
/// units are compile-time publisher concerns, not direct-rustc inputs, so their subgraphs are deliberately excluded.
pub(crate) fn compiler_suite_collect_transitive_proc_macro_externs(
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    compiler_root: &Path,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
    externs_by_name: &mut BTreeMap<String, OvenRustcArtifactExtern>,
    visited_unit_indices: &mut BTreeSet<usize>,
) -> Result<(), OvenLegacyCargoError> {
    for dependency in &unit.dependencies {
        if !visited_unit_indices.insert(dependency.index) {
            continue;
        }
        let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph dependency index {} is outside its unit list",
                dependency.index
            ))
        })?;
        if dependency_unit
            .target
            .kind
            .iter()
            .any(|kind| matches!(kind.as_str(), "bin" | "custom-build"))
        {
            continue;
        }
        if dependency_unit.target.kind.iter().any(|kind| kind == "proc-macro") {
            if compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)? {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "external compiler-suite dependency reaches workspace proc macro {} without a direct workspace edge",
                    dependency_unit.target.src_path.display()
                )));
            }
            let crate_name = dependency.extern_crate_name.as_deref().ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite proc-macro dependency {} has no extern crate name",
                    dependency_unit.target.src_path.display()
                ))
            })?;
            let extern_artifact =
                compiler_suite_dependency_artifact(dependency_unit, artifact_index, catalog, crate_name, None)?;
            compiler_suite_insert_extern(externs_by_name, extern_artifact)?;
            // The dylib is itself the consumer's Rustc input. Its dependency graph was needed only when Cargo built
            // that dylib, and recursing into it can incorrectly expose a second feature/profile variant of another
            // macro as a root `--extern`.
            continue;
        }
        compiler_suite_collect_transitive_proc_macro_externs(
            dependency_unit,
            graph,
            compiler_root,
            artifact_index,
            catalog,
            externs_by_name,
            visited_unit_indices,
        )?;
    }
    Ok(())
}

/// Resolve external direct externs and direct-Rustc workspace-library edges for one root unit.
pub(crate) fn compiler_suite_target_externs(
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    compiler_root: &Path,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
    direct_rustc_target: &str,
) -> Result<(Vec<OvenRustcArtifactExtern>, Vec<OvenCompilerWorkspaceLibraryKey>), OvenLegacyCargoError> {
    let mut externs_by_name: BTreeMap<String, OvenRustcArtifactExtern> = BTreeMap::new();
    let mut workspace_dependencies = BTreeSet::new();
    let mut visited_transitive_units = BTreeSet::new();
    for dependency in &unit.dependencies {
        let Some(crate_name) = dependency.extern_crate_name.as_deref() else {
            continue;
        };
        let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph dependency index {} is outside its unit list",
                dependency.index
            ))
        })?;
        // Cargo records workspace binaries as dependency edges so integration targets can receive their
        // `CARGO_BIN_EXE_*` paths. They are executable inputs, not linkable Rust crates, and therefore must never
        // become `--extern` arguments to Oven's direct-rustc shard.
        if dependency_unit.target.kind.iter().any(|kind| kind == "bin") {
            continue;
        }
        if compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)? {
            workspace_dependencies.insert(compiler_suite_workspace_library_key(compiler_root, dependency_unit)?);
            continue;
        }
        let extern_artifact = compiler_suite_dependency_artifact(
            dependency_unit,
            artifact_index,
            catalog,
            crate_name,
            Some(direct_rustc_target),
        )?;
        compiler_suite_insert_extern(&mut externs_by_name, extern_artifact)?;
        compiler_suite_collect_transitive_proc_macro_externs(
            dependency_unit,
            graph,
            compiler_root,
            artifact_index,
            catalog,
            &mut externs_by_name,
            &mut visited_transitive_units,
        )?;
    }
    Ok((
        externs_by_name.into_values().collect(),
        workspace_dependencies.into_iter().collect(),
    ))
}
