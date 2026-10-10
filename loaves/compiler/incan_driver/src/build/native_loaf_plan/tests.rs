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

/// Source-current bootstrap plans share native generations while output relocation reuses the executable.
#[test]
fn declared_bootstrap_dependencies_share_command_store() -> TestResult {
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
