//! Invoking direct rustc and rustdoc consumers against admitted artifacts.

use super::{
    BTreeSet, Command, Duration, Instant, OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION, OvenDirectRustcBake,
    OvenDirectRustcOutputKind, OvenDirectRustcOutputReceipt, OvenDirectRustcOutputRecord, OvenDirectRustcRunRequest,
    OvenDirectRustcTestBake, OvenDirectRustcTestRequest, OvenReceipt, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenRustcError, OvenRustdocTestReport, OvenTrustedDirectRustcTargetRequest, OvenTrustedRustdocTestRequest, Path,
    PathBuf, Read, Stdio, apply_oven_profile, caller_output_path, caller_temporary_directory,
    clear_inherited_cargo_environment, combined_process_output, digest_bytes, digest_regular_file, fs, io,
    isolate_process_group, parse_rustc_diagnostics, resolve_compile_environment_value,
    rustc_dynamic_library_environment_with_caller_owned_paths, rustdoc_for_rustc, terminate_process_group, thread,
    validate_edition, validate_rust_identifier, verified_regular_file, verify_rustc_identity,
};

/// Run one compiler-suite doctest root directly with Rustdoc, without a Cargo consumer process.
///
/// Rustdoc creates and destroys the individual doctest executables internally. Oven therefore verifies the same
/// receipt-bound source and immutable closure as direct-rustc, pins Rustdoc to the selected Rustc sysroot, and keeps
/// all transient files in a caller-owned directory while the enclosing suite lease remains live.
pub fn run_trusted_rustdoc_test(
    request: &OvenTrustedRustdocTestRequest<'_>,
) -> Result<OvenRustdocTestReport, OvenRustcError> {
    let (source, rustdoc, command) = prepare_trusted_rustdoc_command(request)?;
    let (output, timed_out) = run_supervised_rustdoc_command(command, &rustdoc, request.timeout)?;
    let mut transcript = combined_process_output(&output.stdout, &output.stderr);
    if timed_out {
        if !transcript.ends_with('\n') && !transcript.is_empty() {
            transcript.push('\n');
        }
        if let Some(timeout) = request.timeout {
            transcript.push_str(&format!(
                "Oven Rustdoc execution group timed out after {}ms (source: {})\n",
                timeout.as_millis(),
                source.display(),
            ));
        }
    }
    if timed_out || !output.status.success() {
        return Err(OvenRustcError::RustdocTestFailed { output: transcript });
    }
    Ok(OvenRustdocTestReport { output: transcript })
}

/// Validate Rustdoc inputs and construct its complete receipt-bound invocation.
fn prepare_trusted_rustdoc_command(
    request: &OvenTrustedRustdocTestRequest<'_>,
) -> Result<(PathBuf, PathBuf, Command), OvenRustcError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    verify_rustc_identity(request.rustc, &request.receipt.intent.toolchain)?;
    validate_rust_identifier(request.crate_name)?;
    validate_edition(request.edition)?;
    let source = verified_regular_file(request.source, "source")?;
    let source_bytes = fs::read(&source).map_err(|source_error| OvenRustcError::Io {
        path: source.clone(),
        source: source_error,
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let expected_source_digest = request
        .receipt
        .sources
        .supplemental_digests
        .get(request.source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{}`", request.source_evidence_key),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: request.source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let artifacts = request.artifacts.for_source_evidence(request.source_evidence_key)?;
    // `Option::unwrap_or` evaluates its fallback eagerly.  Indexed compiler-suite doctests provide a composed
    // foundation plan while their thin shard deliberately has no local third-party closure, so touching that
    // fallback would fail before Rustdoc can use the supplied plan.  Keep legacy single-root materialization lazy.
    let plan = match request.artifact_plan {
        Some(plan) => plan.clone(),
        None => artifacts.materialize_trusted_store(request.artifact_root, &request.receipt.intent)?,
    };
    let temporary_directory = caller_temporary_directory(request.temporary_directory, request.artifact_root)?;
    let test_run_directory = match plan.compile_environment.get("CARGO_MANIFEST_DIR") {
        Some(value) => resolve_compile_environment_value("CARGO_MANIFEST_DIR", value, &source)?,
        None => source
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "source",
                message: format!("{} has no parent directory for Rustdoc", source.display()),
            })?,
    };
    let rustdoc = rustdoc_for_rustc(request.rustc)?;
    let mut command = Command::new(&rustdoc);
    command
        .arg("--test")
        .arg("--target")
        .arg(&request.receipt.intent.target)
        .arg(format!("--edition={}", request.edition))
        .arg("--crate-name")
        .arg(request.crate_name)
        .arg("--test-run-directory")
        .arg(&test_run_directory)
        .arg(&source);
    apply_oven_profile(&mut command, &request.receipt.intent.profile);
    if request.is_proc_macro {
        // Cargo's proc-macro Rustdoc invocation supplies both the crate type and the sysroot-provided
        // `proc_macro` extern. Without the latter, Rustdoc treats the source as an ordinary library and rejects
        // `use proc_macro::…` even though the root is receipt-classified as a proc macro.
        command
            .arg("--crate-type")
            .arg("proc-macro")
            .arg("--extern")
            .arg("proc_macro");
    }
    command.current_dir(&test_run_directory);
    clear_inherited_cargo_environment(&mut command);
    command.env("TMPDIR", &temporary_directory);
    for (name, value) in &plan.compile_environment {
        let value = resolve_compile_environment_value(name, value, &source)?;
        command.env(name, value);
    }
    if request.prefer_dynamic {
        // Rustdoc launches the generated doctest runner itself. That runner can link a caller-owned workspace dylib
        // such as the compiler library, so the selected toolchain libraries alone are not a complete runtime
        // closure. Carry only caller-owned dynamic-library directories: immutable store identities contain `sha256:`
        // and must remain direct `-L` compiler inputs rather than ambiguous path-list environment segments.
        let (name, value) = rustc_dynamic_library_environment_with_caller_owned_paths(request.rustc, &plan)?;
        command.args(["-C", "rpath"]);
        command.env(name, value);
    }
    for feature in request.features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    append_native_runtime_rpaths(&mut command, &request.receipt.intent.target, &plan.native_search_paths);
    for (crate_name, path) in &plan.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    Ok((source, rustdoc, command))
}

/// Run Rustdoc and its generated doctest children inside the same bounded process group as native suite roots.
pub(super) fn run_supervised_rustdoc_command(
    mut command: Command,
    rustdoc: &Path,
    timeout: Option<Duration>,
) -> Result<(std::process::Output, bool), OvenRustcError> {
    let Some(timeout) = timeout else {
        let output = command.output().map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })?;
        return Ok((output, false));
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source,
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source: io::Error::other("Rustdoc stdout was not piped"),
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source: io::Error::other("Rustdoc stderr was not piped"),
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                timed_out = true;
                break terminate_process_group(&mut child).map_err(|source| OvenRustcError::Io {
                    path: rustdoc.to_path_buf(),
                    source,
                })?;
            }
            None => thread::sleep(Duration::from_millis(1)),
        }
    };
    let stdout = join_rustdoc_output_reader(stdout_reader, rustdoc, "stdout")?;
    let stderr = join_rustdoc_output_reader(stderr_reader, rustdoc, "stderr")?;
    Ok((std::process::Output { status, stdout, stderr }, timed_out))
}

/// Join one Rustdoc pipe reader and retain the executable path in any I/O diagnostic.
pub(super) fn join_rustdoc_output_reader(
    reader: thread::JoinHandle<Result<Vec<u8>, io::Error>>,
    rustdoc: &Path,
    stream: &str,
) -> Result<Vec<u8>, OvenRustcError> {
    reader
        .join()
        .map_err(|_| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source: io::Error::other(format!("Rustdoc {stream} reader panicked")),
        })?
        .map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })
}

/// Compile one receipt-bound generated Rust test target without a Cargo consumer process.
pub fn bake_direct_rustc_test(request: &OvenDirectRustcTestRequest) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_direct_rustc(
        &request.receipt,
        &request.artifacts,
        &request.artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        false,
        None,
        false,
        &request.receipt.intent.features,
    )
}

/// Compile one compiler-suite test root from a caller-held leased store artifact without a Cargo consumer process.
pub fn bake_trusted_direct_rustc_test(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one regular Rust library from selected Oven foundations without a Cargo target directory.
///
/// The library is caller-owned ephemeral materialization. Its reusable sidecar still binds the exact receipt,
/// selected artifact manifest, source digest, resolved feature set, and library output kind before a later direct
/// Rustc target may link it.
pub fn bake_trusted_direct_rustc_library(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_trusted_direct_rustc_library_with_artifact_role(request, request.source_evidence_key)
}

/// Compile a library using an original declared native role while preserving its receipt source key.
///
/// The caller retains the admitted manifest, physical plan and leases. This argument selects their existing role;
/// it neither aliases physical bindings nor adds source authority to the receipt.
pub fn bake_trusted_direct_rustc_library_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        artifact_role,
        false,
        OvenDirectRustcOutputKind::Library,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one regular dynamic Rust library from selected Oven foundations without a Cargo target directory.
///
/// The compiler suite uses this only for its shared top-level compiler library. Linking that one expensive crate
/// dynamically keeps each independently executed integration-test root from embedding another static copy.
pub fn bake_trusted_direct_rustc_dylib(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Dylib,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one procedural-macro library from selected Oven foundations without a Cargo target directory.
///
/// A workspace materialization DAG treats the resulting dynamic library as a caller-owned `--extern` input for
/// later direct-Rustc steps. Its receipt still binds the exact source, compiler, features, and immutable foundation.
pub fn bake_trusted_direct_rustc_proc_macro(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_trusted_direct_rustc_proc_macro_with_artifact_role(request, request.source_evidence_key)
}

/// Compile a procedural macro using its original declared native role and unchanged source receipt.
///
/// The caller retains the admitted manifest, physical plan and leases. This argument selects their existing role;
/// it neither aliases physical bindings nor adds source authority to the receipt.
pub fn bake_trusted_direct_rustc_proc_macro_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        artifact_role,
        false,
        OvenDirectRustcOutputKind::ProcMacro,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one compiler-suite binary root from a caller-held leased store artifact without a Cargo consumer process.
pub fn bake_trusted_direct_rustc_run(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_trusted_direct_rustc_run_with_artifact_role(request, request.source_evidence_key)
}

/// Compile one trusted caller-owned binary while selecting an existing closure role separately from its source key.
///
/// Project Rust facets have their own receipted source, but consume the same sealed dependency closure as the Incan
/// library whose caller artifact they link. Keeping the two keys explicit prevents either receipt from claiming the
/// other's source bytes.
pub fn bake_trusted_direct_rustc_run_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        artifact_role,
        false,
        OvenDirectRustcOutputKind::Binary,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one receipt-bound generated Rust binary without a Cargo consumer process.
pub fn bake_direct_rustc_run(request: &OvenDirectRustcRunRequest) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        &request.receipt,
        &request.artifacts,
        &request.artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        false,
        None,
        false,
        &request.receipt.intent.features,
    )
}

/// Project a pre-materialized trusted plan onto the receipt-authorized direct extern roots.
///
/// Compiler-suite callers retain an already checked plan to avoid a second traversal of large immutable
/// foundations. That plan can include caller-owned library outputs, so it cannot simply be re-materialized from the
/// filtered manifest. Remove only immutable manifest externs that the source-evidence projection excludes; the
/// caller-owned additions remain exact inputs and continue to participate in output reuse evidence.
pub(super) fn trusted_artifact_plan_for_source(
    plan: &OvenRustcArtifactPlan,
    declared_artifacts: &OvenRustcArtifactManifest,
    selected_artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
    let declared_names = declared_artifacts
        .externs
        .iter()
        .map(|artifact| artifact.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    let selected_names = selected_artifacts
        .externs
        .iter()
        .map(|artifact| artifact.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    // An expose-extern caller library is identified by its crate-name reuse evidence. This matters when a caller
    // deliberately declares the same crate name as a compiler-private helper retained in the complete manifest:
    // source projection must remove the helper while preserving the caller's separately verified output.
    let caller_owned_extern_names = plan
        .caller_owned_library_digests
        .keys()
        .filter(|key| !key.starts_with("transitive:"))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut selected_plan = plan.clone();
    let mut excluded_direct_root_parents = BTreeSet::new();
    let mut selected_direct_root_parents = BTreeSet::new();
    for (crate_name, path) in &plan.externs {
        if !declared_names.contains(crate_name.as_str()) {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        if selected_names.contains(crate_name.as_str()) || caller_owned_extern_names.contains(crate_name.as_str()) {
            selected_direct_root_parents.insert(parent.to_path_buf());
        } else {
            excluded_direct_root_parents.insert(parent.to_path_buf());
        }
    }
    selected_plan.externs.retain(|(crate_name, _)| {
        !declared_names.contains(crate_name.as_str())
            || selected_names.contains(crate_name.as_str())
            || caller_owned_extern_names.contains(crate_name.as_str())
    });
    if declared_artifacts
        .entrypoint_dependency_search_paths
        .contains_key(source_evidence_key)
    {
        let projection = plan
            .source_path_projection
            .as_ref()
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "materialized source search closure",
                message: format!("source evidence `{source_evidence_key}` has no validated physical path bindings"),
            })?;
        let (bound, selected) =
            projection
                .roles
                .get(source_evidence_key)
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "materialized source search closure",
                    message: format!("source evidence `{source_evidence_key}` has no materialized role"),
                })?;
        if bound != &declared_artifacts.entrypoint_dependency_search_paths[source_evidence_key] {
            return Err(OvenRustcError::InvalidInput {
                field: "materialized source search closure",
                message: format!("source evidence `{source_evidence_key}` differs from its materialized contract"),
            });
        }
        selected_plan
            .dependency_search_paths
            .retain(|path| !projection.declared.contains(path) || selected.contains(path));
    } else {
        selected_plan.dependency_search_paths.retain(|search_path| {
            !excluded_direct_root_parents.contains(search_path) || selected_direct_root_parents.contains(search_path)
        });
    }
    Ok(selected_plan)
}

/// Project a verified, already-materialized direct-Rustc plan onto one receipt-authorized source root.
///
/// Callers that add a caller-owned registry leaf must make that decision against this projection, rather than the
/// complete Loaf plan. A complete Loaf deliberately retains compiler-private helpers such as the vocabulary
/// serializer; treating one of those helpers as a public caller extern can either hide a declared dependency or
/// expose a second incompatible Rust crate identity.
pub fn trusted_artifact_plan_for_source_evidence(
    plan: &OvenRustcArtifactPlan,
    artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
    let selected_artifacts = artifacts.for_source_evidence(source_evidence_key)?;
    trusted_artifact_plan_for_source(plan, artifacts, &selected_artifacts, source_evidence_key)
}

/// Return the direct extern names exposed to one receipt-authorized source root.
#[cfg(test)]
pub fn direct_rustc_source_extern_names(
    artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<BTreeSet<String>, OvenRustcError> {
    Ok(artifacts
        .for_source_evidence(source_evidence_key)?
        .externs
        .into_iter()
        .map(|artifact| artifact.crate_name)
        .collect())
}

/// Compile one receipt-bound source using its distinct, original native artifact role.
///
/// The receipt key verifies source bytes; the artifact role selects existing native inputs. Ordinary callers retain
/// the same key for both. Provider callers keep their original physical role without modifying either receipt.
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_direct_rustc(
    receipt: &OvenReceipt,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    rustc: &Path,
    source: &Path,
    output: &Path,
    crate_name: &str,
    edition: &str,
    source_evidence_key: &str,
    artifact_role: &str,
    test_harness: bool,
    output_kind: OvenDirectRustcOutputKind,
    trusted_store: bool,
    trusted_artifact_plan: Option<&OvenRustcArtifactPlan>,
    prefer_dynamic: bool,
    features: &[String],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    verify_rustc_identity(rustc, &receipt.intent.toolchain)?;
    validate_rust_identifier(crate_name)?;
    super::driver_grant::apply_driver_grant(&mut Command::new(rustc), receipt, crate_name)?;
    validate_edition(edition)?;
    let source = verified_regular_file(source, "source")?;
    let output = caller_output_path(output, artifact_root)?;
    let source_bytes = fs::read(&source).map_err(|source_error| OvenRustcError::Io {
        path: source.clone(),
        source: source_error,
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let expected_source_digest = receipt
        .sources
        .supplemental_digests
        .get(source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{source_evidence_key}`"),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let selected_artifacts = artifacts.for_source_evidence(artifact_role)?;
    let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source_error| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source: source_error,
    })?;
    let output_receipt = OvenDirectRustcOutputReceipt {
        schema_version: OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION,
        receipt_identity: receipt.identity.clone(),
        artifact_manifest_digest: digest_bytes(&serde_json::to_vec(&selected_artifacts).map_err(|error| {
            OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("cannot serialize verified manifest identity: {error}"),
            }
        })?),
        source_digest: source_digest.clone(),
        crate_name: crate_name.to_string(),
        edition: edition.to_string(),
        features: features.to_vec(),
        test_harness,
        prefer_dynamic,
        output_kind: output_kind.receipt_value().to_string(),
        caller_owned_library_digests: trusted_artifact_plan
            .map(|plan| plan.caller_owned_library_digests.clone())
            .unwrap_or_default(),
    };
    if let Some(output_digest) = caller_output_reusable_digest(&output, &output_receipt) {
        return Ok(OvenDirectRustcBake {
            source_digest,
            output,
            output_digest,
            cargo_process_started: false,
            reused: true,
            lease: None,
        });
    }

    let output_digest = compile_direct_rustc_output(
        receipt,
        artifacts,
        &selected_artifacts,
        artifact_root,
        rustc,
        &source,
        &output,
        crate_name,
        edition,
        artifact_role,
        test_harness,
        output_kind,
        trusted_store,
        trusted_artifact_plan,
        prefer_dynamic,
        features,
        output_receipt,
    )?;
    Ok(OvenDirectRustcBake {
        source_digest,
        output,
        output_digest,
        cargo_process_started: false,
        reused: false,
        lease: None,
    })
}

/// Materialize the selected plan, invoke rustc, and persist byte-bound reuse evidence.
#[allow(clippy::too_many_arguments)]
fn compile_direct_rustc_output(
    receipt: &OvenReceipt,
    artifacts: &OvenRustcArtifactManifest,
    selected_artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    rustc: &Path,
    source: &Path,
    output: &Path,
    crate_name: &str,
    edition: &str,
    artifact_role: &str,
    test_harness: bool,
    output_kind: OvenDirectRustcOutputKind,
    trusted_store: bool,
    trusted_artifact_plan: Option<&OvenRustcArtifactPlan>,
    prefer_dynamic: bool,
    features: &[String],
    output_receipt: OvenDirectRustcOutputReceipt,
) -> Result<String, OvenRustcError> {
    // Publication has already performed full content verification for a selected store entry. On a genuine caller
    // output miss, normal consumers prove file shape/containment under their active lease rather than rehash every
    // dependency; externally supplied plans retain the stronger byte-for-byte materialization path.
    let plan = if let Some(plan) = trusted_artifact_plan {
        trusted_artifact_plan_for_source(plan, artifacts, selected_artifacts, artifact_role)?
    } else if trusted_store {
        selected_artifacts.materialize_trusted_store(artifact_root, &receipt.intent)?
    } else {
        selected_artifacts.materialize(artifact_root, &receipt.intent)?
    };

    let mut command = Command::new(rustc);
    if test_harness {
        command.arg("--test");
    }
    match output_kind {
        OvenDirectRustcOutputKind::Binary => {}
        OvenDirectRustcOutputKind::Library => {
            command.args(["--crate-type", "lib"]);
        }
        OvenDirectRustcOutputKind::Dylib => {
            command.args(["--crate-type", "dylib"]);
        }
        OvenDirectRustcOutputKind::ProcMacro => {
            // `proc_macro` is supplied by the selected Rustc sysroot rather than by a Cargo-produced artifact.
            // Naming the crate explicitly is still required for edition-2018-and-later sources that import it with
            // `use proc_macro::…`; unlike an `extern crate proc_macro` declaration, that import does not cause
            // Rustc to infer the sysroot dependency.
            command.args(["--crate-type", "proc-macro", "--extern", "proc_macro"]);
        }
    }
    if prefer_dynamic {
        // Cargo emits both flags for a proc-macro libtest. `proc_macro` is provided by the receipt-selected Rust
        // toolchain sysroot rather than a Cargo target artifact, so it is intentionally not represented as a stored
        // third-party `--extern` file. `rpath` is required as well: compiler-suite children can pass a dynamically
        // linked caller-owned CLI through a shell script, and macOS strips `DYLD_*` values when it starts its system
        // shell. The selected Rustc and caller-owned `-L dependency` paths define the embedded loader closure.
        command.args(["-C", "prefer-dynamic", "-C", "rpath", "--extern", "proc_macro"]);
    }
    command
        .arg("--target")
        .arg(&receipt.intent.target)
        .arg(format!("--edition={edition}"))
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--error-format=json")
        .arg(source)
        .arg("-o")
        .arg(output);
    apply_oven_profile(&mut command, &receipt.intent.profile);
    clear_inherited_cargo_environment(&mut command);
    apply_sdk_compilation_policy(&mut command, receipt)?;
    for (name, value) in &plan.compile_environment {
        let value = resolve_compile_environment_value(name, value, source)?;
        command.env(name, value);
    }
    super::driver_grant::apply_driver_grant(&mut command, receipt, crate_name)?;
    for feature in features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    append_native_runtime_rpaths(&mut command, &receipt.intent.target, &plan.native_search_paths);
    for (crate_name, path) in &plan.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    let output_result = command.output().map_err(|source_error| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source: source_error,
    })?;
    if !output_result.status.success() {
        return Err(OvenRustcError::CompilationFailed {
            report: parse_rustc_diagnostics(&output_result.stdout, &output_result.stderr).with_invocation(&command),
        });
    }
    verified_regular_file(output, "output")?;
    let output_digest = digest_regular_file(output, "output")?;
    write_caller_output_record(
        output,
        &OvenDirectRustcOutputRecord {
            inputs: output_receipt,
            output_digest: output_digest.clone(),
        },
    )?;
    Ok(output_digest)
}

/// Embed the exact receipt-selected native directories required by a host-native Unix dynamic runtime.
///
/// Immutable Oven store identities contain a colon (`sha256:`), so they cannot safely be passed through a Unix
/// colon-separated loader environment. The linker records these already materialized, digest-verified native search
/// directories directly in the caller-owned binary instead. Static-only directories are harmless rpath entries;
/// retaining all selected native directories keeps this transport independent of a filename heuristic and never
/// admits an ambient package or host search path. Cross-target Android/iOS artifacts are staged by their explicit
/// adapter rather than receiving a meaningless path to this host's Oven store.
pub(super) fn append_native_runtime_rpaths(command: &mut Command, target: &str, native_search_paths: &[PathBuf]) {
    if !is_host_native_unix_target(target) {
        return;
    }
    for path in native_search_paths {
        command.arg("-C").arg("link-arg=-Wl,-rpath");
        command.arg("-C").arg(format!("link-arg={}", path.display()));
    }
}

/// Return whether a target produces an executable that can resolve this host's selected native-store directories.
pub(super) fn is_host_native_unix_target(target: &str) -> bool {
    let host_architecture = std::env::consts::ARCH;
    (cfg!(target_os = "macos") && target == format!("{host_architecture}-apple-darwin"))
        || (cfg!(all(target_os = "linux", target_env = "gnu"))
            && target == format!("{host_architecture}-unknown-linux-gnu"))
        || (cfg!(all(target_os = "linux", target_env = "musl"))
            && target == format!("{host_architecture}-unknown-linux-musl"))
}

/// Return the caller-owned sidecar path without accepting an output that lacks a safe file name.
pub(super) fn caller_output_receipt_path(output: &Path) -> Result<PathBuf, OvenRustcError> {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "output",
            message: "must end in a UTF-8 file name to retain Oven reuse evidence".to_string(),
        })?;
    Ok(output.with_file_name(format!("{name}.oven-output.json")))
}

/// Verify complete input evidence and hash the current regular output once, returning the digest only on a match.
///
/// Missing, malformed, old or mismatched evidence is a cache miss. Returning the verified digest lets the caller
/// report reuse without rereading the output. The caller must perform receipt/compiler/input admission first.
pub(super) fn caller_output_reusable_digest(output: &Path, expected: &OvenDirectRustcOutputReceipt) -> Option<String> {
    let receipt_path = caller_output_receipt_path(output).ok()?;
    let bytes = fs::read(&receipt_path).ok()?;
    let record: OvenDirectRustcOutputRecord = serde_json::from_slice(&bytes).ok()?;
    if record.inputs != *expected {
        return None;
    }
    let output_digest = digest_regular_file(output, "output").ok()?;
    (output_digest == record.output_digest).then_some(output_digest)
}

/// Atomically publish the input/output binding only after rustc has produced and verified its regular output.
pub(super) fn write_caller_output_record(
    output: &Path,
    record: &OvenDirectRustcOutputRecord,
) -> Result<(), OvenRustcError> {
    let path = caller_output_receipt_path(output)?;
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "reuse evidence has no parent directory".to_string(),
    })?;
    let bytes = serde_json::to_vec(record).map_err(|error| OvenRustcError::InvalidInput {
        field: "output receipt",
        message: format!("cannot serialize reuse evidence: {error}"),
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("oven-output"),
        std::process::id(),
    ));
    fs::write(&temporary, bytes).map_err(|source| OvenRustcError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, &path).map_err(|source| OvenRustcError::Io { path, source })
}

/// Apply receipt-bound SDK adoption policy and exact build-fact cfg values to the normal direct executor.
fn apply_sdk_compilation_policy(command: &mut Command, receipt: &OvenReceipt) -> Result<(), OvenRustcError> {
    if receipt.sources.build_unit_inputs.contains_key("sdk-source-archive") {
        // Adopted sources retain upstream warning policy, but a newer pinned compiler must not turn a newly added
        // warning into a dependency failure. Separate locked versions/domains also need distinct Rust metadata.
        command.args(["--cap-lints", "allow", "-C"]);
        command.arg(format!("metadata={}", receipt.build_unit_identity));
        if let Some(encoded) = receipt.sources.build_unit_inputs.get("sdk-build-fact") {
            let fact: oven_model::manifest::RustFactRecord =
                serde_json::from_str(encoded).map_err(|error| OvenRustcError::InvalidInput {
                    field: "SDK build fact",
                    message: error.to_string(),
                })?;
            if fact.toolchain != receipt.intent.toolchain
                || fact.target != receipt.intent.target
                || fact.profile != receipt.intent.profile
                || fact.features != receipt.intent.features
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "SDK build fact",
                    message: "does not match the complete compilation binding".to_string(),
                });
            }
            for cfg in fact.cfg {
                command.arg("--cfg").arg(cfg);
            }
        }
    }
    Ok(())
}
