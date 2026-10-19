//! Exercise the bootstrap adapter with real native dependencies and compatible checkout relocation.

use super::select_declared_native_loaf_plan;
use oven_rustc::rustc::{
    OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_run_in_store, resolve_active_rustc,
    rustc_host_target, rustc_identity,
};
use oven_store::store::{OvenStore, OvenStoreLimits};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use std::path::Path;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Configured native inputs require one complete coordinate set without silently borrowing an SDK fallback.
#[test]
fn configured_native_inputs_require_complete_coordinates() -> TestResult {
    use super::ConfiguredNativeLoafInputs;
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    let mut values = BTreeMap::<&str, OsString>::new();
    assert!(ConfiguredNativeLoafInputs::from_values(|name| values.get(name).cloned())?.is_none());
    values.insert("INCAN_SDK_NATIVE_COMPILER_GRAPH", "graph.json".into());
    assert!(
        ConfiguredNativeLoafInputs::from_values(|name| values.get(name).cloned())
            .err()
            .ok_or("missing index accepted")?
            .to_string()
            .contains("INCAN_SDK_NATIVE_INDEX")
    );
    values.insert("INCAN_SDK_NATIVE_INDEX", "index".into());
    assert!(
        ConfiguredNativeLoafInputs::from_values(|name| values.get(name).cloned())
            .err()
            .ok_or("missing blobs accepted")?
            .to_string()
            .contains("INCAN_SDK_NATIVE_BLOBS")
    );
    values.insert("INCAN_SDK_NATIVE_BLOBS", "blobs".into());
    let inputs = ConfiguredNativeLoafInputs::from_values(|name| values.get(name).cloned())?
        .ok_or("complete native coordinates missing")?;
    assert_eq!(inputs.graph, Path::new("graph.json"));
    assert_eq!(inputs.index, Path::new("index"));
    assert_eq!(inputs.blobs, Path::new("blobs"));
    values.insert("INCAN_SDK_NATIVE_INDEX", OsString::new());
    assert!(ConfiguredNativeLoafInputs::from_values(|name| values.get(name).cloned()).is_err());
    Ok(())
}

/// Independent consumer subsets keep each admitted record's coordinates, without weakening composition conflicts.
#[test]
fn native_closure_coordinates_follow_records_across_consumer_subsets() -> TestResult {
    // This control intentionally publishes two fresh native units; retain the outer harness reuse requirement.
    if std::env::var_os("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE").is_some() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "build::native_loaf_plan::tests::native_closure_coordinates_follow_records_across_consumer_subsets",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
            .output()?;
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return Ok(());
    }
    use oven_rustc::native_loaf::{
        NativeLoafClosure, NativeLoafConsumerRequest, prepare_declared_native_loafs_in_store,
    };
    use oven_rustc::plan::composition::merge_packaged_provider_artifact_manifests;
    use std::collections::BTreeSet;

    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    write_checkout(&root)?;
    std::fs::create_dir_all(root.join("additional/src"))?;
    std::fs::write(
        root.join("additional/loaf.toml"),
        "[project]\nname='additional'\nversion='1.0.0'\n[rust]\nname='additional'\ntype='lib'\nedition='2024'\n",
    )?;
    std::fs::write(root.join("additional/src/lib.rs"), "pub const VALUE: u8 = 11;\n")?;
    std::fs::write(
        root.join("graph.json"),
        r#"{"index_commit":"0000000000000000000000000000000000000000","registry_lock":"lock.json","facets":[{"project":"dependency","features":[],"domain":"target"},{"project":"additional","features":[],"domain":"target"}]}"#,
    )?;
    let mut dependencies =
        oven_model::manifest::ProjectManifest::load(&root.join("loaf.toml"))?.rust_dependency_values();
    dependencies.push(oven_model::manifest::DependencySpec {
        crate_name: "additional".into(),
        version: None,
        features: Vec::new(),
        default_features: false,
        source: oven_model::manifest::DependencySource::Path {
            path: root.join("additional"),
        },
        optional: false,
        package: None,
    });
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &root,
            "coordinate-consumer",
            "1.0.0",
            &target,
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        )
        .with_generated_source("compiler-main", root.join("src/main.rs")),
    )?;
    let store = OvenStore::new(
        root.join("store"),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    let prepared = prepare_declared_native_loafs_in_store(
        &NativeLoafConsumerRequest {
            graph: &root.join("graph.json"),
            index: &root,
            blobs: &root,
            output: &root.join("native-hints"),
            rustc: &rustc,
            target: &target,
            profile: "debug",
            dependencies: &dependencies,
            declaration_owner: &root,
            domain: "target",
        },
        &store,
    )?;

    // ---- Subset coordinates: both singleton graphs formerly assigned their distinct owners units/0 ----
    let mut union = super::empty_manifest(&receipt);
    let mut union_roots = Vec::new();
    super::append_native_loaf_closure(prepared.closure(), &receipt, &mut union, &mut union_roots, "units")?;
    assert_eq!(union_roots.len(), 2);
    let mut subsets = Vec::new();
    for (index, unit) in prepared.closure().graph().units().values().enumerate() {
        let closure = NativeLoafClosure::admit(&store, &[unit.declared_root(&format!("member_{index}"))?])?;
        let mut manifest = super::empty_manifest(&receipt);
        let mut roots = Vec::new();
        super::append_native_loaf_closure(&closure, &receipt, &mut manifest, &mut roots, "units")?;
        assert_eq!(roots.len(), 1);
        let singleton = roots.first().ok_or("singleton owner missing")?;
        let original = union_roots
            .iter()
            .find(|candidate| candidate.identity == singleton.identity)
            .ok_or("union lost singleton native owner")?;
        assert_eq!(
            singleton.prefix, original.prefix,
            "consumer membership must not rename an admitted native record"
        );
        assert_eq!(singleton.store, original.store);
        assert_eq!(singleton.receipt_identity, original.receipt_identity);
        super::seal_source_roles(&receipt, &mut manifest);
        manifest.validate_shape(&receipt.intent)?;
        subsets.push(manifest);
    }
    let [first, second] = subsets.as_slice() else {
        return Err("expected two singleton manifests".into());
    };
    assert!(
        first
            .dependency_search_paths
            .iter()
            .all(|path| !second.dependency_search_paths.contains(path))
    );

    // ---- Composition: either order preserves both recorded source-role directories ----
    let mut composed_views = Vec::new();
    for inputs in [
        [("first", first), ("second", second)],
        [("second", second), ("first", first)],
    ] {
        let composed = merge_packaged_provider_artifact_manifests(&inputs, &receipt.intent)?;
        let expected = union.dependency_search_paths.iter().cloned().collect::<BTreeSet<_>>();
        assert_eq!(
            composed
                .dependency_search_paths
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(
            composed.source_search_closure("compiler-main")?.publisher_paths.len(),
            2
        );
        composed_views.push(composed);
    }
    assert_eq!(composed_views.first(), composed_views.last());

    // ---- Refusals: one sealed coordinate cannot change bytes, and distinct target externs cannot share an alias ----
    let mut corrupted = first.clone();
    corrupted.externs.first_mut().ok_or("first extern missing")?.digest = oven_store::digest_bytes(b"changed");
    super::seal_source_roles(&receipt, &mut corrupted);
    let same_owner =
        merge_packaged_provider_artifact_manifests(&[("original", first), ("changed", &corrupted)], &receipt.intent);
    assert!(same_owner.is_err_and(|error| error.to_string().contains("conflicting sealed bytes")));
    let mut conflicting = second.clone();
    conflicting
        .externs
        .first_mut()
        .ok_or("second extern missing")?
        .crate_name = first.externs.first().ok_or("first extern missing")?.crate_name.clone();
    super::seal_source_roles(&receipt, &mut conflicting);
    let direct =
        merge_packaged_provider_artifact_manifests(&[("first", first), ("second", &conflicting)], &receipt.intent);
    assert!(direct.is_err_and(|error| error.to_string().contains("direct target Rust crate")));
    Ok(())
}

/// Source-current bootstrap plans share native generations while output relocation reuses the executable.
#[test]
fn declared_bootstrap_dependencies_share_command_store() -> TestResult {
    // This fixture deliberately starts with a new Store before proving reuse inside that Store. Keep the enclosing
    // test-root reuse guard intact and isolate the fixture's intentional first compilation in its exact child.
    if std::env::var_os("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE").is_some() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "build::native_loaf_plan::tests::declared_bootstrap_dependencies_share_command_store",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
            .output()?;
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return Ok(());
    }
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    let rustc = resolve_active_rustc()?;
    let store = OvenStore::new(
        root.join("shared-store"),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    for name in ["one", "two"] {
        write_checkout(&root.join(name))?;
    }
    for phase in ["first", "repeat"] {
        for name in ["one", "two"] {
            let checkout = root.join(name);
            let source = checkout.join("src/main.rs");
            let request = OvenGeneratedProjectRequest::new(
                &checkout,
                "bootstrap-consumer",
                "1.0.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("compiler-main", &source);
            let source_receipt = receipt_generated_project(&request)?;
            let native_output = checkout.join("native-hints");
            let (receipt, selection, report) = select_declared_native_loaf_plan(
                &store,
                &source_receipt,
                &checkout.join("loaf.toml"),
                &rustc,
                &checkout.join("graph.json"),
                &root,
                &root,
                &native_output,
            )?;
            let report: serde_json::Value = serde_json::from_str(&report)?;
            assert_eq!(
                report["compiled"].as_array().ok_or("native work missing")?.len(),
                usize::from(phase == "first" && name == "one")
            );
            if phase == "repeat" {
                assert_eq!(report["preparation_calls"], 0);
            }
            assert!(!native_output.join("store").exists());
            let plan = selection.source_artifact_plan("compiler-main")?;
            let output = checkout.join(format!("outputs/{phase}/consumer"));
            std::fs::create_dir_all(output.parent().ok_or("output parent missing")?)?;
            let baked = bake_trusted_direct_rustc_run_in_store(
                &OvenTrustedDirectRustcTargetRequest {
                    receipt: &receipt,
                    artifacts: selection.artifacts(),
                    artifact_root: selection.output_guard_root(),
                    artifact_plan: Some(&plan),
                    rustc: &rustc,
                    source: &source,
                    output: &output,
                    crate_name: "bootstrap_consumer",
                    edition: "2024",
                    source_evidence_key: "compiler-main",
                    features: &[],
                    prefer_dynamic: false,
                },
                &store,
            )?;
            assert!(!baked.cargo_process_started);
            if phase == "repeat" {
                assert!(baked.reused);
            }
            let executed = std::process::Command::new(&output).output()?;
            assert!(executed.status.success());
            assert_eq!(executed.stdout, b"7");
        }
    }
    let inspection = store.inspect()?;
    for domain in ["sdk-source-unit-target", "native-loaf-record"] {
        assert_eq!(
            inspection
                .entries
                .iter()
                .filter(|entry| entry.manifest.domain == domain)
                .count(),
            1
        );
    }

    // ---- Retained owner: the same original plan serves the unchanged request ----
    let checkout = root.join("one");
    let source = checkout.join("src/main.rs");
    let generated_request = OvenGeneratedProjectRequest::new(
        &checkout,
        "bootstrap-consumer",
        "1.0.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("compiler-main", &source);
    let source_receipt = receipt_generated_project(&generated_request)?;
    let mut request = crate::build::executable::NativeExecutableBuildRequest {
        store: store.root().to_path_buf(),
        limits: OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
        source_receipt,
        declaration: checkout.join("loaf.toml"),
        rustc,
        graph: checkout.join("graph.json"),
        index: root.clone(),
        blobs: root.clone(),
        preparation_output: checkout.join("native-hints"),
        source,
        output: checkout.join("outputs/owner/consumer"),
        crate_name: "bootstrap_consumer".to_string(),
        edition: "2024".to_string(),
        source_key: "compiler-main".to_string(),
    };
    let mut owner = crate::build::executable::ExecutableBuildOwner::default();
    std::fs::create_dir_all(request.output.parent().ok_or("owner output parent missing")?)?;
    owner.build_native(&request)?;
    let repeated = owner.build_native(&request)?;
    assert_eq!(repeated["native_plan_reused"], true);
    assert_eq!(repeated["reused"], true);
    assert_eq!(std::process::Command::new(&request.output).output()?.stdout, b"7");

    // ---- Consumer change: keep the dependency plan while compiling the new body ----
    std::fs::write(
        &request.source,
        "fn main() { print!(\"{}\", dependency::VALUE + 1); }\n",
    )?;
    request.source_receipt = receipt_generated_project(&generated_request)?;
    let changed = owner.build_native(&request)?;
    assert_eq!(changed["native_plan_reused"], true);
    assert_eq!(changed["reused"], false);
    assert_eq!(std::process::Command::new(&request.output).output()?.stdout, b"8");
    let replacement = root.join("replacement");
    std::fs::write(&replacement, "corrupt projected output")?;
    std::fs::rename(&replacement, &request.output)?;
    assert_eq!(owner.build_native(&request)?["native_plan_reused"], true);
    assert_eq!(std::process::Command::new(&request.output).output()?.stdout, b"8");

    // ---- Original plan and native members: corruption refuses, restoration re-admits ----
    let plan = inspection
        .entries
        .iter()
        .find(|entry| entry.manifest.domain == super::DOMAIN)
        .ok_or("ordinary native plan missing")?;
    let native = inspection
        .entries
        .iter()
        .find(|entry| entry.manifest.domain == "sdk-source-unit-target")
        .ok_or("native dependency missing")?;
    let member = native
        .manifest
        .materialized_files
        .iter()
        .find(|member| member.relative_path.ends_with(".rlib"))
        .ok_or("native archive missing")?;
    for path in [
        plan.path.join("payload"),
        native.path.join("artifacts").join(&member.relative_path),
    ] {
        let original = std::fs::read(&path)?;
        std::fs::write(&replacement, b"corrupt original input")?;
        std::fs::rename(&replacement, &path)?;
        assert!(
            owner.build_native(&request).is_err(),
            "changed input accepted: {}",
            path.display()
        );
        std::fs::write(&replacement, original)?;
        std::fs::rename(&replacement, &path)?;
        owner.build_native(&request)?;
        assert_eq!(std::process::Command::new(&request.output).output()?.stdout, b"8");
    }

    // ---- Declaration change: an old dependency plan cannot satisfy a removed requirement ----
    let declaration = std::fs::read(&request.declaration)?;
    std::fs::write(
        &request.declaration,
        "[project]\nname='bootstrap-consumer'\nversion='1.0.0'\n",
    )?;
    assert!(owner.build_native(&request).is_err());
    std::fs::write(&request.declaration, declaration)?;
    owner.build_native(&request)?;
    assert_eq!(std::process::Command::new(&request.output).output()?.stdout, b"8");
    Ok(())
}

/// Original release requests select only release owners; a changed consumer intent cannot reuse their plan.
#[test]
fn declared_native_plan_retains_requested_profile_and_toolchain() -> TestResult {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    write_checkout(&root)?;
    let rustc = resolve_active_rustc()?;
    let store = OvenStore::new(
        root.join("shared-store"),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    let request = OvenGeneratedProjectRequest::new(
        &root,
        "release-consumer",
        "1.0.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "release",
        Vec::new(),
    )
    .with_generated_source("compiler-main", root.join("src/main.rs"));
    let source_receipt = receipt_generated_project(&request)?;
    let select = |receipt: &oven_store::OvenReceipt| {
        select_declared_native_loaf_plan(
            &store,
            receipt,
            &root.join("loaf.toml"),
            &rustc,
            &root.join("graph.json"),
            &root,
            &root,
            &root.join("native-hints"),
        )
    };
    let (receipt, selection, _) = select(&source_receipt)?;
    assert_eq!(receipt.intent.profile, "release");
    assert_eq!(selection.artifacts().intent.profile, "release");
    let wrong_request = OvenGeneratedProjectRequest::new(
        &root,
        "release-consumer",
        "1.0.0",
        rustc_host_target(&rustc)?,
        "different-toolchain",
        "release",
        Vec::new(),
    )
    .with_generated_source("compiler-main", root.join("src/main.rs"));
    assert!(select(&receipt_generated_project(&wrong_request)?).is_err());
    let inspection = store.inspect()?;
    assert!(
        inspection
            .entries
            .iter()
            .all(|entry| entry.manifest.intent.profile == "release"),
        "release-only native requests must not prepare implicit debug owners"
    );
    Ok(())
}

/// Different consumers share required preparation while retaining exact imports, plans and source invalidation.
#[test]
fn grouped_native_consumers_prepare_once_and_keep_distinct_imports() -> TestResult {
    if std::env::var_os("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE").is_some() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "build::native_loaf_plan::tests::grouped_native_consumers_prepare_once_and_keep_distinct_imports",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
            .output()?;
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return Ok(());
    }
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    write_checkout(&root)?;
    std::fs::create_dir_all(root.join("additional/src"))?;
    std::fs::write(
        root.join("additional/loaf.toml"),
        "[project]\nname='additional'\nversion='1.0.0'\n[rust]\nname='additional'\ntype='lib'\nedition='2024'\n",
    )?;
    std::fs::write(root.join("additional/src/lib.rs"), "pub const VALUE: u8 = 11;\n")?;
    std::fs::write(
        root.join("graph.json"),
        r#"{"index_commit":"0000000000000000000000000000000000000000","registry_lock":"lock.json","facets":[{"project":"dependency","features":[],"domain":"target"},{"project":"additional","features":[],"domain":"target"}]}"#,
    )?;
    let manifest = oven_model::manifest::ProjectManifest::load(&root.join("loaf.toml"))?;
    let first_dependencies = manifest.rust_dependency_values();
    let mut all_dependencies = first_dependencies.clone();
    all_dependencies.push(oven_model::manifest::DependencySpec {
        crate_name: "additional".into(),
        version: None,
        features: Vec::new(),
        default_features: false,
        source: oven_model::manifest::DependencySource::Path {
            path: root.join("additional"),
        },
        optional: false,
        package: None,
    });
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let store = OvenStore::new(
        root.join("store"),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    let mut owner = crate::build::executable::ExecutableBuildOwner::default();
    let preparation = root.join("group");
    std::fs::write(
        root.join("src/first.rs"),
        "fn main() { print!(\"{}\", dependency::VALUE); }",
    )?;
    std::fs::write(
        root.join("src/second.rs"),
        "fn main() { print!(\"{}\", dependency::VALUE + additional::VALUE); }",
    )?;

    // ---- Current requirements: compile the union once, preserving each consumer's own direct imports ----
    for phase in ["first", "repeat", "changed"] {
        if phase == "changed" {
            std::fs::write(root.join("additional/src/lib.rs"), "pub const VALUE: u8 = 15;\n")?;
        }
        for name in ["first", "second"] {
            let source = root.join(format!("src/{name}.rs"));
            let receipt = receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    &root,
                    name,
                    "1.0.0",
                    &target,
                    rustc_identity(&rustc)?,
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("compiler-main", &source),
            )?;
            let hints = root.join(format!("hints/{name}"));
            let consumer = oven_rustc::native_loaf::NativeLoafConsumerRequest {
                graph: &root.join("graph.json"),
                index: &root,
                blobs: &root,
                output: &hints,
                rustc: &rustc,
                target: &target,
                profile: "debug",
                dependencies: if name == "first" {
                    &first_dependencies
                } else {
                    &all_dependencies
                },
                declaration_owner: &root,
                domain: "target",
            };
            let (receipt, selected, report, plan_reused) =
                owner.native_loaf_plan_in_group(&store, &receipt, &consumer, &all_dependencies, &preparation)?;
            let report: serde_json::Value = serde_json::from_str(&report)?;
            let expected_preparations = usize::from(name == "first" && phase != "repeat");
            assert_eq!(
                report["preparation_calls"], expected_preparations,
                "{phase}/{name}: {report}"
            );
            let expected_compiled = if expected_preparations == 0 {
                0
            } else if phase == "first" {
                2
            } else {
                1
            };
            assert_eq!(
                report["compiled"].as_array().ok_or("missing compilation report")?.len(),
                expected_compiled
            );
            if phase == "repeat" || (phase == "changed" && name == "first") {
                assert!(plan_reused, "unchanged consumer plan was not retained: {phase}/{name}");
            }
            let plan = selected.source_artifact_plan("compiler-main")?;
            assert!(plan.externs.iter().any(|(alias, _)| alias == "dependency"));
            assert_eq!(
                plan.externs.iter().any(|(alias, _)| alias == "additional"),
                name == "second"
            );
            let output = root.join(format!("{name}-executable"));
            let baked = bake_trusted_direct_rustc_run_in_store(
                &OvenTrustedDirectRustcTargetRequest {
                    receipt: &receipt,
                    artifacts: selected.artifacts(),
                    artifact_root: selected.output_guard_root(),
                    artifact_plan: Some(&plan),
                    rustc: &rustc,
                    source: &source,
                    output: &output,
                    crate_name: name,
                    edition: "2024",
                    source_evidence_key: "compiler-main",
                    features: &[],
                    prefer_dynamic: false,
                },
                &store,
            )?;
            assert!(!baked.cargo_process_started);
            if phase == "repeat" || (phase == "changed" && name == "first") {
                assert!(baked.reused);
            }
            let executed = std::process::Command::new(&output).output()?;
            assert!(executed.status.success());
            let expected: &[u8] = if name == "first" {
                b"7"
            } else if phase == "changed" {
                b"22"
            } else {
                b"18"
            };
            assert_eq!(executed.stdout, expected);
        }
    }
    Ok(())
}

/// A retained group projection cannot preserve removed imports or admit corrupt original native members.
#[test]
fn retained_group_projection_reconciles_requirements_and_native_members() -> TestResult {
    if std::env::var_os("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE").is_some() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "build::native_loaf_plan::tests::retained_group_projection_reconciles_requirements_and_native_members",
                "--nocapture",
                "--test-threads=1",
            ])
            .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
            .output()?;
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return Ok(());
    }
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    write_checkout(&root)?;
    let manifest = oven_model::manifest::ProjectManifest::load(&root.join("loaf.toml"))?;
    let dependencies = manifest.rust_dependency_values();
    let rustc = resolve_active_rustc()?;
    let target = rustc_host_target(&rustc)?;
    let store = OvenStore::new(
        root.join("store"),
        OvenStoreLimits::new(64 * 1024 * 1024, 64 * 1024 * 1024, 64 * 1024 * 1024),
    );
    let source = root.join("src/main.rs");
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &root,
            "consumer",
            "1.0.0",
            &target,
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        )
        .with_generated_source("compiler-main", &source),
    )?;
    let group = oven_rustc::native_loaf::NativeLoafConsumerRequest {
        graph: &root.join("graph.json"),
        index: &root,
        blobs: &root,
        output: &root.join("group"),
        rustc: &rustc,
        target: &target,
        profile: "debug",
        dependencies: &dependencies,
        declaration_owner: &root,
        domain: "target",
    };
    let mut consumer = oven_rustc::native_loaf::NativeLoafConsumerRequest {
        output: &root.join("consumer"),
        ..group
    };
    let mut owner = super::RetainedNativeLoafPlan::default();
    let (_, first, _, reused) = owner.select_group(&store, &receipt, &group, &consumer)?;
    assert!(!reused);
    let (_, repeat, _, reused) = owner.select_group(&store, &receipt, &group, &consumer)?;
    assert!(reused && std::sync::Arc::ptr_eq(&first, &repeat));

    // ---- Consumer requirements: an unchanged producer must not lend removed imports ----
    consumer.dependencies = &[];
    let (_, empty, _, reused) = owner.select_group(&store, &receipt, &group, &consumer)?;
    assert!(!reused);
    assert!(empty.source_artifact_plan("compiler-main")?.externs.is_empty());
    consumer.dependencies = &dependencies;
    let (_, restored, _, reused) = owner.select_group(&store, &receipt, &group, &consumer)?;
    assert!(!reused);
    assert_eq!(restored.source_artifact_plan("compiler-main")?.externs.len(), 1);
    assert!(owner.select_group(&store, &receipt, &group, &consumer)?.3);

    // ---- Original native owner: metadata reconciliation must refuse changed leased bytes ----
    let inspection = store.inspect()?;
    let native = inspection
        .entries
        .iter()
        .find(|entry| entry.manifest.domain == "sdk-source-unit-target")
        .ok_or("native dependency missing")?;
    let member = native
        .manifest
        .materialized_files
        .iter()
        .find(|member| member.relative_path.ends_with(".rlib"))
        .ok_or("native archive missing")?;
    let path = native.path.join("artifacts").join(&member.relative_path);
    let original = std::fs::read(&path)?;
    let replacement = root.join("replacement");
    std::fs::write(&replacement, b"corrupt retained dependency")?;
    std::fs::rename(&replacement, &path)?;
    assert!(owner.select_group(&store, &receipt, &group, &consumer).is_err());
    assert!(owner.preparation.is_none() && !owner.plans.contains_key(consumer.output));
    std::fs::write(&replacement, original)?;
    std::fs::rename(&replacement, &path)?;
    let (_, restored, _, reused) = owner.select_group(&store, &receipt, &group, &consumer)?;
    assert!(!reused);
    assert_eq!(restored.source_artifact_plan("compiler-main")?.externs.len(), 1);
    assert!(owner.select_group(&store, &receipt, &group, &consumer)?.3);
    Ok(())
}

/// Author an ordinary native dependency and consumer without Cargo manifests or registry units.
fn write_checkout(root: &Path) -> TestResult {
    std::fs::create_dir_all(root.join("dependency/src"))?;
    std::fs::create_dir_all(root.join("src"))?;
    for (path, content) in [
        (
            "dependency/loaf.toml",
            "[project]\nname='dependency'\nversion='1.0.0'\n[rust]\nname='dependency'\ntype='lib'\nedition='2024'\n",
        ),
        ("dependency/src/lib.rs", "pub const VALUE: u8 = 7;\n"),
        (
            "loaf.toml",
            "[project]\nname='bootstrap-consumer'\nversion='1.0.0'\n[dependencies]\ndependency={loaf='dependency',path='dependency'}\n",
        ),
        ("src/main.rs", "fn main() { print!(\"{}\", dependency::VALUE); }\n"),
        ("lock.json", r#"{"schema":"incan.oven.loaf-resolution/2","units":[]}"#),
        (
            "graph.json",
            r#"{"index_commit":"0000000000000000000000000000000000000000","registry_lock":"lock.json","facets":[{"project":"dependency","features":[],"domain":"target"}]}"#,
        ),
    ] {
        std::fs::write(root.join(path), content)?;
    }
    Ok(())
}
