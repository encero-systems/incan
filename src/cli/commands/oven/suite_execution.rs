//! Running prepared compiler-suite children: worker pool, per-root execution, and the paths a root reads from.
//!
//! Moved verbatim out of `oven.rs`; selection and reporting live beside it.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use super::{
    CliError, CliResult, CompilerSuiteChildrenReport, CompilerSuiteFixtureCargoProxy, CompilerSuiteFoundationExecution,
    CompilerSuiteNativeTestRootReport, CompilerSuiteRustdocTestRootReport, Component, OVEN_COMPILER_TEST_JOBS_ENV,
    OVEN_COMPILER_TEST_ROOT_TIMEOUT, OvenBuildIntent, OvenCallerOwnedRustcLibrary,
    OvenCompilerTestSuiteFoundationReference, OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey,
    OvenNativeTestBatchReport, OvenNativeTestBatchRequest, OvenReceipt, OvenTrustedDirectRustcTargetRequest,
    OvenTrustedRustdocTestRequest, PreparedCompilerSuiteChild, announce_oven_progress,
    apply_compiler_suite_target_capabilities, attach_compiler_suite_target_workspace_libraries,
    bake_trusted_direct_rustc_test, compiler_suite_composed_artifact_plan, compiler_suite_dynamic_library_environment,
    compiler_suite_environment_path, compiler_suite_workspace_outputs_include_dylib, env, native_test_failure_summary,
    oven_error, resolve_compile_environment_value, run_native_test_batch_all_for_request,
    run_native_tests_exact_in_directory_with_timeout, run_trusted_rustdoc_test, write_native_test_transcript,
};

/// Compile and execute prepared compiler-suite roots with a bounded worker pool.
///
/// Workers start only after the one shared DAG/preparation phase has completed, so parallelism improves root
/// throughput without multiplying provider setup, Cargo-compatible publication, store leases, or generated homes.
pub(crate) fn run_prepared_compiler_suite_children(
    children: Vec<PreparedCompilerSuiteChild<'_>>,
    receipt: &OvenReceipt,
    rustc: &Path,
    exact_test_names: Option<&[String]>,
    measured_case_millis: &BTreeMap<String, BTreeMap<String, u64>>,
) -> CliResult<CompilerSuiteChildrenReport> {
    let child_count = children.len();
    if child_count == 0 {
        return Ok(CompilerSuiteChildrenReport::default());
    }
    let queue = Mutex::new(VecDeque::from(children));
    let results = Mutex::new(Vec::<Result<CompilerSuiteChildrenReport, String>>::with_capacity(
        child_count,
    ));
    let worker_count = compiler_suite_parallel_jobs(child_count);
    let logical_cores = thread::available_parallelism().map(usize::from).unwrap_or(1);
    let libtest_threads = compiler_suite_libtest_threads(logical_cores, worker_count);
    let panicked = thread::scope(|scope| {
        let workers = (0..worker_count)
            .map(|_| {
                scope.spawn(|| {
                    loop {
                        let child = match queue.lock() {
                            Ok(mut queue) => queue.pop_front(),
                            Err(_) => {
                                if let Ok(mut results) = results.lock() {
                                    results.push(Err("compiler-suite worker queue was poisoned".to_string()));
                                }
                                return;
                            }
                        };
                        let Some(child) = child else {
                            return;
                        };
                        let result = run_prepared_compiler_suite_child(
                            child,
                            receipt,
                            rustc,
                            libtest_threads,
                            exact_test_names,
                            measured_case_millis,
                        )
                        .map_err(|error| error.to_string());
                        if let Ok(mut results) = results.lock() {
                            results.push(result);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        workers.into_iter().any(|worker| worker.join().is_err())
    });
    if panicked {
        return Err(CliError::failure(
            "a bounded compiler-suite direct-Rustc worker panicked".to_string(),
        ));
    }
    let results = results
        .into_inner()
        .map_err(|_| CliError::failure("compiler-suite worker results were poisoned".to_string()))?;
    if results.len() != child_count {
        return Err(CliError::failure(format!(
            "compiler-suite worker pool produced {} result(s) for {child_count} prepared child(ren)",
            results.len()
        )));
    }
    let mut report = CompilerSuiteChildrenReport::default();
    for result in results {
        match result {
            Ok(child_report) => report.append(child_report),
            Err(error) => report
                .failed
                .push(format!("Oven direct-Rustc compiler-suite worker failed:\n{error}")),
        }
    }
    report.native_test_roots.sort_by(|left, right| {
        (
            left.package_name.as_str(),
            left.target_kind.as_str(),
            left.target_name.as_str(),
        )
            .cmp(&(
                right.package_name.as_str(),
                right.target_kind.as_str(),
                right.target_name.as_str(),
            ))
    });
    Ok(report)
}

/// Summarize one native root's terminal result for its announcement line.
///
/// Missing or invalid case evidence is reported separately from process success. A green exit cannot repair
/// incomplete selected-case accounting.
pub(crate) fn compiler_suite_root_outcome_detail(report: &OvenNativeTestBatchReport) -> String {
    let elapsed = format!("{:.1}s", report.timing.execution_elapsed_ms as f64 / 1_000.0);
    match &report.case_counts {
        Some(counts) => format!(
            "{} passed, {} failed, {} ignored in {elapsed}",
            counts.passed, counts.failed, counts.ignored
        ),
        None => format!(
            "incomplete case evidence in {elapsed} (process success: {})",
            report.process_success
        ),
    }
}

/// Execute one prepared direct-Rustc compiler-suite child with no Cargo process or store mutation.
pub(crate) fn run_prepared_compiler_suite_child(
    child: PreparedCompilerSuiteChild<'_>,
    receipt: &OvenReceipt,
    rustc: &Path,
    libtest_threads: usize,
    exact_test_names: Option<&[String]>,
    measured_case_millis: &BTreeMap<String, BTreeMap<String, u64>>,
) -> CliResult<CompilerSuiteChildrenReport> {
    let mut artifacts = child.closure.manifest_for_target(child.target, child.intent.clone());
    for (name, value) in &child.binary_compile_environment {
        artifacts.compile_environment.insert(name.clone(), value.clone());
    }
    let mut artifact_plan = match child.foundations {
        Some(foundations) => {
            compiler_suite_composed_artifact_plan(&artifacts, child.foundation_references, foundations, child.intent)?
        }
        None => artifacts
            .materialize_trusted_store(child.artifact_root, child.intent)
            .map_err(oven_error)?,
    };
    attach_compiler_suite_target_workspace_libraries(
        &mut artifact_plan,
        child.target,
        child.workspace_libraries,
        &child.workspace_library_outputs,
    )?;
    match child.target.runner.as_str() {
        "rustc-test" => {
            announce_oven_progress("COMPILE", &child.target.source_relative_path, None);
            let bake_started = Instant::now();
            let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
                receipt,
                artifacts: &artifacts,
                artifact_root: child.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc,
                source: &child.source,
                output: &child.output,
                crate_name: &child.target.crate_name,
                edition: &child.target.edition,
                source_evidence_key: &child.target.source_evidence_key,
                features: &child.target.features,
                prefer_dynamic: child.prefer_dynamic,
            })
            .map_err(oven_error)?;
            let direct_rustc_bake_elapsed_ms = bake_started.elapsed().as_millis();
            let working_directory =
                compiler_suite_target_working_directory(child.compiler_root, &child.source, child.target)?;
            let report = match exact_test_names {
                Some(exact_test_names) => run_native_tests_exact_in_directory_with_timeout(
                    &bake.output,
                    exact_test_names,
                    &child.environment,
                    Some(&working_directory),
                    Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                    Some(&child.target.source_relative_path),
                ),
                None => {
                    let empty = BTreeMap::new();
                    let root_case_millis = measured_case_millis
                        .get(&child.target.source_relative_path)
                        .unwrap_or(&empty);
                    run_native_test_batch_all_for_request(&OvenNativeTestBatchRequest {
                        executable: &bake.output,
                        environment: &child.environment,
                        working_directory: Some(&working_directory),
                        timeout: Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                        test_threads: Some(libtest_threads),
                        root_label: Some(&child.target.source_relative_path),
                        progress: None,
                        case_slice: child.case_slice.map(|(index, count)| {
                            crate::oven::native_test::OvenNativeTestCaseSlice {
                                index,
                                count,
                                measured_case_millis: root_case_millis,
                            }
                        }),
                    })
                }
            }
            .map_err(oven_error)?;
            announce_oven_progress(
                if report.success { "ROOT OK" } else { "ROOT FAIL" },
                &child.target.source_relative_path,
                Some(&compiler_suite_root_outcome_detail(&report)),
            );
            let transcript = write_native_test_transcript(&child.output, &report.output)?;
            let failures = if report.success {
                Vec::new()
            } else {
                vec![format!(
                    "{} target `{}` failed; full libtest transcript: {}\n{}",
                    child.target.target_kind,
                    child.target.target_name,
                    transcript.display(),
                    native_test_failure_summary(&report.output)
                )]
            };
            Ok(CompilerSuiteChildrenReport {
                native_test_count: if exact_test_names.is_some() || report.case_slice.is_some() {
                    report
                        .case_counts
                        .as_ref()
                        .map(|counts| counts.passed + counts.failed + counts.ignored)
                        .unwrap_or(0)
                } else {
                    report.inventory.names.len()
                },
                doctest_targets: 0,
                failed: failures,
                native_test_roots: vec![CompilerSuiteNativeTestRootReport {
                    package_name: child.target.package_name.clone(),
                    target_kind: child.target.target_kind.clone(),
                    target_name: child.target.target_name.clone(),
                    source_relative_path: child.target.source_relative_path.clone(),
                    inventory_count: report.inventory.names.len(),
                    success: report.success,
                    process_success: report.process_success,
                    case_counts: report.case_counts,
                    case_timings: report.case_timings,
                    command_timings: report.command_timings,
                    direct_rustc_bake_elapsed_ms,
                    libtest_inventory_elapsed_ms: report.timing.inventory_elapsed_ms,
                    libtest_execution_elapsed_ms: report.timing.execution_elapsed_ms,
                    case_slice: report.case_slice,
                }],
                rustdoc_test_roots: Vec::new(),
            })
        }
        "rustdoc-test" => {
            announce_oven_progress("DOCTEST", &child.target.source_relative_path, None);
            let temporary_directory = child.output.with_extension("rustdoc-tmp");
            let rustdoc_started = Instant::now();
            run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
                receipt,
                artifacts: &artifacts,
                artifact_root: child.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc,
                source: &child.source,
                temporary_directory: &temporary_directory,
                crate_name: &child.target.crate_name,
                edition: &child.target.edition,
                source_evidence_key: &child.target.source_evidence_key,
                features: &child.target.features,
                is_proc_macro: child.target.target_kind == "proc-macro",
                prefer_dynamic: child.prefer_dynamic,
                timeout: Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
            })
            .map_err(oven_error)?;
            announce_oven_progress(
                "DOCTEST OK",
                &child.target.source_relative_path,
                Some(&format!("{:.1}s", rustdoc_started.elapsed().as_secs_f64())),
            );
            Ok(CompilerSuiteChildrenReport {
                native_test_count: 0,
                doctest_targets: 1,
                failed: Vec::new(),
                native_test_roots: Vec::new(),
                rustdoc_test_roots: vec![CompilerSuiteRustdocTestRootReport {
                    package_name: child.target.package_name.clone(),
                    target_kind: child.target.target_kind.clone(),
                    target_name: child.target.target_name.clone(),
                    source_relative_path: child.target.source_relative_path.clone(),
                    execution_elapsed_ms: rustdoc_started.elapsed().as_millis(),
                }],
            })
        }
        runner => Err(CliError::failure(format!(
            "stored compiler-suite target `{}` declares unsupported Oven runner `{runner}`",
            child.target.target_name
        ))),
    }
}

/// Derive a root-worker count that leaves capacity for each root's libtest and nested Incan children.
///
/// Each direct-Rustc root is a substantial libtest binary that can run test cases in parallel and launch normal Incan
/// commands. On a constrained host, prefer that useful inner concurrency over making several large roots serial:
/// one core runs one root, 2--3 cores run one root with two libtest threads, and wider hosts add roots only when the
/// resulting root-worker and libtest-thread product remains within the host budget. An explicit operator override
/// remains available for measured release-machine tuning.
pub(crate) fn compiler_suite_auto_parallel_jobs(logical_cores: usize) -> usize {
    match logical_cores {
        0 | 1 => 1,
        2..=3 => 1,
        4..=5 => 2,
        6..=7 => 3,
        _ => 4,
    }
}

/// Bound direct compiler-suite root parallelism while allowing a release machine to select a measured safe value.
pub(crate) fn compiler_suite_parallel_jobs(child_count: usize) -> usize {
    let configured = env::var(OVEN_COMPILER_TEST_JOBS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0);
    let detected = thread::available_parallelism().map(usize::from).unwrap_or(1);
    configured
        .unwrap_or_else(|| compiler_suite_auto_parallel_jobs(detected))
        .min(child_count)
        .max(1)
}

/// Divide the detected host capacity among the concurrently scheduled libtest roots.
///
/// Root workers mainly wait for their libtest child, while that child can launch nested normal Incan commands. Keep
/// the aggregate libtest fan-out at or below the host count, and cap each root at two workers so a large machine
/// cannot recreate a wide nested-command burst merely because libtest observed many CPUs.
pub(crate) fn compiler_suite_libtest_threads(logical_cores: usize, root_workers: usize) -> usize {
    let logical_cores = logical_cores.max(1);
    let root_workers = root_workers.max(1);
    (logical_cores / root_workers).clamp(1, 2)
}

/// Split the host budget between bounded root workers and the libtest threads inside each root.
///
/// A root can launch nested normal Incan commands, so reserving one outer-worker share prevents the scheduler from
/// recreating the unbounded `outer roots × default libtest threads` fan-out that delays capacity and timeout guards.
/// Compile/inventory/execute every receipt-bound Rustc or Rustdoc workspace test target while the suite lease is
/// still held by the caller. No Cargo-linked test executable is copied or run from the immutable entry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_planned_compiler_suite_children(
    targets: &[crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget],
    closure: &crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &OvenBuildIntent,
    receipt: &OvenReceipt,
    artifact_root: &Path,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    environment: &BTreeMap<String, String>,
    binary_outputs: &BTreeMap<String, PathBuf>,
    workspace_libraries: &[OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&BTreeMap<String, CompilerSuiteFoundationExecution>>,
    fixture_cargo: Option<&CompilerSuiteFixtureCargoProxy>,
) -> CliResult<CompilerSuiteChildrenReport> {
    let mut suite_report = CompilerSuiteChildrenReport::default();
    for (index, target) in targets.iter().enumerate() {
        let mut artifacts = closure.manifest_for_target(target, intent.clone());
        let source = compiler_suite_target_source(compiler_root, target)?;
        let output = output_directory.join(compiler_suite_target_output_name(index, target));
        let prefer_dynamic = target.target_kind == "proc-macro"
            || compiler_suite_workspace_outputs_include_dylib(workspace_library_outputs);
        let mut target_environment = environment.clone();
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
            artifacts.compile_environment.insert(name.clone(), value.clone());
            target_environment.insert(name, value);
        }
        if prefer_dynamic {
            let (name, value) = compiler_suite_dynamic_library_environment(rustc, workspace_library_outputs)?;
            target_environment.insert(name, value);
        }
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
        match target.runner.as_str() {
            "rustc-test" => {
                let bake_started = Instant::now();
                let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
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
                    prefer_dynamic,
                })
                .map_err(oven_error)?;
                let direct_rustc_bake_elapsed_ms = bake_started.elapsed().as_millis();
                let working_directory = compiler_suite_target_working_directory(compiler_root, &source, target)?;
                let report = run_native_test_batch_all_for_request(&OvenNativeTestBatchRequest {
                    executable: &bake.output,
                    environment: &target_environment,
                    working_directory: Some(&working_directory),
                    timeout: Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                    test_threads: None,
                    root_label: Some(&target.source_relative_path),
                    progress: None,
                    case_slice: None,
                })
                .map_err(oven_error)?;
                suite_report.native_test_count += report.inventory.names.len();
                suite_report.native_test_roots.push(CompilerSuiteNativeTestRootReport {
                    package_name: target.package_name.clone(),
                    target_kind: target.target_kind.clone(),
                    target_name: target.target_name.clone(),
                    source_relative_path: target.source_relative_path.clone(),
                    inventory_count: report.inventory.names.len(),
                    success: report.success,
                    process_success: report.process_success,
                    case_counts: report.case_counts.clone(),
                    case_timings: report.case_timings.clone(),
                    command_timings: report.command_timings.clone(),
                    direct_rustc_bake_elapsed_ms,
                    libtest_inventory_elapsed_ms: report.timing.inventory_elapsed_ms,
                    libtest_execution_elapsed_ms: report.timing.execution_elapsed_ms,
                    case_slice: None,
                });
                let transcript = write_native_test_transcript(&output, &report.output)?;
                if !report.success {
                    suite_report.failed.push(format!(
                        "{} target `{}` failed; full libtest transcript: {}\n{}",
                        target.target_kind,
                        target.target_name,
                        transcript.display(),
                        native_test_failure_summary(&report.output)
                    ));
                }
            }
            "rustdoc-test" => {
                let temporary_directory = output.with_extension("rustdoc-tmp");
                let rustdoc_started = Instant::now();
                let _report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
                    receipt,
                    artifacts: &artifacts,
                    artifact_root,
                    artifact_plan: Some(&artifact_plan),
                    rustc,
                    source: &source,
                    temporary_directory: &temporary_directory,
                    crate_name: &target.crate_name,
                    edition: &target.edition,
                    source_evidence_key: &target.source_evidence_key,
                    features: &target.features,
                    is_proc_macro: target.target_kind == "proc-macro",
                    prefer_dynamic,
                    timeout: Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                })
                .map_err(oven_error)?;
                suite_report.doctest_targets += 1;
                suite_report
                    .rustdoc_test_roots
                    .push(CompilerSuiteRustdocTestRootReport {
                        package_name: target.package_name.clone(),
                        target_kind: target.target_kind.clone(),
                        target_name: target.target_name.clone(),
                        source_relative_path: target.source_relative_path.clone(),
                        execution_elapsed_ms: rustdoc_started.elapsed().as_millis(),
                    });
            }
            runner => {
                return Err(CliError::failure(format!(
                    "stored compiler-suite target `{}` declares unsupported Oven runner `{runner}`",
                    target.target_name
                )));
            }
        }
    }
    Ok(suite_report)
}

/// Resolve a target-plan source only beneath the current receipt-authorized compiler root.
pub(crate) fn compiler_suite_target_source(
    compiler_root: &Path,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> CliResult<PathBuf> {
    compiler_suite_source_path(
        compiler_root,
        &target.source_relative_path,
        &format!("target `{}`", target.target_name),
    )
}

/// Resolve a planned workspace library source beneath the receipt-authorized compiler root.
pub(crate) fn compiler_suite_workspace_library_source(
    compiler_root: &Path,
    library: &OvenCompilerWorkspaceLibrary,
) -> CliResult<PathBuf> {
    compiler_suite_source_path(
        compiler_root,
        &library.key.source_relative_path,
        &format!("workspace library `{}`", library.key.crate_name),
    )
}

/// Resolve one receipt-bound relative source path while rejecting traversal and symlink escape.
pub(crate) fn compiler_suite_source_path(
    compiler_root: &Path,
    relative_path: &str,
    subject: &str,
) -> CliResult<PathBuf> {
    let compiler_root = fs::canonicalize(compiler_root).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite root {}: {error}",
            compiler_root.display()
        ))
    })?;
    let mut source = compiler_root.clone();
    for component in Path::new(relative_path).components() {
        let Component::Normal(component) = component else {
            return Err(CliError::failure(format!(
                "stored compiler-suite {subject} has unsafe source path `{relative_path}`"
            )));
        };
        source.push(component);
    }
    let metadata = fs::symlink_metadata(&source).map_err(|error| {
        CliError::failure(format!(
            "cannot inspect compiler-suite {subject} source {}: {error}",
            source.display(),
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "compiler-suite {subject} source {} must be a regular non-symlink file",
            source.display(),
        )));
    }
    let source = fs::canonicalize(&source).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite {subject} source {}: {error}",
            source.display(),
        ))
    })?;
    if !source.starts_with(&compiler_root) {
        return Err(CliError::failure(format!(
            "compiler-suite {subject} source {} escapes compiler root {}",
            source.display(),
            compiler_root.display()
        )));
    }
    Ok(source)
}

/// Recover the package-root working directory Cargo would have supplied to one compiler test target.
///
/// The target's compile environment is receipt-bound publisher data, but execution must still reject a directory
/// outside the caller-authorized compiler root. That preserves Cargo's package-root semantics for snapshots and
/// fixtures without treating the caller-owned output directory as a source root.
pub(crate) fn compiler_suite_target_working_directory(
    compiler_root: &Path,
    source: &Path,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> CliResult<PathBuf> {
    let compiler_root = fs::canonicalize(compiler_root).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite root {}: {error}",
            compiler_root.display()
        ))
    })?;
    let declared = target.compile_environment.get("CARGO_MANIFEST_DIR").ok_or_else(|| {
        CliError::failure(format!(
            "stored compiler-suite target `{}` has no package working-directory declaration",
            target.target_name
        ))
    })?;
    let working_directory =
        resolve_compile_environment_value("CARGO_MANIFEST_DIR", declared, source).map_err(oven_error)?;
    let working_directory = fs::canonicalize(&working_directory).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite working directory {} for target `{}`: {error}",
            working_directory.display(),
            target.target_name
        ))
    })?;
    if !working_directory.starts_with(&compiler_root) {
        return Err(CliError::failure(format!(
            "stored compiler-suite target `{}` declares working directory {} outside compiler root {}",
            target.target_name,
            working_directory.display(),
            compiler_root.display()
        )));
    }
    if !working_directory.is_dir() {
        return Err(CliError::failure(format!(
            "stored compiler-suite target `{}` working directory {} is not a directory",
            target.target_name,
            working_directory.display()
        )));
    }
    Ok(working_directory)
}

/// Keep caller-owned test shard paths deterministic and safely inside the selected output directory.
pub(crate) fn compiler_suite_target_output_name(
    index: usize,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> String {
    format!(
        "{index:04}-{}-{}-{}",
        compiler_suite_output_segment(&target.package_name),
        compiler_suite_output_segment(&target.target_kind),
        compiler_suite_output_segment(&target.target_name)
    )
}

/// Return the mutable caller-owned state root for one scheduled compiler-suite child.
///
/// The immutable Loaf envelope is shared and lease-protected at the suite level. Homes and generated-project
/// targets are not: each root receives a distinct location so its nested normal commands cannot race a sibling.
pub(crate) fn compiler_suite_child_state_root(
    output_directory: &Path,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> PathBuf {
    output_directory
        .join("children")
        .join(compiler_suite_target_output_name(0, target))
}

/// Keep direct-Rustc workspace library outputs deterministic and separate from executable target outputs.
pub(crate) fn compiler_suite_workspace_library_output_name(
    index: usize,
    key: &OvenCompilerWorkspaceLibraryKey,
) -> String {
    let extension = if key.target_kind == "proc-macro" || compiler_suite_workspace_library_uses_dylib(key) {
        std::env::consts::DLL_SUFFIX
    } else {
        ".rlib"
    };
    format!(
        "lib{}-{index:04}-{}{}",
        compiler_suite_output_segment(&key.crate_name),
        compiler_suite_output_segment(&key.package_name),
        extension,
    )
}

/// Build the top-level compiler library once as a dynamic direct-Rustc boundary.
///
/// Every integration root otherwise statically embeds this large shared library, which defeats whole-suite reuse
/// even after its workspace DAG is deduplicated. This remains an Oven caller output, not a Cargo target artifact.
pub(crate) fn compiler_suite_workspace_library_uses_dylib(key: &OvenCompilerWorkspaceLibraryKey) -> bool {
    key.target_kind == "lib" && key.package_name == "incan" && key.crate_name == "incan"
}

/// Convert publisher-provided display labels into one portable output-path segment.
pub(crate) fn compiler_suite_output_segment(value: &str) -> String {
    let segment = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if segment.is_empty() {
        "unnamed".to_string()
    } else {
        segment
    }
}
