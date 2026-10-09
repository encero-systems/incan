//! Ordinary-error rollback of library and executable projections through the public replay publishers.

use incan_driver::backend::selection::{BackendKind, finalize_receipt, select_backend};
use incan_driver::build::backend_selection::default_backend_receipt_path;
use incan_driver::build::output_materialization::{
    materialize_completed_executable_output_with_completion, materialize_completed_library_outputs,
    materialize_project_output, project_output_projection_is_current,
};
use incan_driver::build::publication::{project_output_payload_for_bake, publish_project_output_loaf};
use incan_driver::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority,
};
use incan_driver::build::{
    OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OvenBakeProjectTarget, OvenProjectOutputBakeFile, OvenProjectOutputBakeRequest,
    OvenStoredProjectOutput,
};
use incan_driver::error::{CliError, CliResult};
use incan_frontend::diagnostics;
use incan_lang::version::INCAN_VERSION;
use oven_rustc::rustc::{OvenProjectInspectionAuthorityRef, resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::store::{OvenStore, OvenStoreLimits};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use std::fs;

/// Publish a small immutable fixture using the same authority and payload constructor as an explicit bake.
fn published_fixture(
    target: OvenBakeProjectTarget,
) -> Result<
    (
        tempfile::TempDir,
        tempfile::TempDir,
        OvenStoredProjectOutput,
        incan_driver::backend::selection::BackendExecutionReceipt,
    ),
    Box<dyn std::error::Error>,
> {
    let project = tempfile::tempdir()?;
    let (entry, native_relative, generated_relative, store_relative) = match target {
        OvenBakeProjectTarget::Library => (
            "src/lib.incn",
            "target/lib/oven/debug/libfixture.rlib",
            "target/lib/src/lib.rs",
            "generated/src/lib.rs",
        ),
        OvenBakeProjectTarget::Executable => (
            "src/main.incn",
            "target/incan/fixture/oven/debug/fixture",
            "target/incan/fixture/src/main.rs",
            "generated/src/main.rs",
        ),
    };
    let entrypoint = project.path().join(entry);
    let native = project.path().join(native_relative);
    let generated = project.path().join(generated_relative);
    fs::create_dir_all(entrypoint.parent().ok_or("source has no parent")?)?;
    fs::create_dir_all(native.parent().ok_or("native output has no parent")?)?;
    fs::create_dir_all(generated.parent().ok_or("generated source has no parent")?)?;
    fs::write(
        project.path().join("loaf.toml"),
        "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(&entrypoint, "pub def value() -> int:\n    return 42\n")?;
    fs::write(&generated, "pub fn value() -> i64 { 42 }\n")?;
    fs::write(&native, "fixture native bytes")?;
    let rustc = resolve_active_rustc()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &generated)
        .with_build_unit_input("compiler-version", INCAN_VERSION),
    )?;
    let files = vec![
        OvenProjectOutputBakeFile {
            source_path: generated.clone(),
            caller_relative_path: generated_relative.into(),
            output_relative_path: store_relative.into(),
        },
        OvenProjectOutputBakeFile {
            source_path: native.clone(),
            caller_relative_path: native_relative.into(),
            output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.into(),
        },
    ];
    let backend_receipt = finalize_receipt(
        &select_backend(BackendKind::Legacy, "sha256:fixture-source"),
        "sha256:fixture-output",
        diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )?;
    let payload = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
        project_root: project.path(),
        entrypoint: &entrypoint,
        target,
        receipt: &receipt,
        plan_identity: "fixture-plan".into(),
        profile: "debug",
        source_authority_digest: &digest_baked_project_source_authority(project.path())?,
        lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project.path())?,
        files: files.clone(),
        inspection_authority: OvenProjectInspectionAuthorityRef {
            identity: "sha256:fixture-authority".into(),
            receipt_identity: "sha256:fixture-authority-receipt".into(),
            build_unit_identity: "sha256:fixture-authority-unit".into(),
        },
        required_project_loafs: Vec::new(),
        package_loaf_store_relative_path: None,
        backend_receipt: backend_receipt.clone(),
        build_report: None,
    })?;
    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
    );
    let selected = publish_project_output_loaf(&store, &receipt, &payload, &files)?;
    Ok((project, store_root, selected, backend_receipt))
}

#[test]
fn failed_library_replay_restores_changed_and_absent_projection_markers() -> Result<(), Box<dyn std::error::Error>> {
    let (project, _store_root, selected, backend_receipt) = published_fixture(OvenBakeProjectTarget::Library)?;
    let native = project.path().join("target/lib/oven/debug/libfixture.rlib");
    let generated = project.path().join("target/lib/src/lib.rs");
    materialize_project_output(project.path(), &selected)?;
    let marker_directory = project.path().join(".incan/oven/project-output-projections");
    let markers = fs::read_dir(&marker_directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(markers.len(), 1);
    let marker = &markers[0];
    let backend_path = default_backend_receipt_path(project.path());
    fs::create_dir_all(backend_path.parent().ok_or("backend receipt has no parent")?)?;
    fs::write(&backend_path, "prior backend receipt")?;
    fs::write(&generated, "prior generated source")?;
    fs::write(&native, "prior native bytes")?;
    fs::write(marker, "prior projection marker")?;
    let cache = project.path().join("target/lib/oven/loafs/retained/object");
    fs::create_dir_all(cache.parent().ok_or("cache has no parent")?)?;
    fs::write(&cache, "retained immutable package cache")?;
    let late_failure = || -> CliResult<()> { Err(CliError::failure("late replay failure")) };
    let failed = materialize_completed_library_outputs(
        project.path(),
        std::slice::from_ref(&selected),
        &backend_receipt,
        late_failure,
    );
    assert!(failed.is_err());
    assert_eq!(fs::read_to_string(marker)?, "prior projection marker");
    assert_eq!(fs::read_to_string(&generated)?, "prior generated source");
    assert_eq!(fs::read_to_string(&native)?, "prior native bytes");
    assert_eq!(fs::read_to_string(&backend_path)?, "prior backend receipt");
    assert_eq!(fs::read_to_string(&cache)?, "retained immutable package cache");
    fs::remove_file(marker)?;
    assert!(
        materialize_completed_library_outputs(
            project.path(),
            std::slice::from_ref(&selected),
            &backend_receipt,
            late_failure
        )
        .is_err()
    );
    assert!(!marker.exists());
    assert_eq!(fs::read_to_string(&native)?, "prior native bytes");
    materialize_completed_library_outputs(
        project.path(),
        std::slice::from_ref(&selected),
        &backend_receipt,
        || Ok(()),
    )?;
    assert!(project_output_projection_is_current(project.path(), &selected)?);
    let marker_modified = fs::metadata(marker)?.modified()?;
    let native_modified = fs::metadata(&native)?.modified()?;
    materialize_completed_library_outputs(
        project.path(),
        std::slice::from_ref(&selected),
        &backend_receipt,
        || Ok(()),
    )?;
    assert_eq!(fs::metadata(marker)?.modified()?, marker_modified);
    assert_eq!(fs::metadata(&native)?.modified()?, native_modified);
    Ok(())
}

#[test]
fn failed_executable_replay_restores_files_markers_and_backend_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let (project, _store_root, selected, backend_receipt) = published_fixture(OvenBakeProjectTarget::Executable)?;
    let native = project.path().join("target/incan/fixture/oven/debug/fixture");
    let generated = project.path().join("target/incan/fixture/src/main.rs");
    materialize_project_output(project.path(), &selected)?;
    let marker = fs::read_dir(project.path().join(".incan/oven/project-output-projections"))?
        .next()
        .ok_or("projection marker missing")??
        .path();
    let backend = default_backend_receipt_path(project.path());
    fs::create_dir_all(backend.parent().ok_or("backend receipt has no parent")?)?;
    fs::write(&native, "prior executable")?;
    fs::write(&generated, "prior generated source")?;
    fs::write(&marker, "prior projection marker")?;
    fs::write(&backend, "prior backend receipt")?;
    let immutable_alias = project.path().join("prior-native-alias");
    fs::hard_link(&native, &immutable_alias)?;
    let mut permissions = fs::metadata(&native)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&native, permissions)?;
    let failed = materialize_completed_executable_output_with_completion(
        project.path(),
        &selected,
        &backend_receipt,
        || -> CliResult<()> {
            assert!(project_output_projection_is_current(project.path(), &selected)?);
            fs::write(&backend, "late callback changed receipt")
                .map_err(|error| CliError::failure(error.to_string()))?;
            Err(CliError::failure("late executable replay failure"))
        },
    );
    assert!(failed.is_err());
    assert_eq!(fs::read_to_string(&native)?, "prior executable");
    assert_eq!(fs::read_to_string(&immutable_alias)?, "prior executable");
    assert!(fs::metadata(&native)?.permissions().readonly());
    assert_eq!(fs::read_to_string(&generated)?, "prior generated source");
    assert_eq!(fs::read_to_string(&marker)?, "prior projection marker");
    assert_eq!(fs::read_to_string(&backend)?, "prior backend receipt");
    fs::remove_file(&native)?;
    fs::remove_file(&generated)?;
    fs::remove_file(&marker)?;
    fs::remove_file(&backend)?;
    assert!(
        materialize_completed_executable_output_with_completion(
            project.path(),
            &selected,
            &backend_receipt,
            || -> CliResult<()> { Err(CliError::failure("late absent-file replay failure")) }
        )
        .is_err()
    );
    for path in [&native, &generated, &marker, &backend] {
        assert!(!path.exists());
    }
    materialize_completed_executable_output_with_completion(project.path(), &selected, &backend_receipt, || Ok(()))?;
    let modified = [&native, &generated, &marker, &backend]
        .into_iter()
        .map(|path| fs::metadata(path)?.modified())
        .collect::<Result<Vec<_>, _>>()?;
    materialize_completed_executable_output_with_completion(project.path(), &selected, &backend_receipt, || Ok(()))?;
    let repeated = [&native, &generated, &marker, &backend]
        .into_iter()
        .map(|path| fs::metadata(path)?.modified())
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(modified, repeated);
    assert_eq!(fs::read_to_string(&immutable_alias)?, "prior executable");
    Ok(())
}
