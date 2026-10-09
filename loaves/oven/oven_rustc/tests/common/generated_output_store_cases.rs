//! Shared runtime cases for generated native-output storage.
use super::native_rustc as executor;
use executor::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenRustcError,
    OvenTrustedDirectRustcTargetRequest, rustc_host_target,
};
use oven_store::store::{OvenStore, OvenStoreLimits};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Changed direct callers replace immutable projections without modifying their aliases or losing failed-build bytes.
pub(super) fn direct_binary_rebuild_replaces_readonly_output_and_preserves_failed_build()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    let source = project.path().join("main.rs");
    fs::write(&source, "fn main() { println!(\"42\"); }\n")?;
    let rustc = rustc_path()?;
    let input = OvenGeneratedProjectRequest::new(
        project.path(),
        "direct_rebuild",
        "0.1.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("generated-root", &source);
    let receipt = receipt_generated_project(&input)?;
    let artifacts = empty_manifest(&receipt);
    let executable = output.path().join("program");
    let sidecar = output.path().join("program.oven-output.json");
    let request = OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        output: &executable,
        crate_name: "direct_rebuild",
        edition: "2024",
        source_evidence_key: "generated-root",
        features: &[],
        prefer_dynamic: false,
    };
    let original = executor::bake_trusted_direct_rustc_run(&request)?;
    let original_bytes = fs::read(&executable)?;
    let immutable = output.path().join("immutable-owner");
    fs::hard_link(&executable, &immutable)?;
    let mut permissions = fs::metadata(&immutable)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&immutable, permissions)?;
    fs::write(&source, "fn main() { println!(\"43\"); }\n")?;
    let changed_receipt = receipt_generated_project(&input)?;
    let changed_artifacts = empty_manifest(&changed_receipt);
    let changed_request = OvenTrustedDirectRustcTargetRequest {
        receipt: &changed_receipt,
        artifacts: &changed_artifacts,
        ..request
    };
    let rebuilt = executor::bake_trusted_direct_rustc_run(&changed_request)?;
    assert!(!rebuilt.reused);
    assert_ne!(rebuilt.output_digest, original.output_digest);
    assert_eq!(Command::new(&executable).output()?.stdout, b"43\n");
    assert!(!fs::metadata(&executable)?.permissions().readonly());
    assert_eq!(fs::read(&immutable)?, original_bytes);
    assert!(fs::metadata(&immutable)?.permissions().readonly());
    assert!(executor::bake_trusted_direct_rustc_run(&changed_request)?.reused);

    let admitted_bytes = fs::read(&executable)?;
    let admitted_sidecar = fs::read(&sidecar)?;
    fs::write(&source, "fn main() { missing_function(); }\n")?;
    let invalid_receipt = receipt_generated_project(&input)?;
    let invalid_artifacts = empty_manifest(&invalid_receipt);
    let invalid_request = OvenTrustedDirectRustcTargetRequest {
        receipt: &invalid_receipt,
        artifacts: &invalid_artifacts,
        ..request
    };
    assert!(matches!(
        executor::bake_trusted_direct_rustc_run(&invalid_request),
        Err(OvenRustcError::CompilationFailed { .. })
    ));
    assert_eq!(fs::read(&executable)?, admitted_bytes);
    assert_eq!(fs::read(&sidecar)?, admitted_sidecar);
    assert_eq!(Command::new(&executable).output()?.stdout, b"43\n");
    Ok(())
}

/// Native harnesses remain distinct from ordinary binaries and reuse their earlier bytes after source restoration.
pub(super) fn generated_test_store_keeps_harness_identity_and_restored_sources()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    let source = project.path().join("harness.rs");
    let original = "fn value() -> u8 { 42 }\nfn main() { println!(\"ordinary binary\"); }\n#[test] fn selected_case() { assert_eq!(value(), 42); }\n";
    fs::write(&source, original)?;
    let rustc = rustc_path()?;
    let input = OvenGeneratedProjectRequest::new(
        project.path(),
        "test_store",
        "0.1.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("test-root", &source);
    let receipt = receipt_generated_project(&input)?;
    let artifacts = empty_manifest(&receipt);
    let executable = output.path().join("harness");
    let request = OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        output: &executable,
        crate_name: "test_store",
        edition: "2024",
        source_evidence_key: "test-root",
        features: &[],
        prefer_dynamic: false,
    };
    let ordinary = executor::bake_trusted_direct_rustc_run_in_store(&request, &store)?;
    assert!(!ordinary.reused);
    let cold = executor::bake_trusted_direct_rustc_test_in_store(&request, &store)?;
    assert!(!cold.reused, "an ordinary binary cannot satisfy a harness request");
    assert_ne!(cold.output_digest, ordinary.output_digest);
    let selected = Command::new(&executable).args(["--exact", "selected_case"]).output()?;
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stdout)
    );
    assert!(String::from_utf8(selected.stdout)?.contains("1 passed"));
    let warm = executor::bake_trusted_direct_rustc_test_in_store(&request, &store)?;
    assert!(warm.reused);
    assert_eq!(warm.output_digest, cold.output_digest);
    fs::write(&source, original.replace("{ 42 }", "{ 43 }"))?;
    assert!(matches!(
        executor::bake_trusted_direct_rustc_test_in_store(&request, &store),
        Err(OvenRustcError::SourceEvidenceMismatch { .. })
    ));
    let changed_receipt = receipt_generated_project(&input)?;
    let changed_artifacts = empty_manifest(&changed_receipt);
    let changed_request = OvenTrustedDirectRustcTargetRequest {
        receipt: &changed_receipt,
        artifacts: &changed_artifacts,
        ..request
    };
    let changed = executor::bake_trusted_direct_rustc_test_in_store(&changed_request, &store)?;
    assert!(!changed.reused);
    assert_ne!(changed.output_digest, cold.output_digest);
    let selected = Command::new(&executable).args(["--exact", "selected_case"]).output()?;
    assert!(
        !selected.status.success(),
        "edited behavior must reach the unchanged test oracle"
    );
    fs::write(&source, original)?;
    let restored = executor::bake_trusted_direct_rustc_test_in_store(&request, &store)?;
    assert!(
        restored.reused,
        "restoring an admitted source must not compile the same harness again"
    );
    assert_eq!(restored.output_digest, cold.output_digest);
    let selected = Command::new(&executable).args(["--exact", "selected_case"]).output()?;
    assert!(selected.status.success());
    let second = output.path().join("second-harness");
    let relocated = OvenTrustedDirectRustcTargetRequest {
        output: &second,
        ..request
    };
    let shared = executor::bake_trusted_direct_rustc_test_in_store(&relocated, &store)?;
    assert!(shared.reused);
    assert_eq!(shared.output_digest, cold.output_digest);
    assert!(
        Command::new(&second)
            .args(["--exact", "selected_case"])
            .status()?
            .success()
    );
    Ok(())
}

/// Fresh caller directories reuse an admitted library, while changed source cannot borrow its cached bytes.
pub(super) fn generated_library_store_reuses_across_outputs_and_refuses_stale_sources()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
    );
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let source = project.path().join("src/library.rs");
    fs::write(&source, "pub fn value() -> u32 { 42 }\n")?;
    let rustc = rustc_path()?;
    let input = OvenGeneratedProjectRequest::new(
        project.path(),
        "library_store",
        "0.1.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("generated-library", &source);
    let receipt = receipt_generated_project(&input)?;
    let artifacts = empty_manifest(&receipt);
    let first_output = output.path().join("first.rlib");
    let second_output = output.path().join("second.rlib");
    let first = OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        output: &first_output,
        crate_name: "library_store",
        edition: "2024",
        source_evidence_key: "generated-library",
        features: &[],
        prefer_dynamic: false,
    };
    let cold = executor::bake_trusted_direct_rustc_library_in_store(&first, &store)?;
    assert!(!cold.reused);
    let second = OvenTrustedDirectRustcTargetRequest {
        output: &second_output,
        ..first
    };
    let warm = executor::bake_trusted_direct_rustc_library_in_store(&second, &store)?;
    assert!(warm.reused);
    assert_eq!(cold.output_digest, warm.output_digest);
    assert!(!warm.cargo_process_started);

    fs::write(&second_output, b"invalid caller projection")?;
    let repaired = executor::bake_trusted_direct_rustc_library_in_store(&second, &store)?;
    assert!(repaired.reused);
    assert_eq!(cold.output_digest, repaired.output_digest);
    let mut environment_plan = artifacts.materialize(artifact_root.path(), &receipt.intent)?;
    environment_plan
        .compile_environment
        .insert("CARGO_PKG_DESCRIPTION".to_string(), "different environment".to_string());
    let environment_request = OvenTrustedDirectRustcTargetRequest {
        artifact_plan: Some(&environment_plan),
        ..second
    };
    let environment_changed = executor::bake_trusted_direct_rustc_library_in_store(&environment_request, &store)?;
    assert!(
        !environment_changed.reused,
        "a changed declared environment must invalidate store reuse"
    );
    fs::write(&source, "pub fn value() -> u32 { 43 }\n")?;
    assert!(matches!(
        executor::bake_trusted_direct_rustc_library_in_store(&second, &store),
        Err(OvenRustcError::SourceEvidenceMismatch { .. })
    ));
    let changed = receipt_generated_project(&input)?;
    let changed_artifacts = empty_manifest(&changed);
    let changed_request = OvenTrustedDirectRustcTargetRequest {
        receipt: &changed,
        artifacts: &changed_artifacts,
        ..second
    };
    let rebuilt = executor::bake_trusted_direct_rustc_library_in_store(&changed_request, &store)?;
    assert!(!rebuilt.reused);
    assert_ne!(cold.output_digest, rebuilt.output_digest);
    Ok(())
}

/// Shared executables retain executable permissions and refuse changed source, environment and linker authority.
pub(super) fn generated_binary_store_reuses_executable_bytes_across_outputs() -> Result<(), Box<dyn std::error::Error>>
{
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(32 * 1024 * 1024, 32 * 1024 * 1024, 32 * 1024 * 1024),
    );
    write_project(project.path())?;
    let source = project.path().join("program.rs");
    fs::write(
        &source,
        "fn main() { println!(\"{}\", env!(\"CARGO_PKG_DESCRIPTION\")); }\n",
    )?;
    let rustc = rustc_path()?;
    let input = OvenGeneratedProjectRequest::new(
        project.path(),
        "binary_store",
        "0.1.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("generated-program", &source);
    let receipt = receipt_generated_project(&input)?;
    let mut artifacts = empty_manifest(&receipt);
    artifacts
        .compile_environment
        .insert("CARGO_PKG_DESCRIPTION".to_string(), "first environment".to_string());
    let first_output = output.path().join("first");
    let second_output = output.path().join("second");
    let first = OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        output: &first_output,
        crate_name: "binary_store",
        edition: "2024",
        source_evidence_key: "generated-program",
        features: &[],
        prefer_dynamic: false,
    };
    let cold = executor::bake_trusted_direct_rustc_run_in_store(&first, &store)?;
    assert!(!cold.reused);
    let second = OvenTrustedDirectRustcTargetRequest {
        output: &second_output,
        ..first
    };
    let warm = executor::bake_trusted_direct_rustc_run_in_store(&second, &store)?;
    assert!(warm.reused);
    assert_eq!(cold.output_digest, warm.output_digest);
    assert!(!warm.cargo_process_started);
    let executed = Command::new(&second_output).output()?;
    assert!(executed.status.success());
    assert_eq!(executed.stdout, b"first environment\n");
    assert!(!fs::metadata(&second_output)?.permissions().readonly());
    let modified = fs::metadata(&second_output)?.modified()?;
    let sidecar = second_output.with_file_name("second.oven-output.json");
    let sidecar_modified = fs::metadata(&sidecar)?.modified()?;
    let repeated = executor::bake_trusted_direct_rustc_run_in_store(&second, &store)?;
    assert!(repeated.reused);
    assert_eq!(fs::metadata(&second_output)?.modified()?, modified);
    assert_eq!(fs::metadata(&sidecar)?.modified()?, sidecar_modified);

    fs::write(&second_output, "tampered caller executable")?;
    let repaired = executor::bake_trusted_direct_rustc_run_in_store(&second, &store)?;
    assert!(repaired.reused);
    assert_eq!(Command::new(&second_output).output()?.stdout, b"first environment\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&second_output, fs::Permissions::from_mode(0o555))?;
        assert!(executor::bake_trusted_direct_rustc_run_in_store(&second, &store)?.reused);
        assert!(!fs::metadata(&second_output)?.permissions().readonly());
        let external = output.path().join("external");
        fs::write(&external, "external bytes")?;
        fs::remove_file(&second_output)?;
        std::os::unix::fs::symlink(&external, &second_output)?;
        assert!(executor::bake_trusted_direct_rustc_run_in_store(&second, &store)?.reused);
        assert!(!fs::symlink_metadata(&second_output)?.file_type().is_symlink());
        assert_eq!(fs::read_to_string(&external)?, "external bytes");
        assert!(Command::new(&second_output).output()?.status.success());
    }

    let mut environment_plan = artifacts.materialize(artifact_root.path(), &receipt.intent)?;
    environment_plan
        .compile_environment
        .insert("CARGO_PKG_DESCRIPTION".to_string(), "second environment".to_string());
    let changed_environment = OvenTrustedDirectRustcTargetRequest {
        artifact_plan: Some(&environment_plan),
        ..second
    };
    let changed = executor::bake_trusted_direct_rustc_run_in_store(&changed_environment, &store)?;
    assert!(!changed.reused);
    assert_eq!(Command::new(&second_output).output()?.stdout, b"second environment\n");
    assert!(executor::bake_trusted_direct_rustc_run_in_store(&changed_environment, &store)?.reused);

    let wrong_link = receipt_generated_project(
        &input
            .clone()
            .with_build_unit_input("link-closure", "sha256:wrong-link-closure"),
    )?;
    let wrong_artifacts = empty_manifest(&wrong_link);
    let substituted = OvenTrustedDirectRustcTargetRequest {
        receipt: &wrong_link,
        artifacts: &wrong_artifacts,
        ..second
    };
    assert!(matches!(
        executor::bake_trusted_direct_rustc_run_in_store(&substituted, &store),
        Err(OvenRustcError::InvalidInput {
            field: "link closure",
            ..
        })
    ));
    fs::write(&source, "fn main() { println!(\"changed source\"); }\n")?;
    assert!(matches!(
        executor::bake_trusted_direct_rustc_run_in_store(&second, &store),
        Err(OvenRustcError::SourceEvidenceMismatch { .. })
    ));
    let changed_receipt = receipt_generated_project(&input)?;
    let changed_artifacts = empty_manifest(&changed_receipt);
    let changed_source = OvenTrustedDirectRustcTargetRequest {
        receipt: &changed_receipt,
        artifacts: &changed_artifacts,
        ..second
    };
    let rebuilt = executor::bake_trusted_direct_rustc_run_in_store(&changed_source, &store)?;
    assert!(!rebuilt.reused);
    assert_eq!(Command::new(&second_output).output()?.stdout, b"changed source\n");
    let retained = OvenStore::new(store_root.path(), OvenStoreLimits::new(1, 1, 1));
    retained.prune()?;
    assert!(
        executor::bake_trusted_direct_rustc_run_in_store(&changed_source, &store)?.reused,
        "leased executable must survive collection"
    );
    Ok(())
}

fn empty_manifest(receipt: &oven_store::OvenReceipt) -> OvenRustcArtifactManifest {
    OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    }
}

fn rustc_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = Command::new("rustup").args(["which", "rustc"]).output()?;
    if !output.status.success() {
        return Err("rustup could not locate rustc".into());
    }
    let path = String::from_utf8(output.stdout)?;
    let path = PathBuf::from(path.trim());
    if !path.is_file() {
        return Err(format!("rustup returned a non-file rustc path: {}", path.display()).into());
    }
    Ok(path)
}

fn rustc_identity(rustc: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(rustc).arg("--version").output()?;
    if !output.status.success() {
        return Err(format!("rustc could not report its version: {}", rustc.display()).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn write_project(root: &Path) -> Result<(), std::io::Error> {
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"rustc_fixture\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(root.join("Cargo.lock"), "version = 4\n")?;
    fs::write(root.join("fixture.rs"), "pub fn fixture() {}\n")
}

/// Exercise real admitted units, including one repeated owner under distinct logical prefixes.
pub(super) fn shared_native_plan_retains_multi_store_owners_and_refuses_substitution()
-> Result<(), Box<dyn std::error::Error>> {
    use oven_rustc::plan::selection::select_receipt_direct_rustc_execution_plan;
    use oven_rustc::plan::shared::{OvenSharedNativePlan, OvenSharedNativeRoot};
    use oven_rustc::sdk_closure::{compile_local_sdk_facet, prepare_sdk_seed};
    use oven_store::store::{OvenArtifactKind, OvenArtifactPublishRequest};
    let root = tempfile::tempdir()?;
    let rustc = rustc_path()?;
    let seed = root.path().join("seed.json");
    fs::write(&seed, r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#)?;
    let mut closures = Vec::new();
    let mut references = Vec::new();
    let mut artifacts = None;
    let source = root.path().join("main.rs");
    fs::write(&source, "fn main() {}\n")?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            root.path(),
            "shared_owners",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &source),
    )?;
    let mut manifest = empty_manifest(&receipt);
    for (store_name, names) in [("first", vec!["alpha", "beta"]), ("second", vec!["gamma"])] {
        let output = root.path().join(store_name);
        let mut closure = prepare_sdk_seed(&seed, root.path(), &output, &rustc, root.path())?;
        for name in names {
            let project = root.path().join(name);
            fs::create_dir_all(project.join("src"))?;
            fs::write(
                project.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='lib'\nedition='2024'\n"
                ),
            )?;
            fs::write(project.join("src/lib.rs"), "pub fn value() -> u8 { 42 }\n")?;
            compile_local_sdk_facet(&mut closure, &project, &[], "target", &output, &rustc)?;
        }
        for unit in closure.units() {
            let native = unit.native_artifact()?;
            assert_eq!(
                native.binding.archive_digest,
                oven_rustc::sdk_closure::local_sdk_facet_source_digest(
                    &root.path().join(&native.binding.loaf),
                    root.path()
                )?,
                "mapped source hashing must retain the actual publisher identity"
            );
            let owner = OvenStore::new(
                output.join("store"),
                OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
            )
            .select_payloads_for_execution(std::slice::from_ref(&native.store_identity))?
            .pop()
            .ok_or("compiled native unit has no owner")?;
            for repetition in 0..if native.binding.loaf == "alpha" { 2 } else { 1 } {
                let prefix = format!("units/{}", references.len());
                manifest.dependency_search_paths.push(prefix.clone());
                for file in owner
                    .admitted_materialized_files()
                    .iter()
                    .filter(|f| !f.relative_path.starts_with("source/"))
                {
                    manifest
                        .supporting_artifacts
                        .push(executor::OvenRustcSupportingArtifact {
                            relative_path: format!("{prefix}/{}", file.relative_path),
                            digest: file.digest.clone(),
                        });
                }
                manifest.externs.push(executor::OvenRustcArtifactExtern {
                    crate_name: format!("{}_{repetition}", native.binding.loaf),
                    relative_path: format!("{prefix}/{}", native.relative_path),
                    digest: native.digest.clone(),
                });
                references.push(OvenSharedNativeRoot {
                    store: output.join("store"),
                    identity: native.store_identity.clone(),
                    receipt_identity: native.receipt_identity.clone(),
                    prefix,
                });
            }
        }
        closures.push(closure);
    }
    manifest.supporting_artifacts.retain(|f| {
        !manifest
            .externs
            .iter()
            .any(|external| external.relative_path == f.relative_path)
    });
    let shared = OvenSharedNativePlan {
        artifacts: manifest,
        shared_native_roots: references,
    };
    let payload = serde_json::to_value(&shared)?;
    for (name, change) in [
        ("valid", None),
        ("receipt", Some("receipt_identity")),
        ("identity", Some("identity")),
        ("digest", Some("digest")),
        ("prefix", Some("prefix")),
    ] {
        let store = OvenStore::new(
            root.path().join(name),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let mut value = payload.clone();
        if let Some(field) = change {
            if field == "digest" {
                value["externs"][0][field] = serde_json::json!(oven_store::digest_bytes(b"substituted"));
            } else if field == "prefix" {
                value["shared_native_roots"][1][field] = value["shared_native_roots"][0][field].clone();
            } else {
                value["shared_native_roots"][0][field] = serde_json::json!(oven_store::digest_bytes(b"substituted"));
            }
        }
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "sdk-native-consumer-plan".into(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&value)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        let selected = select_receipt_direct_rustc_execution_plan(&store, &receipt);
        if change.is_some() {
            assert!(selected.is_err(), "substitution {name} must refuse");
        } else {
            let selected = selected?.ok_or("shared consumer was not selected")?;
            assert_eq!(selected.artifact_plan.externs.len(), 4);
            assert_eq!(selected.artifact_plan.externs[0].1, selected.artifact_plan.externs[1].1);
            artifacts = Some(selected);
        }
    }
    drop(closures);
    let selected = artifacts.ok_or("valid consumer selection was lost")?;
    for name in ["first", "second"] {
        OvenStore::new(root.path().join(name).join("store"), OvenStoreLimits::new(1, 1, 1)).prune()?;
    }
    for (_, file) in &selected.artifact_plan.externs {
        assert!(file.is_file(), "leased output must survive pruning");
    }
    drop(selected);
    for name in ["first", "second"] {
        OvenStore::new(root.path().join(name).join("store"), OvenStoreLimits::new(1, 1, 1)).prune()?;
    }
    let valid_store = OvenStore::new(
        root.path().join("valid"),
        OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
    );
    assert!(
        select_receipt_direct_rustc_execution_plan(&valid_store, &receipt).is_err(),
        "missing native owners must refuse"
    );
    Ok(())
}
