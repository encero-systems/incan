//! Direct rustc and rustdoc execution regression tests.

use super::*;

#[test]
fn failed_direct_rustc_report_keeps_the_bounded_invocation() {
    let mut command = Command::new("rustc");
    command.args(["--crate-name", "closure_probe"]);
    let report = super::super::parse_rustc_diagnostics(b"", b"").with_invocation(&command);

    assert_eq!(
        report.invocation.as_deref(),
        Some("\"rustc\" \"--crate-name\" \"closure_probe\"")
    );
    assert_eq!(
        report.to_string(),
        "direct rustc invocation: \"rustc\" \"--crate-name\" \"closure_probe\""
    );
}

#[test]
fn native_runtime_rpaths_apply_only_to_the_host_native_target() {
    let host_architecture = std::env::consts::ARCH;

    #[cfg(target_os = "macos")]
    {
        assert!(is_host_native_unix_target(&format!("{host_architecture}-apple-darwin")));
        assert!(!is_host_native_unix_target(&format!("{host_architecture}-apple-ios")));
        assert!(!is_host_native_unix_target("aarch64-linux-android"));
    }

    #[cfg(target_os = "linux")]
    {
        let host_target = if cfg!(target_env = "gnu") {
            format!("{host_architecture}-unknown-linux-gnu")
        } else if cfg!(target_env = "musl") {
            format!("{host_architecture}-unknown-linux-musl")
        } else {
            String::new()
        };
        if !host_target.is_empty() {
            assert!(is_host_native_unix_target(&host_target));
        }
        assert!(!is_host_native_unix_target(&format!(
            "{host_architecture}-apple-darwin"
        )));
        assert!(
            !is_host_native_unix_target(&format!("{host_architecture}-unknown-linux-musl"))
                || cfg!(target_env = "musl")
        );
        assert!(!is_host_native_unix_target("aarch64-linux-android"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        assert!(!is_host_native_unix_target(&format!(
            "{host_architecture}-unknown-linux-gnu"
        )));
    }
}

#[test]
fn plan_selection_skips_a_legacy_manifest_schema_before_materializing_a_replacement()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let receipt = intent(project.path())?;
    let mut legacy = empty_manifest(&receipt);
    legacy.schema_version = super::super::OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION - 1;
    let current = empty_manifest(&receipt);
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "legacy-alpha".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&legacy)?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    let replacement = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "current-alpha".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&current)?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;

    assert_eq!(
        select_direct_rustc_plan_identity(&store, &receipt)?,
        replacement.identity
    );
    Ok(())
}

#[test]
fn direct_rustc_test_runs_without_cargo_in_the_consumer_environment() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    write_project(project.path())?;
    let source = output.path().join("consumer.rs");
    fs::write(
        &source,
        "#[test]\nfn cargo_is_not_visible_to_the_consumer() { assert!(option_env!(\"CARGO\").is_none()); assert!(option_env!(\"CARGO_PKG_NAME\").is_none()); }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("direct-rustc-source", &source),
    )?;
    let artifact_root = tempfile::tempdir()?;
    let request = OvenDirectRustcTestRequest {
        receipt: receipt.clone(),
        artifacts: OvenRustcArtifactManifest {
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
        },
        artifact_root: artifact_root.path().to_path_buf(),
        rustc,
        source,
        output: output.path().join("consumer-test"),
        crate_name: "oven_consumer".to_string(),
        edition: "2024".to_string(),
        source_evidence_key: "direct-rustc-source".to_string(),
    };

    let bake = bake_direct_rustc_test(&request)?;
    assert!(!bake.reused);
    assert!(Command::new(&bake.output).status()?.success());
    let reused = bake_direct_rustc_test(&request)?;
    assert!(reused.reused);
    assert_eq!(reused.output, bake.output);
    Ok(())
}

#[test]
fn trusted_direct_rustc_runs_a_proc_macro_libtest_with_its_selected_dynamic_toolchain()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let source = output.path().join("proc-macro-test.rs");
    fs::write(
        &source,
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn passthrough(input: TokenStream) -> TokenStream { input }\n#[test]\nfn direct_proc_macro_test() {}\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("proc-macro-source", &source),
    )?;
    let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_manifest(&receipt),
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        output: &output.path().join("proc-macro-test"),
        crate_name: "oven_proc_macro_test",
        edition: "2024",
        source_evidence_key: "proc-macro-source",
        features: &[],
        prefer_dynamic: true,
    })?;
    #[cfg(unix)]
    {
        let output = Command::new("/bin/sh")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-c", "exec \"$1\" --list --format terse", "sh"])
            .arg(&bake.output)
            .output()?;
        assert!(
            output.status.success(),
            "a dynamic direct-Rustc test must survive a shell hop without ambient loader state: {}",
            combined_process_output(&output.stdout, &output.stderr)
        );
    }
    let (name, value) = rustc_dynamic_library_environment(&rustc)?;
    let report = run_native_test_batch_all(&bake.output, &BTreeMap::from([(name, value)]))?;
    assert!(report.success);
    assert_eq!(report.inventory.names, ["direct_proc_macro_test"]);
    Ok(())
}

#[test]
fn trusted_direct_rustc_materializes_a_reusable_library_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let source = project.path().join("src/materialized.rs");
    fs::write(&source, "pub fn oven_materialized() -> u32 { 42 }\n")?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("materialized-library", &source),
    )?;
    let request = OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_manifest(&receipt),
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        output: &output.path().join("liboven_materialized.rlib"),
        crate_name: "oven_materialized",
        edition: "2024",
        source_evidence_key: "materialized-library",
        features: &[],
        prefer_dynamic: false,
    };

    let bake = bake_trusted_direct_rustc_library(&request)?;
    assert!(!bake.reused);
    assert!(bake.output.is_file());
    let sidecar = super::super::caller_output_receipt_path(&bake.output)?;
    let original_output = fs::read(&bake.output)?;
    let original_sidecar = fs::read(&sidecar)?;
    let output_modified = fs::metadata(&bake.output)?.modified()?;
    let sidecar_modified = fs::metadata(&sidecar)?.modified()?;
    let reused = bake_trusted_direct_rustc_library(&request)?;
    assert!(reused.reused);
    assert_eq!(reused.output, bake.output);
    assert_eq!(reused.output_digest, bake.output_digest);
    assert_eq!(fs::read(&bake.output)?, original_output);
    assert_eq!(fs::read(&sidecar)?, original_sidecar);
    assert_eq!(fs::metadata(&bake.output)?.modified()?, output_modified);
    assert_eq!(fs::metadata(&sidecar)?.modified()?, sidecar_modified);

    // A still-valid input sidecar cannot authorize replacement output bytes.
    fs::write(&bake.output, b"changed output bytes")?;
    let rebuilt = bake_trusted_direct_rustc_library(&request)?;
    assert!(!rebuilt.reused);
    assert!(!rebuilt.cargo_process_started);
    assert_eq!(rebuilt.output_digest, bake.output_digest);
    assert_eq!(rebuilt.output_digest, digest_bytes(&fs::read(&rebuilt.output)?));
    let record: serde_json::Value = serde_json::from_slice(&fs::read(&sidecar)?)?;
    assert_eq!(record["output_digest"], rebuilt.output_digest);
    assert_eq!(
        record["schema_version"],
        super::super::OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION
    );

    let mut missing_digest = record.clone();
    missing_digest
        .as_object_mut()
        .ok_or("output sidecar must be an object")?
        .remove("output_digest");
    let mut legacy = missing_digest.clone();
    legacy["schema_version"] = serde_json::json!(2);
    let mut malformed_digest = record.clone();
    malformed_digest["output_digest"] = serde_json::json!(42);
    let mut wrong_digest = record.clone();
    wrong_digest["output_digest"] = serde_json::json!(digest_bytes(b"another output"));
    let mut wrong_inputs = record.clone();
    wrong_inputs["receipt_identity"] = serde_json::json!(digest_bytes(b"another receipt"));
    for (case, invalid_record) in [
        ("legacy sidecar", legacy),
        ("missing digest", missing_digest),
        ("malformed digest", malformed_digest),
        ("wrong digest", wrong_digest),
        ("wrong input receipt", wrong_inputs),
    ] {
        fs::write(&sidecar, serde_json::to_vec(&invalid_record)?)?;
        let rebuilt = bake_trusted_direct_rustc_library(&request)?;
        assert!(!rebuilt.reused, "{case} must rebuild");
        assert_eq!(rebuilt.output_digest, bake.output_digest, "{case}");
        let warm = bake_trusted_direct_rustc_library(&request)?;
        assert!(warm.reused, "{case} must publish reusable repaired evidence");
        assert_eq!(warm.output_digest, rebuilt.output_digest, "{case}");
    }
    fs::write(&sidecar, b"{interrupted sidecar")?;
    assert!(!bake_trusted_direct_rustc_library(&request)?.reused);
    Ok(())
}

#[test]
fn transitive_caller_owned_library_keeps_its_search_path_and_reuse_evidence_private()
-> Result<(), Box<dyn std::error::Error>> {
    let output = tempfile::tempdir()?;
    let transitive = output.path().join("libprovider_child.rlib");
    fs::write(&transitive, "receipt-bound child")?;
    let transitive_digest = digest_bytes(b"receipt-bound child");
    let mut plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };

    attach_caller_owned_rustc_libraries(
        &mut plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "provider_child".to_string(),
            output: transitive.clone(),
            digest: transitive_digest.clone(),
            expose_extern: false,
        }],
    )?;

    assert!(plan.externs.is_empty());
    assert_eq!(plan.dependency_search_paths, vec![output.path().to_path_buf()]);
    assert_eq!(plan.caller_owned_library_digests.len(), 1);
    assert_eq!(
        plan.caller_owned_library_digests
            .get(&format!("transitive:provider_child:{transitive_digest}")),
        Some(&transitive_digest),
    );
    assert!(
        plan.caller_owned_library_digests
            .keys()
            .all(|key| key.starts_with("transitive:provider_child:"))
    );
    Ok(())
}

#[test]
fn one_sealed_artifact_reached_twice_under_one_alias_is_attached_once_issue1459()
-> Result<(), Box<dyn std::error::Error>> {
    let output = tempfile::tempdir()?;
    let stock = output.path().join("libstock.rlib");
    fs::write(&stock, "sealed catalog")?;
    let digest = digest_bytes(b"sealed catalog");
    let library = |output: &Path| OvenCallerOwnedRustcLibrary {
        crate_name: "stock".to_string(),
        output: output.to_path_buf(),
        digest: digest.clone(),
        expose_extern: true,
    };
    let mut plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    // The consumer names `stock` directly and its intermediate provider names the same sealed artifact `stock`.
    attach_caller_owned_rustc_libraries(&mut plan, &[library(&stock), library(&stock)])?;
    assert_eq!(plan.externs.len(), 1, "one artifact, one extern: {:?}", plan.externs);

    // A different artifact under the same alias is still the conflict it always was.
    let other = output.path().join("libother.rlib");
    fs::write(&other, "a different catalog")?;
    let conflict = attach_caller_owned_rustc_libraries(
        &mut plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "stock".to_string(),
            output: other,
            digest: digest_bytes(b"a different catalog"),
            expose_extern: true,
        }],
    );
    assert!(
        conflict
            .as_ref()
            .is_err_and(|error| error.to_string().contains("duplicates direct-Rustc extern `stock`")),
        "{conflict:?}"
    );
    Ok(())
}

#[test]
fn trusted_direct_rustc_links_an_oven_materialized_library_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let first_library_source = project.path().join("src/materialized_one.rs");
    let second_library_source = project.path().join("src/materialized_two.rs");
    let binary_source = project.path().join("src/consumer.rs");
    fs::write(&first_library_source, "pub fn answer() -> u32 { 42 }\n")?;
    fs::write(&second_library_source, "pub fn answer() -> u32 { 43 }\n")?;
    fs::write(
        &binary_source,
        "fn main() { println!(\"{}\", oven_materialized::answer()); }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("materialized-library-one", &first_library_source)
        .with_generated_source("materialized-library-two", &second_library_source)
        .with_generated_source("materialized-consumer", &binary_source),
    )?;
    let empty_artifacts = empty_manifest(&receipt);
    let library = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &first_library_source,
        output: &output.path().join("liboven_materialized_one.rlib"),
        crate_name: "oven_materialized",
        edition: "2024",
        source_evidence_key: "materialized-library-one",
        features: &[],
        prefer_dynamic: false,
    })?;
    let mut consumer_plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    attach_caller_owned_rustc_libraries(
        &mut consumer_plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "oven_materialized".to_string(),
            output: library.output.clone(),
            digest: library.output_digest.clone(),
            expose_extern: true,
        }],
    )?;
    assert_eq!(
        consumer_plan.caller_owned_library_digests.get("oven_materialized"),
        Some(&library.output_digest),
    );
    let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&consumer_plan),
        rustc: &rustc,
        source: &binary_source,
        output: &output.path().join("oven-consumer"),
        crate_name: "oven_consumer",
        edition: "2024",
        source_evidence_key: "materialized-consumer",
        features: &[],
        prefer_dynamic: false,
    })?;
    let first_run = Command::new(&consumer.output).output()?;
    assert!(first_run.status.success());
    assert_eq!(String::from_utf8(first_run.stdout)?.trim(), "42");

    let replacement_library = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &second_library_source,
        output: &output.path().join("liboven_materialized_two.rlib"),
        crate_name: "oven_materialized",
        edition: "2024",
        source_evidence_key: "materialized-library-two",
        features: &[],
        prefer_dynamic: false,
    })?;
    let mut replacement_plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    attach_caller_owned_rustc_libraries(
        &mut replacement_plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "oven_materialized".to_string(),
            output: replacement_library.output.clone(),
            digest: replacement_library.output_digest.clone(),
            expose_extern: true,
        }],
    )?;
    let rebuilt_consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&replacement_plan),
        rustc: &rustc,
        source: &binary_source,
        output: &output.path().join("oven-consumer"),
        crate_name: "oven_consumer",
        edition: "2024",
        source_evidence_key: "materialized-consumer",
        features: &[],
        prefer_dynamic: false,
    })?;
    assert!(!rebuilt_consumer.reused);
    let second_run = Command::new(&rebuilt_consumer.output).output()?;
    assert!(second_run.status.success());
    assert_eq!(String::from_utf8(second_run.stdout)?.trim(), "43");
    Ok(())
}

#[test]
fn trusted_direct_rustc_links_an_oven_materialized_dylib_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let library_source = project.path().join("src/materialized.rs");
    let binary_source = project.path().join("src/consumer.rs");
    fs::write(&library_source, "pub fn answer() -> u32 { 42 }\n")?;
    fs::write(
        &binary_source,
        "fn main() { println!(\"{}\", oven_materialized::answer()); }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("materialized-dylib", &library_source)
        .with_generated_source("dylib-consumer", &binary_source),
    )?;
    let empty_artifacts = empty_manifest(&receipt);
    let dylib = bake_trusted_direct_rustc_dylib(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &library_source,
        output: &output
            .path()
            .join(format!("liboven_materialized{}", std::env::consts::DLL_SUFFIX)),
        crate_name: "oven_materialized",
        edition: "2024",
        source_evidence_key: "materialized-dylib",
        features: &[],
        prefer_dynamic: true,
    })?;
    let mut consumer_plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    attach_caller_owned_rustc_libraries(
        &mut consumer_plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "oven_materialized".to_string(),
            output: dylib.output.clone(),
            digest: dylib.output_digest.clone(),
            expose_extern: true,
        }],
    )?;
    let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&consumer_plan),
        rustc: &rustc,
        source: &binary_source,
        output: &output.path().join("oven-dylib-consumer"),
        crate_name: "oven_dylib_consumer",
        edition: "2024",
        source_evidence_key: "dylib-consumer",
        features: &[],
        prefer_dynamic: true,
    })?;
    let (name, toolchain_libraries) = rustc_dynamic_library_environment(&rustc)?;
    let dylib_directory = dylib.output.parent().ok_or("dylib parent missing")?;
    let search_path = std::env::join_paths(
        std::iter::once(dylib_directory.to_path_buf()).chain(std::env::split_paths(&toolchain_libraries)),
    )?;
    let result = Command::new(&consumer.output).env(name, search_path).output()?;
    assert!(result.status.success());
    assert_eq!(String::from_utf8(result.stdout)?.trim(), "42");
    Ok(())
}

#[test]
fn trusted_direct_rustc_links_an_oven_materialized_proc_macro_without_cargo() -> Result<(), Box<dyn std::error::Error>>
{
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let macro_source = project.path().join("src/oven_macros.rs");
    let consumer_source = project.path().join("src/consumer.rs");
    fs::write(
        &macro_source,
        "use proc_macro::{Literal, TokenStream, TokenTree};\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream { TokenStream::from(TokenTree::Literal(Literal::u32_unsuffixed(43))) }\n",
    )?;
    fs::write(
        &consumer_source,
        "use oven_macros::answer;\nfn main() { println!(\"{}\", answer!()); }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("materialized-proc-macro", &macro_source)
        .with_generated_source("proc-macro-consumer", &consumer_source),
    )?;
    let empty_artifacts = empty_manifest(&receipt);
    let proc_macro = bake_trusted_direct_rustc_proc_macro(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &macro_source,
        output: &output
            .path()
            .join(format!("liboven_macros{}", std::env::consts::DLL_SUFFIX)),
        crate_name: "oven_macros",
        edition: "2024",
        source_evidence_key: "materialized-proc-macro",
        features: &[],
        prefer_dynamic: false,
    })?;
    let mut consumer_plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    attach_caller_owned_rustc_libraries(
        &mut consumer_plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "oven_macros".to_string(),
            output: proc_macro.output.clone(),
            digest: proc_macro.output_digest.clone(),
            expose_extern: true,
        }],
    )?;
    let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &empty_artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&consumer_plan),
        rustc: &rustc,
        source: &consumer_source,
        output: &output.path().join("oven-proc-macro-consumer"),
        crate_name: "oven_proc_macro_consumer",
        edition: "2024",
        source_evidence_key: "proc-macro-consumer",
        features: &[],
        prefer_dynamic: false,
    })?;
    let output = Command::new(&consumer.output).output()?;
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?.trim(), "43");
    Ok(())
}

#[test]
fn trusted_rustdoc_runs_a_receipt_bound_doctest_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let source = project.path().join("src/doctest.rs");
    fs::write(
        &source,
        "//! ```\n//! assert!(std::env::var_os(\"CARGO\").is_none());\n//! ```\npub struct DoctestFixture;\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("doctest-source", &source),
    )?;
    let mut artifacts = empty_manifest(&receipt);
    artifacts
        .compile_environment
        .insert("CARGO_MANIFEST_DIR".to_string(), "@oven-source-ancestor:2".to_string());
    let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        temporary_directory: &output.path().join("rustdoc-temporary"),
        crate_name: "oven_doctest_fixture",
        edition: "2024",
        source_evidence_key: "doctest-source",
        features: &[],
        is_proc_macro: false,
        prefer_dynamic: false,
        timeout: None,
    })?;
    assert!(report.output.contains("test result: ok"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn trusted_rustdoc_timeout_terminates_a_stalled_doctest_descendant() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};

    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let source = project.path().join("src/stalled_doctest.rs");
    fs::write(
        &source,
        "//! ```\n//! assert!(true);\n//! ```\npub struct StalledDoctest;\n",
    )?;

    let sysroot = output.path().join("sysroot");
    let rustc = output.path().join("rustc");
    let rustdoc = sysroot.join("bin/rustdoc");
    let descendant_started = output.path().join("descendant-started");
    fs::create_dir_all(rustdoc.parent().ok_or("Rustdoc parent missing")?)?;
    fs::write(
        &rustc,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' 'rustc oven-timeout-fixture'\n  exit 0\nfi\nif [ \"$1\" = \"-vV\" ]; then\n  printf '%s\\n' 'rustc oven-timeout-fixture' 'host: fixture-target'\n  exit 0\nfi\nif [ \"$1\" = \"--print\" ] && [ \"$2\" = \"sysroot\" ]; then\n  printf '%s\\n' \"{}\"\n  exit 0\nfi\nexit 97\n",
            sysroot.display(),
        ),
    )?;
    fs::write(
        &rustdoc,
        format!(
            "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\nwait\n",
            descendant_started.display(),
        ),
    )?;
    for executable in [&rustc, &rustdoc] {
        let mut permissions = fs::metadata(executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions)?;
    }
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            "fixture-target",
            "rustc oven-timeout-fixture",
            "release",
            Vec::new(),
        )
        .with_generated_source("stalled-doctest", &source),
    )?;

    let started = Instant::now();
    let Err(error) = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
        receipt: &receipt,
        artifacts: &empty_manifest(&receipt),
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        temporary_directory: &output.path().join("rustdoc-temporary"),
        crate_name: "stalled_doctest",
        edition: "2024",
        source_evidence_key: "stalled-doctest",
        features: &[],
        is_proc_macro: false,
        prefer_dynamic: false,
        timeout: Some(Duration::from_secs(10)),
    }) else {
        return Err("stalled Rustdoc unexpectedly completed within its receipt-bound root deadline".into());
    };
    assert!(descendant_started.is_file(), "fake Rustdoc descendant was not started");
    assert!(matches!(error, OvenRustcError::RustdocTestFailed { .. }));
    assert!(error.to_string().contains("timed out after 10000ms"), "{error}");
    assert!(error.to_string().contains(&source.display().to_string()), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "stalled doctest descendant outlived the Rustdoc deadline"
    );
    Ok(())
}

#[test]
fn trusted_rustdoc_uses_a_composed_plan_without_materializing_a_thin_artifact_root()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let thin_artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let source = project.path().join("src/composed_doctest.rs");
    fs::write(
        &source,
        "//! ```\n//! assert_eq!(2 + 2, 4);\n//! ```\npub struct DoctestFixture;\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("composed-doctest-source", &source),
    )?;
    let mut thin_artifacts = empty_manifest(&receipt);
    thin_artifacts.supporting_artifacts.push(OvenRustcSupportingArtifact {
        relative_path: "missing-foundation/libfixture.rlib".to_string(),
        digest: "sha256:fixture".to_string(),
    });
    let composed_plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };

    let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
        receipt: &receipt,
        artifacts: &thin_artifacts,
        artifact_root: thin_artifact_root.path(),
        artifact_plan: Some(&composed_plan),
        rustc: &rustc,
        source: &source,
        temporary_directory: &output.path().join("rustdoc-temporary"),
        crate_name: "oven_composed_doctest_fixture",
        edition: "2024",
        source_evidence_key: "composed-doctest-source",
        features: &[],
        is_proc_macro: false,
        prefer_dynamic: false,
        timeout: None,
    })?;
    assert!(report.output.contains("test result: ok"));
    Ok(())
}

#[test]
fn trusted_rustdoc_runs_with_a_caller_owned_dynamic_library_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let library_source = project.path().join("src/doctest_dylib.rs");
    let source = project.path().join("src/dynamic_doctest.rs");
    fs::write(&library_source, "pub fn answer() -> u32 { 42 }\n")?;
    fs::write(
        &source,
        "//! ```\n//! assert_eq!(oven_doctest_dylib::answer(), 42);\n//! ```\npub struct DynamicDoctestFixture;\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("dynamic-doctest-library", &library_source)
        .with_generated_source("dynamic-doctest-source", &source),
    )?;
    let artifacts = empty_manifest(&receipt);
    let dylib = bake_trusted_direct_rustc_dylib(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &library_source,
        output: &output
            .path()
            .join(format!("liboven_doctest_dylib{}", std::env::consts::DLL_SUFFIX)),
        crate_name: "oven_doctest_dylib",
        edition: "2024",
        source_evidence_key: "dynamic-doctest-library",
        features: &[],
        prefer_dynamic: true,
    })?;
    let mut plan = OvenRustcArtifactPlan {
        source_path_projection: None,
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };
    attach_caller_owned_rustc_libraries(
        &mut plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "oven_doctest_dylib".to_string(),
            output: dylib.output,
            digest: dylib.output_digest,
            expose_extern: true,
        }],
    )?;
    let sealed_dependency_directory = output.path().join("sealed/sha256:immutable-doctest-dependency");
    fs::create_dir_all(&sealed_dependency_directory)?;
    plan.dependency_search_paths.push(sealed_dependency_directory);

    let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&plan),
        rustc: &rustc,
        source: &source,
        temporary_directory: &output.path().join("rustdoc-temporary"),
        crate_name: "oven_dynamic_doctest",
        edition: "2024",
        source_evidence_key: "dynamic-doctest-source",
        features: &[],
        is_proc_macro: false,
        prefer_dynamic: true,
        timeout: None,
    })?;
    assert!(report.output.contains("test result: ok"));
    Ok(())
}

#[test]
fn trusted_rustdoc_compiles_a_proc_macro_doctest_root() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    fs::create_dir_all(project.path().join("src"))?;
    let source = project.path().join("src/proc_macro_doctest.rs");
    fs::write(
        &source,
        "use proc_macro::TokenStream;\n\n#[proc_macro_derive(OvenFixture)]\npub fn oven_fixture(_input: TokenStream) -> TokenStream { TokenStream::new() }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("proc-macro-doctest-source", &source),
    )?;
    let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
        receipt: &receipt,
        artifacts: &empty_manifest(&receipt),
        artifact_root: artifact_root.path(),
        artifact_plan: None,
        rustc: &rustc,
        source: &source,
        temporary_directory: &output.path().join("rustdoc-temporary"),
        crate_name: "oven_proc_macro_doctest",
        edition: "2024",
        source_evidence_key: "proc-macro-doctest-source",
        features: &[],
        is_proc_macro: true,
        prefer_dynamic: true,
        timeout: None,
    })?;
    assert!(report.output.contains("test result: ok"));
    Ok(())
}

#[test]
fn stored_receipt_bound_plan_runs_without_cargo_and_retains_its_lease() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let source = output.path().join("stored-consumer.rs");
    fs::write(
        &source,
        "#[test]\nfn oven_plan_is_the_consumer_input() { assert!(option_env!(\"CARGO\").is_none()); }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("direct-rustc-source", &source),
    )?;
    let plan = empty_manifest(&receipt);
    let payload = serde_json::to_vec(&plan)?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let stored = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "alpha-test".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    assert_eq!(select_direct_rustc_plan_identity(&store, &receipt)?, stored.identity);

    let request = OvenStoredDirectRustcTestRequest {
        store: &store,
        plan_identity: stored.identity.clone(),
        receipt: receipt.clone(),
        rustc: rustc.clone(),
        source: source.clone(),
        output: output.path().join("stored-consumer-test"),
        crate_name: "oven_stored_consumer".to_string(),
        edition: "2024".to_string(),
        source_evidence_key: "direct-rustc-source".to_string(),
    };
    let bake = bake_stored_direct_rustc_test(&request)?;

    assert!(!bake.reused);
    let reused = bake_stored_direct_rustc_test(&request)?;
    assert!(reused.reused);
    assert_eq!(reused.output, bake.output);
    drop(reused);
    let first_physical = store.inspect()?.physical_bytes;
    let bounded = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(first_physical.saturating_add(1), 128 * 1024, 64 * 1024),
    );
    let replacement = OvenArtifactPublishRequest {
        receipt,
        domain: "alpha-replacement".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&plan)?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    };
    assert!(matches!(
        bounded.publish(&replacement),
        Err(oven_store::store::OvenStoreError::CapacityBlocked { .. })
    ));
    assert!(Command::new(&bake.output).status()?.success());
    drop(bake);
    bounded.publish(&replacement)?;
    assert_eq!(bounded.inspect()?.entries.len(), 1);
    Ok(())
}

#[test]
fn receipt_selection_rejects_ambiguous_stored_direct_rustc_plans() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let receipt = intent(project.path())?;
    let plan = empty_manifest(&receipt);
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    for domain in ["alpha-primary", "alpha-duplicate"] {
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: domain.to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
    }

    assert!(matches!(
        select_direct_rustc_plan_identity(&store, &receipt),
        Err(OvenRustcError::PlanSelection { .. })
    ));
    Ok(())
}

#[test]
fn stored_receipt_bound_binary_runs_without_cargo_and_retains_its_lease() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let source = output.path().join("stored-binary.rs");
    fs::write(
        &source,
        "fn main() { assert!(option_env!(\"CARGO\").is_none()); assert!(option_env!(\"CARGO_PKG_NAME\").is_none()); }\n",
    )?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("direct-rustc-source", &source),
    )?;
    let plan = empty_manifest(&receipt);
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let stored = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "alpha-run".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&plan)?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;

    let bake = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
        store: &store,
        plan_identity: stored.identity.clone(),
        receipt: receipt.clone(),
        rustc,
        source,
        output: output.path().join("stored-binary"),
        crate_name: "oven_stored_binary".to_string(),
        edition: "2024".to_string(),
        source_evidence_key: "direct-rustc-source".to_string(),
    })?;

    let first_physical = store.inspect()?.physical_bytes;
    let bounded = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(first_physical.saturating_add(1), 128 * 1024, 64 * 1024),
    );
    let replacement = OvenArtifactPublishRequest {
        receipt,
        domain: "alpha-run-replacement".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&plan)?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    };
    assert!(matches!(
        bounded.publish(&replacement),
        Err(oven_store::store::OvenStoreError::CapacityBlocked { .. })
    ));
    assert!(Command::new(&bake.output).status()?.success());
    drop(bake);
    bounded.publish(&replacement)?;
    assert_eq!(bounded.inspect()?.entries.len(), 1);
    Ok(())
}

#[test]
fn direct_rustc_refuses_source_changes_before_invoking_the_compiler() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let source = output.path().join("consumer.rs");
    fs::write(&source, "#[test]\nfn first() {}\n")?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "release",
            Vec::new(),
        )
        .with_generated_source("direct-rustc-source", &source),
    )?;
    fs::write(&source, "#[test]\nfn changed() {}\n")?;
    let request = OvenDirectRustcTestRequest {
        artifacts: empty_manifest(&receipt),
        receipt,
        artifact_root: artifact_root.path().to_path_buf(),
        rustc,
        source,
        output: output.path().join("consumer-test"),
        crate_name: "oven_consumer".to_string(),
        edition: "2024".to_string(),
        source_evidence_key: "direct-rustc-source".to_string(),
    };
    let result = bake_direct_rustc_test(&request);
    assert!(matches!(result, Err(OvenRustcError::SourceEvidenceMismatch { .. })));
    Ok(())
}
