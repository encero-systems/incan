//! Normal command precedence keeps the compiler-suite receipt probe distinct from SDK publication.

use std::{env, fs};

use crate::build::plan_selection::select_or_bake_generated_project_plan_with_native_sdk;
use crate::build::{NativeSdkCommandContext, OvenProjectDependencySurface, OvenProjectPlanMode};
use oven_interop::OVEN_INTEROP_EXECUTION_RECEIPT_INPUT;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_rustc::loaf::{OVEN_NESTED_DEPENDENCY_MISS_SUMMARY, OVEN_NO_IMPLICIT_DEPENDENCY_BUILD};
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::store::{OvenStore, OvenStoreLimits};
use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project};

const CHILD_CASE: &str = "INCAN_PLAN_PRECEDENCE_TEST_CASE";
const SELECTOR: &str =
    "build::plan_selection::tests::consumer_precedence::consume_only_retains_canonical_compiler_suite_precedence";

/// The real admitted SDK cannot satisfy the foundation's bitflags 1 pin; the private receipt probe must still reach its
/// expected miss.
#[test]
fn consume_only_retains_canonical_compiler_suite_precedence() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Isolated command environments retain the actual SDK publication ----
    let Some(case) = env::var_os(CHILD_CASE) else {
        let inventory = env::var_os("INCAN_SDK_INVENTORY").ok_or("a genuine prepared SDK inventory is required")?;
        for case in ["probe", "public", "publisher", "missing-root", "interop"] {
            let root = tempfile::tempdir()?;
            let data = root.path().join("compiler-data");
            fs::create_dir_all(data.join("share/incan/oven/loafs"))?;
            let mut command = std::process::Command::new(env::current_exe()?);
            command
                .args(["--exact", SELECTOR, "--nocapture", "--test-threads=1"])
                .env(CHILD_CASE, case)
                .env("INCAN_SDK_INVENTORY", &inventory)
                .env("INCAN_HOME", root.path().join("home"))
                .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", &data)
                .env_remove("INCAN_SDK_NATIVE_COMPILER_GRAPH")
                .env_remove("INCAN_INTERNAL_OVEN_LOAF_EXECUTION");
            if case != "public" {
                command.env("INCAN_INTERNAL_OVEN_LOAF_EXECUTION", "1");
            }
            if case == "missing-root" {
                command.env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", root.path().join("absent"));
            }
            let output = command.output()?;
            assert!(
                output.status.success(),
                "{case}: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
        return Ok(());
    };
    let case = case.to_str().ok_or("invalid child case")?;

    // ---- Generated receipt and original admitted SDK match the production handoff ----
    let context = NativeSdkCommandContext::discover()?.ok_or("a native SDK publication is required")?;
    let project = tempfile::tempdir()?;
    let generated = project.path().join("src/main.rs");
    fs::create_dir_all(generated.parent().ok_or("missing source parent")?)?;
    fs::write(&generated, "fn main() {}\n")?;
    let rustc = resolve_active_rustc()?;
    let mut request = OvenGeneratedProjectRequest::new(
        project.path(),
        "compiler_suite_precedence",
        "0.1.0",
        rustc_host_target(&rustc)?,
        rustc_identity(&rustc)?,
        "debug",
        Vec::new(),
    )
    .with_generated_source("generated-root", &generated)
    .with_build_unit_input("provider-plan", digest_bytes(b""));
    if case == "interop" {
        request = request.with_build_unit_input(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, digest_bytes(b"final interop"));
    }
    let receipt = receipt_generated_project(&request)?;
    let receipt_path = project.path().join("receipt.json");
    let receipt_bytes = serde_json::to_vec(&receipt)?;
    fs::write(&receipt_path, &receipt_bytes)?;
    let store = OvenStore::new(
        project.path().join("store"),
        OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
    );
    let dependencies = [DependencySpec {
        crate_name: "bitflags".to_string(),
        version: Some("=1.3.2".to_string()),
        features: Vec::new(),
        default_features: true,
        source: DependencySource::Registry,
        optional: false,
        package: None,
    }];
    let mode = if case == "publisher" {
        OvenProjectPlanMode::ExplicitBake
    } else {
        OvenProjectPlanMode::ConsumeOnly
    };
    let result = select_or_bake_generated_project_plan_with_native_sdk(
        mode,
        &store,
        &receipt,
        OvenProjectDependencySurface {
            selection: &dependencies,
            provider_compilations: &[],
        },
        project.path(),
        &generated,
        &rustc,
        Some(&context),
    );

    // ---- No route manufactures execution authority or compatibility publisher state ----
    let Err(error) = result else {
        return Err(format!("{case} unexpectedly selected or published a plan").into());
    };
    let message = error.to_string();
    match case {
        "probe" => {
            assert!(message.contains(OVEN_NESTED_DEPENDENCY_MISS_SUMMARY), "{message}");
            assert!(message.contains(OVEN_NO_IMPLICIT_DEPENDENCY_BUILD), "{message}");
            assert!(message.contains("bitflags"), "{message}");
        }
        "public" | "publisher" => assert!(
            message.contains("native SDK plan does not cover dependency `bitflags`"),
            "{message}"
        ),
        "missing-root" => assert!(
            message.contains("requires a readable immutable toolchain-data root"),
            "{message}"
        ),
        "interop" => assert!(
            message.contains("no matching final native direct-Rustc plan"),
            "{message}"
        ),
        _ => return Err(format!("unknown case {case}").into()),
    }
    assert_eq!(fs::read(&receipt_path)?, receipt_bytes);
    assert_eq!(fs::read_to_string(&generated)?, "fn main() {}\n");
    assert!(!project.path().join("Cargo.lock").exists());
    assert!(!project.path().join("target").exists());
    Ok(())
}
