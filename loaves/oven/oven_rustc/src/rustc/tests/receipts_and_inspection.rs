//! Receipt selection and inspection-authority regression tests.

use super::*;

#[test]
fn project_extension_keeps_its_own_build_script_leaf_despite_matching_release_semantics()
-> Result<(), Box<dyn std::error::Error>> {
    // Regression: two independent builds of a build-script crate (like `libc`) can compile to link-incompatible
    // artifacts even when package, version, source checksum, and declared features all match, because a build
    // script can read ambient build-environment state that isn't captured by any of those coordinates. Matching
    // "leaf semantics" alone must not be trusted as proof of link compatibility for such a crate.
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let source = OvenRustcRegistrySource {
        registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        checksum: "libc-checksum".to_string(),
        relative_root: "registry-sources/libc".to_string(),
        digest: "sha256:libc-source".to_string(),
    };
    let registry_source = OvenRustcRegistrySourcePackage {
        package: "libc".to_string(),
        version: "0.2.155".to_string(),
        features: vec!["default".to_string()],
        source: source.clone(),
    };
    // Build-script output is staged under `build/<package>/<identity>/out/`, unlike an ordinary crate's flat
    // `deps/` output; this is the structural signal that distinguishes it from a crate like `serde`.
    let release_libc = OvenRustcArtifactExtern {
        crate_name: "libc".to_string(),
        relative_path: "build/libc/release-identity/out/liblibc-release-identity.rlib".to_string(),
        digest: "sha256:release-libc".to_string(),
    };
    let project_libc = OvenRustcArtifactExtern {
        relative_path: "build/libc/project-identity/out/liblibc-project-identity.rlib".to_string(),
        digest: "sha256:project-libc".to_string(),
        ..release_libc.clone()
    };
    let leaf = |artifact| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: "libc".to_string(),
        version: "0.2.155".to_string(),
        crate_name: "libc".to_string(),
        features: vec!["default".to_string()],
        source: source.clone(),
        artifact,
    };
    let project = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["build/libc/project-identity/out".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-project.rlib".to_string(),
                digest: "sha256:project-stdlib".to_string(),
            },
            project_libc.clone(),
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![leaf(project_libc.clone())],
        registry_sources: vec![registry_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![OvenRustcSupportingArtifact {
            relative_path: "registry-sources/libc/Cargo.toml".to_string(),
            digest: "sha256:libc-manifest".to_string(),
        }],
    };
    let base = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent,
        dependency_search_paths: vec!["build/libc/release-identity/out".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-release.rlib".to_string(),
                digest: "sha256:release-stdlib".to_string(),
            },
            release_libc.clone(),
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![leaf(release_libc)],
        registry_sources: vec![registry_source],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![OvenRustcSupportingArtifact {
            relative_path: "registry-sources/libc/Cargo.toml".to_string(),
            digest: "sha256:libc-manifest".to_string(),
        }],
    };

    let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    assert_eq!(
        composed.registry_leaves[0].artifact, project_libc,
        "a build-script leaf must keep the project's own compiled artifact, not the base's independently built one"
    );
    assert_eq!(composed.externs[1], project_libc);
    Ok(())
}
#[test]
fn project_extension_source_authority_requires_the_same_recomposed_payload_as_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let source = OvenRustcRegistrySource {
        registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        checksum: "project-only-checksum".to_string(),
        relative_root: "registry-sources/project-only".to_string(),
        digest: "sha256:project-only-source".to_string(),
    };
    let registry_source = OvenRustcRegistrySourcePackage {
        package: "project-only".to_string(),
        version: "1.0.0".to_string(),
        features: vec!["default".to_string()],
        source,
    };
    let mut publisher = empty_manifest(&receipt);
    publisher.dependency_search_paths = vec!["deps".to_string()];
    publisher.externs = vec![OvenRustcArtifactExtern {
        crate_name: "incan_std_core".to_string(),
        relative_path: "deps/libincan_std_core-project.rlib".to_string(),
        digest: "sha256:project-stdlib".to_string(),
    }];
    publisher.registry_sources = vec![registry_source];
    publisher.supporting_artifacts = vec![OvenRustcSupportingArtifact {
        relative_path: "registry-sources/project-only/Cargo.toml".to_string(),
        digest: "sha256:project-only-manifest".to_string(),
    }];
    let mut base = empty_manifest(&receipt);
    base.dependency_search_paths = vec!["deps".to_string()];
    base.externs = vec![OvenRustcArtifactExtern {
        crate_name: "incan_std_core".to_string(),
        relative_path: "deps/libincan_std_core-release.rlib".to_string(),
        digest: "sha256:release-stdlib".to_string(),
    }];
    let complete = publisher.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    let partition = complete.partition_against_base(&base)?;
    let payload = OvenProjectExtensionPayload {
        schema_version: OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
        base_loaf_identity: "sha256:release-loaf".to_string(),
        base_build_unit_identity: "sha256:release-unit".to_string(),
        publisher_plan: publisher,
        complete_plan: complete,
        registry_source_dependencies: vec![OvenProjectRegistrySourceDependency {
            alias: "project_only".to_string(),
            package: "project-only".to_string(),
            version: "1.0.0".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "project-only-checksum".to_string(),
        }],
        dev_registry_source_dependencies: Vec::new(),
        extension_paths: partition.extension_paths.iter().cloned().collect(),
    };
    assert_eq!(
        validate_project_extension_payload_against_base(&payload, "sha256:release-loaf", "sha256:release-unit", &base,)?,
        partition
    );

    let mut malformed_fragment = payload.clone();
    malformed_fragment
        .extension_paths
        .push("undeclared/artifact.rlib".to_string());
    let Err(error) = super::super::validate_project_extension_payload_shape(&malformed_fragment, &receipt.intent)
    else {
        return Err(std::io::Error::other("malformed stored extension fragment was accepted").into());
    };
    assert!(error.to_string().contains("strictly sorted"));

    let mut malformed_dev_root = payload.clone();
    malformed_dev_root.dev_registry_source_dependencies = vec![OvenProjectRegistrySourceDependency {
        alias: "missing_dev".to_string(),
        package: "missing-dev".to_string(),
        version: "1.0.0".to_string(),
        registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        checksum: "missing-dev-checksum".to_string(),
    }];
    let Err(error) = super::super::validate_project_extension_payload_shape(&malformed_dev_root, &receipt.intent)
    else {
        return Err(std::io::Error::other("malformed stored extension dev root was accepted").into());
    };
    assert!(error.to_string().contains("exact records"));

    let mut mismatched = payload;
    mismatched.complete_plan.registry_sources[0]
        .features
        .push("payload-only-drift".to_string());
    let Err(error) = validate_project_extension_payload_against_base(
        &mismatched,
        "sha256:release-loaf",
        "sha256:release-unit",
        &base,
    ) else {
        return Err(std::io::Error::other("mismatched source-authority payload was accepted").into());
    };
    assert!(matches!(error, OvenRustcError::InvalidInput { .. }));
    Ok(())
}
#[test]
fn artifact_manifest_rejects_an_empty_search_directory() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let empty = root.path().join("empty");
    fs::create_dir(&empty)?;
    let receipt = intent(root.path())?;
    let manifest = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["empty".to_string()],
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };

    let result = manifest.materialize(root.path(), &receipt.intent);
    assert!(matches!(result, Err(OvenRustcError::InvalidArtifactPath { .. })));
    Ok(())
}
#[test]
fn trusted_artifact_plan_uses_declared_inputs_without_rescanning_search_children()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let dependencies = root.path().join("deps");
    fs::create_dir(&dependencies)?;
    let declared = dependencies.join("libdeclared.rlib");
    let unrelated = dependencies.join("unrelated-source-file.rs");
    fs::write(&declared, b"declared artifact")?;
    fs::write(&unrelated, b"not a direct rustc input")?;
    let receipt = intent(root.path())?;
    let manifest = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["deps".to_string()],
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![OvenRustcSupportingArtifact {
            relative_path: "deps/libdeclared.rlib".to_string(),
            digest: digest_bytes(b"declared artifact"),
        }],
    };

    let trusted = manifest.materialize_trusted_store(root.path(), &receipt.intent)?;
    assert_eq!(trusted.dependency_search_paths, vec![fs::canonicalize(&dependencies)?]);
    assert!(matches!(
        manifest.materialize(root.path(), &receipt.intent),
        Err(OvenRustcError::UnrecordedSearchArtifact { .. })
    ));
    Ok(())
}
#[test]
fn a_closure_proof_retires_the_per_file_walk_for_the_exact_closure_it_proved_issue1546()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let proofs = tempfile::tempdir()?;
    let dependencies = root.path().join("deps");
    fs::create_dir(&dependencies)?;
    fs::write(dependencies.join("libdeclared.rlib"), b"declared artifact")?;
    let receipt = intent(root.path())?;
    let manifest = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["deps".to_string()],
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![OvenRustcSupportingArtifact {
            relative_path: "deps/libdeclared.rlib".to_string(),
            digest: digest_bytes(b"declared artifact"),
        }],
    };
    let proof_path = OvenClosureProof::path(proofs.path(), "sha256:closure");

    // Symlinked in place of the declared file, the full walk refuses and writes no proof.
    let declared = dependencies.join("libdeclared.rlib");
    let elsewhere = root.path().join("elsewhere");
    fs::write(&elsewhere, b"declared artifact")?;
    fs::remove_file(&declared)?;
    std::os::unix::fs::symlink(&elsewhere, &declared)?;
    assert!(matches!(
        manifest.materialize_proven_store(root.path(), &receipt.intent, "sha256:closure", &proof_path),
        Err(OvenRustcError::InvalidArtifactPath { .. })
    ));
    assert!(!proof_path.is_file());

    // A regular file again: the walk passes and the proof is written for this closure and file count.
    fs::remove_file(&declared)?;
    fs::write(&declared, b"declared artifact")?;
    manifest.materialize_proven_store(root.path(), &receipt.intent, "sha256:closure", &proof_path)?;
    assert_eq!(
        OvenClosureProof::read_matching(&proof_path, "sha256:closure", 1).map(|proof| proof.artifact_count),
        Some(1)
    );

    // With the proof in place the per-file walk is retired: the same symlink swap is not re-checked here (the
    // proof says the closure was checked once; `inspect oven` is the audit), but the plan still names the
    // declared path below the root.
    fs::remove_file(&declared)?;
    std::os::unix::fs::symlink(&elsewhere, &declared)?;
    let proven = manifest.materialize_proven_store(root.path(), &receipt.intent, "sha256:closure", &proof_path)?;
    assert_eq!(proven.dependency_search_paths, vec![fs::canonicalize(&dependencies)?]);

    // A manifest declaring more files under the same identity does not ride on the proof.
    let mut wider = manifest.clone();
    wider.supporting_artifacts.push(OvenRustcSupportingArtifact {
        relative_path: "deps/libother.rlib".to_string(),
        digest: digest_bytes(b"other"),
    });
    assert!(matches!(
        wider.materialize_proven_store(root.path(), &receipt.intent, "sha256:closure", &proof_path),
        Err(OvenRustcError::InvalidArtifactPath { .. }) | Err(OvenRustcError::Io { .. })
    ));
    Ok(())
}
#[test]
fn plan_selection_reuses_one_build_unit_across_distinct_generated_sources() -> Result<(), Box<dyn std::error::Error>> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    for (root, source) in [
        (first.path(), "fn main() { println!(\"first\"); }\n"),
        (second.path(), "fn main() { println!(\"second\"); }\n"),
    ] {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/main.rs"), source)?;
    }
    let receipt_for = |root: &Path| {
        receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                root,
                "shared-oven-fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", root.join("src/main.rs"))
            .with_generated_source_tree("generated-tree", root.join("src"))
            .with_build_unit_input("runtime-lock", "sha256:shared"),
        )
    };
    let first_receipt = receipt_for(first.path())?;
    let second_receipt = receipt_for(second.path())?;
    assert_ne!(first_receipt.identity, second_receipt.identity);
    assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);

    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let stored = store.publish(&OvenArtifactPublishRequest {
        receipt: first_receipt.clone(),
        domain: "shared-alpha".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&empty_manifest(&first_receipt))?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    assert_eq!(
        select_direct_rustc_plan_identity(&store, &second_receipt)?,
        stored.identity
    );
    Ok(())
}
#[test]
fn project_inspection_authority_reuses_a_compatible_direct_plan_across_receipts()
-> Result<(), Box<dyn std::error::Error>> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    for (root, source) in [
        (first.path(), "fn main() { println!(\"first\"); }\n"),
        (second.path(), "fn main() { println!(\"second\"); }\n"),
    ] {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/main.rs"), source)?;
    }
    let receipt_for = |root: &Path| {
        receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                root,
                "shared-oven-fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", root.join("src/main.rs"))
            .with_generated_source_tree("generated-tree", root.join("src"))
            .with_build_unit_input("runtime-lock", "sha256:shared"),
        )
    };
    let first_receipt = receipt_for(first.path())?;
    let second_receipt = receipt_for(second.path())?;
    assert_ne!(first_receipt.identity, second_receipt.identity);
    assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);

    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let direct_plan = store.publish(&OvenArtifactPublishRequest {
        receipt: first_receipt,
        domain: "shared-alpha".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&empty_manifest(&second_receipt))?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;
    let project_identity = "sha256:project";
    let source_authority_digest = "sha256:source";
    let compiler_version = "0.5.1-test";
    let authority_payload = OvenProjectInspectionAuthorityPayload {
        schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        project_identity: project_identity.to_string(),
        source_authority_digest: source_authority_digest.to_string(),
        compiler_version: compiler_version.to_string(),
        registry_lock_digest: digest_bytes(b"registry lock"),
        generated_out_dirs: Vec::new(),
        registry_source_dependencies: Vec::new(),
        dev_registry_source_dependencies: Vec::new(),
        test_dependency_envelope: None,
        constituents: vec![OvenProjectInspectionConstituent::Stored {
            identity: direct_plan.identity.clone(),
            artifact_kind: OvenArtifactKind::DirectRustcPlan,
            receipt: second_receipt.clone(),
            base_loaf_identity: None,
        }],
        registry_sources: Vec::new(),
    };
    let authority = store.publish(&OvenArtifactPublishRequest {
        receipt: second_receipt.clone(),
        domain: "shared-alpha".to_string(),
        kind: OvenArtifactKind::ProjectInspectionAuthority,
        payload: serde_json::to_vec(&authority_payload)?,
        materialized_files: Vec::new(),
        materialized_directories: Vec::new(),
    })?;

    let (selected_manifest, _, _, _) = store.select_payload_for_execution(&direct_plan.identity)?;
    assert!(project_inspection_constituent_matches_receipt(
        &selected_manifest,
        OvenArtifactKind::DirectRustcPlan,
        &second_receipt,
    ));
    assert!(!project_inspection_constituent_matches_receipt(
        &selected_manifest,
        OvenArtifactKind::ProjectPayload,
        &second_receipt,
    ));
    let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&direct_plan.identity))?;
    let selected = selected
        .pop()
        .ok_or("direct plan disappeared after authority selection")?;
    let selection = crate::plan::selection::project_test_dependency_plan_from_constituent(selected, &second_receipt)?;
    assert!(matches!(
        selection,
        crate::plan::OvenDirectRustcPlanSelection::Stored(_)
    ));

    let loaded = load_project_inspection_authority(
        &store,
        &OvenProjectInspectionAuthorityRef {
            identity: authority.identity.clone(),
            receipt_identity: second_receipt.identity.clone(),
            build_unit_identity: second_receipt.build_unit_identity.clone(),
        },
        project_identity,
        source_authority_digest,
        compiler_version,
    )?;
    assert_eq!(loaded.identity(), authority.identity);
    // Both sides are canonicalized before comparison. The loader resolves its root through the filesystem and
    // the store hands back the path it was given, so on a host where the temporary directory sits behind a
    // symlink -- `/var` to `/private/var` on macOS -- the two spell the same directory differently and the
    // assertion fails for a reason that has nothing to do with selection.
    assert_eq!(
        loaded.artifact_root().canonicalize()?,
        store
            .select(&authority.identity)?
            .0
            .materialized_root()
            .canonicalize()?
    );
    assert_eq!(loaded.payload, authority_payload);
    Ok(())
}
#[test]
fn project_inspection_authority_rejects_unknown_schema_before_decoding_its_body()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    let materialized = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let evidence = materialized.path().join("future-evidence.txt");
    fs::write(&evidence, b"future evidence")?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let future_schema = OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION + 1;
    let authority = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "future-project-inspection-authority".to_string(),
        kind: OvenArtifactKind::ProjectInspectionAuthority,
        payload: serde_json::to_vec(&serde_json::json!({
            "schema_version": future_schema,
            "generated_out_dirs": [{"crate_name": 42}],
        }))?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: evidence,
            relative_path: "authority/future-evidence.txt".to_string(),
        }],
        materialized_directories: Vec::new(),
    })?;
    let (entry, lease) = store.select(&authority.identity)?;
    drop(lease);
    let stored_evidence = entry.materialized_root().join("authority/future-evidence.txt");
    fs::remove_file(&stored_evidence)?;
    fs::write(stored_evidence, b"corrupt evidence")?;

    let result = load_project_inspection_authority(
        &store,
        &OvenProjectInspectionAuthorityRef {
            identity: authority.identity,
            receipt_identity: receipt.identity,
            build_unit_identity: receipt.build_unit_identity,
        },
        "unused-project-identity",
        "unused-source-authority",
        "unused-compiler-version",
    );
    assert!(matches!(
        result,
        Err(OvenRustcError::UnsupportedProjectInspectionAuthoritySchema {
            found,
            expected: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        }) if found == future_schema
    ));
    Ok(())
}
#[test]
fn project_inspection_authority_binds_the_requested_store_identity() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let project_identity = "sha256:project";
    let source_authority_digest = "sha256:source";
    let compiler_version = "0.5.1-test";
    let payload = OvenProjectInspectionAuthorityPayload {
        schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        project_identity: project_identity.to_string(),
        source_authority_digest: source_authority_digest.to_string(),
        compiler_version: compiler_version.to_string(),
        registry_lock_digest: digest_bytes(b"registry lock"),
        generated_out_dirs: Vec::new(),
        registry_source_dependencies: Vec::new(),
        dev_registry_source_dependencies: Vec::new(),
        test_dependency_envelope: None,
        constituents: Vec::new(),
        registry_sources: Vec::new(),
    };
    let encoded_payload = serde_json::to_vec(&payload)?;
    let publish = |domain: &str| {
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: domain.to_string(),
            kind: OvenArtifactKind::ProjectInspectionAuthority,
            payload: encoded_payload.clone(),
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })
    };
    let requested = publish("requested-project-inspection-authority")?;
    let substitute = publish("substitute-project-inspection-authority")?;
    assert_ne!(requested.identity, substitute.identity);
    let (requested_entry, requested_lease) = store.select(&requested.identity)?;
    let (substitute_entry, substitute_lease) = store.select(&substitute.identity)?;
    drop(requested_lease);
    drop(substitute_lease);
    fs::remove_dir_all(&requested_entry.path)?;
    fs::rename(&substitute_entry.path, &requested_entry.path)?;

    let result = load_project_inspection_authority(
        &store,
        &OvenProjectInspectionAuthorityRef {
            identity: requested.identity.clone(),
            receipt_identity: receipt.identity,
            build_unit_identity: receipt.build_unit_identity,
        },
        project_identity,
        source_authority_digest,
        compiler_version,
    );
    let refusal = match result {
        Ok(_) => return Err("a substituted entry must not satisfy the requested authority".into()),
        Err(error) => error.to_string(),
    };
    // The refusal moved down a layer and got stronger. It used to surface as `InvalidStoredPlan` from the
    // authority loader, which noticed the payload disagreed after reading it. The store now refuses the
    // substituted directory by its immutable coordinate before the loader sees it, so the assertion is on the
    // property the test is named for -- the requested identity is what could not be selected -- rather than on
    // which layer happened to say so.
    assert!(
        refusal.contains(&requested.identity),
        "the refusal must name the authority that was requested: {refusal}"
    );
    assert!(
        refusal.contains("entry directory and manifest names do not match"),
        "a substituted entry must be refused on its immutable coordinate: {refusal}"
    );
    Ok(())
}
#[test]
fn loaded_project_inspection_authority_retains_and_revalidates_its_store_owner()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let store_root = tempfile::tempdir()?;
    let materialized = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let evidence = materialized.path().join("authority-evidence.txt");
    fs::write(&evidence, b"authority evidence")?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
    );
    let project_identity = "sha256:project";
    let source_authority_digest = "sha256:source";
    let compiler_version = "0.5.1-test";
    let payload = OvenProjectInspectionAuthorityPayload {
        schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        project_identity: project_identity.to_string(),
        source_authority_digest: source_authority_digest.to_string(),
        compiler_version: compiler_version.to_string(),
        registry_lock_digest: digest_bytes(b"registry lock"),
        generated_out_dirs: Vec::new(),
        registry_source_dependencies: Vec::new(),
        dev_registry_source_dependencies: Vec::new(),
        test_dependency_envelope: None,
        constituents: Vec::new(),
        registry_sources: Vec::new(),
    };
    let authority = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "retained-project-inspection-authority".to_string(),
        kind: OvenArtifactKind::ProjectInspectionAuthority,
        payload: serde_json::to_vec(&payload)?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: evidence,
            relative_path: "authority/evidence.txt".to_string(),
        }],
        materialized_directories: Vec::new(),
    })?;
    let authority_ref = OvenProjectInspectionAuthorityRef {
        identity: authority.identity.clone(),
        receipt_identity: receipt.identity.clone(),
        build_unit_identity: receipt.build_unit_identity.clone(),
    };

    let loaded = load_project_inspection_authority(
        &store,
        &authority_ref,
        project_identity,
        source_authority_digest,
        compiler_version,
    )?;
    assert_eq!(loaded.identity(), authority.identity);
    assert!(loaded.artifact_root().join("authority/evidence.txt").is_file());
    assert!(store.inspect()?.active_lease_physical_bytes > 0);
    drop(loaded);
    assert_eq!(store.inspect()?.active_lease_physical_bytes, 0);

    let (entry, lease) = store.select(&authority.identity)?;
    drop(lease);
    let stored_evidence = entry.materialized_root().join("authority/evidence.txt");
    fs::remove_file(&stored_evidence)?;
    fs::write(stored_evidence, b"authority evidencf")?;
    let result = load_project_inspection_authority(
        &store,
        &authority_ref,
        project_identity,
        source_authority_digest,
        compiler_version,
    );
    assert!(matches!(result, Err(OvenRustcError::Store(_))));
    Ok(())
}
