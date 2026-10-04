//! Source-role closure and trusted-plan projection regression tests.

use super::*;

#[test]
fn direct_rustc_refuses_a_compiler_that_disagrees_with_the_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let source = output.path().join("consumer.rs");
    fs::write(&source, "#[test]\nfn identity_is_checked() {}\n")?;
    let rustc = rustc_path()?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            rustc_host_target(&rustc)?,
            "rustc deliberately-not-the-selected-compiler",
            "release",
            Vec::new(),
        )
        .with_generated_source("direct-rustc-source", &source),
    )?;
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

    assert!(matches!(
        bake_direct_rustc_test(&request),
        Err(OvenRustcError::ToolchainMismatch { .. })
    ));
    Ok(())
}
#[test]
fn manifest_distinguishes_a_repeated_registry_source_from_a_second_source_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let source = OvenRustcRegistrySourcePackage {
        package: "segmentation".to_string(),
        version: "1.12.0".to_string(),
        features: Vec::new(),
        source: OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "segmentation-checksum".to_string(),
            relative_root: "registry-sources/segmentation".to_string(),
            digest: digest_bytes(b"segmentation source"),
        },
    };
    let manifest = |registry_sources: Vec<OvenRustcRegistrySourcePackage>| OvenRustcArtifactManifest {
        registry_sources,
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/segmentation/Cargo.toml".to_string(),
                digest: digest_bytes(b"segmentation manifest"),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/segmentation-other/Cargo.toml".to_string(),
                digest: digest_bytes(b"other segmentation manifest"),
            },
        ],
        ..empty_manifest(&receipt)
    };
    let refusal = |registry_sources| -> Result<String, Box<dyn std::error::Error>> {
        let plan = manifest(registry_sources);
        Ok(plan
            .validate_shape(&plan.intent)
            .err()
            .ok_or("the manifest must refuse the registry source declarations")?
            .to_string())
    };

    let single = manifest(vec![source.clone()]);
    single.validate_shape(&single.intent)?;

    let repeated = refusal(vec![source.clone(), source.clone()])?;
    assert!(
        repeated.contains("declares registry source `segmentation` version `1.12.0` more than once"),
        "{repeated}"
    );
    let mut featured = source.clone();
    featured.features = vec!["std".to_string()];
    let repeated_with_features = refusal(vec![source.clone(), featured])?;
    assert!(
        repeated_with_features.contains("more than once"),
        "{repeated_with_features}"
    );

    let mut other_checksum = source.clone();
    other_checksum.source.checksum = "other-segmentation-checksum".to_string();
    let mut other_root = source.clone();
    other_root.source.relative_root = "registry-sources/segmentation-other".to_string();
    let mut other_digest = source.clone();
    other_digest.source.digest = digest_bytes(b"other segmentation source");
    for conflicting in [other_checksum, other_root, other_digest] {
        let conflict = refusal(vec![source.clone(), conflicting])?;
        assert!(
            conflict.contains(
                "declares more than one source identity for registry package `segmentation` version `1.12.0`"
            ),
            "{conflict}"
        );
    }
    Ok(())
}
#[test]
fn release_only_project_inspection_authority_binds_root_features_and_orders_constituents()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let source = OvenRustcRegistrySourcePackage {
        package: "serde_json".to_string(),
        version: "1.0.140".to_string(),
        features: vec!["preserve_order".to_string(), "std".to_string()],
        source: OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-json-checksum".to_string(),
            relative_root: "registry-sources/serde_json-1.0.140".to_string(),
            digest: digest_bytes(b"serde_json source"),
        },
    };
    let root = OvenProjectInspectionRootDependency {
        alias: "serde_json".to_string(),
        package: source.package.clone(),
        version: source.version.clone(),
        registry: source.source.registry.clone(),
        checksum: source.source.checksum.clone(),
        requested_features: vec!["preserve_order".to_string()],
        default_features: false,
    };
    let mut payload = OvenProjectInspectionAuthorityPayload {
        schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        project_identity: "sha256:project".to_string(),
        source_authority_digest: "sha256:source".to_string(),
        compiler_version: "0.5.0-rc0".to_string(),
        registry_lock_digest: digest_bytes(b"lock"),
        generated_out_dirs: Vec::new(),
        registry_source_dependencies: vec![root.clone()],
        dev_registry_source_dependencies: Vec::new(),
        test_dependency_envelope: None,
        constituents: vec![OvenProjectInspectionConstituent::ReleaseLoaf {
            loaf_identity: "sha256:release-loaf".to_string(),
            build_unit_identity: "sha256:release-unit".to_string(),
            receipt: receipt.clone(),
        }],
        registry_sources: vec![OvenProjectInspectionSource {
            package: source,
            owner: OvenProjectInspectionSourceOwner::Constituent { index: 0 },
        }],
    };
    validate_project_inspection_authority_payload(&payload)?;

    let matching = DependencySpec {
        crate_name: "serde_json".to_string(),
        version: Some("1".to_string()),
        features: vec!["preserve_order".to_string()],
        default_features: false,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    };
    assert!(project_inspection_authority_supports_dependencies(
        &payload,
        std::slice::from_ref(&matching)
    ));
    let mut wrong_features = matching.clone();
    wrong_features.features = vec!["raw_value".to_string()];
    assert!(!project_inspection_authority_supports_dependencies(
        &payload,
        &[wrong_features]
    ));
    let mut wrong_defaults = matching.clone();
    wrong_defaults.default_features = true;
    assert!(!project_inspection_authority_supports_dependencies(
        &payload,
        &[wrong_defaults]
    ));

    let debug_receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project.path(),
            "rustc_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "debug",
            Vec::new(),
        )
        .with_generated_source("fixture-source", project.path().join("fixture.rs")),
    )?;
    payload.constituents.push(OvenProjectInspectionConstituent::Stored {
        identity: "sha256:test-dependency-extension".to_string(),
        artifact_kind: OvenArtifactKind::ProjectPayload,
        receipt: debug_receipt.clone(),
        base_loaf_identity: Some("sha256:release-loaf".to_string()),
    });
    payload.test_dependency_envelope = Some(OvenProjectInspectionTestDependencyEnvelope {
        constituent_index: 1,
        provider_constituents: Vec::new(),
        dependency_surface_digest: digest_bytes(b"normal+dev dependency surface"),
        dependency_roots: BTreeMap::from([(
            "serde_json".to_string(),
            OvenProjectInspectionTestDependencyRoot::Registry {
                dependency_digest: oven_store::digest_dependency_specs(
                    std::slice::from_ref(&matching),
                    &oven_store::NoProviderHooks,
                )?,
                locked: root,
            },
        )]),
    });
    validate_project_inspection_authority_payload(&payload)?;
    assert!(project_inspection_test_dependency_envelope_supports_dependencies(
        &payload,
        std::slice::from_ref(&matching),
        &oven_store::NoProviderHooks,
    )?);
    let mut missing = matching.clone();
    missing.crate_name = "missing_alias".to_string();
    assert!(!project_inspection_test_dependency_envelope_supports_dependencies(
        &payload,
        std::slice::from_ref(&missing),
        &oven_store::NoProviderHooks,
    )?);
    assert_eq!(
        project_inspection_test_dependency_envelope_mismatch(&payload, &[missing], &oven_store::NoProviderHooks)?
            .as_deref(),
        Some("`missing_alias` has no sealed root")
    );

    payload.constituents.push(OvenProjectInspectionConstituent::Stored {
        identity: "sha256:test-provider-direct-plan".to_string(),
        artifact_kind: OvenArtifactKind::DirectRustcPlan,
        receipt: debug_receipt.clone(),
        base_loaf_identity: None,
    });
    payload
        .test_dependency_envelope
        .as_mut()
        .ok_or("test dependency role disappeared")?
        .provider_constituents
        .push(OvenProjectInspectionTestProviderConstituent {
            dependency_key: "provider_fixture".to_string(),
            constituent_index: 2,
        });
    validate_project_inspection_authority_payload(&payload)?;
    let mut repeated_role = payload.clone();
    repeated_role
        .test_dependency_envelope
        .as_mut()
        .ok_or("test dependency role disappeared")?
        .provider_constituents[0]
        .constituent_index = 1;
    let Err(error) = validate_project_inspection_authority_payload(&repeated_role) else {
        return Err("authority accepted one constituent in two test dependency roles".into());
    };
    assert!(error.to_string().contains("must not repeat a role-bearing constituent"));

    let mut direct_plan_payload = payload.clone();
    direct_plan_payload.constituents[1] = OvenProjectInspectionConstituent::Stored {
        identity: "sha256:test-dependency-direct-plan".to_string(),
        artifact_kind: OvenArtifactKind::DirectRustcPlan,
        receipt: debug_receipt.clone(),
        base_loaf_identity: None,
    };
    validate_project_inspection_authority_payload(&direct_plan_payload)?;
    if let OvenProjectInspectionConstituent::Stored { base_loaf_identity, .. } =
        &mut direct_plan_payload.constituents[1]
    {
        *base_loaf_identity = Some("sha256:invalid-direct-plan-base".to_string());
    }
    let Err(error) = validate_project_inspection_authority_payload(&direct_plan_payload) else {
        return Err("authority accepted base-Loaf evidence on a self-contained direct-plan constituent".into());
    };
    assert!(
        error
            .to_string()
            .contains("inconsistent identity, kind, or base evidence")
    );

    // The dependency's path has to exist: digesting a path dependency reads the directory, so a bare
    // placeholder makes this case fail on "provider artifact root is not a directory" long before reaching
    // the source-evidence refusal it exists to check.
    let legacy_path_dependency = project.path().join("legacy-path-dependency");
    std::fs::create_dir_all(legacy_path_dependency.join("src"))?;
    std::fs::write(
        legacy_path_dependency.join("Cargo.toml"),
        "[package]\nname = \"dev_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(legacy_path_dependency.join("src/lib.rs"), "")?;
    let path_dependency = DependencySpec {
        crate_name: "dev_fixture".to_string(),
        version: Some("0.1.0".to_string()),
        features: vec!["test-support".to_string()],
        default_features: false,
        source: DependencySource::Path {
            path: legacy_path_dependency.clone(),
        },
        optional: false,
        package: None,
    };
    payload
        .test_dependency_envelope
        .as_mut()
        .ok_or("test dependency role disappeared")?
        .dependency_roots
        .insert(
            "dev_fixture".to_string(),
            OvenProjectInspectionTestDependencyRoot::Path {
                dependency_digest: "sha256:legacy-path-envelope".to_string(),
            },
        );
    // A persisted Cargo path root whose digest does not match the dependency is not supported, so the
    // envelope cannot authorize it.
    //
    // This substrate carries the shapes, not the decision. Gate 6 of RFC 119 turns the same case into a hard
    // refusal naming "no checked Oven source-unit identity" and replaces this assertion with that negative
    // case; asserting the refusal here would be asserting behavior this tree does not yet have.
    assert!(
        !project_inspection_test_dependency_envelope_supports_dependencies(
            &payload,
            std::slice::from_ref(&path_dependency),
            &oven_store::NoProviderHooks,
        )?,
        "an unmatched persisted Cargo path root must not be reported as supported"
    );
    let mismatch = project_inspection_test_dependency_envelope_mismatch(
        &payload,
        std::slice::from_ref(&path_dependency),
        &oven_store::NoProviderHooks,
    )?
    .ok_or("an unmatched persisted Cargo path root reported no mismatch")?;
    assert!(
        mismatch.starts_with("`dev_fixture` was sealed with dependency digest sha256:legacy-path-envelope, but"),
        "the refusal names the alias and both digests: {mismatch}"
    );

    payload
        .test_dependency_envelope
        .as_mut()
        .ok_or("test dependency role disappeared")?
        .constituent_index = 0;
    let Err(error) = validate_project_inspection_authority_payload(&payload) else {
        return Err("authority accepted a non-debug release Loaf as its project test dependency envelope".into());
    };
    assert!(error.to_string().contains("debug-profile"));
    if let OvenProjectInspectionConstituent::ReleaseLoaf { receipt, .. } = &mut payload.constituents[0] {
        *receipt = debug_receipt;
    }
    validate_project_inspection_authority_payload(&payload)?;
    payload.test_dependency_envelope = None;
    let _ = payload.constituents.pop();

    payload.constituents.insert(
        0,
        OvenProjectInspectionConstituent::Stored {
            identity: "sha256:direct-plan".to_string(),
            artifact_kind: OvenArtifactKind::DirectRustcPlan,
            receipt,
            base_loaf_identity: None,
        },
    );
    let Err(error) = validate_project_inspection_authority_payload(&payload) else {
        return Err("authority accepted a release Loaf after a store-owned constituent".into());
    };
    assert!(error.to_string().contains("precede"));
    Ok(())
}
#[test]
fn source_search_roles_validate_wire_evidence_and_preserve_legacy_projection() -> Result<(), Box<dyn std::error::Error>>
{
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let artifacts = source_search_fixture(&receipt)?;
    let roundtrip: OvenRustcArtifactManifest = serde_json::from_slice(&serde_json::to_vec(&artifacts)?)?;
    roundtrip.validate_shape(&receipt.intent)?;
    assert_eq!(roundtrip, artifacts);
    let selected = roundtrip.for_source_evidence("generated-root")?;
    assert_eq!(selected.dependency_search_paths, vec!["host", "target"]);
    assert_eq!(
        selected
            .externs
            .iter()
            .map(|entry| entry.crate_name.as_str())
            .collect::<Vec<_>>(),
        vec!["runtime"]
    );
    selected.validate_shape(&receipt.intent)?;

    for invalid_paths in [vec!["../host"], vec!["unlisted"], vec!["target", "target"]] {
        let mut invalid = artifacts.clone();
        invalid.entrypoint_dependency_search_paths.insert(
            "generated-root".to_string(),
            invalid
                .capture_source_search_closure(&invalid_paths.into_iter().map(str::to_string).collect::<Vec<_>>())?,
        );
        assert!(invalid.for_source_evidence("generated-root").is_err());
    }
    for change in 0..4 {
        let mut invalid = artifacts.clone();
        let directory = &mut invalid
            .entrypoint_dependency_search_paths
            .get_mut("generated-root")
            .ok_or("missing role")?
            .publisher_paths[0];
        match change {
            0 => directory.artifacts.clear(),
            1 => directory.artifacts[0].digest = digest_bytes(b"changed"),
            2 => directory.artifacts[0].relative_path = "helper/libprivate_helper.rlib".to_string(),
            _ => directory.artifacts.push(directory.artifacts[0].clone()),
        }
        assert!(invalid.validate_shape(&receipt.intent).is_err());
    }
    assert!(artifacts.source_search_closure("missing-current-role").is_err());
    let mut missing = artifacts.clone();
    missing.entrypoint_dependency_search_paths.clear();
    assert!(missing.for_source_evidence("generated-root").is_err());
    let mut unknown = artifacts.clone();
    unknown.schema_version += 1;
    assert!(matches!(
        unknown.validate_shape(&receipt.intent),
        Err(OvenRustcError::UnsupportedSchema { .. })
    ));
    let mut legacy = artifacts.clone();
    legacy.schema_version = super::super::OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION;
    assert!(legacy.validate_shape(&receipt.intent).is_err());
    legacy.entrypoint_dependency_search_paths.clear();
    legacy.validate_shape(&receipt.intent)?;
    assert_eq!(
        legacy.for_source_evidence("generated-root")?.dependency_search_paths,
        vec!["target"]
    );
    let prior = legacy.source_search_closure("generated-root")?;
    assert!(prior.publisher_paths.is_empty());
    assert_eq!(prior.legacy_projections.len(), 1);
    assert_eq!(
        prior.legacy_projections[0]
            .dependency_search_paths
            .iter()
            .map(|directory| directory.relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["target"]
    );
    Ok(())
}
#[test]
fn source_search_roles_compose_current_extension_with_legacy_release_without_baking()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let mut base = empty_manifest(&receipt);
    base.schema_version = super::super::OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION;
    base.dependency_search_paths = vec!["deps".to_string()];
    base.externs.push(OvenRustcArtifactExtern {
        crate_name: "incan_std_core".to_string(),
        relative_path: "deps/libincan_std_core-verified.rlib".to_string(),
        digest: digest_bytes(b"runtime"),
    });
    base.entrypoint_externs
        .insert("generated-root".to_string(), vec!["incan_std_core".to_string()]);
    let mut current = base.clone();
    current.schema_version = OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION;
    current.entrypoint_dependency_search_paths.insert(
        "generated-root".to_string(),
        current.capture_source_search_closure(&["deps".to_string()])?,
    );
    let original_base = base.clone();
    let compiled = tempfile::tempdir()?;
    fs::create_dir(compiled.path().join("deps"))?;
    let runtime_path = compiled.path().join("deps/libincan_std_core-verified.rlib");
    fs::write(&runtime_path, b"runtime")?;
    let before = fs::read(&runtime_path)?;
    let composed = current.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    let materialized = composed.materialize(compiled.path(), &receipt.intent)?;
    let projected =
        super::super::trusted_artifact_plan_for_source_evidence(&materialized, &composed, "generated-root")?;
    assert_eq!(projected.externs, materialized.externs);
    assert_eq!(projected.dependency_search_paths, materialized.dependency_search_paths);
    assert_eq!(fs::read(&runtime_path)?, before);
    assert_eq!(base, original_base);
    assert_eq!(composed.externs, current.externs);
    let expected = base.source_search_closure("generated-root")?;
    assert_eq!(
        composed.entrypoint_dependency_search_paths["generated-root"].legacy_projections,
        expected.legacy_projections
    );
    let serialized = serde_json::to_vec(&composed)?;
    let decoded: OvenRustcArtifactManifest = serde_json::from_slice(&serialized)?;
    decoded.validate_shape(&receipt.intent)?;
    let mut invalid = decoded.clone();
    invalid
        .entrypoint_dependency_search_paths
        .get_mut("generated-root")
        .ok_or("missing role")?
        .legacy_projections[0]
        .source_schema_version = 10;
    assert!(invalid.validate_shape(&receipt.intent).is_err());
    let mut invalid = decoded.clone();
    invalid
        .entrypoint_dependency_search_paths
        .get_mut("generated-root")
        .ok_or("missing role")?
        .legacy_projections[0]
        .source_manifest_digest = "sha256:forged".to_string();
    assert!(invalid.validate_shape(&receipt.intent).is_err());
    let mut invalid = decoded.clone();
    invalid
        .entrypoint_dependency_search_paths
        .get_mut("generated-root")
        .ok_or("missing role")?
        .legacy_projections[0]
        .source_evidence_key
        .clear();
    assert!(invalid.validate_shape(&receipt.intent).is_err());
    let mut invalid = decoded.clone();
    invalid.dependency_search_paths.push("unowned".to_string());
    invalid
        .entrypoint_dependency_search_paths
        .get_mut("generated-root")
        .ok_or("missing role")?
        .legacy_projections[0]
        .dependency_search_paths
        .push(super::super::OvenRustcSourceSearchDirectory {
            relative_path: "unowned".to_string(),
            artifacts: Vec::new(),
        });
    assert!(invalid.validate_shape(&receipt.intent).is_err());
    let mut invalid = decoded;
    invalid
        .entrypoint_dependency_search_paths
        .get_mut("generated-root")
        .ok_or("missing role")?
        .legacy_projections[0]
        .dependency_search_paths
        .push(super::super::OvenRustcSourceSearchDirectory {
            relative_path: "../escape".to_string(),
            artifacts: Vec::new(),
        });
    assert!(invalid.validate_shape(&receipt.intent).is_err());
    Ok(())
}
#[test]
fn source_search_roles_bind_separate_roots_and_keep_caller_paths() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let artifacts = source_search_fixture(&receipt)?;
    let first = tempfile::tempdir()?;
    let first_root = fs::canonicalize(first.path())?;
    let second = tempfile::tempdir()?;
    let second_root = fs::canonicalize(second.path())?;
    let third = tempfile::tempdir()?;
    let third_root = fs::canonicalize(third.path())?;
    let roots = [first.path(), second.path(), third.path()];
    for (root, artifact) in roots.iter().zip(&artifacts.externs) {
        let path = root.join(&artifact.relative_path);
        fs::create_dir_all(path.parent().ok_or("fixture artifact has no parent")?)?;
        let bytes: &[u8] = match artifact.crate_name.as_str() {
            "runtime" => b"runtime",
            "derive" => b"macro",
            _ => b"helper",
        };
        fs::write(path, bytes)?;
    }
    let members = artifacts
        .externs
        .iter()
        .map(|artifact| {
            vec![OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path.clone(),
                digest: artifact.digest.clone(),
            }]
        })
        .collect::<Vec<_>>();
    let directories = [
        vec!["target".to_string()],
        vec!["host".to_string()],
        vec!["helper".to_string()],
    ];
    let fragments = roots
        .iter()
        .zip(&members)
        .zip(&directories)
        .map(|((root, members), paths)| super::super::OvenTrustedRustcArtifactRoot {
            artifact_root: root,
            dependency_search_paths: paths,
            native_search_paths: &[],
            supporting_artifacts: members,
            root_inventory: Some(members),
        })
        .collect::<Vec<_>>();
    let mut plan = artifacts.materialize_trusted_store_composed(&fragments, &receipt.intent)?;
    let caller = tempfile::tempdir()?;
    let caller_root = fs::canonicalize(caller.path())?;
    plan.dependency_search_paths.push(caller_root.clone());
    let projected = super::super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
    assert!(projected.dependency_search_paths.contains(&first_root.join("target")));
    assert!(projected.dependency_search_paths.contains(&second_root.join("host")));
    assert!(projected.dependency_search_paths.contains(&caller_root.clone()));
    assert!(!projected.dependency_search_paths.contains(&third_root.join("helper")));
    let projected_twice =
        super::super::trusted_artifact_plan_for_source_evidence(&projected, &artifacts, "generated-root")?;
    assert_eq!(projected, projected_twice);
    // This fragment was physically admitted above. An explicit provider dependency may retain its directory
    // after ordinary source filtering without granting the helper's direct extern name.
    let retained_fragment = third_root.join("helper");
    let mut with_provider = projected;
    with_provider.retain_caller_dependency_search_path(retained_fragment.clone());
    let repeated =
        super::super::trusted_artifact_plan_for_source_evidence(&with_provider, &artifacts, "generated-root")?;
    assert!(repeated.dependency_search_paths.contains(&retained_fragment));
    assert_eq!(repeated, with_provider);
    assert!(!repeated.externs.iter().any(|(name, _)| name == "private_helper"));
    plan.source_path_projection = None;
    assert!(super::super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root").is_err());
    Ok(())
}
#[test]
fn source_search_roles_keep_same_spelled_directories_with_their_owners() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let mut legacy = empty_manifest(&receipt);
    legacy.schema_version = super::super::OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION;
    legacy.dependency_search_paths = vec!["target".to_string(), "host".to_string()];
    legacy.externs = [
        ("runtime", "target/libruntime.rlib", b"runtime".as_slice()),
        ("private_helper", "host/libprivate_helper.rlib", b"helper".as_slice()),
    ]
    .into_iter()
    .map(|(name, path, bytes)| OvenRustcArtifactExtern {
        crate_name: name.to_string(),
        relative_path: path.to_string(),
        digest: digest_bytes(bytes),
    })
    .collect();
    legacy
        .entrypoint_externs
        .insert("generated-root".to_string(), vec!["runtime".to_string()]);
    let mut current = empty_manifest(&receipt);
    current.dependency_search_paths = vec!["host".to_string()];
    current.externs.push(OvenRustcArtifactExtern {
        crate_name: "current".to_string(),
        relative_path: "host/libcurrent.rlib".to_string(),
        digest: digest_bytes(b"current"),
    });
    let mut closure = legacy.source_search_closure("generated-root")?;
    closure.merge(&current.source_search_closure("generated-root")?);
    let mut combined = legacy.clone();
    combined.schema_version = OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION;
    combined.externs.extend(current.externs.clone());
    combined.entrypoint_externs.insert(
        "generated-root".to_string(),
        vec!["runtime".to_string(), "current".to_string()],
    );
    combined
        .entrypoint_dependency_search_paths
        .insert("generated-root".to_string(), closure);
    let combined: OvenRustcArtifactManifest = serde_json::from_slice(&serde_json::to_vec(&combined)?)?;
    let legacy_members = legacy.composition_artifacts()?;
    let current_members = current.composition_artifacts()?;
    // Two relocations retain identical logical paths and original schema-9 provenance.
    for _ in 0..2 {
        let first = tempfile::tempdir()?;
        let first_root = fs::canonicalize(first.path())?;
        let second = tempfile::tempdir()?;
        let second_root = fs::canonicalize(second.path())?;
        for (root, path, bytes) in [
            (first.path(), "target/libruntime.rlib", b"runtime".as_slice()),
            (first.path(), "host/libprivate_helper.rlib", b"helper".as_slice()),
            (second.path(), "host/libcurrent.rlib", b"current".as_slice()),
        ] {
            let file = root.join(path);
            fs::create_dir_all(file.parent().ok_or("fixture has no parent")?)?;
            fs::write(file, bytes)?;
        }
        let roots = [
            super::super::OvenTrustedRustcArtifactRoot {
                artifact_root: first.path(),
                dependency_search_paths: &legacy.dependency_search_paths,
                native_search_paths: &[],
                supporting_artifacts: &legacy_members,
                root_inventory: Some(&legacy_members),
            },
            super::super::OvenTrustedRustcArtifactRoot {
                artifact_root: second.path(),
                dependency_search_paths: &current.dependency_search_paths,
                native_search_paths: &[],
                supporting_artifacts: &current_members,
                root_inventory: Some(&current_members),
            },
        ];
        let plan = combined.materialize_trusted_store_composed(&roots, &receipt.intent)?;
        let projected = super::super::trusted_artifact_plan_for_source_evidence(&plan, &combined, "generated-root")?;
        assert!(projected.dependency_search_paths.contains(&second_root.join("host")));
        assert!(
            !projected.dependency_search_paths.contains(&first_root.join("host")),
            "the current contributor's host path must not grant the legacy helper owner"
        );
        assert!(!projected.externs.iter().any(|(name, _)| name == "private_helper"));
        assert_eq!(
            projected,
            super::super::trusted_artifact_plan_for_source_evidence(&projected, &combined, "generated-root")?
        );
        let mut changed_contract = combined.clone();
        changed_contract
            .entrypoint_dependency_search_paths
            .get_mut("generated-root")
            .ok_or("missing role")?
            .publisher_paths
            .clear();
        changed_contract.validate_shape(&receipt.intent)?;
        assert!(
            super::super::trusted_artifact_plan_for_source_evidence(&plan, &changed_contract, "generated-root")
                .is_err()
        );
        let mut missing_inventory = roots;
        missing_inventory[1].root_inventory = None;
        assert!(
            combined
                .materialize_trusted_store_composed(&missing_inventory, &receipt.intent)
                .is_err()
        );
        // The assigned fragment can be clean while its original physical root also contains an excluded file.
        fs::write(second_root.join("host/libprivate_helper.rlib"), b"helper")?;
        let mut co_resident = current_members.clone();
        co_resident.push(
            legacy_members
                .iter()
                .find(|artifact| artifact.relative_path == "host/libprivate_helper.rlib")
                .ok_or("missing helper")?
                .clone(),
        );
        missing_inventory[1].root_inventory = Some(&co_resident);
        let error = combined
            .materialize_trusted_store_composed(&missing_inventory, &receipt.intent)
            .err()
            .ok_or("co-resident excluded helper was accepted")?;
        // The refusal must name the artifact that disqualified the directory, not merely report that one
        // exists: "a co-resident unselected artifact" is what made this class cost a full bake to diagnose.
        let message = error.to_string();
        assert!(message.contains("never selected"), "{message}");
        assert!(message.contains("cannot isolate selected member"), "{message}");
        // An aggregate copy cannot isolate those two contributions either; no fallback republishing is allowed.
        fs::create_dir_all(second_root.join("target"))?;
        fs::write(second_root.join("target/libruntime.rlib"), b"runtime")?;
        assert!(combined.materialize(second.path(), &receipt.intent).is_err());
        let mut caller = projected;
        caller.retain_caller_dependency_search_path(first_root.join("host"));
        let repeated = super::super::trusted_artifact_plan_for_source_evidence(&caller, &combined, "generated-root")?;
        assert_eq!(caller, repeated);
        assert!(!repeated.externs.iter().any(|(name, _)| name == "private_helper"));
    }
    Ok(())
}
#[test]
fn source_search_roles_follow_exact_extension_partition_members() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let mut artifacts = empty_manifest(&receipt);
    artifacts.dependency_search_paths = vec!["deps".to_string()];
    artifacts.supporting_artifacts = [
        ("deps/libbase.rlib", b"base".as_slice()),
        ("deps/libextension.rlib", b"extension".as_slice()),
        ("deps/libextension.rmeta", b"metadata".as_slice()),
    ]
    .into_iter()
    .map(|(path, bytes)| OvenRustcSupportingArtifact {
        relative_path: path.to_string(),
        digest: digest_bytes(bytes),
    })
    .collect();
    let closure = artifacts.capture_source_search_closure(&artifacts.dependency_search_paths)?;
    artifacts
        .entrypoint_externs
        .insert("generated-root".to_string(), Vec::new());
    artifacts
        .entrypoint_dependency_search_paths
        .insert("generated-root".to_string(), closure);
    let base_fragment = artifacts.artifact_fragment(&BTreeSet::from(["deps/libbase.rlib".to_string()]))?;
    let partition = artifacts.partition_against_base(&base_fragment)?;
    let extension_fragment = artifacts.artifact_fragment(&partition.extension_paths)?;
    let base_inventory = base_fragment.composition_artifacts()?;
    let extension_inventory = extension_fragment.composition_artifacts()?;
    assert_eq!(extension_inventory.len(), 2);
    let base = tempfile::tempdir()?;
    let base_root = fs::canonicalize(base.path())?;
    let extension = tempfile::tempdir()?;
    let extension_root = fs::canonicalize(extension.path())?;
    for (root, path, bytes) in [
        (base.path(), "deps/libbase.rlib", b"base".as_slice()),
        (extension.path(), "deps/libextension.rlib", b"extension".as_slice()),
        (extension.path(), "deps/libextension.rmeta", b"metadata".as_slice()),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().ok_or("fixture parent missing")?)?;
        fs::write(path, bytes)?;
    }
    let roots = [
        super::super::OvenTrustedRustcArtifactRoot {
            artifact_root: base.path(),
            dependency_search_paths: &base_fragment.dependency_search_paths,
            native_search_paths: &[],
            supporting_artifacts: &base_inventory,
            root_inventory: Some(&base_inventory),
        },
        super::super::OvenTrustedRustcArtifactRoot {
            artifact_root: extension.path(),
            dependency_search_paths: &extension_fragment.dependency_search_paths,
            native_search_paths: &[],
            supporting_artifacts: &extension_inventory,
            root_inventory: Some(&extension_inventory),
        },
    ];
    let mut base_complete = base_inventory.clone();
    base_complete.push(
        extension_inventory
            .iter()
            .find(|artifact| artifact.relative_path.ends_with(".rlib"))
            .ok_or("extension rlib missing")?
            .clone(),
    );
    fs::write(base.path().join("deps/libextension.rlib"), b"extension")?;
    let mut roots = roots;
    roots[0].root_inventory = Some(&base_complete);
    let plan = artifacts.materialize_trusted_store_composed(&roots, &receipt.intent)?;
    let projected = super::super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
    assert_eq!(
        projected
            .dependency_search_paths
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([base_root.join("deps"), extension_root.join("deps")])
    );
    assert_eq!(
        projected,
        super::super::trusted_artifact_plan_for_source_evidence(&projected, &artifacts, "generated-root")?
    );
    // Canonical extension assignment stays intact when another admitted copy supplies the clean directory.
    let clean_extension = tempfile::tempdir()?;
    fs::create_dir_all(clean_extension.path().join("deps"))?;
    fs::write(clean_extension.path().join("deps/libextension.rlib"), b"extension")?;
    fs::write(clean_extension.path().join("deps/libextension.rmeta"), b"metadata")?;
    fs::write(extension.path().join("deps/libexcluded.rlib"), b"excluded")?;
    let mut extension_complete = extension_inventory.clone();
    extension_complete.push(OvenRustcSupportingArtifact {
        relative_path: "deps/libexcluded.rlib".to_string(),
        digest: digest_bytes(b"excluded"),
    });
    roots[1].root_inventory = Some(&extension_complete);
    let error = artifacts
        .materialize_trusted_store_composed(&roots, &receipt.intent)
        .err()
        .ok_or("co-resident extension helper must refuse without an admitted clean copy")?;
    // The refusal must name the artifact that disqualified the directory, not merely report that one
    // exists: "a co-resident unselected artifact" is what made this class cost a full bake to diagnose.
    let message = error.to_string();
    assert!(message.contains("never selected"), "{message}");
    assert!(message.contains("cannot isolate selected member"), "{message}");
    let candidates = [super::super::OvenTrustedRustcSearchRoot {
        artifact_root: clean_extension.path(),
        dependency_search_paths: &extension_fragment.dependency_search_paths,
        root_inventory: &extension_inventory,
    }];
    let plan = artifacts.materialize_trusted_store_composed_with_search_roots(&roots, &candidates, &receipt.intent)?;
    let projected = super::super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
    assert_eq!(
        projected.dependency_search_paths.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            base_root.join("deps"),
            fs::canonicalize(clean_extension.path())?.join("deps")
        ])
    );
    Ok(())
}
#[cfg(unix)]
#[test]
fn source_search_roles_keep_host_macro_metadata_without_a_direct_macro_grant() -> Result<(), Box<dyn std::error::Error>>
{
    let project = tempfile::tempdir()?;
    let output = tempfile::tempdir()?;
    let artifact_root = tempfile::tempdir()?;
    write_project(project.path())?;
    let rustc = rustc_path()?;
    let target = rustc_host_target(&rustc)?;
    let macro_source = project.path().join("derive.rs");
    let runtime_source = project.path().join("runtime.rs");
    let source = project.path().join("consumer.rs");
    let private_source = project.path().join("private.rs");
    fs::write(
        &macro_source,
        "extern crate proc_macro; use proc_macro::TokenStream; #[proc_macro_derive(Marker)] pub fn marker(_: TokenStream) -> TokenStream { TokenStream::new() }",
    )?;
    fs::write(
        &runtime_source,
        "#[derive(probe_derive::Marker)] pub struct Item; pub fn answer() -> i32 { 42 }",
    )?;
    fs::write(
        &source,
        "fn main() { assert_eq!(probe_runtime::answer(), 42); println!(\"42\"); }",
    )?;
    fs::write(
        &private_source,
        "use probe_derive::Marker; fn main() { assert_eq!(probe_runtime::answer(), 42); }",
    )?;
    fs::create_dir(artifact_root.path().join("host"))?;
    fs::create_dir(artifact_root.path().join("target"))?;
    let macro_name = format!(
        "host/{}probe_derive{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let macro_output = artifact_root.path().join(&macro_name);
    let runtime_output = artifact_root.path().join("target/libprobe_runtime.rlib");
    compile_source_search_probe(
        Command::new(&rustc)
            .args([
                "--edition=2024",
                "--crate-name",
                "probe_derive",
                "--crate-type",
                "proc-macro",
            ])
            .arg(&macro_source)
            .arg("-o")
            .arg(&macro_output),
    )?;
    compile_source_search_probe(
        Command::new(&rustc)
            .args([
                "--edition=2024",
                "--target",
                &target,
                "--crate-name",
                "probe_runtime",
                "--crate-type",
                "rlib",
            ])
            .arg(&runtime_source)
            .arg("--extern")
            .arg(format!("probe_derive={}", macro_output.display()))
            .arg("-o")
            .arg(&runtime_output),
    )?;
    let receipt = import_frozen_project(
        &OvenImportRequest::new(project.path(), target, rustc_identity(&rustc)?, "release", Vec::new())
            .with_supplemental_source_digest("generated-root", digest_bytes(&fs::read(&source)?))
            .with_supplemental_source_digest("private-root", digest_bytes(&fs::read(&private_source)?)),
    )?;
    let mut artifacts = empty_manifest(&receipt);
    artifacts.dependency_search_paths = vec!["target".to_string(), "host".to_string()];
    artifacts.externs = vec![
        OvenRustcArtifactExtern {
            crate_name: "probe_runtime".to_string(),
            relative_path: "target/libprobe_runtime.rlib".to_string(),
            digest: digest_bytes(&fs::read(&runtime_output)?),
        },
        OvenRustcArtifactExtern {
            crate_name: "probe_derive".to_string(),
            relative_path: macro_name,
            digest: digest_bytes(&fs::read(&macro_output)?),
        },
    ];
    for key in ["generated-root", "private-root"] {
        artifacts
            .entrypoint_externs
            .insert(key.to_string(), vec!["probe_runtime".to_string()]);
        artifacts.entrypoint_dependency_search_paths.insert(
            key.to_string(),
            artifacts.capture_source_search_closure(&artifacts.dependency_search_paths)?,
        );
    }
    let plan = artifacts.materialize(artifact_root.path(), &receipt.intent)?;
    for (label, trusted) in [("materialized", None), ("trusted", Some(&plan))] {
        let bake = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: trusted,
            rustc: &rustc,
            source: &source,
            output: &output.path().join(label),
            crate_name: "probe_consumer",
            edition: "2024",
            source_evidence_key: "generated-root",
            features: &[],
            prefer_dynamic: false,
        })?;
        let result = Command::new(&bake.output).output()?;
        assert!(result.status.success());
        assert_eq!(result.stdout, b"42\n");
    }
    let private = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &artifacts,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&plan),
        rustc: &rustc,
        source: &private_source,
        output: &output.path().join("private"),
        crate_name: "probe_private",
        edition: "2024",
        source_evidence_key: "private-root",
        features: &[],
        prefer_dynamic: false,
    });
    let Err(error) = private else {
        return Err("host search path granted a direct macro name".into());
    };
    assert!(error.to_string().contains("E0432"));
    let mut legacy = artifacts.clone();
    legacy.schema_version = super::super::OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION;
    legacy.entrypoint_dependency_search_paths.clear();
    let without_host = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &legacy,
        artifact_root: artifact_root.path(),
        artifact_plan: Some(&plan),
        rustc: &rustc,
        source: &source,
        output: &output.path().join("legacy"),
        crate_name: "probe_legacy",
        edition: "2024",
        source_evidence_key: "generated-root",
        features: &[],
        prefer_dynamic: false,
    });
    let Err(error) = without_host else {
        return Err("legacy projection unexpectedly retained the excluded host directory".into());
    };
    assert!(error.to_string().contains("E0463"));
    Ok(())
}
#[test]
fn trusted_plan_respects_entrypoint_externs_without_dropping_caller_libraries() -> Result<(), Box<dyn std::error::Error>>
{
    let project = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let mut artifacts = empty_manifest(&receipt);
    artifacts.dependency_search_paths = vec!["runtime-deps".to_string(), "vocab-deps".to_string()];
    artifacts.externs = vec![
        OvenRustcArtifactExtern {
            crate_name: "runtime".to_string(),
            relative_path: "runtime-deps/libruntime.rlib".to_string(),
            digest: digest_bytes(b"runtime"),
        },
        OvenRustcArtifactExtern {
            crate_name: "vocab_helper".to_string(),
            relative_path: "vocab-deps/libvocab_helper.rlib".to_string(),
            digest: digest_bytes(b"vocab helper"),
        },
    ];
    artifacts
        .entrypoint_externs
        .insert("generated-root".to_string(), vec!["runtime".to_string()]);
    artifacts.entrypoint_dependency_search_paths.insert(
        "generated-root".to_string(),
        artifacts.capture_source_search_closure(&["runtime-deps".to_string()])?,
    );
    let selected_artifacts = artifacts.for_source_evidence("generated-root")?;
    assert_eq!(selected_artifacts.dependency_search_paths, vec!["runtime-deps"]);
    let plan = OvenRustcArtifactPlan {
        source_path_projection: artifacts.source_search_roles_at_root(Path::new("/immutable"))?,
        dependency_search_paths: vec![
            PathBuf::from("/immutable/runtime-deps"),
            PathBuf::from("/immutable/vocab-deps"),
            PathBuf::from("/caller"),
        ],
        native_search_paths: Vec::new(),
        externs: vec![
            (
                "runtime".to_string(),
                PathBuf::from("/immutable/runtime-deps/libruntime.rlib"),
            ),
            (
                "vocab_helper".to_string(),
                PathBuf::from("/immutable/vocab-deps/libvocab_helper.rlib"),
            ),
            (
                "caller_library".to_string(),
                PathBuf::from("/caller/libcaller_library.rlib"),
            ),
        ],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::from([("caller_library".to_string(), digest_bytes(b"caller"))]),
    };

    let projected =
        super::super::trusted_artifact_plan_for_source(&plan, &artifacts, &selected_artifacts, "generated-root")?;

    assert_eq!(
        projected
            .externs
            .iter()
            .map(|(crate_name, _)| crate_name.as_str())
            .collect::<Vec<_>>(),
        vec!["runtime", "caller_library"]
    );
    assert_eq!(
        projected.dependency_search_paths,
        vec![PathBuf::from("/immutable/runtime-deps"), PathBuf::from("/caller")]
    );
    assert_eq!(
        projected.caller_owned_library_digests,
        plan.caller_owned_library_digests
    );
    Ok(())
}
#[test]
fn source_projection_allows_a_caller_registry_leaf_to_replace_a_private_compiler_helper()
-> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let caller = tempfile::tempdir()?;
    let receipt = intent(project.path())?;
    let caller_serde_json = caller.path().join("libserde_json.rlib");
    fs::write(&caller_serde_json, "caller serde_json")?;

    let mut artifacts = empty_manifest(&receipt);
    artifacts.dependency_search_paths = vec!["runtime-deps".to_string(), "compiler-private".to_string()];
    artifacts.externs = vec![
        OvenRustcArtifactExtern {
            crate_name: "runtime".to_string(),
            relative_path: "runtime-deps/libruntime.rlib".to_string(),
            digest: digest_bytes(b"runtime"),
        },
        OvenRustcArtifactExtern {
            crate_name: "serde_json".to_string(),
            relative_path: "compiler-private/libserde_json.rlib".to_string(),
            digest: digest_bytes(b"compiler serde_json"),
        },
    ];
    artifacts
        .entrypoint_externs
        .insert("generated-root".to_string(), vec!["runtime".to_string()]);
    artifacts.entrypoint_dependency_search_paths.insert(
        "generated-root".to_string(),
        artifacts.capture_source_search_closure(&["runtime-deps".to_string()])?,
    );
    let plan = OvenRustcArtifactPlan {
        source_path_projection: artifacts.source_search_roles_at_root(Path::new("/immutable"))?,
        dependency_search_paths: vec![
            PathBuf::from("/immutable/runtime-deps"),
            PathBuf::from("/immutable/compiler-private"),
        ],
        native_search_paths: Vec::new(),
        externs: vec![
            (
                "runtime".to_string(),
                PathBuf::from("/immutable/runtime-deps/libruntime.rlib"),
            ),
            (
                "serde_json".to_string(),
                PathBuf::from("/immutable/compiler-private/libserde_json.rlib"),
            ),
        ],
        compile_environment: BTreeMap::new(),
        caller_owned_library_digests: BTreeMap::new(),
    };

    let mut projected = super::super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
    attach_caller_owned_rustc_libraries(
        &mut projected,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "serde_json".to_string(),
            output: caller_serde_json.clone(),
            digest: digest_bytes(&fs::read(&caller_serde_json)?),
            expose_extern: true,
        }],
    )?;
    // The final bake projects a trusted plan once more. That second projection must distinguish the caller's
    // exact output from the compiler-private serde_json artifact it replaces.
    let projected = super::super::trusted_artifact_plan_for_source_evidence(&projected, &artifacts, "generated-root")?;

    assert_eq!(
        projected
            .externs
            .iter()
            .map(|(crate_name, path)| (crate_name.as_str(), path.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("runtime", PathBuf::from("/immutable/runtime-deps/libruntime.rlib")),
            ("serde_json", caller_serde_json),
        ]
    );
    assert!(
        !projected
            .dependency_search_paths
            .contains(&PathBuf::from("/immutable/compiler-private"))
    );
    Ok(())
}
