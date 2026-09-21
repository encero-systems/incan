//! Direct-rustc target plans for the compiler suite: roots, workspace libraries, shards and binaries.
//!
//! A Cargo root unit becomes one portable target plan naming its source root, edition, features, externs and
//! runner; workspace libraries and binaries the roots depend on become plans of their own; the compile environment a
//! shard needs is derived here too. The publisher that calls them lives in `legacy_cargo.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(test)]
use super::compiler_suite_catalog::CompilerSuiteTargetPlanCoverage;
#[cfg(test)]
use super::{
    CargoInvocationOutput, OvenCompilerTestSuiteArtifactClosure, cargo_profile_directory,
    compiler_suite_artifact_catalog, compiler_suite_artifact_index, compiler_suite_dependency_directories,
    compiler_suite_output_artifact_paths,
};
use super::{
    CargoUnitArtifactKey, CargoUnitGraph, CargoUnitGraphUnit, CompilerSuiteArtifactCatalog,
    OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION, OvenArtifactMaterializedFile, OvenCompilerTestSuiteShardPayload,
    OvenCompilerTestSuiteTarget, OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey, OvenLegacyCargoError,
    OvenReceipt, canonical_directory, compiler_suite_package_name_for_source, compiler_suite_source_evidence_key,
    compiler_suite_target_externs, compiler_suite_workspace_library_key, digest_bytes, regular_file_bytes,
    verified_regular_file,
};

/// Convert one Cargo root unit to a portable direct-rustc Oven target plan.
pub fn compiler_suite_target_from_unit(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<OvenCompilerTestSuiteTarget, OvenLegacyCargoError> {
    let source = verified_regular_file(&unit.target.src_path, "compiler-suite target source")?;
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source_relative_path = source
        .strip_prefix(&compiler_root)
        .map_err(|_| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("target source {} escapes compiler root", source.display()),
        })?
        .to_string_lossy()
        .replace('\\', "/");
    if source_relative_path.is_empty() || source_relative_path.contains("..") {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("target source path `{source_relative_path}` is not portable"),
        });
    }
    let source_evidence_key = compiler_suite_source_evidence_key(&source_relative_path);
    if !receipt.sources.supplemental_digests.contains_key(&source_evidence_key) {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite receipt does not authorize direct-rustc target source `{source_relative_path}`"
            ),
        });
    }
    let target_kind = compiler_suite_target_kind(&unit.target.kind)?;
    let runner = compiler_suite_target_runner(&unit.mode)?;
    let package_name = compiler_suite_package_name_for_source(&compiler_root, &source)?;
    let crate_name = unit.target.name.replace('-', "_");
    let compile_environment = direct_rustc_compile_environment(&compiler_root, &source)?;
    let binary_dependencies = compiler_suite_binary_dependencies(unit, graph, &compiler_root)?;
    let mut features = unit.features.clone();
    features.sort();
    features.dedup();
    let (externs, workspace_library_dependencies) = compiler_suite_target_externs(
        unit,
        graph,
        &compiler_root,
        artifact_index,
        catalog,
        &receipt.intent.target,
    )
    .map_err(|error| match error {
        OvenLegacyCargoError::MissingDirectArtifact { crate_name, path } => OvenLegacyCargoError::Plan(format!(
            "compiler-suite target `{}` ({}) cannot resolve direct dependency `{crate_name}` from {}",
            source_relative_path,
            unit.mode,
            path.display()
        )),
        error => error,
    })?;
    Ok(OvenCompilerTestSuiteTarget {
        package_name,
        target_name: unit.target.name.clone(),
        target_kind,
        runner,
        source_relative_path,
        source_evidence_key,
        crate_name,
        edition: unit.target.edition.clone(),
        features,
        compile_environment,
        binary_dependencies,
        workspace_library_dependencies,
        externs,
    })
}

/// Read one target source once more at publication and bind its footprint to the same receipt digest Rustc will
/// enforce at execution.
///
/// This intentionally records a source-derived upper-level scheduling signal rather than a historic duration. A
/// clean worktree with the same admitted receipt therefore derives the same replay layout without preserving host
/// performance observations or a mutable test-specific profile.
pub fn compiler_suite_verified_target_source_bytes(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    target: &OvenCompilerTestSuiteTarget,
) -> Result<u64, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = verified_regular_file(
        &compiler_root.join(&target.source_relative_path),
        "compiler-suite target source footprint",
    )?;
    if !source.starts_with(&compiler_root) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite target source footprint",
            message: format!(
                "target source {} escapes compiler root {}",
                source.display(),
                compiler_root.display()
            ),
        });
    }
    let source_bytes = regular_file_bytes(&source)?;
    let expected_digest = receipt
        .sources
        .supplemental_digests
        .get(&target.source_evidence_key)
        .ok_or_else(|| OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite receipt does not authorize target source `{}`",
                target.source_relative_path
            ),
        })?;
    let actual_digest = digest_bytes(&source_bytes);
    if &actual_digest != expected_digest {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite target source `{}` does not match its receipt evidence",
                target.source_relative_path
            ),
        });
    }
    u64::try_from(source_bytes.len()).map_err(|_| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite target source footprint",
        message: format!("target source {} exceeds the supported byte range", source.display()),
    })
}

/// Convert one Cargo workspace library/proc-macro unit into a caller-owned direct-Rustc materialization step.
pub fn compiler_suite_workspace_library_from_unit(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<OvenCompilerWorkspaceLibrary, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let key = compiler_suite_workspace_library_key(&compiler_root, unit)?;
    let source_evidence_key = compiler_suite_source_evidence_key(&key.source_relative_path);
    if !receipt.sources.supplemental_digests.contains_key(&source_evidence_key) {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite receipt does not authorize direct-Rustc workspace library source `{}`",
                key.source_relative_path
            ),
        });
    }
    let source = verified_regular_file(&unit.target.src_path, "compiler-suite workspace library source")?;
    let compile_environment = direct_rustc_compile_environment(&compiler_root, &source)?;
    let (externs, dependencies) = compiler_suite_target_externs(
        unit,
        graph,
        &compiler_root,
        artifact_index,
        catalog,
        &receipt.intent.target,
    )
    .map_err(|error| match error {
        OvenLegacyCargoError::MissingDirectArtifact { crate_name, path } => OvenLegacyCargoError::Plan(format!(
            "compiler-suite workspace library `{}` cannot resolve direct dependency `{crate_name}` from {}",
            key.source_relative_path,
            path.display()
        )),
        error => error,
    })?;
    Ok(OvenCompilerWorkspaceLibrary {
        key,
        source_evidence_key,
        edition: unit.target.edition.clone(),
        compile_environment,
        externs,
        dependencies,
    })
}

/// Plan every workspace library/proc-macro edge required by direct-Rustc roots and their binary inputs.
///
/// Cargo's unit graph is publisher-only reachability evidence. The resulting compact DAG contains no Cargo output
/// paths: its members are receipt-authorized sources, selected immutable third-party externs, and other DAG keys.
pub fn compiler_suite_workspace_libraries_for_roots(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    root_indices: &[usize],
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<Vec<OvenCompilerWorkspaceLibrary>, OvenLegacyCargoError> {
    /// Visit one publisher graph unit and retain its receipt-authorized workspace closure.
    #[allow(clippy::too_many_arguments)]
    fn visit(
        index: usize,
        compiler_root: &Path,
        receipt: &OvenReceipt,
        graph: &CargoUnitGraph,
        artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
        catalog: &CompilerSuiteArtifactCatalog,
        visited: &mut BTreeSet<usize>,
        libraries: &mut BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCompilerWorkspaceLibrary>,
    ) -> Result<(), OvenLegacyCargoError> {
        if !visited.insert(index) {
            return Ok(());
        }
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite workspace-library graph index {index} is outside its unit list"
            ))
        })?;
        for dependency in &unit.dependencies {
            let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite workspace-library dependency index {} is outside its unit list",
                    dependency.index
                ))
            })?;
            if !compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)? {
                continue;
            }
            let target_kind = compiler_suite_target_kind(&dependency_unit.target.kind)?;
            match target_kind.as_str() {
                "bin" => {
                    // Integration tests receive the executable itself through CARGO_BIN_EXE_*, but that executable
                    // must first receive its own direct-Rustc library inputs.
                    visit(
                        dependency.index,
                        compiler_root,
                        receipt,
                        graph,
                        artifact_index,
                        catalog,
                        visited,
                        libraries,
                    )?;
                }
                "lib" | "proc-macro" => {
                    let library = compiler_suite_workspace_library_from_unit(
                        compiler_root,
                        receipt,
                        dependency_unit,
                        graph,
                        artifact_index,
                        catalog,
                    )?;
                    match libraries.insert(library.key.clone(), library.clone()) {
                        Some(previous) if previous == library => {}
                        Some(previous) => {
                            return Err(OvenLegacyCargoError::Plan(format!(
                                "compiler-suite workspace library `{}` has conflicting plans for {} and {}",
                                library.key.crate_name,
                                previous.key.source_relative_path,
                                library.key.source_relative_path
                            )));
                        }
                        None => {}
                    }
                    visit(
                        dependency.index,
                        compiler_root,
                        receipt,
                        graph,
                        artifact_index,
                        catalog,
                        visited,
                        libraries,
                    )?;
                }
                target_kind => {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "compiler-suite workspace dependency {} has unsupported direct-Rustc target kind `{target_kind}`",
                        dependency_unit.target.src_path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut visited = BTreeSet::new();
    let mut libraries = BTreeMap::new();
    for root_index in root_indices {
        visit(
            *root_index,
            &compiler_root,
            receipt,
            graph,
            artifact_index,
            catalog,
            &mut visited,
            &mut libraries,
        )?;
    }
    Ok(libraries.into_values().collect())
}

/// Extract the workspace binary names Cargo provides to this target through the `CARGO_BIN_EXE_*` contract.
pub fn compiler_suite_binary_dependencies(
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    compiler_root: &Path,
) -> Result<Vec<String>, OvenLegacyCargoError> {
    let mut names = BTreeSet::new();
    for dependency in &unit.dependencies {
        let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite binary dependency index {} is outside its unit list",
                dependency.index
            ))
        })?;
        if dependency_unit.target.kind.iter().any(|kind| kind == "bin")
            && compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)?
        {
            names.insert(dependency_unit.target.name.clone());
        }
    }
    Ok(names.into_iter().collect())
}

/// Accept only native test roots the direct-rustc executor can faithfully compile today.
pub fn compiler_suite_target_kind(kinds: &[String]) -> Result<String, OvenLegacyCargoError> {
    for kind in ["test", "lib", "bin", "proc-macro"] {
        if kinds.iter().any(|candidate| candidate == kind) {
            return Ok(kind.to_string());
        }
    }
    Err(OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite unit graph",
        message: format!(
            "does not yet support direct-rustc execution of Cargo target kind(s): {}",
            kinds.join(", ")
        ),
    })
}

/// Translate Cargo's publisher-only root mode into an explicit Oven executor rather than inferring it at runtime.
pub fn compiler_suite_target_runner(mode: &str) -> Result<String, OvenLegacyCargoError> {
    match mode {
        "test" => Ok("rustc-test".to_string()),
        "doctest" => Ok("rustdoc-test".to_string()),
        "build" => Ok("rustc-run".to_string()),
        _ => Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("does not support Cargo root mode `{mode}`"),
        }),
    }
}

/// Find the package manifest owning a direct-rustc root and encode only portable package metadata.
///
/// Both compiler-suite shards and compiler-owned base Loafs must recreate the small, deterministic subset of
/// Cargo's compile-time package metadata after the executor has removed inherited `CARGO_*` state. The caller passes
/// the root of the publisher-owned project tree so workspace-inherited package versions can be resolved without
/// consulting Cargo at execution time.
pub fn direct_rustc_compile_environment(
    project_root: &Path,
    source: &Path,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let mut directory = source.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite target source",
        message: format!("{} has no parent directory", source.display()),
    })?;
    let mut ancestor = 1_usize;
    let package_manifest = loop {
        let manifest = directory.join("Cargo.toml");
        if manifest.is_file() {
            break (directory.to_path_buf(), manifest, ancestor);
        }
        if directory == project_root {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "direct-rustc source",
                message: format!("{} has no owning Cargo.toml", source.display()),
            });
        }
        directory = directory.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite target source",
            message: format!("{} escapes compiler root", source.display()),
        })?;
        ancestor = ancestor.saturating_add(1);
    };
    let (_, manifest, ancestor) = package_manifest;
    if ancestor > 16 {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite target source",
            message: format!(
                "{} is too deeply nested for a portable package-root token",
                source.display()
            ),
        });
    }
    let manifest_bytes = regular_file_bytes(&manifest)?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite package Cargo.toml",
        message: format!("{} is not UTF-8: {error}", manifest.display()),
    })?;
    let document =
        toml::from_str::<toml::Value>(manifest_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite package Cargo.toml",
            message: format!("{} is not valid TOML: {error}", manifest.display()),
        })?;
    let package =
        document
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite package Cargo.toml",
                message: format!("{} has no [package] table", manifest.display()),
            })?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite package Cargo.toml",
            message: format!("{} has no package name", manifest.display()),
        })?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| workspace_package_value(project_root, "version"))
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite package Cargo.toml",
            message: format!("{} has no resolvable package version", manifest.display()),
        })?;
    Ok(BTreeMap::from([
        (
            "CARGO_MANIFEST_DIR".to_string(),
            format!("@oven-source-ancestor:{ancestor}"),
        ),
        ("CARGO_PKG_NAME".to_string(), name.to_string()),
        ("CARGO_PKG_VERSION".to_string(), version),
    ]))
}

/// Return only the portable generated-project environment that may live in a reusable project Loaf.
///
/// Package name and version are properties of the caller's current generated `Cargo.toml`; retaining them in an
/// immutable extension would let a first project's metadata leak into another project with the same compatible
/// dependency closure. Normal direct-Rustc execution derives those two values again from its own generated source
/// root immediately before invoking Rustc. The source-relative manifest token is intentionally reusable.
pub fn direct_rustc_reusable_project_plan_environment(
    project_root: &Path,
    source: &Path,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let mut environment = direct_rustc_compile_environment(project_root, source)?;
    environment.remove("CARGO_PKG_NAME");
    environment.remove("CARGO_PKG_VERSION");
    Ok(environment)
}

/// Resolve a workspace-inherited package field from the checked-in root manifest without asking Cargo at execution.
pub fn workspace_package_value(compiler_root: &Path, field: &str) -> Option<String> {
    let manifest = regular_file_bytes(&compiler_root.join("Cargo.toml")).ok()?;
    let document = toml::from_slice::<toml::Value>(&manifest).ok()?;
    document
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get(field))
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
}

/// Turn bounded test-build-unit outputs and one CLI output into one immutable direct-rustc target plan.
///
/// The transient Cargo target is used only to provide compiled dependency artifacts and its resolved unit graph. The
/// returned targets name caller-owned source roots and immutable artifact inputs; they deliberately retain neither
/// Cargo's test executables nor its target directory as a normal runtime substrate.
#[cfg(test)]
pub fn compiler_suite_target_plan_coverage(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<CompilerSuiteTargetPlanCoverage, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    let mut failures = Vec::new();
    for index in &graph.roots {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite test unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !matches!(unit.mode.as_str(), "test" | "doctest")
            || !compiler_suite_unit_is_in_workspace(&compiler_root, unit)?
        {
            continue;
        }
        match compiler_suite_target_from_unit(&compiler_root, receipt, unit, graph, artifact_index, catalog) {
            Ok(target) if seen.insert(target.key()) => targets.push(target),
            Ok(_) => {}
            Err(error) => failures.push(format!("{} ({}) — {error}", unit.target.src_path.display(), unit.mode)),
        }
    }
    targets.sort_by(|left, right| {
        (
            &left.package_name,
            &left.runner,
            &left.target_kind,
            &left.target_name,
            &left.source_relative_path,
        )
            .cmp(&(
                &right.package_name,
                &right.runner,
                &right.target_kind,
                &right.target_name,
                &right.source_relative_path,
            ))
    });
    failures.sort();
    failures.dedup();
    if targets.is_empty() && failures.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite unit graph contains no workspace native test targets".to_string(),
        ));
    }
    Ok(CompilerSuiteTargetPlanCoverage { targets, failures })
}

#[cfg(test)]
/// Plan the direct-rustc compiler-suite target closure for test-only publisher verification.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn compiler_suite_direct_target_plan(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    staging: &Path,
    target: &Path,
    test_graph: &CargoUnitGraph,
    test_outputs: &[CargoInvocationOutput],
    cli_graph: &CargoUnitGraph,
    cli_output: &CargoInvocationOutput,
) -> Result<
    (
        Vec<OvenCompilerTestSuiteTarget>,
        Vec<OvenCompilerTestSuiteTarget>,
        OvenCompilerTestSuiteTarget,
        OvenCompilerTestSuiteArtifactClosure,
        Vec<OvenArtifactMaterializedFile>,
    ),
    OvenLegacyCargoError,
> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let profile_directory = cargo_profile_directory(&receipt.intent.profile)?;
    let target_deps = target.join(&receipt.intent.target).join(profile_directory).join("deps");
    let host_deps = target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    let cli_direct_artifact_files = compiler_suite_output_artifact_paths(cli_output)?;
    let catalog = compiler_suite_artifact_catalog(staging, &dependency_directories, &cli_direct_artifact_files)?;
    // A root libtest and the normal library used by the direct CLI can legitimately compile the same Cargo unit
    // with distinct hashes. Keep each invocation's compiler-artifact records separate so a test target is always
    // linked against the exact closure Cargo selected for the test unit, and the CLI against its normal build unit.
    let mut test_artifact_index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for output in test_outputs {
        for (key, mut paths) in compiler_suite_artifact_index(output, &receipt.intent.target)? {
            test_artifact_index.entry(key).or_default().append(&mut paths);
        }
    }
    for paths in test_artifact_index.values_mut() {
        paths.sort();
        paths.dedup();
    }

    let coverage =
        compiler_suite_target_plan_coverage(&compiler_root, receipt, test_graph, &test_artifact_index, &catalog)?;
    if !coverage.failures.is_empty() {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite direct target coverage is incomplete: {}",
            coverage.failures.join("; ")
        )));
    }
    let targets = coverage.targets;

    let binary_targets =
        compiler_suite_binary_targets(&compiler_root, receipt, test_graph, &test_artifact_index, &catalog)?;

    let (cli_target, _cli_workspace_libraries) = compiler_suite_cli_target_from_catalog(
        &compiler_root,
        receipt,
        cli_graph,
        std::slice::from_ref(cli_output),
        &catalog,
    )?;
    Ok((
        targets,
        binary_targets,
        cli_target,
        catalog.closure,
        catalog.materialized_files,
    ))
}

/// Convert the isolated normal compiler CLI build into an immutable direct-rustc plan.
///
/// The CLI is index-owned rather than shared with test shards. Its publisher target can therefore be copied into
/// prepared index staging and reclaimed before the complete shard batch is admitted.
#[cfg(test)]
pub fn compiler_suite_direct_cli_plan(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    staging: &Path,
    target: &Path,
    cli_graph: &CargoUnitGraph,
    cli_outputs: &[CargoInvocationOutput],
) -> Result<
    (
        OvenCompilerTestSuiteTarget,
        Vec<OvenCompilerWorkspaceLibrary>,
        OvenCompilerTestSuiteArtifactClosure,
        Vec<OvenArtifactMaterializedFile>,
    ),
    OvenLegacyCargoError,
> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let profile_directory = cargo_profile_directory(&receipt.intent.profile)?;
    let target_deps = target.join(&receipt.intent.target).join(profile_directory).join("deps");
    let host_deps = target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    let mut direct_artifact_files = Vec::new();
    for output in cli_outputs {
        direct_artifact_files.extend(compiler_suite_output_artifact_paths(output)?);
    }
    direct_artifact_files.sort();
    direct_artifact_files.dedup();
    let catalog = compiler_suite_artifact_catalog(staging, &dependency_directories, &direct_artifact_files)?;
    let (cli_target, cli_workspace_libraries) =
        compiler_suite_cli_target_from_catalog(&compiler_root, receipt, cli_graph, cli_outputs, &catalog)?;
    Ok((
        cli_target,
        cli_workspace_libraries,
        catalog.closure,
        catalog.materialized_files,
    ))
}

/// Resolve exactly one normal `incan` CLI target from publisher artifacts already cataloged for an isolated target.
#[cfg(test)]
pub fn compiler_suite_cli_target_from_catalog(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    cli_graph: &CargoUnitGraph,
    cli_outputs: &[CargoInvocationOutput],
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<(OvenCompilerTestSuiteTarget, Vec<OvenCompilerWorkspaceLibrary>), OvenLegacyCargoError> {
    let mut artifact_index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for output in cli_outputs {
        for (key, mut paths) in compiler_suite_artifact_index(output, &receipt.intent.target)? {
            artifact_index.entry(key).or_default().append(&mut paths);
        }
    }
    for paths in artifact_index.values_mut() {
        paths.sort();
        paths.dedup();
    }
    compiler_suite_cli_target_from_artifact_index(compiler_root, receipt, cli_graph, &artifact_index, catalog)
}

/// Resolve the normal `incan` CLI from a publisher-built third-party artifact index.
///
/// The index can come from a sealed foundation manifest rather than a Cargo compilation of the compiler workspace.
/// This is the key separation that lets direct Rustc own the compiler's workspace-library edges.
pub fn compiler_suite_cli_target_from_artifact_index(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    cli_graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<(OvenCompilerTestSuiteTarget, Vec<OvenCompilerWorkspaceLibrary>), OvenLegacyCargoError> {
    let mut cli_candidates = Vec::new();
    for index in &cli_graph.roots {
        let unit = cli_graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite CLI unit graph root index {index} is outside its unit list"
            ))
        })?;
        if unit.mode == "build"
            && unit.target.name == "incan"
            && unit.target.kind.iter().any(|kind| kind == "bin")
            && compiler_suite_unit_is_in_workspace(compiler_root, unit)?
        {
            cli_candidates.push((
                *index,
                compiler_suite_target_from_unit(compiler_root, receipt, unit, cli_graph, artifact_index, catalog)?,
            ));
        }
    }
    match cli_candidates.as_slice() {
        [(index, target)] => Ok((
            target.clone(),
            compiler_suite_workspace_libraries_for_roots(
                compiler_root,
                receipt,
                cli_graph,
                &[*index],
                artifact_index,
                catalog,
            )?,
        )),
        [] => Err(OvenLegacyCargoError::Plan(
            "compiler-suite CLI unit graph has no normal `incan` binary target".to_string(),
        )),
        _ => Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite CLI unit graph has multiple normal `incan` binary targets: {}",
            cli_candidates
                .iter()
                .map(|(_, target)| target.source_relative_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Convert one isolated publisher target-selection closure into one immutable Oven test shard.
///
/// The caller must give this function a target directory used for only the exact package/target selection that
/// produced `output`. This is intentionally different from slicing the former shared catalog after it was built:
/// every returned closure is rooted in its own temporary selection, so a later publisher can admit/reclaim it as an
/// independently bounded shard instead of copying a full workspace closure for every test root.
#[cfg(test)]
pub fn compiler_suite_direct_target_shard_plan(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    staging: &Path,
    target: &Path,
    graph: &CargoUnitGraph,
    root_index: usize,
    output: &CargoInvocationOutput,
) -> Result<(OvenCompilerTestSuiteShardPayload, Vec<OvenArtifactMaterializedFile>), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let root = graph.units.get(root_index).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is outside its unit list"
        ))
    })?;
    if !matches!(root.mode.as_str(), "test" | "doctest") || !compiler_suite_unit_is_in_workspace(&compiler_root, root)?
    {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is not a supported workspace test root"
        )));
    }
    let profile_directory = cargo_profile_directory(&receipt.intent.profile)?;
    let target_deps = target.join(&receipt.intent.target).join(profile_directory).join("deps");
    let host_deps = target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    let catalog = compiler_suite_artifact_catalog(staging, &dependency_directories, &[])?;
    let artifact_index = compiler_suite_artifact_index(output, &receipt.intent.target)?;
    compiler_suite_direct_target_shard_from_catalog(
        &compiler_root,
        receipt,
        graph,
        root_index,
        &artifact_index,
        &catalog,
    )
}

/// Convert one publisher-only unit-graph root using a sealed third-party foundation artifact catalog.
///
/// No compiler-workspace Cargo target is needed: workspace libraries and proc macros remain source DAG nodes, while
/// this catalog contributes only external crate artifacts selected by the unit graph.
pub fn compiler_suite_direct_target_shard_from_catalog(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    root_index: usize,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<(OvenCompilerTestSuiteShardPayload, Vec<OvenArtifactMaterializedFile>), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let root = graph.units.get(root_index).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is outside its unit list"
        ))
    })?;
    if !matches!(root.mode.as_str(), "test" | "doctest") || !compiler_suite_unit_is_in_workspace(&compiler_root, root)?
    {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is not a supported workspace test root"
        )));
    }
    let target = compiler_suite_target_from_unit(&compiler_root, receipt, root, graph, artifact_index, catalog)?;
    let binary_targets = compiler_suite_binary_targets_for_roots(
        &compiler_root,
        receipt,
        graph,
        &[root_index],
        artifact_index,
        catalog,
    )?;
    let workspace_libraries = compiler_suite_workspace_libraries_for_roots(
        &compiler_root,
        receipt,
        graph,
        &[root_index],
        artifact_index,
        catalog,
    )?;
    Ok((
        OvenCompilerTestSuiteShardPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
            target,
            binary_targets,
            workspace_libraries,
            foundation_references: Vec::new(),
            artifact_closure: catalog.closure.clone(),
        },
        catalog.materialized_files.clone(),
    ))
}

/// Plan the non-CLI workspace binary targets Cargo exposes to integration roots through `CARGO_BIN_EXE_*`.
///
/// Cargo's unit graph represents those binaries as dependencies, but they are not Rust `--extern` artifacts. Oven
/// instead compiles the declared binary source directly into caller-owned output, then restores its exact path only
/// for the target that named it. The main `incan` binary is handled by the dedicated suite CLI plan because test
/// children use that same executable to exercise the compiler command surface.
#[cfg(test)]
pub fn compiler_suite_binary_targets(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<Vec<OvenCompilerTestSuiteTarget>, OvenLegacyCargoError> {
    compiler_suite_binary_targets_for_roots(compiler_root, receipt, graph, &graph.roots, artifact_index, catalog)
}

/// Plan only the caller-selected test roots' `CARGO_BIN_EXE_*` dependencies.
///
/// A shard may not inherit workspace binaries from unrelated roots merely because a prior publisher selection warmed
/// the same transient target. Isolated per-selection materialization passes one root index here.
pub fn compiler_suite_binary_targets_for_roots(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    root_indices: &[usize],
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<Vec<OvenCompilerTestSuiteTarget>, OvenLegacyCargoError> {
    let mut binary_indices = BTreeSet::new();
    for index in root_indices {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite test unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !matches!(unit.mode.as_str(), "test" | "doctest")
            || !compiler_suite_unit_is_in_workspace(compiler_root, unit)?
        {
            continue;
        }
        for dependency in &unit.dependencies {
            let binary = graph.units.get(dependency.index).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite binary dependency index {} is outside its unit list",
                    dependency.index
                ))
            })?;
            if binary.target.kind.iter().any(|kind| kind == "bin")
                && compiler_suite_unit_is_in_workspace(compiler_root, binary)?
            {
                if binary.mode != "build" {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "compiler-suite binary dependency `{}` has unsupported Cargo mode `{}`",
                        binary.target.name, binary.mode
                    )));
                }
                if binary.target.name != "incan" {
                    binary_indices.insert(dependency.index);
                }
            }
        }
    }

    let mut targets = BTreeMap::new();
    for index in binary_indices {
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite binary dependency index {index} is outside its unit list"
            ))
        })?;
        let target = compiler_suite_target_from_unit(compiler_root, receipt, unit, graph, artifact_index, catalog)?;
        if target.runner != "rustc-run" || target.target_kind != "bin" {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler-suite binary dependency `{}` did not produce a direct-rustc run plan",
                target.target_name
            )));
        }
        match targets.insert(target.target_name.clone(), target.clone()) {
            Some(previous) if previous == target => {}
            Some(previous) => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "compiler-suite binary dependency name `{}` resolves to conflicting sources {} and {}",
                    target.target_name, previous.source_relative_path, target.source_relative_path
                )));
            }
            None => {}
        }
    }
    Ok(targets.into_values().collect())
}

/// Return whether a Cargo unit source belongs to this receipt-authorized workspace rather than a registry dependency.
pub fn compiler_suite_unit_is_in_workspace(
    compiler_root: &Path,
    unit: &CargoUnitGraphUnit,
) -> Result<bool, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = fs::canonicalize(&unit.target.src_path).map_err(|source| OvenLegacyCargoError::Io {
        path: unit.target.src_path.clone(),
        source,
    })?;
    Ok(source.starts_with(compiler_root))
}
