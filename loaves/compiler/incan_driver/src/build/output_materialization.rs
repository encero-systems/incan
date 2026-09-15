//! Materializing completed outputs for the caller: default outputs, sealed outputs, projections and lock policy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use std::{fs, io};

use sha2::Sha256;

use crate::backend::selection::{
    BackendExecutionReceipt, BackendKind, FallbackOutcome, FallbackPolicy, ShadowComparisonState,
};
use crate::build::backend_selection::{default_backend_receipt_path, write_backend_receipt};
use crate::build::library_exports::{resolve_library_project_root, validate_library_entrypoint};
use crate::build::library_outputs::library_publication_receipts;
use crate::build::output_paths::{
    normalized_project_entrypoint, project_root_for_completed_output, validated_project_output_relative_path,
};
use crate::build::output_selection::{
    current_project_output_receipt, has_stale_baked_project_output,
    matching_baked_project_outputs_with_source_authority, restore_project_output_report_paths,
    select_baked_project_output_with_source_authority,
};
use crate::build::plan_authority::explicit_bake_profiles;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority,
};
use crate::build::{
    BackendSelectionOptions, CompletedOutputPolicy, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH,
    OVEN_PROJECT_OUTPUT_PROJECTION_SCHEMA_VERSION, OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION, OvenBakeProjectTarget,
    OvenProjectOutputProjection, OvenProjectOutputProjectionFile, OvenStoredProjectOutput, elapsed_ms,
    library_publication, manifest_project_report,
};
use crate::build_report::{
    BUILD_REPORT_SCHEMA_VERSION, BuildOvenReport, BuildReportDraft, BuildReportMode, artifact_report,
    oven_generated_project_report,
};
use crate::error::{CliError, CliResult};
use crate::lock::resolution::validate_oven_lock_policy;
use crate::oven_store::open_default_oven_store;
use crate::project::discover_effective_project_manifest;
use incan_core::version::INCAN_VERSION;
use incan_provider::FeatureSelection;
use oven_model::manifest::{LOAF_MANIFEST_FILENAME, ProjectManifest};
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::digest_bytes;
use oven_store::store::OvenStore;
use sha2::Digest as _;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Return the verified implicit-default backend receipt sealed into one completed project output.
///
/// A completed output is reusable only when its own immutable payload proves that an explicit bake selected and
/// executed the ordinary legacy default. Older, malformed, or differently selected outputs deliberately return
/// `None` so the caller takes the normal source-aware path rather than treating cached provenance as current.
pub fn completed_output_default_backend_receipt(output: &OvenStoredProjectOutput) -> Option<BackendExecutionReceipt> {
    let receipt = &output.payload.backend_receipt;
    if receipt.verify_identity().is_err()
        || receipt.selection.selected_backend != BackendKind::Legacy
        || receipt.selection.selection_reason != crate::backend::selection::SelectionReason::Default
        || receipt.selection.fallback_policy != FallbackPolicy::Refuse
        || receipt.selection.shadow_requested
        || receipt.executed_backend != BackendKind::Legacy
        || receipt.fallback_outcome != FallbackOutcome::NotNeeded
        || receipt.shadow_comparison != ShadowComparisonState::NotRequested
    {
        return None;
    }
    Some(receipt.clone())
}

/// Materialize an executable completed output and republish the verified backend provenance it carries.
///
/// Keeping this coupled prevents either normal `build` output path from restoring native bytes while leaving a stale
/// or unrelated project-local backend receipt behind.
pub fn materialize_completed_executable_output(
    project_root: &Path,
    output: &OvenStoredProjectOutput,
    backend_receipt: &BackendExecutionReceipt,
) -> CliResult<()> {
    if completed_output_default_backend_receipt(output).as_ref() != Some(backend_receipt) {
        return Err(CliError::failure(
            "completed Oven executable output does not carry the backend receipt selected for reuse",
        ));
    }
    materialize_project_output(project_root, output)?;
    write_backend_receipt(backend_receipt, &default_backend_receipt_path(project_root))
}

/// Restore and validate one bake-time executable report without reconstructing frontend-owned facts.
pub fn completed_executable_output_report(
    project_root: &Path,
    output: &OvenStoredProjectOutput,
    backend_receipt: &BackendExecutionReceipt,
    total_start: Instant,
) -> CliResult<serde_json::Value> {
    let snapshot = output.payload.build_report.as_ref().ok_or_else(|| {
        CliError::failure(
            "completed Oven executable output has no sealed build report; rerun `incan oven bake --project .`",
        )
    })?;
    if snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION {
        return Err(CliError::failure(
            "completed Oven executable output has an unsupported build-report schema",
        ));
    }
    let mut report = snapshot.report.clone();
    restore_project_output_report_paths(&mut report, project_root)?;
    let expected_entrypoint = project_root
        .join(validated_project_output_relative_path(
            &output.payload.entrypoint_relative_path,
            "entrypoint",
        )?)
        .to_string_lossy()
        .to_string();
    let expected = [
        ("/schema_version", BUILD_REPORT_SCHEMA_VERSION.to_string()),
        ("/compiler_version", INCAN_VERSION.to_string()),
        ("/status", "success".to_string()),
        ("/mode", "executable".to_string()),
        ("/profile", output.profile.clone()),
        ("/project/project_root", project_root.to_string_lossy().to_string()),
        ("/entrypoint", expected_entrypoint),
        ("/oven/receipt_identity", output.payload.receipt_identity.clone()),
        ("/oven/build_unit_identity", output.payload.build_unit_identity.clone()),
        ("/oven/plan_identity", output.payload.plan_identity.clone()),
    ];
    for (pointer, expected) in expected {
        let actual = report.pointer(pointer).and_then(|value| match value {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            _ => None,
        });
        if actual.as_deref() != Some(expected.as_str()) {
            return Err(CliError::failure(format!(
                "completed Oven executable build report disagrees with sealed output field `{pointer}`"
            )));
        }
    }
    let object = report
        .as_object_mut()
        .ok_or_else(|| CliError::failure("completed Oven executable build report is not a JSON object"))?;
    object.remove("workspace");
    let backend = serde_json::to_value(backend_receipt)
        .map_err(|error| CliError::failure(format!("failed to serialize completed-output backend receipt: {error}")))?;
    object.insert("backend".to_string(), backend);
    let elapsed = elapsed_ms(total_start);
    object.insert(
        "timings_ms".to_string(),
        serde_json::json!({
            "completed_project_output_reuse": elapsed,
            "total": elapsed,
        }),
    );
    let notes = object
        .get_mut("notes")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| CliError::failure("completed Oven executable build report has no notes array"))?;
    notes.push(serde_json::Value::String(
        "Reused a completed Oven project-output Loaf; detailed source, dependency, semantic, and interop facts were verified and sealed at explicit bake time.".to_string(),
    ));
    Ok(report)
}

/// Reconstruct the machine-readable library result from completed Loaf authority without re-entering compiler work.
pub fn completed_library_output_report(
    project_root: &Path,
    outputs: &[OvenStoredProjectOutput],
    total_start: Instant,
) -> CliResult<crate::build_report::BuildReport> {
    let Some(manifest) = discover_effective_project_manifest(project_root)? else {
        return Err(CliError::failure(format!(
            "completed Oven library output has no project manifest at {}",
            project_root.display()
        )));
    };
    let entrypoint = validate_library_entrypoint(&manifest)?;
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "incan_library".to_string());
    // Prefer the release output, but accept the profile this project actually baked: a bake narrowed by
    // `explicit_bake_profiles` has no release output, and this only names the reused library for the report.
    let release = outputs
        .iter()
        .find(|output| output.profile == "release")
        .or_else(|| outputs.first())
        .ok_or_else(|| CliError::failure("completed Oven library output has no profile to report"))?;
    let mut artifacts = Vec::new();
    for output in outputs {
        let native = output
            .payload
            .files
            .iter()
            .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
            .ok_or_else(|| CliError::failure("completed Oven library output has no native artifact"))?;
        artifacts.push(artifact_report(
            format!("rust_library_{}", output.profile),
            &caller_project_output_path(project_root, &native.caller_relative_path)?,
        ));
    }
    let backend_receipt = completed_output_default_backend_receipt(release).ok_or_else(|| {
        CliError::failure("completed Oven library output has no verified implicit-default backend receipt")
    })?;
    if outputs
        .iter()
        .any(|output| completed_output_default_backend_receipt(output).as_ref() != Some(&backend_receipt))
    {
        return Err(CliError::failure(
            "completed Oven library outputs disagree on their verified implicit-default backend receipt",
        ));
    }
    let report = BuildReportDraft {
        mode: BuildReportMode::Library,
        profile: "release".to_string(),
        project: manifest_project_report(Some(&manifest), &project_name, project_root),
        entrypoint: Some(entrypoint.to_string_lossy().to_string()),
        library_root: Some(project_root.to_string_lossy().to_string()),
        source_files: Vec::new(),
        generated: oven_generated_project_report(
            &project_root.join("target/lib"),
            &project_root.join("target/lib/src/lib.rs"),
            &project_root.join("target/lib/oven"),
        ),
        artifacts,
        dependencies: crate::build_report::BuildDependencyReport {
            rust: Vec::new(),
            rust_dev: Vec::new(),
            incan: Vec::new(),
            stdlib_facets: Vec::new(),
        },
        semantic: crate::build_report::BuildSemanticReport {
            sdk: None,
            packages: Vec::new(),
            feature_edges: Vec::new(),
            providers: Vec::new(),
        },
        cargo: None,
        oven: Some(BuildOvenReport {
            receipt_identity: release.payload.receipt_identity.clone(),
            build_unit_identity: release.payload.build_unit_identity.clone(),
            plan_identity: release.payload.plan_identity.clone(),
        }),
        interop: crate::build_report::BuildInteropReport {
            rust_imports: Vec::new(),
            rust_externs: Vec::new(),
            rust_abi_query_paths: Vec::new(),
        },
        notes: vec![
            "Reused a completed Oven project-output Loaf; source, dependency, semantic, and interop details were verified at explicit bake time and are intentionally not recomputed during this replay."
                .to_string(),
        ],
        backend: Some(backend_receipt),
    };
    let mut timings_ms = BTreeMap::new();
    timings_ms.insert("completed_project_output_reuse".to_string(), elapsed_ms(total_start));
    timings_ms.insert("total".to_string(), elapsed_ms(total_start));
    Ok(report.finish(timings_ms))
}

/// Return the caller-owned path that an exact project-output Loaf may materialize. The baked relative path is a
/// portability contract, never an arbitrary store-controlled destination.
pub fn caller_project_output_path(project_root: &Path, relative_path: &str) -> CliResult<PathBuf> {
    Ok(project_root.join(validated_project_output_relative_path(relative_path, "caller output")?))
}

/// Return the small caller-owned projection marker path for one completed output profile.
fn project_output_projection_marker_path(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<PathBuf> {
    let native = output
        .payload
        .files
        .iter()
        .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .ok_or_else(|| CliError::failure("completed Oven project-output Loaf has no native artifact"))?;
    let native_path = caller_project_output_path(project_root, &native.caller_relative_path)?;
    let parent = native_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "completed Oven project output has no native artifact directory: {}",
            native_path.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".oven-project-output-{}.json",
        digest_bytes(output.payload.target_identity.as_bytes()).trim_start_matches("sha256:")
    )))
}

/// Return the stable small projection descriptor expected beside one caller-owned native output.
fn project_output_projection(output: &OvenStoredProjectOutput) -> OvenProjectOutputProjection {
    let mut files = output
        .payload
        .files
        .iter()
        .map(|file| OvenProjectOutputProjectionFile {
            caller_relative_path: file.caller_relative_path.clone(),
            digest: file.digest.clone(),
            logical_bytes: file.logical_bytes,
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.caller_relative_path.cmp(&right.caller_relative_path));
    OvenProjectOutputProjection {
        schema_version: OVEN_PROJECT_OUTPUT_PROJECTION_SCHEMA_VERSION,
        output_identity: output.identity.clone(),
        files,
    }
}

/// Hash one mutable projection in bounded memory while retaining exact byte-count verification.
pub fn digest_project_output_projection_file(path: &Path) -> CliResult<(u64, String)> {
    let mut file = fs::File::open(path).map_err(|error| {
        CliError::failure(format!(
            "failed to open Oven project-output projection {}: {error}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut logical_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            CliError::failure(format!(
                "failed to verify Oven project-output projection {}: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        let read = u64::try_from(read)
            .map_err(|_| CliError::failure("Oven project-output projection byte count exceeds the supported range"))?;
        logical_bytes = logical_bytes.checked_add(read).ok_or_else(|| {
            CliError::failure("Oven project-output projection byte count exceeds the supported range")
        })?;
    }
    Ok((logical_bytes, format!("sha256:{}", hex::encode(hasher.finalize()))))
}

/// Verify the one store-owned executable immediately before a normal `incan run` launches it.
///
/// Completed-output selection validates immutable manifest structure, while build materialization verifies every copied
/// file. Run executes directly from the store for the hot path, so it must perform this bounded native-file check
/// itself rather than trusting file length or a mutable caller projection.
pub fn verify_stored_project_output_native(output: &OvenStoredProjectOutput) -> CliResult<()> {
    let native = output
        .payload
        .files
        .iter()
        .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .ok_or_else(|| CliError::failure("completed Oven project-output Loaf has no native artifact"))?;
    let metadata = fs::symlink_metadata(&output.native_output).map_err(|error| {
        CliError::failure(format!(
            "failed to inspect sealed Oven project output {}: {error}",
            output.native_output.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::failure(format!(
            "sealed Oven project output must be a regular file: {}",
            output.native_output.display()
        )));
    }
    let (logical_bytes, digest) = digest_project_output_projection_file(&output.native_output)?;
    if logical_bytes != native.logical_bytes || digest != native.digest {
        return Err(CliError::failure(format!(
            "sealed Oven project output digest differs at {}",
            output.native_output.display()
        )));
    }
    Ok(())
}

/// Return whether the caller already holds the exact selected result.
pub fn project_output_projection_is_current(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<bool> {
    let marker_path = project_output_projection_marker_path(project_root, output)?;
    let marker = match fs::read(&marker_path) {
        Ok(bytes) => match serde_json::from_slice::<OvenProjectOutputProjection>(&bytes) {
            Ok(marker) => marker,
            Err(_) => return Ok(false),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to read Oven project-output projection {}: {error}",
                marker_path.display()
            )));
        }
    };
    let expected = project_output_projection(output);
    if marker != expected {
        return Ok(false);
    }
    for file in &expected.files {
        let path = caller_project_output_path(project_root, &file.caller_relative_path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to inspect Oven project-output projection {}: {error}",
                    path.display()
                )));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != file.logical_bytes {
            return Ok(false);
        }
        let (logical_bytes, digest) = digest_project_output_projection_file(&path)?;
        if logical_bytes != file.logical_bytes || digest != file.digest {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Atomically retain the successful caller projection after its immutable source bytes were verified.
fn write_project_output_projection(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<()> {
    let marker_path = project_output_projection_marker_path(project_root, output)?;
    let parent = marker_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "Oven project-output projection has no parent: {}",
            marker_path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "failed to create Oven project-output projection directory {}: {error}",
            parent.display()
        ))
    })?;
    let bytes = serde_json::to_vec_pretty(&project_output_projection(output))
        .map_err(|error| CliError::failure(format!("failed to encode Oven project-output projection: {error}")))?;
    let staged = parent.join(format!(".oven-project-output-{}.tmp", std::process::id()));
    oven_store::write_receipt_staged(&bytes, &staged, &marker_path, parent).map_err(|error| {
        CliError::failure(format!(
            "failed to publish Oven project-output projection {}: {error}",
            marker_path.display()
        ))
    })
}

/// Select an exact default-profile project output without constructing a compilation session. Explicit package-feature
/// or SDK selections retain the normal source-aware preparation route until their own selection facts are part of the
/// project-output payload schema.
pub fn select_default_project_output(
    file_path: &str,
    policy: &CompletedOutputPolicy<'_>,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    policy.reject_cargo_feature_controls("build and run")?;
    if policy.package_features != &FeatureSelection::default() || policy.sdk_profile.is_some() {
        return Ok(None);
    }
    let entrypoint = normalized_project_entrypoint(file_path)?;
    let Some(project_root) = project_root_for_completed_output(&entrypoint)? else {
        return Ok(None);
    };
    let manifest = ProjectManifest::load(&project_root.join(LOAF_MANIFEST_FILENAME))
        .map_err(|error| CliError::failure(error.to_string()))?;
    validate_completed_output_lock_policy(&project_root, &manifest, &entrypoint, policy)?;
    let store = open_default_oven_store()?;
    select_current_sealed_project_output(&store, &project_root, &entrypoint, target, profile)
}

/// Select the sealed output for exactly this source authority, or `None` when the project must take the source-aware
/// route. A sealed output left behind by an earlier bake of different sources is reported once and then ignored: it
/// is not evidence about the current tree, and it must not stop the build.
pub fn select_current_sealed_project_output(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    let source_authority_digest = digest_baked_project_source_authority(project_root)?;
    if let Some(selected) = select_baked_project_output_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        &source_authority_digest,
        None,
    )? {
        return Ok(Some(selected));
    }
    if has_stale_baked_project_output(store, project_root, entrypoint, target, profile)? {
        warn_stale_sealed_project_output("build or run");
    }
    Ok(None)
}

/// Say once why a baked project is taking the source-aware route instead of replaying its sealed output.
///
/// A sealed project-output Loaf is exact replay for one source authority. When the source or lock moved after the
/// bake, that Loaf is simply not the answer any more; the dependency closure the same bake published still is, and
/// the normal consume-only route builds the edited project against it through direct `rustc`. Refusing here would
/// turn every edit of a baked project into a 20-second re-bake, which is the opposite of what the bake is for.
fn warn_stale_sealed_project_output(command_kind: &str) {
    // One command selects more than once on its way to the source route; the reader needs the sentence once.
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!(
            "warning: the sealed project output from the last `incan oven bake --project .` no longer matches this \
             source tree; this {command_kind} compiles the project from source against the baked dependency \
             closure. Bake again when you want the sealed output refreshed."
        );
    });
}

/// Validate strict lock promises before a completed-output fast path can return a stale-output diagnostic.
///
/// `--locked` and `--frozen` are user-visible assertions about canonical `oven.lock`. They remain read-only and
/// Cargo-free here, but must retain their canonical diagnostic precedence even when a project has a previous completed
/// Loaf.
fn validate_completed_output_lock_policy(
    project_root: &Path,
    manifest: &ProjectManifest,
    entrypoint: &Path,
    policy: &CompletedOutputPolicy<'_>,
) -> CliResult<()> {
    if !policy.cargo_policy.locked && !policy.cargo_policy.frozen {
        return Ok(());
    }
    let cargo_features = policy.cargo_feature_selection();
    validate_oven_lock_policy(
        project_root,
        Some(manifest),
        entrypoint,
        &cargo_features,
        policy.cargo_policy,
        policy.package_features,
        policy.sdk_profile,
    )
}

/// Preserve the normal non-strict stale-lock warning when only the lock's derived fingerprint differs.
pub fn warn_for_completed_output_lock_fingerprint_drift<'a>(
    project_root: &Path,
    outputs: impl IntoIterator<Item = &'a OvenStoredProjectOutput>,
) -> CliResult<()> {
    let mut expected = None;
    for output in outputs {
        let Some(fingerprint) = output.payload.lock_dependencies_fingerprint.as_deref() else {
            continue;
        };
        match expected {
            Some(previous) if previous != fingerprint => {
                return Err(CliError::failure(
                    "completed Oven project outputs disagree on their canonical lock dependency fingerprint",
                ));
            }
            Some(_) => {}
            None => expected = Some(fingerprint),
        }
    }
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(actual) = baked_project_lock_dependencies_fingerprint(project_root)? else {
        return Ok(());
    };
    if actual == expected {
        return Ok(());
    }
    let workspace = oven_model::workspace::WorkspaceGraph::discover(project_root)
        .map_err(|error| CliError::failure(format!("failed to resolve Oven project workspace: {error}")))?;
    if workspace.is_some() {
        eprintln!(
            "warning: workspace oven.lock is out of date; continuing without using it as Oven lock authority or rewriting it. Run `incan lock` to refresh it."
        );
    } else {
        eprintln!(
            "warning: oven.lock is out of date; continuing without using it as Oven lock authority or rewriting it. Run `incan lock` to refresh it."
        );
    }
    Ok(())
}

/// Select both profile outputs required by a normal `build --lib` without entering the library frontend. A partial hit
/// is deliberately ignored: the historical command contract publishes debug and release rlibs together.
pub fn select_default_library_project_outputs(
    file_path: Option<&str>,
    policy: &CompletedOutputPolicy<'_>,
    backend_options: &BackendSelectionOptions,
) -> CliResult<Option<Vec<OvenStoredProjectOutput>>> {
    policy.reject_cargo_feature_controls("library builds")?;
    if policy.package_features != &FeatureSelection::default()
        || policy.sdk_profile.is_some()
        || !backend_options.allows_completed_output_reuse()
    {
        return Ok(None);
    }
    let project_root = resolve_library_project_root(file_path)?;
    let Some(manifest) = discover_effective_project_manifest(&project_root)? else {
        return Ok(None);
    };
    let entrypoint = validate_library_entrypoint(&manifest)?;
    validate_completed_output_lock_policy(&project_root, &manifest, &entrypoint, policy)?;
    let store = open_default_oven_store()?;
    let source_authority_digest = digest_baked_project_source_authority(&project_root)?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let mut profile_candidates = Vec::new();
    let mut current_receipts = Vec::new();
    for profile in explicit_bake_profiles() {
        let candidates = matching_baked_project_outputs_with_source_authority(
            &store,
            &project_root,
            &entrypoint,
            OvenBakeProjectTarget::Library,
            profile,
            &source_authority_digest,
            Some((&target, &toolchain)),
        )?;
        if candidates.is_empty() {
            if has_stale_baked_project_output(
                &store,
                &project_root,
                &entrypoint,
                OvenBakeProjectTarget::Library,
                profile,
            )? {
                warn_stale_sealed_project_output("library build");
            }
            return Ok(None);
        };
        if let Some(receipt) =
            current_project_output_receipt(&project_root, OvenBakeProjectTarget::Library, &entrypoint, profile)?
        {
            current_receipts.push(receipt);
        }
        profile_candidates.push(candidates);
    }
    select_coherent_library_outputs(
        profile_candidates,
        &current_receipts,
        baked_project_lock_dependencies_fingerprint(&project_root)?.as_deref(),
    )
}

/// Retain one shared lock cohort across all requested library profiles, including a coherent stale fallback.
pub fn select_coherent_library_outputs(
    profile_candidates: Vec<Vec<OvenStoredProjectOutput>>,
    current_receipts: &[oven_store::OvenReceipt],
    current_fingerprint: Option<&str>,
) -> CliResult<Option<Vec<OvenStoredProjectOutput>>> {
    // Score semantic preferences directly: retained duplicates must never outweigh a verified local lineage.
    let fingerprints = profile_candidates
        .first()
        .map(|candidates| {
            candidates
                .iter()
                .map(|output| output.payload.lock_dependencies_fingerprint.clone())
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();
    let selected_fingerprint = fingerprints
        .into_iter()
        .filter_map(|fingerprint| {
            if !profile_candidates.iter().all(|candidates| {
                candidates
                    .iter()
                    .any(|output| output.payload.lock_dependencies_fingerprint == fingerprint)
            }) {
                return None;
            }
            let lineage_matches = current_receipts
                .iter()
                .filter(|receipt| {
                    profile_candidates.iter().flatten().any(|output| {
                        output.payload.lock_dependencies_fingerprint == fingerprint
                            && output.payload.receipt_identity == receipt.identity
                            && output.payload.build_unit_identity == receipt.build_unit_identity
                            && output.intent == receipt.intent
                    })
                })
                .count();
            Some((
                (
                    std::cmp::Reverse(lineage_matches),
                    fingerprint.as_deref() != current_fingerprint,
                ),
                fingerprint,
            ))
        })
        .min_by(|left, right| left.0.cmp(&right.0));
    let Some((_, fingerprint)) = selected_fingerprint else {
        return Err(CliError::failure(
            "Oven has no coherent completed library output cohort across the requested profiles. Run `incan oven bake --project .` before a normal library build.",
        ));
    };
    let mut outputs = Vec::new();
    for candidates in profile_candidates {
        let Some(output) = candidates
            .into_iter()
            .find(|output| output.payload.lock_dependencies_fingerprint == fingerprint)
        else {
            return Ok(None);
        };
        if completed_output_default_backend_receipt(&output).is_none() {
            return Ok(None);
        }
        outputs.push(output);
    }
    Ok(Some(outputs))
}

/// Restore caller-visible generated sources, package handoff records, and native artifacts from a completed immutable
/// result into the caller's project.
///
/// The selected project's dependency closure deliberately remains in the primary Oven store. Re-publishing that already
/// verified closure into `target/lib/oven/loafs` here would make an ordinary hot build synchronously copy and fsync
/// every dependency file. An explicit provider bake retains the portable package-store export; a normal build needs
/// only the completed output itself.
pub fn materialize_project_output(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<()> {
    if project_output_projection_is_current(project_root, output)? {
        return Ok(());
    }
    for file in &output.payload.files {
        let destination = caller_project_output_path(project_root, &file.caller_relative_path)?;
        if let Ok(existing) = fs::read(&destination)
            && u64::try_from(existing.len()).ok() == Some(file.logical_bytes)
            && digest_bytes(&existing) == file.digest
        {
            continue;
        }
        let source = output.artifact_root.join(validated_project_output_relative_path(
            &file.output_relative_path,
            "stored output",
        )?);
        let source_bytes = fs::read(&source).map_err(|error| {
            CliError::failure(format!(
                "selected Oven project-output Loaf is missing {}: {error}",
                source.display()
            ))
        })?;
        if u64::try_from(source_bytes.len()).ok() != Some(file.logical_bytes)
            || digest_bytes(&source_bytes) != file.digest
        {
            return Err(CliError::failure(format!(
                "selected Oven project-output Loaf digest differs at {}",
                source.display()
            )));
        }
        let parent = destination.parent().ok_or_else(|| {
            CliError::failure(format!(
                "Oven project output has no parent path: {}",
                destination.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            CliError::failure(format!(
                "failed to create Oven output directory {}: {error}",
                parent.display()
            ))
        })?;
        let temporary = parent.join(format!(
            ".{}.oven-project-output-{}.tmp",
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output"),
            std::process::id()
        ));
        fs::write(&temporary, &source_bytes).map_err(|error| {
            CliError::failure(format!(
                "failed to materialize Oven project output {} -> {}: {error}",
                source.display(),
                temporary.display()
            ))
        })?;
        let source_permissions = fs::metadata(&source)
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to read Oven project output permissions {}: {error}",
                    source.display()
                ))
            })?
            .permissions();
        // Store materializations are immutable by design. The caller-owned projection must remain writable so a later
        // source miss can generate a replacement rather than failing against a read-only cache copy.
        #[cfg(unix)]
        let caller_permissions = fs::Permissions::from_mode(if source_permissions.mode() & 0o111 == 0 {
            0o644
        } else {
            0o755
        });
        #[cfg(not(unix))]
        let caller_permissions = {
            let mut permissions = source_permissions;
            permissions.set_readonly(false);
            permissions
        };
        fs::set_permissions(&temporary, caller_permissions).map_err(|error| {
            CliError::failure(format!(
                "failed to preserve Oven project output permissions {}: {error}",
                temporary.display()
            ))
        })?;
        fs::rename(&temporary, &destination).map_err(|error| {
            CliError::failure(format!(
                "failed to atomically publish Oven project output {}: {error}",
                destination.display()
            ))
        })?;
    }
    write_project_output_projection(project_root, output)?;
    Ok(())
}

/// Publish a selected library cohort as one ordinary-error transaction, retaining a verified warm no-op.
pub fn materialize_completed_library_outputs<T>(
    project_root: &Path,
    outputs: &[OvenStoredProjectOutput],
    backend_receipt: &crate::backend::selection::BackendExecutionReceipt,
    complete: impl FnOnce() -> CliResult<T>,
) -> CliResult<T> {
    let receipt_path = default_backend_receipt_path(project_root);
    let mut expected_receipt = serde_json::to_vec_pretty(backend_receipt)
        .map_err(|error| CliError::failure(format!("failed to encode completed library receipt: {error}")))?;
    expected_receipt.push(b'\n');
    let current = outputs.iter().try_fold(true, |current, output| {
        Ok::<_, CliError>(project_output_projection_is_current(project_root, output)? && current)
    })?;
    if current && fs::read(&receipt_path).is_ok_and(|bytes| bytes == expected_receipt) {
        return complete();
    }
    let publication = if current {
        library_publication::LibraryPublication::begin_receipt_update(project_root, vec![receipt_path.clone()])?
    } else {
        library_publication::LibraryPublication::begin(
            project_root,
            &project_root.join("target/lib"),
            library_publication_receipts(project_root)?,
        )?
        .retaining_package_cache()
    };
    let result = (|| {
        if !current {
            for output in outputs {
                materialize_project_output(project_root, output)?;
            }
        }
        write_backend_receipt(backend_receipt, &receipt_path)?;
        complete()
    })();
    publication.finish(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::test_support::fixture_project_output_publication;
    use std::fs;
    use std::path::Path;
    use std::time::Instant;

    use crate::backend::selection::{
        BackendExecutionReceipt, BackendKind, FallbackPolicy, ShadowComparisonState, finalize_receipt, select_backend,
    };
    use crate::build::backend_selection::default_backend_receipt_path;
    use crate::build::output_selection::{
        baked_project_owner_identity, make_project_output_report_portable, restore_project_output_report_paths,
        select_baked_project_output,
    };
    use crate::build::publication::publish_project_output_loaf;
    use crate::build::source_authority::{
        baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority,
    };
    use crate::build::{
        OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
        OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT, OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG,
        OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION, OvenBakeProjectTarget, OvenProjectOutputBakeFile,
        OvenProjectOutputFile, OvenProjectOutputPayload, OvenProjectOutputReportSnapshot,
    };
    use crate::build_report::BUILD_REPORT_SCHEMA_VERSION;
    use crate::error::{CliError, CliResult};
    use incan_core::version::INCAN_VERSION;
    use incan_frontend::diagnostics;
    use oven_rustc::plan::OvenPackagedLibraryLoafEntry;
    use oven_rustc::rustc::{
        OvenProjectInspectionAuthorityRef, resolve_active_rustc, rustc_host_target, rustc_identity,
    };
    use oven_store::store::{OvenArtifactKind, OvenStore};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

    #[test]
    fn completed_executable_report_replays_sealed_dependencies_and_rebases_project_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let external = tempfile::tempdir()?;
        let relocated = tempfile::tempdir()?;
        let lexical_external = project.path().join("../set_library");
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, mut payload, files) = fixture_project_output_publication(project.path(), "release", "report")?;
        let authored_sentinel = "$INCAN_PROJECT_ROOT/ordinary-authored-string";
        let mut report = serde_json::json!({
            "schema_version": BUILD_REPORT_SCHEMA_VERSION,
            "compiler_version": INCAN_VERSION,
            "status": "success",
            "mode": "executable",
            "profile": "release",
            "project": {
                "name": "fixture",
                "version": null,
                "project_root": project.path().to_string_lossy(),
            },
            "entrypoint": project.path().join("src/main.incn").to_string_lossy(),
            "library_root": null,
            "source_files": [{
                "path": external.path().join("src/provider.incn").to_string_lossy(),
                "module_path": ["provider"],
            }],
            "generated": {
                "project_path": project.path().join("target/fixture").to_string_lossy(),
                "manifest_path": project.path().join("target/fixture/Cargo.toml").to_string_lossy(),
                "crate_root": project.path().join("target/fixture/src/main.rs").to_string_lossy(),
                "cargo_target_dir": null,
                "oven_output_dir": project.path().join("target/fixture/oven").to_string_lossy(),
            },
            "artifacts": [{
                "kind": "binary",
                "path": project.path().join("target/fixture/native-report").to_string_lossy(),
                "exists": true,
                "size_bytes": 1,
            }],
            "dependencies": {
                "rust": [{
                    "crate_name": "itoa",
                    "source": "path",
                    "source_detail": external.path().join("rust/itoa").to_string_lossy(),
                }],
                "rust_dev": [],
                "incan": [{
                    "library_name": "provider",
                    "path": external.path().join("incan/provider").to_string_lossy(),
                }, {
                    "library_name": "lexical-sibling",
                    "path": lexical_external.to_string_lossy(),
                }],
                "stdlib_facets": [authored_sentinel],
            },
            "semantic": {
                "sdk": null,
                "packages": [{
                    "package": "provider",
                    "project_root": external.path().join("incan/provider").to_string_lossy(),
                }],
                "feature_edges": [{
                    "from": project.path().to_string_lossy(),
                    "to": external.path().join("incan/provider").to_string_lossy(),
                }],
                "providers": [{
                    "manifest_path": external.path().join("incan/provider/target/lib/library.incnlib").to_string_lossy(),
                    "provenance": {
                        "kind": "sdk",
                        "inventory_path": external.path().join("sdk/inventory.json").to_string_lossy(),
                    },
                }],
            },
            "oven": {
                "receipt_identity": payload.receipt_identity.clone(),
                "build_unit_identity": payload.build_unit_identity.clone(),
                "plan_identity": payload.plan_identity.clone(),
            },
            "interop": {
                "rust_imports": [authored_sentinel],
                "rust_externs": [],
                "rust_abi_query_paths": ["itoa::Buffer"],
            },
            "timings_ms": { "bake": 1 },
            "notes": [authored_sentinel],
            "workspace": { "member_name": "must-not-survive" },
        });
        make_project_output_report_portable(&mut report, project.path())?;
        let lexical_tag = report
            .pointer("/dependencies/incan/1/path")
            .and_then(serde_json::Value::as_object)
            .and_then(|path| path.get(OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG))
            .and_then(serde_json::Value::as_object)
            .ok_or("lexical sibling dependency was not sealed as a portable path")?;
        assert_eq!(lexical_tag.get("root"), Some(&serde_json::json!("external")));
        let sealed = serde_json::to_string(&report)?;
        assert!(!sealed.contains(project.path().to_string_lossy().as_ref()));
        assert!(!sealed.contains(external.path().to_string_lossy().as_ref()));
        let mut relocated_report = report.clone();
        restore_project_output_report_paths(&mut relocated_report, relocated.path())?;
        assert_eq!(
            relocated_report.pointer("/project/project_root"),
            Some(&serde_json::json!(relocated.path().to_string_lossy()))
        );
        assert_eq!(
            relocated_report.pointer("/entrypoint"),
            Some(&serde_json::json!(
                relocated.path().join("src/main.incn").to_string_lossy()
            ))
        );
        let relocated_rust_source = relocated_report
            .pointer("/dependencies/rust/0/source_detail")
            .and_then(serde_json::Value::as_str)
            .ok_or("relocated Rust dependency report lost its source authority")?;
        assert!(relocated_rust_source.starts_with(OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT));
        assert!(!Path::new(relocated_rust_source).is_absolute());
        let relocated_lexical_sibling = relocated_report
            .pointer("/dependencies/incan/1/path")
            .and_then(serde_json::Value::as_str)
            .ok_or("relocated lexical sibling dependency lost its external authority slot")?;
        assert!(relocated_lexical_sibling.starts_with(OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT));
        assert!(!Path::new(relocated_lexical_sibling).is_absolute());
        assert_eq!(
            relocated_report.pointer("/notes/0"),
            Some(&serde_json::json!(authored_sentinel)),
            "ordinary authored strings that resemble old path tokens must remain byte-exact"
        );
        payload.build_report = Some(OvenProjectOutputReportSnapshot {
            schema_version: OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION,
            report,
        });
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let selected = select_baked_project_output(
            &store,
            project.path(),
            &project.path().join("src/main.incn"),
            OvenBakeProjectTarget::Executable,
            "release",
        )?
        .ok_or("published completed executable report was not selected")?;
        let backend_receipt = completed_output_default_backend_receipt(&selected)
            .ok_or("completed output did not retain the verified default backend receipt")?;
        materialize_completed_executable_output(project.path(), &selected, &backend_receipt)?;
        let report = completed_executable_output_report(project.path(), &selected, &backend_receipt, Instant::now())?;

        assert_eq!(
            report.pointer("/dependencies/rust/0/crate_name"),
            Some(&serde_json::json!("itoa"))
        );
        assert_eq!(
            report.pointer("/project/project_root"),
            Some(&serde_json::json!(project.path().to_string_lossy()))
        );
        assert!(report.get("workspace").is_none());
        assert_eq!(
            report.pointer("/backend/identity"),
            Some(&serde_json::json!(&backend_receipt.identity))
        );
        let persisted = fs::read(default_backend_receipt_path(project.path()))?;
        let persisted = serde_json::from_slice::<BackendExecutionReceipt>(&persisted)?;
        assert_eq!(persisted, backend_receipt);
        assert!(report.pointer("/timings_ms/completed_project_output_reuse").is_some());
        Ok(())
    }

    #[test]
    fn project_output_loaf_restores_without_republishing_its_dependency_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/lib.incn");
        let generated_source = project.path().join("target/lib/src/lib.rs");
        let native_output = project.path().join("target/lib/oven/release/libfixture.rlib");
        fs::create_dir_all(entrypoint.parent().ok_or("entrypoint has no parent")?)?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::create_dir_all(native_output.parent().ok_or("native output has no parent")?)?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(&entrypoint, "pub def value() -> int:\n    return 42\n")?;
        fs::write(&generated_source, "pub fn value() -> i64 { 42 }\n")?;
        fs::write(&native_output, "fixture native output")?;

        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("compiler-version", INCAN_VERSION),
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let payload = OvenProjectOutputPayload {
            schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
            project_target: OvenBakeProjectTarget::Library.as_str().to_string(),
            target_identity: OvenBakeProjectTarget::Library.as_str().to_string(),
            project_identity: baked_project_owner_identity(project.path())?,
            source_authority_digest: digest_baked_project_source_authority(project.path())?,
            lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            entrypoint_relative_path: "src/lib.incn".to_string(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            receipt_identity: receipt.identity.clone(),
            plan_identity: "fixture-plan".to_string(),
            inspection_authority: Some(OvenProjectInspectionAuthorityRef {
                identity: "fixture-inspection-authority".to_string(),
                receipt_identity: receipt.identity.clone(),
                build_unit_identity: receipt.build_unit_identity.clone(),
            }),
            files: vec![
                OvenProjectOutputFile {
                    caller_relative_path: "target/lib/src/lib.rs".to_string(),
                    output_relative_path: "generated/src/lib.rs".to_string(),
                    digest: digest_bytes(&fs::read(&generated_source)?),
                    logical_bytes: fs::metadata(&generated_source)?.len(),
                },
                OvenProjectOutputFile {
                    caller_relative_path: "target/lib/oven/release/libfixture.rlib".to_string(),
                    output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
                    digest: digest_bytes(&fs::read(&native_output)?),
                    logical_bytes: fs::metadata(&native_output)?.len(),
                },
            ],
            required_project_loafs: vec![OvenPackagedLibraryLoafEntry {
                receipt: receipt.clone(),
                identity: "fixture-dependency-plan".to_string(),
                kind: OvenArtifactKind::ProjectPayload,
                base_loaf_identity: None,
            }],
            package_loaf_store_relative_path: Some("target/lib/oven/loafs".to_string()),
            backend_receipt: finalize_receipt(
                &select_backend(
                    BackendKind::Legacy,
                    false,
                    false,
                    "sha256:fixture-library-source",
                    FallbackPolicy::Refuse,
                ),
                BackendKind::Legacy,
                "sha256:fixture-library-output",
                ShadowComparisonState::NotRequested,
                diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
            )?,
            build_report: None,
        };
        let files = vec![
            OvenProjectOutputBakeFile {
                source_path: generated_source.clone(),
                caller_relative_path: "target/lib/src/lib.rs".to_string(),
                output_relative_path: "generated/src/lib.rs".to_string(),
            },
            OvenProjectOutputBakeFile {
                source_path: native_output.clone(),
                caller_relative_path: "target/lib/oven/release/libfixture.rlib".to_string(),
                output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
            },
        ];
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;

        fs::remove_dir_all(project.path().join("target/lib"))?;
        let mut selected = select_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Library,
            "release",
        )?
        .ok_or("exact library output Loaf should be selected")?;
        materialize_project_output(project.path(), &selected)?;

        assert_eq!(fs::read(&generated_source)?, b"pub fn value() -> i64 { 42 }\n");
        assert_eq!(fs::read(&native_output)?, b"fixture native output");
        assert!(!project.path().join("target/lib/oven/loafs").exists());
        assert!(project_output_projection_is_current(project.path(), &selected)?);
        fs::remove_file(&native_output)?;
        assert!(!project_output_projection_is_current(project.path(), &selected)?);
        materialize_project_output(project.path(), &selected)?;
        assert_eq!(fs::read(&native_output)?, b"fixture native output");
        assert!(project_output_projection_is_current(project.path(), &selected)?);

        let mut tampered = fs::read(&native_output)?;
        let first = tampered.first_mut().ok_or("fixture native output must not be empty")?;
        *first = b'F';
        fs::write(&native_output, &tampered)?;
        assert_eq!(fs::metadata(&native_output)?.len(), payload.files[1].logical_bytes);
        assert!(
            !project_output_projection_is_current(project.path(), &selected)?,
            "same-length caller-output corruption must invalidate the mutable projection"
        );
        materialize_project_output(project.path(), &selected)?;
        assert_eq!(fs::read(&native_output)?, b"fixture native output");
        assert!(project_output_projection_is_current(project.path(), &selected)?);

        // Exercise the same cohort publisher as normal build --lib. A late report/receipt failure and a
        // mid-copy payload failure must both restore the entire old generation, including its projection marker.
        let artifact_root = project.path().join("target/lib");
        let cache_object = artifact_root.join("oven/loafs/retained/object");
        fs::create_dir_all(cache_object.parent().ok_or("cache object has no parent")?)?;
        fs::write(&cache_object, "immutable package cache")?;
        let receipt_path = default_backend_receipt_path(project.path());
        fs::create_dir_all(receipt_path.parent().ok_or("receipt has no parent")?)?;
        fs::write(&receipt_path, "previous backend receipt")?;
        fs::write(&generated_source, "previous generated Rust")?;
        fs::write(&native_output, "previous native artifact")?;
        let marker = project_output_projection_marker_path(project.path(), &selected)?;
        let previous_marker = fs::read(&marker)?;
        let backend_receipt = selected.payload.backend_receipt.clone();
        let failed = materialize_completed_library_outputs(
            project.path(),
            std::slice::from_ref(&selected),
            &backend_receipt,
            || -> CliResult<()> { Err(CliError::failure("late completed report failure")) },
        );
        assert!(failed.is_err());
        assert_eq!(fs::read_to_string(&generated_source)?, "previous generated Rust");
        assert_eq!(fs::read_to_string(&native_output)?, "previous native artifact");
        assert_eq!(fs::read(&marker)?, previous_marker);
        assert_eq!(fs::read_to_string(&receipt_path)?, "previous backend receipt");
        assert_eq!(fs::read_to_string(&cache_object)?, "immutable package cache");

        let original_digest = selected.payload.files[1].digest.clone();
        selected.payload.files[1].digest = format!("sha256:{}", "0".repeat(64));
        let failed = materialize_completed_library_outputs(
            project.path(),
            std::slice::from_ref(&selected),
            &backend_receipt,
            || Ok(()),
        );
        selected.payload.files[1].digest = original_digest;
        let Err(error) = failed else {
            return Err("mismatched second stored file unexpectedly materialized".into());
        };
        assert!(error.message.contains("digest differs"));
        assert_eq!(fs::read_to_string(&generated_source)?, "previous generated Rust");
        assert_eq!(fs::read_to_string(&native_output)?, "previous native artifact");
        assert_eq!(fs::read(&marker)?, previous_marker);
        assert_eq!(fs::read_to_string(&receipt_path)?, "previous backend receipt");

        materialize_completed_library_outputs(
            project.path(),
            std::slice::from_ref(&selected),
            &backend_receipt,
            || Ok(()),
        )?;
        assert_eq!(fs::read_to_string(&cache_object)?, "immutable package cache");
        assert!(project_output_projection_is_current(project.path(), &selected)?);
        let root_modified = fs::metadata(&artifact_root)?.modified()?;
        let receipt_modified = fs::metadata(&receipt_path)?.modified()?;
        materialize_completed_library_outputs(
            project.path(),
            std::slice::from_ref(&selected),
            &backend_receipt,
            || Ok(()),
        )?;
        assert_eq!(fs::metadata(&artifact_root)?.modified()?, root_modified);
        assert_eq!(fs::metadata(&receipt_path)?.modified()?, receipt_modified);
        Ok(())
    }
}
