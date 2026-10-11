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
