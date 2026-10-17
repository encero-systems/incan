//! Ordinary test envelopes retain exact path macros through the shared native runner.

use std::fs;
use std::path::PathBuf;

use super::{TestExecutionRequest, execute_tests_with_owner, list_test_config};
use crate::test_runner::output::TestOutputSink;

const CHILD_ROOT: &str = "INCAN_TEST_ENVELOPE_PATH_CHILD";
const SELECTOR: &str = "test_runner::tests::envelope_paths::ordinary_test_envelope_retains_declared_path_macro";

/// Run the real publisher and shared runner with a private host macro in an original native graph.
fn run_child() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Original native graph and a dependency-free Rust Loaf ----
    let root = tempfile::tempdir()?;
    let macros = root.path().join("derive_probe");
    fs::create_dir_all(macros.join("src"))?;
    fs::write(
        macros.join("loaf.toml"),
        "[project]\nname='derive_probe'\nversion='1.0.0'\n[rust]\nname='derive_probe'\ntype='proc-macro'\nedition='2024'\n",
    )?;
    fs::write(
        macros.join("src/lib.rs"),
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Generated)]\npub fn generated(_: TokenStream) -> TokenStream { TokenStream::new() }\n",
    )?;
    let original_graph = std::env::var_os("INCAN_SDK_NATIVE_COMPILER_GRAPH").ok_or("original native graph required")?;
    let mut graph: serde_json::Value = serde_json::from_slice(&fs::read(original_graph)?)?;
    graph["facets"]
        .as_array_mut()
        .ok_or("original facets missing")?
        .push(serde_json::json!({"project": macros, "domain": "host", "features": []}));
    let graph_path = root.path().join("graph.json");
    fs::write(&graph_path, serde_json::to_vec_pretty(&graph)?)?;

    // ---- Real compiler-owned layout and isolated child environment ----
    let compiler =
        PathBuf::from(std::env::var_os("CARGO_BIN_EXE_incan").ok_or("actual compiler required")?).canonicalize()?;
    let copied = tempfile::Builder::new()
        .prefix("incan-test-envelope-libtest-")
        .tempfile_in(compiler.parent().ok_or("compiler owner missing")?)?
        .into_temp_path();
    fs::copy(std::env::current_exe()?, &copied)?;
    let output = std::process::Command::new(copied.as_os_str())
        .args(["--exact", SELECTOR, "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT, root.path())
        .env("CARGO_BIN_EXE_incan", &compiler)
        .env("INCAN_SDK_NATIVE_COMPILER_GRAPH", &graph_path)
        .env("INCAN_OVEN_BAKE_PROFILES", "debug")
        .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
        .env_remove("INCAN_TEST_REQUIRE_COMPLETED_PROJECT_REUSE")
        .env_remove("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE")
        .output()?;
    if !output.status.success() {
        let project = root.path().join("consumer");
        let envelope = project.join("target/incan/oven/test-dependency-envelope/Cargo.toml");
        if envelope.is_file() {
            eprintln!("published test envelope:\n{}", fs::read_to_string(&envelope)?);
        }
        let batches = project.join("target/incan_tests");
        if batches.is_dir() {
            for entry in fs::read_dir(batches)? {
                let manifest = entry?.path().join("Cargo.toml");
                if manifest.is_file() {
                    eprintln!("requested test batch:\n{}", fs::read_to_string(manifest)?);
                }
            }
        }
    }
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// A quoted derive with no Rust item import uses the original test envelope rather than Cargo path rebuilding.
#[test]
#[ignore = "requires original native graph, ordinary standard family and compiler-owned source layout"]
fn ordinary_test_envelope_retains_declared_path_macro() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return run_child();
    };
    let root = PathBuf::from(root);
    let project = root.join("consumer");
    fs::create_dir_all(project.join("src"))?;
    fs::write(
        project.join("loaf.toml"),
        "[project]\nname='ordinary_test_macro'\nversion='0.1.0'\n[dependencies]\nderive_probe={loaf='derive_probe',path='../derive_probe',version='1.0.0'}\n",
    )?;
    fs::create_dir_all(project.join("tests"))?;
    fs::write(
        project.join("src/models.incn"),
        "@rust.derive(\"derive_probe::Generated\")\npub model Item:\n    pub value: int\n",
    )?;
    fs::write(
        project.join("src/main.incn"),
        "from models import Item\n\ndef main() -> None:\n    item = Item(value=42)\n    println(item.value)\n",
    )?;
    let test_file = project.join("tests/test_derived_item.incn");
    fs::write(
        &test_file,
        "from models import Item\n\ndef test_derived_item() -> None:\n    item = Item(value=42)\n    assert item.value == 42\n",
    )?;

    // ---- Actual publication and original envelope refusal ----
    let baked = incan_driver::build::bake::bake_oven_project_targets(&project, &Default::default(), None)?;
    assert_eq!(baked.profiles.len(), 1);
    let store = incan_driver::oven_store::open_default_oven_store()?;
    let authority =
        incan_driver::build::output_selection::load_current_project_registry_source_authorities(&store, &project)?
            .ok_or("published project has no current original inspection authority")?;
    assert!(authority.payload.native_roots.is_some());
    let prepared = incan_driver::lock::registry_sources::prepare_project_registry_source_authorities_with_native_sdk(
        authority, None,
    )?;
    let manifest = oven_model::manifest::ProjectManifest::load(&project.join("loaf.toml"))?;
    let mut incompatible = manifest.rust_dependency_values();
    prepared
        .test_dependency_plan(&incompatible)?
        .ok_or("unchanged original declaration list has no selected test envelope")?;
    let declared = incompatible
        .iter_mut()
        .find(|dependency| dependency.crate_name == "derive_probe")
        .ok_or("original macro declaration missing")?;
    declared.version = Some("99".to_string());
    let error = prepared
        .test_dependency_plan(&incompatible)
        .err()
        .ok_or("incompatible macro version entered original test envelope")?;
    assert!(
        error
            .message
            .contains("missing, stale, or incompatible test dependency root"),
        "{}",
        error.message
    );
    drop(prepared);

    // ---- Same production runner and owner for FIRST and repeat ----
    let mut config = list_test_config(test_file.to_str().ok_or("test path is not UTF-8")?);
    config.list_only = false;
    let request = TestExecutionRequest::from_config(&config);
    let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
    for label in ["first", "repeat"] {
        let report = execute_tests_with_owner(&request, &mut owner, TestOutputSink::default())?;
        assert_eq!(report.summary.total, 1, "{label}: {report:?}");
        assert_eq!(report.summary.passed, 1, "{label}: {report:?}");
        assert_eq!(report.summary.failed, 0, "{label}: {report:?}");
        assert_eq!(report.results[0].test.function_name, "test_derived_item");
        println!("ordinary-test-envelope-path {label}: 1/1 passed");
    }
    assert!(!root.join("derive_probe/Cargo.toml").exists());
    Ok(())
}
