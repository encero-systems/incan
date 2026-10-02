//! Artifact-manifest and release-foundation composition regression tests.

use super::*;

#[test]
fn release_profile_optimizes_because_rustc_optimizes_nothing_by_default() {
    // Cargo used to supply this from `[profile.release]`. When Oven replaced Cargo on the normal build path
    // nothing did, and `incan build --release` shipped `opt-level=0` binaries that ran about six times slower
    // than the identical sources at `-C opt-level=3`. Assert the flag rather than trusting a default.
    let mut command = Command::new("rustc");
    apply_oven_profile(&mut command, "release");
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(arguments, vec!["-C", "opt-level=3"]);
}

#[test]
fn generated_root_registry_externs_follow_the_release_unless_directly_declared()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let build_intent = intent(root.path())?.intent;
    let registry_source = OvenRustcRegistrySource {
        registry: "registry+https://example.invalid/index".to_string(),
        checksum: "fictional-checksum".to_string(),
        relative_root: "registry-sources/fictional-shared-codec".to_string(),
        digest: "sha256:fictional-source".to_string(),
    };
    let release_artifact = OvenRustcArtifactExtern {
        crate_name: "shared_codec".to_string(),
        relative_path: "release/deps/libshared_codec.rlib".to_string(),
        digest: "sha256:release-codec".to_string(),
    };
    let extension_artifact = OvenRustcArtifactExtern {
        crate_name: "shared_codec".to_string(),
        relative_path: "extension/deps/libshared_codec.rlib".to_string(),
        digest: "sha256:extension-codec".to_string(),
    };
    let manifest = |artifact: OvenRustcArtifactExtern| OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: build_intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: vec![artifact.clone()],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::from([("generated-root".to_string(), vec![artifact.crate_name.clone()])]),
        registry_leaves: vec![OvenRustcRegistryLeaf {
            domain: Default::default(),
            crate_kind: Default::default(),
            selected_unit_identity: Some(format!("{}-unit", artifact.digest)),
            package: "fictional-shared-codec".to_string(),
            version: "1.0.0".to_string(),
            crate_name: artifact.crate_name.clone(),
            features: Vec::new(),
            source: registry_source.clone(),
            artifact,
        }],
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    let base = manifest(release_artifact.clone());
    let project = manifest(extension_artifact.clone());

    let mut transitive = project.clone();
    crate::rustc::select_generated_root_registry_externs(&mut transitive, &base, &BTreeSet::new())?;
    assert_eq!(transitive.externs.first(), Some(&release_artifact));
    assert_eq!(transitive.registry_leaves[0].artifact, extension_artifact);

    let mut direct = project;
    crate::rustc::select_generated_root_registry_externs(
        &mut direct,
        &base,
        &BTreeSet::from(["fictional_shared_codec".to_string()]),
    )?;
    assert_eq!(direct.externs, [extension_artifact]);
    Ok(())
}
#[test]
fn artifact_manifest_rejects_an_escaping_path() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let manifest = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: intent(root.path())?.intent,
        dependency_search_paths: vec!["../escape".to_string()],
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
    let result = manifest.materialize(root.path(), &intent(root.path())?.intent);
    assert!(matches!(result, Err(OvenRustcError::InvalidArtifactPath { .. })));
    Ok(())
}
#[test]
fn composed_trusted_plan_uses_each_foundation_root_without_a_composite_directory()
-> Result<(), Box<dyn std::error::Error>> {
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    fs::create_dir_all(first.path().join("deps"))?;
    fs::create_dir_all(second.path().join("deps"))?;
    fs::write(first.path().join("deps/libfirst.rlib"), b"first")?;
    fs::write(second.path().join("deps/libsecond.rlib"), b"second")?;
    let receipt = intent(first.path())?;
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
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "deps/libfirst.rlib".to_string(),
                digest: "sha256:first".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libsecond.rlib".to_string(),
                digest: "sha256:second".to_string(),
            },
        ],
    };
    let first_fragment = vec![OvenRustcSupportingArtifact {
        relative_path: "deps/libfirst.rlib".to_string(),
        digest: "sha256:first".to_string(),
    }];
    let second_fragment = vec![OvenRustcSupportingArtifact {
        relative_path: "deps/libsecond.rlib".to_string(),
        digest: "sha256:second".to_string(),
    }];
    let search_paths = vec!["deps".to_string()];
    let roots = [
        OvenTrustedRustcArtifactRoot {
            artifact_root: first.path(),
            dependency_search_paths: &search_paths,
            native_search_paths: &[],
            supporting_artifacts: &first_fragment,
            root_inventory: Some(&first_fragment),
        },
        OvenTrustedRustcArtifactRoot {
            artifact_root: second.path(),
            dependency_search_paths: &search_paths,
            native_search_paths: &[],
            supporting_artifacts: &second_fragment,
            root_inventory: Some(&second_fragment),
        },
    ];

    let plan = manifest.materialize_trusted_store_composed(&roots, &receipt.intent)?;
    let first_deps = fs::canonicalize(first.path().join("deps"))?;
    let second_deps = fs::canonicalize(second.path().join("deps"))?;
    assert_eq!(plan.dependency_search_paths.len(), 2);
    assert!(plan.dependency_search_paths.iter().any(|path| path == &first_deps));
    assert!(plan.dependency_search_paths.iter().any(|path| path == &second_deps));

    let base_paths = BTreeSet::from(["deps/libfirst.rlib".to_string()]);
    let base_manifest = manifest.artifact_fragment(&base_paths)?;
    let partition = manifest.partition_against_base(&base_manifest)?;
    assert_eq!(partition.base_paths, base_paths);
    assert_eq!(
        partition.extension_paths,
        BTreeSet::from(["deps/libsecond.rlib".to_string()])
    );
    let extension_manifest = manifest.artifact_fragment(&partition.extension_paths)?;
    assert_eq!(extension_manifest.supporting_artifacts, second_fragment);

    let mut conflicting_base = base_manifest.clone();
    conflicting_base.supporting_artifacts[0].digest = "sha256:other".to_string();
    let conflict = manifest.partition_against_base(&conflicting_base);
    assert!(matches!(conflict, Err(OvenRustcError::InvalidInput { .. })));
    Ok(())
}
#[test]
fn registry_leaf_substitution_requires_the_same_compilation_identity() {
    // Cargo's `-<hash>` extra-filename suffix summarizes the unit's declared compilation identity, including
    // resolved transitive features and dependency identities. Two closures can share every declared coordinate
    // and still compile a crate to different identities (#1227); substituting across identities removes the only
    // artifact that satisfies retained dependents' recorded hashes. The filename alone is still not a build
    // witness: a base prebuilt on another machine publishes the same filename with a different strict version
    // hash, so the conservative regime additionally demands bit-identical content.
    let leaf = |relative_path: &str, digest_input: &str| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: "rand_core".to_string(),
        version: "0.6.4".to_string(),
        crate_name: "rand_core".to_string(),
        features: vec!["std".to_string()],
        source: fixture_registry_source(),
        artifact: OvenRustcArtifactExtern {
            crate_name: "rand_core".to_string(),
            relative_path: relative_path.to_string(),
            digest: digest_bytes(digest_input.as_bytes()),
        },
    };
    let project = leaf("entry/deps/librand_core-cf99342fb36f73de.rlib", "local-build");
    let matching_release = leaf("base/deps/librand_core-cf99342fb36f73de.rlib", "local-build");
    let foreign_release = leaf("base/deps/librand_core-cf99342fb36f73de.rlib", "foreign-build");
    let divergent_release = leaf("base/deps/librand_core-50f0d7ca30c6ae15.rlib", "foreign-build");
    assert!(super::super::same_registry_leaf_semantics(&project, &divergent_release));
    assert!(
        super::super::registry_leaf_substitution_is_safe(&project, &matching_release, false),
        "a bit-identical artifact in a different directory is a pure byte-canonicalization"
    );
    assert!(
        !super::super::registry_leaf_substitution_is_safe(&project, &foreign_release, false),
        "a same-filename artifact from a foreign build has a different SVH: retained dependents cannot load it"
    );
    assert!(
        !super::super::registry_leaf_substitution_is_safe(&project, &divergent_release, false),
        "a transitive leaf must not substitute across compilation identities: prebuilt dependents recorded its hash"
    );
    assert!(
        super::super::registry_leaf_substitution_is_safe(&project, &divergent_release, true),
        "a root-extern leaf may cross identities because the generated root recompiles against the release copy"
    );
}
#[test]
fn conservative_regime_keeps_a_root_linked_leaf_on_the_project_identity() -> Result<(), Box<dyn std::error::Error>> {
    // `shared` is root-linked and has a release counterpart with different bytes. `engine` — root-linked with
    // no counterpart — keeps its extension-built subtree, and that subtree recorded the project's `shared` by
    // exact identity hash, as does the extension's runtime the root now links. Nothing may cross identities:
    // the root keeps the project's `shared`, re-rooted beside the release twin, or the retained consumer stops
    // loading (#1227, IncQL on a packaged toolchain: `substrait` against the project's `serde`).
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let source = OvenRustcRegistrySource {
        registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        checksum: "shared-checksum".to_string(),
        relative_root: "registry-sources/shared".to_string(),
        digest: "sha256:shared-source".to_string(),
    };
    let registry_source = OvenRustcRegistrySourcePackage {
        package: "shared".to_string(),
        version: "1.0.0".to_string(),
        features: Vec::new(),
        source: source.clone(),
    };
    let leaf = |crate_name: &str, artifact| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: crate_name.to_string(),
        version: "1.0.0".to_string(),
        crate_name: crate_name.to_string(),
        features: Vec::new(),
        source: source.clone(),
        artifact,
    };
    let project_shared = OvenRustcArtifactExtern {
        crate_name: "shared".to_string(),
        relative_path: "target/debug/deps/libshared-aaaa.rlib".to_string(),
        digest: "sha256:project-shared".to_string(),
    };
    let base_shared = OvenRustcArtifactExtern {
        crate_name: "shared".to_string(),
        relative_path: "target/debug/deps/libshared-aaaa.rlib".to_string(),
        digest: "sha256:base-shared".to_string(),
    };
    let engine_extern = OvenRustcArtifactExtern {
        crate_name: "engine".to_string(),
        relative_path: "target/debug/deps/libengine-bbbb.rlib".to_string(),
        digest: "sha256:project-engine".to_string(),
    };
    let engine_source = OvenRustcRegistrySourcePackage {
        package: "engine".to_string(),
        ..registry_source.clone()
    };
    let project = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["target/debug/deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "target/debug/deps/libincan_std_core-project.rlib".to_string(),
                digest: "sha256:project-stdlib".to_string(),
            },
            engine_extern.clone(),
            project_shared.clone(),
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![
            leaf("engine", engine_extern.clone()),
            leaf("shared", project_shared.clone()),
        ],
        registry_sources: vec![registry_source.clone(), engine_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        // A root-linked leaf's rlib is declared once, as the root extern; only its sidecar is supporting.
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "target/debug/deps/libshared-aaaa.rmeta".to_string(),
                digest: "sha256:project-shared-meta".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                digest: "sha256:shared-manifest".to_string(),
            },
        ],
    };
    let base = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["target/debug/deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![OvenRustcArtifactExtern {
            crate_name: "incan_std_core".to_string(),
            relative_path: "target/debug/deps/libincan_std_core-release.rlib".to_string(),
            digest: "sha256:release-stdlib".to_string(),
        }],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![leaf("shared", base_shared.clone())],
        registry_sources: vec![registry_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: base_shared.relative_path.clone(),
                digest: base_shared.digest.clone(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "target/debug/deps/libshared-aaaa.rmeta".to_string(),
                digest: "sha256:base-shared-meta".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                digest: "sha256:shared-manifest".to_string(),
            },
        ],
    };
    let composed = project
        .with_release_cohort_from_base(&base, &BTreeSet::new())
        .map_err(|error| format!("first composition: {error:?}"))?;
    // The regime retains extension-built consumers, so the root keeps the extension's runtime: one unified
    // resolution links once, and a process-global runtime cannot be brought in twice through the base.
    let runtime = composed
        .externs
        .iter()
        .find(|artifact| artifact.crate_name == "incan_std_core")
        .ok_or("composed plan lost the runtime extern")?;
    assert_eq!(
        (runtime.relative_path.as_str(), runtime.digest.as_str()),
        (
            "target/debug/deps/libincan_std_core-project.rlib",
            "sha256:project-stdlib"
        ),
        "the conservative regime links the extension's runtime, not the base's"
    );
    let root_link = composed
        .externs
        .iter()
        .find(|artifact| artifact.crate_name == "shared")
        .ok_or("composed plan lost the shared root extern")?;
    assert_eq!(
        (root_link.relative_path.as_str(), root_link.digest.as_str()),
        (
            "target/debug/extension-deps/libshared-aaaa.rlib",
            "sha256:project-shared"
        ),
        "the root keeps the project's copy, re-rooted beside the release twin"
    );
    let leaf_record = composed
        .registry_leaves
        .iter()
        .find(|candidate| candidate.crate_name == "shared")
        .ok_or("composed plan lost the shared leaf")?;
    assert_eq!(
        (
            leaf_record.artifact.relative_path.as_str(),
            leaf_record.artifact.digest.as_str()
        ),
        (
            "target/debug/extension-deps/libshared-aaaa.rlib",
            "sha256:project-shared"
        )
    );
    let supporting_paths = composed
        .supporting_artifacts
        .iter()
        .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        supporting_paths.contains(&(
            "target/debug/extension-deps/libshared-aaaa.rmeta",
            "sha256:project-shared-meta"
        )),
        "the split-metadata sidecar must move with the project's rlib"
    );
    // The base's copy and sidecar join the plan at their own paths for the sealed runtime's execution closure.
    assert!(
        supporting_paths.contains(&("target/debug/deps/libshared-aaaa.rlib", "sha256:base-shared")),
        "the base copy must join the plan at its own path"
    );
    assert!(
        supporting_paths.contains(&("target/debug/deps/libshared-aaaa.rmeta", "sha256:base-shared-meta")),
        "the base sidecar must join the plan beside the base copy"
    );
    assert_eq!(
        composed
            .declared_artifact_paths()?
            .iter()
            .filter(|path| path.ends_with("libshared-aaaa.rlib"))
            .count(),
        2,
        "exactly the base copy in `deps` and the project copy in `extension-deps` are declared"
    );
    assert!(
        !supporting_paths.contains(&("target/debug/deps/libshared-aaaa.rlib", "sha256:project-shared")),
        "the project's copy must not remain at the colliding path"
    );
    assert!(
        composed
            .dependency_search_paths
            .iter()
            .any(|path| path == "target/debug/extension-deps"),
        "the re-rooted directory must join the search paths"
    );
    let role = &composed.entrypoint_dependency_search_paths["generated-root"];
    let relocated = role
        .directories()
        .find(|directory| directory.relative_path == "target/debug/extension-deps")
        .ok_or("rerooted source directory missing")?;
    assert!(relocated.artifacts.iter().any(|artifact| artifact.relative_path
        == "target/debug/extension-deps/libshared-aaaa.rlib"
        && artifact.digest == "sha256:project-shared"));
    assert!(relocated.artifacts.iter().any(|artifact| artifact.relative_path
        == "target/debug/extension-deps/libshared-aaaa.rmeta"
        && artifact.digest == "sha256:project-shared-meta"));
    assert!(
        role.directories()
            .filter(|directory| directory.relative_path == "target/debug/deps")
            .all(|directory| !directory
                .artifacts
                .iter()
                .any(|artifact| artifact.digest == "sha256:project-shared"))
    );
    // Recomposition is a fixed point: the stored composed plan validates against its base unchanged.
    let recomposed = composed
        .with_release_cohort_from_base(&base, &BTreeSet::new())
        .map_err(|error| format!("recomposition: {error:?}"))?;
    assert_eq!(recomposed, composed);
    Ok(())
}
#[test]
fn a_named_role_claims_every_composed_artifact_in_a_directory_it_already_owns() -> Result<(), Box<dyn std::error::Error>>
{
    // Every other composition test here builds `entrypoint_externs: BTreeMap::new()`, which sends
    // `source_search_closure` down its fresh-capture branch. A publisher with a named role takes the other
    // branch and returns its *stored* closure, and that branch has had no composition coverage at all -- which
    // is why a stale role survived into a bake and only surfaced as "cannot isolate selected member".
    //
    // The invariant asserted here is the one nothing asserts today: a role that claims a directory must claim
    // every artifact the composed manifest declares in it. `bind_source_search_roles` refuses the directory
    // otherwise, so a composition that breaks this produces a plan that cannot be materialized.
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let project_runtime = OvenRustcArtifactExtern {
        crate_name: "incan_std_core".to_string(),
        relative_path: "target/debug/deps/libincan_std_core-project.rlib".to_string(),
        digest: "sha256:project-stdlib".to_string(),
    };
    let base_runtime = OvenRustcArtifactExtern {
        crate_name: "incan_std_core".to_string(),
        relative_path: "target/debug/deps/libincan_std_core-base.rlib".to_string(),
        digest: "sha256:base-stdlib".to_string(),
    };
    let mut project = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["target/debug/deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![project_runtime.clone()],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    // Give the publisher a named role whose closure is captured now, before the base contributes anything.
    let captured = project.capture_source_search_closure(&project.dependency_search_paths)?;
    project
        .entrypoint_externs
        .insert("generated-root".to_string(), vec!["incan_std_core".to_string()]);
    project
        .entrypoint_dependency_search_paths
        .insert("generated-root".to_string(), captured);

    // The base needs its own role closure: inheriting the project's would claim artifacts the base never
    // declares, which `validate_shape` rejects before composition even starts.
    let mut base = OvenRustcArtifactManifest {
        externs: vec![base_runtime.clone()],
        supporting_artifacts: vec![OvenRustcSupportingArtifact {
            relative_path: "target/debug/deps/libincan_std_core-base.rmeta".to_string(),
            digest: "sha256:base-stdlib-meta".to_string(),
        }],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        ..project.clone()
    };
    let base_captured = base.capture_source_search_closure(&base.dependency_search_paths)?;
    base.entrypoint_externs
        .insert("generated-root".to_string(), vec!["incan_std_core".to_string()]);
    base.entrypoint_dependency_search_paths
        .insert("generated-root".to_string(), base_captured);

    assert_role_closures_cover_declared_artifacts(&project.with_release_cohort_from_base(&base, &BTreeSet::new())?)?;

    // The conservative regime is the one the replay lane actually hits. A root-linked leaf with no release
    // counterpart makes the composition retain extension-built consumers, so the root keeps the extension's
    // runtime while the base still contributes its own release execution artifacts. The composed manifest then
    // declares both runtimes in one directory while the role's closure claims only the extension's.
    let mut conservative = project.clone();
    conservative.supporting_artifacts.push(OvenRustcSupportingArtifact {
        relative_path: "target/debug/deps/libprebuilt-consumer.rlib".to_string(),
        digest: "sha256:prebuilt-consumer".to_string(),
    });
    let conservative_captured = conservative.capture_source_search_closure(&conservative.dependency_search_paths)?;
    conservative
        .entrypoint_dependency_search_paths
        .insert("generated-root".to_string(), conservative_captured);
    assert_role_closures_cover_declared_artifacts(
        &conservative.with_release_cohort_from_base(&base, &BTreeSet::new())?,
    )?;
    Ok(())
}
#[test]
fn conservative_regime_reroots_retained_leaves_that_collide_with_a_foreign_base_copy()
-> Result<(), Box<dyn std::error::Error>> {
    // A salted extension unit shares its Cargo filename with the sealed base's twin while carrying a distinct
    // StableCrateId, so the composed plan must keep the project's copy — re-rooted into `extension-deps` with
    // its split-metadata sidecar — while the base's copy joins the plan at its own path for the sealed runtime,
    // and rustc selects each dependent's copy by recorded hash.
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let source = OvenRustcRegistrySource {
        registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        checksum: "cfg-ish-checksum".to_string(),
        relative_root: "registry-sources/cfg_ish".to_string(),
        digest: "sha256:cfg-ish-source".to_string(),
    };
    let registry_source = OvenRustcRegistrySourcePackage {
        package: "cfg_ish".to_string(),
        version: "1.0.0".to_string(),
        features: Vec::new(),
        source: source.clone(),
    };
    let leaf = |crate_name: &str, artifact| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: crate_name.replace('_', "-"),
        version: "1.0.0".to_string(),
        crate_name: crate_name.to_string(),
        features: Vec::new(),
        source: source.clone(),
        artifact,
    };
    let project_cfg_ish = OvenRustcArtifactExtern {
        crate_name: "cfg_ish".to_string(),
        relative_path: "target/debug/deps/libcfg_ish-aaaa.rlib".to_string(),
        digest: "sha256:project-cfg-ish".to_string(),
    };
    let engine_extern = OvenRustcArtifactExtern {
        crate_name: "engine".to_string(),
        relative_path: "target/debug/deps/libengine-bbbb.rlib".to_string(),
        digest: "sha256:project-engine".to_string(),
    };
    let cfg_ish_source = OvenRustcRegistrySourcePackage {
        package: "cfg-ish".to_string(),
        ..registry_source.clone()
    };
    let engine_source = OvenRustcRegistrySourcePackage {
        package: "engine".to_string(),
        ..registry_source.clone()
    };
    let project = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["target/debug/deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "target/debug/deps/libincan_std_core-project.rlib".to_string(),
                digest: "sha256:project-stdlib".to_string(),
            },
            engine_extern.clone(),
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![
            leaf("engine", engine_extern.clone()),
            leaf("cfg_ish", project_cfg_ish.clone()),
        ],
        registry_sources: vec![cfg_ish_source.clone(), engine_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: project_cfg_ish.relative_path.clone(),
                digest: project_cfg_ish.digest.clone(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "target/debug/deps/libcfg_ish-aaaa.rmeta".to_string(),
                digest: "sha256:project-cfg-ish-meta".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/cfg_ish/Cargo.toml".to_string(),
                digest: "sha256:cfg-ish-manifest".to_string(),
            },
        ],
    };
    let base = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["target/debug/deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![OvenRustcArtifactExtern {
            crate_name: "incan_std_core".to_string(),
            relative_path: "target/debug/deps/libincan_std_core-release.rlib".to_string(),
            digest: "sha256:release-stdlib".to_string(),
        }],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![leaf(
            "cfg_ish",
            OvenRustcArtifactExtern {
                crate_name: "cfg_ish".to_string(),
                relative_path: "target/debug/deps/libcfg_ish-aaaa.rlib".to_string(),
                digest: "sha256:base-cfg-ish".to_string(),
            },
        )],
        registry_sources: vec![cfg_ish_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "target/debug/deps/libcfg_ish-aaaa.rlib".to_string(),
                digest: "sha256:base-cfg-ish".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "target/debug/deps/libcfg_ish-aaaa.rmeta".to_string(),
                digest: "sha256:base-cfg-ish-meta".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/cfg_ish/Cargo.toml".to_string(),
                digest: "sha256:cfg-ish-manifest".to_string(),
            },
        ],
    };

    // `engine` is root-linked with no base counterpart, so the plan retains extension-built consumers and the
    // shared-filename `cfg_ish` leaf must not adopt the base bytes its dependents never recorded.
    let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    let rerooted = composed
        .registry_leaves
        .iter()
        .find(|candidate| candidate.crate_name == "cfg_ish")
        .ok_or("composed plan lost the retained cfg_ish leaf")?;
    assert_eq!(
        rerooted.artifact.relative_path,
        "target/debug/extension-deps/libcfg_ish-aaaa.rlib"
    );
    assert_eq!(rerooted.artifact.digest, "sha256:project-cfg-ish");
    let supporting_paths = composed
        .supporting_artifacts
        .iter()
        .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str()))
        .collect::<BTreeSet<_>>();
    assert!(
        supporting_paths.contains(&(
            "target/debug/extension-deps/libcfg_ish-aaaa.rlib",
            "sha256:project-cfg-ish"
        )),
        "the retained rlib must move to extension-deps with its project digest"
    );
    assert!(
        supporting_paths.contains(&(
            "target/debug/extension-deps/libcfg_ish-aaaa.rmeta",
            "sha256:project-cfg-ish-meta"
        )),
        "the split-metadata sidecar must move with its rlib"
    );
    assert!(
        supporting_paths.contains(&("target/debug/deps/libcfg_ish-aaaa.rlib", "sha256:base-cfg-ish")),
        "the base copy must join the plan at its own path for the sealed runtime"
    );
    assert!(
        composed
            .dependency_search_paths
            .iter()
            .any(|path| path == "target/debug/extension-deps"),
        "the re-rooted directory must join the dependency search paths"
    );
    composed.validate_release_cohort_from_base(&base)?;
    let partition = composed.partition_against_base(&base)?;
    assert!(
        partition
            .extension_paths
            .contains("target/debug/extension-deps/libcfg_ish-aaaa.rlib")
    );
    assert!(partition.base_paths.contains("target/debug/deps/libcfg_ish-aaaa.rlib"));
    assert_eq!(
        super::super::rerooted_artifact_staging_source("target/debug/extension-deps/libcfg_ish-aaaa.rlib").as_deref(),
        Some("target/debug/deps/libcfg_ish-aaaa.rlib")
    );
    Ok(())
}
#[test]
fn project_extension_replaces_the_complete_release_execution_cohort_with_base_artifacts()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let project = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-project.rlib".to_string(),
                digest: "sha256:project-runtime".to_string(),
            },
            OvenRustcArtifactExtern {
                crate_name: "project_dependency".to_string(),
                relative_path: "deps/libproject_dependency.rlib".to_string(),
                digest: "sha256:project-dependency".to_string(),
            },
            OvenRustcArtifactExtern {
                crate_name: "incan_partner".to_string(),
                relative_path: "deps/libincan_partner-project.rlib".to_string(),
                digest: "sha256:project-partner".to_string(),
            },
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_lang-project.rlib".to_string(),
                digest: "sha256:project-core".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_derive-project.dylib".to_string(),
                digest: "sha256:project-derive".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_partner_helper-project.rlib".to_string(),
                digest: "sha256:project-partner-helper".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                digest: "sha256:shared-source".to_string(),
            },
        ],
    };
    let base = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![OvenRustcArtifactExtern {
            crate_name: "incan_std_core".to_string(),
            relative_path: "deps/libincan_std_core-release.rlib".to_string(),
            digest: "sha256:release-runtime".to_string(),
        }],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: vec![OvenRustcAuxiliaryTarget {
            target: "wasm32-wasip1".to_string(),
            dependency_search_paths: vec!["vocab/deps".to_string()],
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "incan_vocab".to_string(),
                relative_path: "vocab/deps/libincan_vocab-release.rlib".to_string(),
                digest: "sha256:release-vocab".to_string(),
            }],
        }],
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_lang-release.rlib".to_string(),
                digest: "sha256:release-core".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_derive-release.dylib".to_string(),
                digest: "sha256:release-derive".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_stdlib_system-release.rlib".to_string(),
                digest: "sha256:release-system".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libbase_runtime_dependency.rlib".to_string(),
                digest: "sha256:base-runtime-dependency".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "vocab/deps/libvocab_dependency.rlib".to_string(),
                digest: "sha256:vocab-dependency".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                digest: "sha256:shared-source".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/base-only/Cargo.toml".to_string(),
                digest: "sha256:base-only-source".to_string(),
            },
        ],
    };

    assert_eq!(
        base.compiler_runtime_crate_names()?,
        BTreeSet::from([
            "incan_lang".to_string(),
            "incan_derive".to_string(),
            "incan_std_core".to_string(),
            "incan_stdlib_system".to_string(),
        ])
    );
    let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    assert_eq!(composed.externs[0].relative_path, "deps/libincan_std_core-release.rlib");
    assert_eq!(composed.externs[1], project.externs[1]);
    assert_eq!(composed.externs[2], project.externs[2]);
    assert_eq!(composed.vocab_auxiliary_targets, base.vocab_auxiliary_targets);
    assert_eq!(
        composed.supporting_artifacts,
        vec![
            OvenRustcSupportingArtifact {
                relative_path: "deps/libbase_runtime_dependency.rlib".to_string(),
                digest: "sha256:base-runtime-dependency".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_derive-release.dylib".to_string(),
                digest: "sha256:release-derive".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_lang-release.rlib".to_string(),
                digest: "sha256:release-core".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_partner_helper-project.rlib".to_string(),
                digest: "sha256:project-partner-helper".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "deps/libincan_stdlib_system-release.rlib".to_string(),
                digest: "sha256:release-system".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                digest: "sha256:shared-source".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "vocab/deps/libvocab_dependency.rlib".to_string(),
                digest: "sha256:vocab-dependency".to_string(),
            },
        ]
    );
    assert!(
        composed
            .supporting_artifacts
            .iter()
            .all(|artifact| artifact.relative_path != "registry-sources/base-only/Cargo.toml")
    );
    let partition = composed.partition_against_base(&base)?;
    assert_eq!(
        partition.base_paths,
        BTreeSet::from([
            "deps/libbase_runtime_dependency.rlib".to_string(),
            "deps/libincan_lang-release.rlib".to_string(),
            "deps/libincan_derive-release.dylib".to_string(),
            "deps/libincan_std_core-release.rlib".to_string(),
            "deps/libincan_stdlib_system-release.rlib".to_string(),
            "registry-sources/shared/Cargo.toml".to_string(),
            "vocab/deps/libincan_vocab-release.rlib".to_string(),
            "vocab/deps/libvocab_dependency.rlib".to_string(),
        ])
    );
    assert_eq!(
        partition.extension_paths,
        BTreeSet::from([
            "deps/libincan_partner-project.rlib".to_string(),
            "deps/libincan_partner_helper-project.rlib".to_string(),
            "deps/libproject_dependency.rlib".to_string(),
        ])
    );
    assert!(partition.extension_paths.contains("deps/libincan_partner-project.rlib"));

    let mut incomplete_base = base.clone();
    incomplete_base.externs.clear();
    let incomplete = project.with_release_cohort_from_base(&incomplete_base, &BTreeSet::new());
    assert!(matches!(incomplete, Err(OvenRustcError::InvalidInput { .. })));
    Ok(())
}
#[test]
fn project_extension_canonicalizes_the_locked_release_registry_cohort() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let receipt = intent(root.path())?;
    let source = OvenRustcRegistrySource {
        registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
        checksum: "serde-checksum".to_string(),
        relative_root: "registry-sources/serde".to_string(),
        digest: "sha256:serde-source".to_string(),
    };
    let registry_source = OvenRustcRegistrySourcePackage {
        package: "serde".to_string(),
        version: "1.0.228".to_string(),
        features: vec!["derive".to_string()],
        source: source.clone(),
    };
    let release_registry_source = OvenRustcRegistrySourcePackage {
        features: vec!["default".to_string(), "derive".to_string()],
        ..registry_source.clone()
    };
    let release_serde = OvenRustcArtifactExtern {
        crate_name: "serde".to_string(),
        relative_path: "deps/libserde-shared.rlib".to_string(),
        digest: "sha256:release-serde".to_string(),
    };
    let project_serde = OvenRustcArtifactExtern {
        digest: "sha256:publisher-local-serde".to_string(),
        ..release_serde.clone()
    };
    let leaf = |features: &[&str], artifact| OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: None,
        package: "serde".to_string(),
        version: "1.0.228".to_string(),
        crate_name: "serde".to_string(),
        features: features.iter().map(|feature| (*feature).to_string()).collect(),
        source: source.clone(),
        artifact,
    };
    let mut project = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: vec!["deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-project.rlib".to_string(),
                digest: "sha256:project-stdlib".to_string(),
            },
            project_serde.clone(),
            OvenRustcArtifactExtern {
                crate_name: "project_only".to_string(),
                relative_path: "deps/libproject_only.rlib".to_string(),
                digest: "sha256:project-only".to_string(),
            },
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![leaf(&["derive"], project_serde)],
        registry_sources: vec![registry_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "deps/libserde_derive-shared.dylib".to_string(),
                digest: "sha256:publisher-local-derive".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/serde/Cargo.toml".to_string(),
                digest: "sha256:serde-manifest".to_string(),
            },
        ],
    };
    let base = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent,
        dependency_search_paths: vec!["deps".to_string()],
        native_search_paths: Vec::new(),
        externs: vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_std_core".to_string(),
                relative_path: "deps/libincan_std_core-release.rlib".to_string(),
                digest: "sha256:release-stdlib".to_string(),
            },
            release_serde.clone(),
        ],
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: vec![leaf(&["derive"], release_serde.clone())],
        registry_sources: vec![release_registry_source.clone()],
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: vec![
            OvenRustcSupportingArtifact {
                relative_path: "deps/libserde_derive-shared.dylib".to_string(),
                digest: "sha256:release-derive".to_string(),
            },
            OvenRustcSupportingArtifact {
                relative_path: "registry-sources/serde/Cargo.toml".to_string(),
                digest: "sha256:serde-manifest".to_string(),
            },
        ],
    };

    let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    assert_eq!(composed.registry_leaves[0].artifact, release_serde);
    assert_eq!(composed.registry_leaves[0].features, ["derive"]);
    assert_eq!(composed.registry_sources[0], release_registry_source);
    assert_eq!(
        composed
            .supporting_artifacts
            .iter()
            .find(|artifact| artifact.relative_path == "deps/libserde_derive-shared.dylib")
            .map(|artifact| artifact.digest.as_str()),
        Some("sha256:release-derive")
    );
    let partition = composed.partition_against_base(&base)?;
    assert!(partition.base_paths.contains("deps/libserde-shared.rlib"));
    assert!(partition.base_paths.contains("deps/libserde_derive-shared.dylib"));
    assert!(partition.extension_paths.contains("deps/libproject_only.rlib"));

    let mut project_with_distinct_leaf_features = project.clone();
    let feature_artifact = OvenRustcArtifactExtern {
        crate_name: "serde".to_string(),
        relative_path: "deps/libserde-project-feature.rlib".to_string(),
        digest: "sha256:project-feature-serde".to_string(),
    };
    // The feature-divergent leaf forces the conservative regime, where a shared unit with different bytes is a
    // reproducibility failure. The publisher's serde_derive is the same locked unit as the release's, so under
    // deterministic path remapping it reproduces the release bytes exactly.
    project_with_distinct_leaf_features.supporting_artifacts[0].digest = "sha256:release-derive".to_string();
    project_with_distinct_leaf_features.externs[1] = feature_artifact.clone();
    project_with_distinct_leaf_features.registry_leaves[0]
        .features
        .push("rc".to_string());
    project_with_distinct_leaf_features.registry_leaves[0].artifact = feature_artifact.clone();
    project_with_distinct_leaf_features.registry_sources[0]
        .features
        .push("rc".to_string());
    let feature_distinct =
        project_with_distinct_leaf_features.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    assert_eq!(feature_distinct.registry_leaves[0].artifact, feature_artifact);
    assert!(
        feature_distinct
            .partition_against_base(&base)?
            .extension_paths
            .contains("deps/libserde-project-feature.rlib")
    );

    let mut project_with_alternate = project.clone();
    project_with_alternate
        .registry_sources
        .push(OvenRustcRegistrySourcePackage {
            package: "serde".to_string(),
            version: "2.0.0".to_string(),
            features: vec!["alloc".to_string()],
            source: OvenRustcRegistrySource {
                registry: source.registry.clone(),
                checksum: "project-serde-v2-checksum".to_string(),
                relative_root: "registry-sources/serde-v2".to_string(),
                digest: "sha256:project-serde-v2-source".to_string(),
            },
        });
    project_with_alternate
        .supporting_artifacts
        .push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/serde-v2/Cargo.toml".to_string(),
            digest: "sha256:project-serde-v2-manifest".to_string(),
        });
    let composed_with_alternate = project_with_alternate.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    assert!(composed_with_alternate.registry_sources.iter().any(|package| {
        package.package == "serde"
            && package.version == "2.0.0"
            && package.source.checksum == "project-serde-v2-checksum"
    }));
    assert!(
        composed_with_alternate
            .partition_against_base(&base)?
            .extension_paths
            .contains("registry-sources/serde-v2/Cargo.toml")
    );

    project.registry_sources[0].features.push("rc".to_string());
    let feature_extended = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
    assert_eq!(feature_extended.registry_sources[0].features, ["derive", "rc"]);
    project.registry_sources[0].source.checksum = "mismatched-checksum".to_string();
    let mismatch = project.with_release_cohort_from_base(&base, &BTreeSet::new());
    assert!(matches!(mismatch, Err(OvenRustcError::InvalidInput { .. })));
    Ok(())
}
