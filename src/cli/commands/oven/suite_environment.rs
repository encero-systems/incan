//! The environment, scratch and fixtures one compiler-suite child runs under.
//!
//! A suite child is a libtest or rustdoc root executed through Oven's native runner with a bounded, receipt-derived
//! environment: no ambient Cargo, an explicit-bake Cargo proxy for the fixtures that publish, and a generated-Rust
//! closure only for the targets that consume it. The runner that uses them is `suite_execution`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    CliError, CliResult, CompilerSuiteFoundationExecution, CompilerSuiteShardExecution, LoafTemporaryDirectory,
    OVEN_COMPILER_SUITE_CAPABILITY_ENV, OVEN_COMPILER_SUITE_EXPLICIT_BAKE_CARGO_ENV,
    OVEN_COMPILER_SUITE_EXPLICIT_BAKE_HOME_ENV, OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV,
    OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV, OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV,
    OVEN_COMPILER_SUITE_RUSTC_ENV, OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV, OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
    OvenBuildIntent, OvenCallerOwnedRustcLibrary, OvenCompilerSuiteCapability, OvenCompilerSuiteTargetCapabilities,
    OvenCompilerTestSuiteFoundationReference, OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey,
    OvenReceipt, OvenRustcArtifactPlan, OvenTrustedDirectRustcTargetRequest, PhaseProgress, PreparedCompilerSuiteChild,
    attach_caller_owned_rustc_libraries, attach_compiler_suite_target_workspace_libraries,
    bake_planned_compiler_suite_workspace_libraries, bake_trusted_direct_rustc_run,
    compiler_suite_artifact_closure_cache_identity, compiler_suite_child_state_root,
    compiler_suite_composed_artifact_plan, compiler_suite_dynamic_library_environment, compiler_suite_target_cache_key,
    compiler_suite_target_output_name, compiler_suite_target_source, compiler_suite_workspace_outputs_include_dylib,
    default_rustup_home, env, oven_error, rustc_dynamic_library_environment, user_home,
};

/// Pin compiler-suite fixture children to the source checkout authorized by the current receipt.
///
/// Tests invoke the stored CLI to prepare local fixture providers. Inheriting `INCAN_SOURCE_ROOT` from an unrelated
/// developer checkout silently mixes compiler sources and makes an otherwise valid suite non-reproducible.
#[cfg(test)]
pub(crate) fn compiler_suite_environment(
    compiler_root: &Path,
    sdk_inventory: &Path,
    rustc: &Path,
    warning_check_artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
    compiler_data_root: Option<&Path>,
    output_directory: &Path,
) -> CliResult<BTreeMap<String, String>> {
    compiler_suite_environment_with_vocab(
        compiler_root,
        sdk_inventory,
        rustc,
        warning_check_artifacts,
        warning_check_artifacts,
        compiler_data_root,
        output_directory,
    )
}

/// Construct fixture-child state with distinct direct-Rustc closures for generated code and vocab extraction.
pub(crate) fn compiler_suite_environment_with_vocab(
    compiler_root: &Path,
    sdk_inventory: &Path,
    rustc: &Path,
    warning_check_artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
    vocab_extraction_artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
    compiler_data_root: Option<&Path>,
    output_directory: &Path,
) -> CliResult<BTreeMap<String, String>> {
    let compiler_root = fs::canonicalize(compiler_root).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite root {}: {error}",
            compiler_root.display()
        ))
    })?;
    let stdlib_root = compiler_root.join("loaves/stdlib");
    if !stdlib_root.is_dir() {
        return Err(CliError::failure(format!(
            "compiler-suite root {} has no stdlib directory {}",
            compiler_root.display(),
            stdlib_root.display()
        )));
    }
    let sdk_inventory = fs::canonicalize(sdk_inventory).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite SDK provider inventory {}: {error}",
            sdk_inventory.display()
        ))
    })?;
    if !sdk_inventory.is_file() {
        return Err(CliError::failure(format!(
            "compiler-suite SDK provider inventory {} is not a regular file",
            sdk_inventory.display()
        )));
    }
    let sdk_provider_root = sdk_inventory.parent().ok_or_else(|| {
        CliError::failure(format!(
            "compiler-suite SDK provider inventory {} has no provider root",
            sdk_inventory.display()
        ))
    })?;
    let output_directory = compiler_suite_environment_path(output_directory)?;
    let compiler_data_root = compiler_data_root.map(compiler_suite_environment_path).transpose()?;
    let runtime_root = sdk_inventory
        .parent()
        .map(|parent| parent.join("runtime"))
        .filter(|root| root.join("Cargo.lock").is_file())
        .ok_or_else(|| {
            CliError::failure(format!(
                "compiler-suite SDK provider inventory {} has no sealed runtime closure",
                sdk_inventory.display()
            ))
        })?;
    let stdlib_extern = warning_check_artifacts
        .externs
        .iter()
        .find_map(|(crate_name, path)| (crate_name == "incan_stdlib").then_some(path))
        .ok_or_else(|| {
            CliError::failure(
                "stored compiler-suite direct-rustc closure has no `incan_stdlib` extern for generated-code checks",
            )
        })?;
    let stdlib_extern = compiler_suite_environment_path(stdlib_extern)?;
    let rustup_home = default_rustup_home(env::var_os("RUSTUP_HOME"), user_home());
    // Store identities contain `sha256:`. On Unix, `:` is the path-list separator, so joining these verified
    // absolute paths into one environment variable would either be ambiguous or rejected. Transport each opaque
    // direct-rustc search path in its own environment value instead.
    let mut environment = BTreeMap::from([
        // Insta otherwise discovers its workspace by launching `cargo metadata`. The receipt already authorizes
        // this canonical compiler root, so make snapshot resolution direct and Cargo-free for stored suite children.
        ("INSTA_WORKSPACE_ROOT".to_string(), compiler_root.display().to_string()),
        ("INCAN_SOURCE_ROOT".to_string(), compiler_root.display().to_string()),
        ("INCAN_STDLIB".to_string(), stdlib_root.display().to_string()),
        ("INCAN_SDK_INVENTORY".to_string(), sdk_inventory.display().to_string()),
        (
            "INCAN_INTERNAL_SDK_PROVIDER_STORE".to_string(),
            sdk_provider_root.display().to_string(),
        ),
        // Provider discovery may intentionally clear INCAN_SDK_INVENTORY in a cold-store fixture. Keep runtime
        // source identity separately receipt-bound, so that action cannot make the same native closure appear to
        // belong to an ambient checkout.
        (
            "INCAN_INTERNAL_OVEN_RUNTIME_ROOT".to_string(),
            runtime_root.display().to_string(),
        ),
        // Stored-suite children may need a managed Oven store, but that state belongs to the suite invocation. Never
        // allow a scheduler run to silently write under the developer's default `~/.incan` home.
        (
            "INCAN_HOME".to_string(),
            output_directory.join("incan-home").display().to_string(),
        ),
        // A fixture may intentionally clear INCAN_HOME while testing the default command path. Keep that default
        // inside the scheduler-owned output as well: inherited developer HOME state must neither make a stored suite
        // non-reproducible nor send normal nested Oven commands outside the active invocation's bounded policy.
        ("HOME".to_string(), output_directory.join("home").display().to_string()),
        // The scheduler has already validated this exact direct-rustc executable. Propagate it explicitly so the
        // isolated suite home cannot make a nested normal command ask rustup to rediscover a developer toolchain.
        // This selects the compiler; it does not authorize Cargo.
        (
            "RUSTC".to_string(),
            compiler_suite_environment_path(rustc)?.display().to_string(),
        ),
        // Some compiler self-tests deliberately clear RUSTC to exercise Rustup's compiler discovery. Keep only the
        // parent toolchain-manager state available for that test behavior; normal child commands use RUSTC above,
        // and this does not expose or authorize Cargo state.
        (
            "RUSTUP_HOME".to_string(),
            rustup_home.map_or_else(String::new, |root| root.display().to_string()),
        ),
        // The parent scheduler derives this only from its active installed toolchain. An empty value deliberately
        // clears any inherited override when the parent is a development binary without package data.
        (
            "INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT".to_string(),
            compiler_data_root
                .as_ref()
                .map_or_else(String::new, |root| root.display().to_string()),
        ),
        // This is an internal scheduler capability, not a user-selectable artifact path. It authorizes nested normal
        // commands to consume the parent-leased, read-only Loaf directly instead of copying its closure into
        // every fixture's small mutable Oven home.
        ("INCAN_INTERNAL_OVEN_LOAF_EXECUTION".to_string(), "1".to_string()),
        (OVEN_COMPILER_SUITE_RUSTC_ENV.to_string(), rustc.display().to_string()),
    ]);
    let warning_capability = OvenCompilerSuiteCapability::new(
        compiler_suite_environment_path(rustc)?,
        warning_check_artifacts
            .dependency_search_paths
            .iter()
            .map(|path| compiler_suite_environment_path(path))
            .collect::<CliResult<Vec<_>>>()?,
        warning_check_artifacts
            .externs
            .iter()
            .map(|(crate_name, path)| Ok((crate_name.clone(), compiler_suite_environment_path(path)?)))
            .collect::<CliResult<BTreeMap<_, _>>>()?,
    );
    if warning_capability.externs.get("incan_stdlib") != Some(&compiler_suite_environment_path(&stdlib_extern)?) {
        return Err(CliError::failure(
            "compiler-suite warning capability does not bind its selected incan_stdlib artifact",
        ));
    }
    environment.insert(
        OVEN_COMPILER_SUITE_CAPABILITY_ENV.to_string(),
        warning_capability.encode().map_err(CliError::failure)?,
    );
    // The suite-built CLI is a caller-owned direct-Rustc output. A root can launch that CLI through
    // `CARGO_BIN_EXE_incan` even when the root itself has no dynamic workspace-library dependency, so exporting
    // this receipt-selected loader path only for `prefer_dynamic` roots leaves those nested launches broken.
    // Keep the toolchain path in the base child environment: native test execution and every child it starts then
    // inherit the exact selected standard-library closure without trusting a Cargo-provided loader environment.
    let (dynamic_library_environment_name, dynamic_library_environment_value) =
        rustc_dynamic_library_environment(rustc).map_err(oven_error)?;
    environment.insert(dynamic_library_environment_name, dynamic_library_environment_value);
    append_compiler_suite_direct_rustc_environment(
        &mut environment,
        OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV,
        rustc,
        vocab_extraction_artifacts,
    )?;
    Ok(environment)
}

/// Create short-lived scratch space for compiler-suite fixture tools.
///
/// The v0.5 suite runs on Unix hosts. `/tmp` keeps rust-analyzer's nested Cargo lockfile paths below platform limits
/// even when the caller's worktree or selected suite output has a long absolute path. The guard object remains live
/// for the entire scheduler invocation, so children cannot observe a reclaimed directory.
#[cfg(unix)]
pub(crate) fn compiler_suite_temporary_directory() -> CliResult<LoafTemporaryDirectory> {
    LoafTemporaryDirectory::create(Path::new("/tmp"), ".incan-oven-suite-").map_err(|error| {
        CliError::failure(format!(
            "cannot create short compiler-suite temporary directory: {error}"
        ))
    })
}

/// Create invocation-owned scratch space on platforms outside the supported v0.5 compiler-suite hosts.
#[cfg(not(unix))]
pub(crate) fn compiler_suite_temporary_directory() -> CliResult<LoafTemporaryDirectory> {
    LoafTemporaryDirectory::create(&env::temp_dir(), ".incan-oven-suite-")
        .map_err(|error| CliError::failure(format!("cannot create compiler-suite temporary directory: {error}")))
}

/// Execute prepared children and reclaim their scratch before returning either success or failure.
///
/// Deletion is measured and observable instead of happening silently after the report. If execution and cleanup
/// both fail, preserve both explanations; a cleanup error alone cannot erase a child's primary failure.
pub(crate) fn run_compiler_suite_with_scratch<T>(
    directory: LoafTemporaryDirectory,
    run: impl FnOnce() -> CliResult<T>,
) -> CliResult<(T, u128)> {
    let result = run();
    let subject = format!("compiler-suite scratch cleanup {}", directory.path().display());
    let phase = PhaseProgress::start(&subject);
    let cleanup = directory.close();
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok((value, phase.finish())),
        (Err(error), Ok(())) => {
            phase.finish();
            Err(error)
        }
        (Ok(_), Err(error)) => Err(CliError::failure(format!("{subject} failed: {error}"))),
        (Err(error), Err(cleanup_error)) => Err(CliError::failure(format!(
            "{error}\n{subject} also failed: {cleanup_error}"
        ))),
    }
}

/// Convert a scheduler-selected path into an absolute environment value before a test changes directory.
///
/// Stored suite entries may be selected from a relative `--store` path, while compiler tests deliberately create
/// nested fixture directories. Forwarding those paths verbatim would make a receipt-authorized inventory or native
/// Loaf disappear from a nested `incan` process despite still being held by the parent suite lease.
pub(crate) fn compiler_suite_environment_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| {
            CliError::failure(format!(
                "cannot resolve compiler-suite environment path {} from the current directory: {error}",
                path.display()
            ))
        })
}

/// Export a second, named direct-Rustc closure for compiler-internal vocab companion extraction.
///
/// The generated-code warning checker and the compiler CLI can legitimately use different dependency closures. Keep
/// them separate instead of merging same-named Rust crates from independent receipt-bound foundations into an
/// ambiguous `--extern` list.
pub(crate) fn append_compiler_suite_direct_rustc_environment(
    environment: &mut BTreeMap<String, String>,
    variable: &str,
    rustc: &Path,
    artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
) -> CliResult<()> {
    let capability = OvenCompilerSuiteCapability::new(
        compiler_suite_environment_path(rustc)?,
        artifacts
            .dependency_search_paths
            .iter()
            .map(|path| compiler_suite_environment_path(path))
            .collect::<CliResult<Vec<_>>>()?,
        artifacts
            .externs
            .iter()
            .map(|(crate_name, path)| Ok((crate_name.clone(), compiler_suite_environment_path(path)?)))
            .collect::<CliResult<BTreeMap<_, _>>>()?,
    );
    environment.insert(variable.to_string(), capability.encode().map_err(CliError::failure)?);
    Ok(())
}

/// Place the suite-built CLI beside its caller-owned workspace libraries.
///
/// Direct CLI builds can prefer dynamic workspace libraries. A fixed compiler-root target path would outlive the
/// caller output and become unusable when the scheduler responsibly reclaims an interrupted run's artifacts.
pub(crate) fn compiler_suite_cli_output(output_directory: &Path) -> PathBuf {
    output_directory.join("compiler-cli/incan")
}

/// Select one workspace library and only the declared workspace libraries it transitively requires.
///
/// A compiler-suite shard can contain the full workspace DAG needed by its test root, while a generated-code warning
/// check consumes only `incan_stdlib`. Rebuilding unrelated compiler, LSP, or inspection crates merely to obtain
/// that one checked input repeats expensive work without strengthening the warning-check contract.
pub(crate) fn compiler_suite_workspace_library_dependency_closure(
    libraries: &[OvenCompilerWorkspaceLibrary],
    root: &OvenCompilerWorkspaceLibraryKey,
) -> CliResult<Vec<OvenCompilerWorkspaceLibrary>> {
    let mut declared = BTreeMap::new();
    for library in libraries {
        if declared.insert(library.key.clone(), library).is_some() {
            return Err(CliError::failure(format!(
                "stored compiler-suite workspace library `{}` is declared more than once",
                library.key.crate_name
            )));
        }
    }
    let mut selected = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(key) = pending.pop() {
        if !selected.insert(key.clone()) {
            continue;
        }
        let library = declared.get(&key).ok_or_else(|| {
            CliError::failure(format!(
                "stored compiler-suite workspace library closure is missing `{}`",
                key.crate_name
            ))
        })?;
        pending.extend(library.dependencies.iter().cloned());
    }
    Ok(declared
        .into_iter()
        .filter_map(|(key, library)| selected.contains(&key).then(|| library.clone()))
        .collect())
}

/// Rebuild the generated-code warning check's `incan_stdlib` input from a receipt-bound workspace-library shard.
///
/// Schema 12 replaces the former second Cargo target with this caller-owned direct-Rustc bake. The selected shard
/// and every foundation remain leased for the complete suite command, so this plan cannot fall back to a Cargo
/// target or an ambient compiler cache after publication.
pub(crate) fn bake_compiler_suite_warning_check_artifacts(
    shards: &[CompilerSuiteShardExecution],
    receipt: &OvenReceipt,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    foundations: &BTreeMap<String, CompilerSuiteFoundationExecution>,
    workspace_library_cache: &mut BTreeMap<String, OvenCallerOwnedRustcLibrary>,
) -> CliResult<OvenRustcArtifactPlan> {
    let mut selected: Option<(&CompilerSuiteShardExecution, &OvenCompilerWorkspaceLibrary)> = None;
    for shard in shards {
        for library in &shard.payload.workspace_libraries {
            if library.key.package_name != "incan_stdlib"
                || library.key.crate_name != "incan_stdlib"
                || library.key.target_kind != "lib"
                || library.key.source_relative_path != "crates/incan_stdlib/src/lib.rs"
            {
                continue;
            }
            match selected {
                Some((_, previous)) if previous.key != library.key => {
                    return Err(CliError::failure(
                        "schema-12 compiler-suite shards disagree on the source or feature identity of their direct-Rustc `incan_stdlib` warning-check plan",
                    ));
                }
                Some(_) => {}
                None => selected = Some((shard, library)),
            }
        }
    }
    let (shard, library) = selected.ok_or_else(|| {
        CliError::failure(
            "schema-12 compiler suite has no receipt-bound `incan_stdlib` workspace library for generated-code checks",
        )
    })?;
    let warning_check_libraries =
        compiler_suite_workspace_library_dependency_closure(&shard.payload.workspace_libraries, &library.key)?;
    let workspace_outputs = bake_planned_compiler_suite_workspace_libraries(
        &warning_check_libraries,
        &shard.payload.artifact_closure,
        &shard.stored.manifest.intent,
        receipt,
        &shard.stored.artifact_root,
        rustc,
        compiler_root,
        &output_directory.join("warning-check"),
        &shard.payload.foundation_references,
        Some(foundations),
        workspace_library_cache,
    )?;
    let artifacts = shard
        .payload
        .artifact_closure
        .manifest_for_workspace_library(library, shard.stored.manifest.intent.clone());
    let mut artifact_plan = compiler_suite_composed_artifact_plan(
        &artifacts,
        &shard.payload.foundation_references,
        foundations,
        &shard.stored.manifest.intent,
    )?;
    let dependencies = library
        .dependencies
        .iter()
        .map(|dependency| {
            workspace_outputs.get(dependency).cloned().ok_or_else(|| {
                CliError::failure(format!(
                    "schema-12 generated-code warning check requires missing workspace library `{}`",
                    dependency.crate_name
                ))
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, &dependencies).map_err(oven_error)?;
    let stdlib = workspace_outputs.get(&library.key).cloned().ok_or_else(|| {
        CliError::failure(
            "schema-12 generated-code warning check did not materialize its `incan_stdlib` workspace library",
        )
    })?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, std::slice::from_ref(&stdlib)).map_err(oven_error)?;
    Ok(artifact_plan)
}

/// Compile every receipt-bound workspace binary that Cargo supplied to integration targets through
/// `CARGO_BIN_EXE_*`. The outputs are caller-owned and are injected only into the declared direct-rustc targets;
/// no Cargo-produced binary is retained or executed from the immutable Oven entry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn bake_planned_compiler_suite_binaries(
    targets: &[crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget],
    closure: &crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &OvenBuildIntent,
    receipt: &OvenReceipt,
    artifact_root: &Path,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    cli_output: &Path,
    workspace_libraries: &[OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&BTreeMap<String, CompilerSuiteFoundationExecution>>,
    binary_cache: &mut BTreeMap<String, PathBuf>,
) -> CliResult<BTreeMap<String, PathBuf>> {
    let mut outputs = BTreeMap::from([("incan".to_string(), cli_output.to_path_buf())]);
    if targets.is_empty() {
        return Ok(outputs);
    }
    let closure_identity = compiler_suite_artifact_closure_cache_identity(closure)?;
    for (index, target) in targets.iter().enumerate() {
        if target.runner != "rustc-run" || target.target_kind != "bin" {
            return Err(CliError::failure(format!(
                "stored compiler-suite binary target `{}` must use the direct-rustc binary executor",
                target.target_name
            )));
        }
        if target.target_name == "incan" {
            return Err(CliError::failure(
                "stored compiler-suite binary targets must not duplicate the dedicated `incan` CLI target".to_string(),
            ));
        }
        let cache_key = compiler_suite_target_cache_key(
            receipt,
            target,
            &closure_identity,
            foundation_references,
            workspace_library_outputs,
        )?;
        if let Some(cached) = binary_cache.get(&cache_key) {
            if outputs.insert(target.target_name.clone(), cached.clone()).is_some() {
                return Err(CliError::failure(format!(
                    "stored compiler-suite binary target `{}` is declared more than once",
                    target.target_name
                )));
            }
            continue;
        }
        let artifacts = closure.manifest_for_target(target, intent.clone());
        let mut artifact_plan = match foundations {
            Some(foundations) => {
                compiler_suite_composed_artifact_plan(&artifacts, foundation_references, foundations, intent)?
            }
            None => artifacts
                .materialize_trusted_store(artifact_root, intent)
                .map_err(oven_error)?,
        };
        attach_compiler_suite_target_workspace_libraries(
            &mut artifact_plan,
            target,
            workspace_libraries,
            workspace_library_outputs,
        )?;
        let source = compiler_suite_target_source(compiler_root, target)?;
        let output = output_directory
            .join("binaries")
            .join(compiler_suite_target_output_name(index, target));
        let bake = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt,
            artifacts: &artifacts,
            artifact_root,
            artifact_plan: Some(&artifact_plan),
            rustc,
            source: &source,
            output: &output,
            crate_name: &target.crate_name,
            edition: &target.edition,
            source_evidence_key: &target.source_evidence_key,
            features: &target.features,
            prefer_dynamic: compiler_suite_workspace_outputs_include_dylib(workspace_library_outputs),
        })
        .map_err(oven_error)?;
        binary_cache.insert(cache_key, bake.output.clone());
        if outputs.insert(target.target_name.clone(), bake.output).is_some() {
            return Err(CliError::failure(format!(
                "stored compiler-suite binary target `{}` is declared more than once",
                target.target_name
            )));
        }
    }
    Ok(outputs)
}

/// Suite-owned proxy for the explicit Cargo capability used by compatibility tests.
pub(crate) struct CompilerSuiteFixtureCargoProxy {
    pub(crate) executable: PathBuf,
    pub(crate) real: PathBuf,
    /// Fallback compiler only when the named publisher did not select one from its receipt.
    pub(crate) real_rustc: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) log: PathBuf,
}

impl CompilerSuiteFixtureCargoProxy {
    /// Count the invocations appended by the proxy after all suite workers have joined.
    pub(crate) fn invocation_count(&self) -> CliResult<usize> {
        fs::read_to_string(&self.log)
            .map(|payload| payload.lines().count())
            .map_err(|error| {
                CliError::failure(format!(
                    "cannot read compiler-suite fixture Cargo log {}: {error}",
                    self.log.display()
                ))
            })
    }
}

/// Create a transient logged Cargo proxy inside caller-owned suite output.
#[cfg(unix)]
pub(crate) fn prepare_compiler_suite_fixture_cargo_proxy(
    output_directory: &Path,
    cargo: &Path,
) -> CliResult<CompilerSuiteFixtureCargoProxy> {
    use std::os::unix::fs::PermissionsExt;

    let real = fs::canonicalize(cargo).map_err(|error| {
        CliError::failure(format!(
            "cannot resolve explicit compiler-suite fixture Cargo {}: {error}",
            cargo.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(&real).map_err(|error| {
        CliError::failure(format!(
            "cannot inspect explicit compiler-suite fixture Cargo {}: {error}",
            real.display()
        ))
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(CliError::failure(format!(
            "explicit compiler-suite fixture Cargo must be an executable regular file: {}",
            real.display()
        )));
    }
    let real_rustc = real
        .parent()
        .map(|directory| directory.join("rustc"))
        .ok_or_else(|| CliError::failure("explicit compiler-suite fixture Cargo has no toolchain directory"))?;
    let rustc_metadata = fs::symlink_metadata(&real_rustc).map_err(|error| {
        CliError::failure(format!(
            "cannot inspect matching compiler-suite fixture Rustc {}: {error}",
            real_rustc.display()
        ))
    })?;
    if !rustc_metadata.is_file() || rustc_metadata.permissions().mode() & 0o111 == 0 {
        return Err(CliError::failure(format!(
            "explicit compiler-suite fixture Cargo requires its matching executable Rustc: {}",
            real_rustc.display()
        )));
    }
    let home = user_home()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .ok_or_else(|| {
            CliError::failure(
                "explicit compiler-suite fixture Cargo requires a readable HOME for its offline source cache",
            )
        })?;
    let root = output_directory.join("fixture-cargo");
    fs::create_dir_all(&root).map_err(|error| {
        CliError::failure(format!(
            "cannot create compiler-suite fixture Cargo proxy directory {}: {error}",
            root.display()
        ))
    })?;
    let executable = root.join("cargo");
    let log = root.join("invocations.log");
    let proxy = concat!(
        "#!/bin/sh\n",
        "printf '%s\\n' \"$*\" >> \"$INCAN_INTERNAL_OVEN_FIXTURE_CARGO_LOG\"\n",
        "if [ -z \"${RUSTC:-}\" ]; then export RUSTC=\"$INCAN_INTERNAL_OVEN_FIXTURE_RUSTC_REAL\"; fi\n",
        "exec \"$INCAN_INTERNAL_OVEN_FIXTURE_CARGO_REAL\" \"$@\"\n",
    );
    fs::write(&executable, proxy).map_err(|error| {
        CliError::failure(format!(
            "cannot write compiler-suite fixture Cargo proxy {}: {error}",
            executable.display()
        ))
    })?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o500)).map_err(|error| {
        CliError::failure(format!(
            "cannot make compiler-suite fixture Cargo proxy executable {}: {error}",
            executable.display()
        ))
    })?;
    fs::write(&log, []).map_err(|error| {
        CliError::failure(format!(
            "cannot initialize compiler-suite fixture Cargo log {}: {error}",
            log.display()
        ))
    })?;
    Ok(CompilerSuiteFixtureCargoProxy {
        executable: compiler_suite_environment_path(&executable)?,
        real,
        real_rustc,
        home: compiler_suite_environment_path(&home)?,
        log: compiler_suite_environment_path(&log)?,
    })
}

/// The v0.5 compiler suite is supported on the Unix hosts used by release CI.
#[cfg(not(unix))]
pub(crate) fn prepare_compiler_suite_fixture_cargo_proxy(
    _output_directory: &Path,
    _cargo: &Path,
) -> CliResult<CompilerSuiteFixtureCargoProxy> {
    Err(CliError::failure(
        "the compiler-suite compatibility Cargo fixture proxy is supported only on Unix hosts".to_string(),
    ))
}

/// Resolve every mutable input for one compiler-suite child before the bounded parallel execution phase starts.
///
/// This phase runs after the suite has acquired every shard/foundation lease and after shared workspace libraries and
/// helper binaries are materialized once. A prepared child owns its output/environment state and borrows only the
/// already-leased immutable inputs. Each worker reconstitutes its own target manifest, so the scheduler does not clone
/// the complete shared artifact closure serially before the bounded parallel execution phase starts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_compiler_suite_child<'a>(
    target: &'a crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    closure: &'a crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &'a OvenBuildIntent,
    artifact_root: &'a Path,
    rustc: &Path,
    compiler_root: &'a Path,
    output_directory: &Path,
    environment: &BTreeMap<String, String>,
    binary_outputs: &BTreeMap<String, PathBuf>,
    workspace_libraries: &'a [OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &'a [OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&'a BTreeMap<String, CompilerSuiteFoundationExecution>>,
    fixture_cargo: Option<&CompilerSuiteFixtureCargoProxy>,
    case_slice: Option<(usize, usize)>,
) -> CliResult<PreparedCompilerSuiteChild<'a>> {
    let source = compiler_suite_target_source(compiler_root, target)?;
    let output = output_directory.join(compiler_suite_target_output_name(0, target));
    let prefer_dynamic = target.target_kind == "proc-macro"
        || compiler_suite_workspace_outputs_include_dylib(&workspace_library_outputs);
    let mut target_environment = environment.clone();
    let mut binary_compile_environment = BTreeMap::new();
    let child_state_root = compiler_suite_child_state_root(output_directory, target);
    // A parallel child may itself invoke `incan`. Give that nested command an isolated mutable home while it reads
    // the shared, leased provider store through the receipt-bound environment above.
    target_environment.insert(
        "INCAN_HOME".to_string(),
        compiler_suite_environment_path(&child_state_root.join("incan-home"))?
            .display()
            .to_string(),
    );
    // Tests that intentionally clear `INCAN_HOME` must still resolve their default state below the same child-owned
    // boundary rather than a sibling root's home. This remains separate from the parent-leased immutable closure.
    target_environment.insert(
        "HOME".to_string(),
        compiler_suite_environment_path(&child_state_root.join("home"))?
            .display()
            .to_string(),
    );
    // Nested normal commands may still use generated-project state for source and release-asset fixtures. Keep it
    // per child: parallel roots can otherwise race over mutable build-script, lock, and cleanup outputs.
    target_environment.insert(
        "INCAN_GENERATED_CARGO_TARGET_DIR".to_string(),
        compiler_suite_environment_path(&child_state_root.join("generated-cargo-target"))?
            .display()
            .to_string(),
    );
    // Apply the exceptional baker capability only after child-local state has been installed. The capability may
    // replace HOME with the caller-approved offline Cargo source cache; ordinary roots retain this child-owned home.
    apply_compiler_suite_target_capabilities(target, &mut target_environment, fixture_cargo)?;
    for dependency in &target.binary_dependencies {
        let output = binary_outputs.get(dependency).ok_or_else(|| {
            CliError::failure(format!(
                "stored compiler-suite target `{}` requires missing direct binary `{dependency}`",
                target.target_name
            ))
        })?;
        let name = format!("CARGO_BIN_EXE_{dependency}");
        let value = compiler_suite_environment_path(output)?.display().to_string();
        target_environment.insert(name.clone(), value.clone());
        binary_compile_environment.insert(name, value);
    }
    if prefer_dynamic {
        let (name, value) = compiler_suite_dynamic_library_environment(rustc, &workspace_library_outputs)?;
        target_environment.insert(name, value);
    }
    Ok(PreparedCompilerSuiteChild {
        target,
        closure,
        intent,
        artifact_root,
        compiler_root,
        source,
        output,
        environment: target_environment,
        prefer_dynamic,
        binary_compile_environment,
        workspace_libraries,
        workspace_library_outputs,
        foundation_references,
        foundations,
        case_slice,
    })
}

/// Return whether one indexed compiler-suite schema composes its direct-Rustc plan from leased foundations.
///
/// Schema 14 and later change only how children reach the compiler-owned standard-library Loaf family. They retain
/// the schema-10 foundation contract for Cargo-published third-party artifacts, so they must never bypass this path.
pub(crate) fn compiler_suite_uses_indexed_foundations(schema_version: u32) -> bool {
    matches!(schema_version, 10..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION)
}

/// Apply the package-qualified process capabilities owned by the compiler-suite registry.
pub(crate) fn apply_compiler_suite_target_capabilities(
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    environment: &mut BTreeMap<String, String>,
    fixture_cargo: Option<&CompilerSuiteFixtureCargoProxy>,
) -> CliResult<()> {
    let capabilities = OvenCompilerSuiteTargetCapabilities::for_target(
        &target.package_name,
        &target.target_kind,
        &target.source_relative_path,
    );
    if !capabilities.generated_rust_closure {
        compiler_suite_remove_generated_rust_closure(environment);
    }
    if capabilities.cargo_fixture {
        let fixture_cargo = fixture_cargo.ok_or_else(|| {
            CliError::failure(format!(
                "compiler-suite root `{}` exercises Cargo compatibility and requires explicit --fixture-cargo",
                target.source_relative_path
            ))
        })?;
        environment.insert("CARGO".to_string(), fixture_cargo.executable.display().to_string());
        environment.insert(
            OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV.to_string(),
            fixture_cargo.real.display().to_string(),
        );
        environment.insert(
            OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV.to_string(),
            fixture_cargo.real_rustc.display().to_string(),
        );
        environment.insert(
            OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV.to_string(),
            fixture_cargo.log.display().to_string(),
        );
        // Only a package-qualified explicit-bake root receives the caller's offline Cargo source cache. The baker
        // copies and digests the sources it uses; ordinary roots remain in their child-owned homes without Cargo.
        environment.insert("HOME".to_string(), fixture_cargo.home.display().to_string());
    }
    if capabilities.explicit_bake_cargo {
        let fixture_cargo = fixture_cargo.ok_or_else(|| {
            CliError::failure(format!(
                "compiler-suite root `{}` explicitly bakes a Loaf and requires --fixture-cargo",
                target.source_relative_path
            ))
        })?;
        // Do not set `CARGO` here. The test helper receives the three values below and installs them only on its
        // explicit `incan oven bake` child; all normal command probes retain the scheduler's Cargo guard.
        environment.insert(
            OVEN_COMPILER_SUITE_EXPLICIT_BAKE_CARGO_ENV.to_string(),
            fixture_cargo.real.display().to_string(),
        );
        environment.insert(
            OVEN_COMPILER_SUITE_EXPLICIT_BAKE_HOME_ENV.to_string(),
            fixture_cargo.home.display().to_string(),
        );
    }
    Ok(())
}

/// Remove direct generated-Rust closure details while retaining the suite marker used by Cargo-free fixture paths.
pub(crate) fn compiler_suite_remove_generated_rust_closure(environment: &mut BTreeMap<String, String>) {
    environment.retain(|key, _| !key.starts_with("INCAN_OVEN_COMPILER_SUITE_") || key == OVEN_COMPILER_SUITE_RUSTC_ENV);
}
