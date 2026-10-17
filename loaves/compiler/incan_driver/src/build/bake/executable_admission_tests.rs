//! Executable publishers retain original ordinary native authority across an SDK-bearing lock transition (#1337).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Output;

/// Invoke the actual compiler with both profiles, preserving the configured native graph and Store.
fn bake(project: &Path, target: &str, require_reuse: bool) -> std::io::Result<Output> {
    let mut command = incan_test_support::incan_command();
    command
        .args(["oven", "bake", "--project"])
        .arg(project)
        .args(["--target", target, "--format", "json"])
        .env("INCAN_OVEN_BAKE_PROFILES", "all")
        .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
        .env_remove("INCAN_TEST_REQUIRE_COMPLETED_PROJECT_REUSE")
        .env_remove("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE");
    if require_reuse {
        command.env("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE", "1");
    }
    command.output()
}

/// Require actual success before interpreting a compiler-owned report, retaining full failure diagnostics.
fn report(output: &Output) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// Both profiles and a script-only standard dependency survive the genuine old-lock migration and exact replay.
#[test]
#[ignore = "requires source-current compiler, ordinary standard publications and original debug/release native graph"]
fn executable_bake_migrates_sdk_lock_and_reuses_all_targets() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let project = root.path();
    fs::create_dir(project.join("src"))?;
    fs::write(
        project.join("loaf.toml"),
        "[project]\nname='ordinary_bake_handoff'\nversion='0.1.0'\n[project.scripts]\nextra='src/extra.incn'\n",
    )?;
    fs::write(project.join("src/main.incn"), "def main() -> None:\n    println(42)\n")?;
    fs::write(
        project.join("src/extra.incn"),
        "from std.math import gcd\ndef main() -> None:\n    println(gcd(42, 30))\n",
    )?;
    // A retained copy of the compiler-bootstrap lock as it was before SDK migration: genuine old state, not a forged
    // publication capability. The tracked lock itself cannot serve here, because any ordinary bake of that workspace
    // legitimately migrates it in place.
    let old_lock = include_bytes!("fixtures/sdk-migration.oven.lock").to_vec();
    let old: toml::Value = toml::from_str(std::str::from_utf8(&old_lock)?)?;
    assert!(old.get("semantic").and_then(|value| value.get("sdk")).is_some());
    fs::write(project.join("oven.lock"), &old_lock)?;
    let rustc = oven_rustc::rustc::resolve_active_rustc()?;
    let target = oven_rustc::rustc::rustc_host_target(&rustc)?;
    let toolchain = oven_rustc::rustc::rustc_identity(&rustc)?;
    let first = report(&bake(project, &target, false)?)?;
    let profiles = first["profiles"].as_array().ok_or("missing profile reports")?;
    assert_eq!(profiles.len(), 4);
    let mut coordinates = BTreeSet::new();
    for profile in profiles {
        assert_eq!(profile["target"], target);
        assert_eq!(profile["toolchain"], toolchain);
        coordinates.insert((
            profile["project_target"].as_str().ok_or("missing target")?,
            profile["profile"].as_str().ok_or("missing profile")?,
        ));
        let receipt: oven_store::OvenReceipt =
            serde_json::from_slice(&fs::read(profile["receipt"].as_str().ok_or("missing receipt path")?)?)?;
        receipt.verify_identity()?;
        assert_eq!(receipt.intent.target, target);
        assert_eq!(receipt.intent.toolchain, toolchain);
        assert_eq!(
            receipt.intent.profile,
            profile["profile"].as_str().ok_or("missing profile")?
        );
        assert_eq!(
            receipt.identity.as_str(),
            profile["receipt_identity"].as_str().ok_or("missing receipt identity")?
        );
    }
    assert_eq!(
        coordinates,
        BTreeSet::from([
            ("executable", "debug"),
            ("executable", "release"),
            ("executable:src/extra.incn", "debug"),
            ("executable:src/extra.incn", "release"),
        ])
    );
    assert_eq!(first["outputs"].as_array().ok_or("missing outputs")?.len(), 4);
    let lock_after = fs::read(project.join("oven.lock"))?;
    let migrated: toml::Value = toml::from_str(std::str::from_utf8(&lock_after)?)?;
    assert!(migrated.get("semantic").and_then(|value| value.get("sdk")).is_none());
    let binary = project.join("target/incan/ordinary_bake_handoff/oven/release/ordinary_bake_handoff");
    let bytes = fs::read(&binary)?;
    let modified = fs::metadata(&binary)?.modified()?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&fs::metadata(&binary)?);
    let run = std::process::Command::new(&binary).output()?;
    assert!(run.status.success());
    assert_eq!(run.stdout, b"42\n");
    let script_root = crate::build::oven_bake_executable_output_dir(project, &project.join("src/extra.incn"))?
        .ok_or("extra script lacks separate output root")?;
    let script = script_root.join("oven/release/ordinary_bake_handoff");
    let script_bytes = fs::read(&script)?;
    let run = std::process::Command::new(&script).output()?;
    assert!(run.status.success());
    assert_eq!(run.stdout, b"6\n");
    let repeated = report(&bake(project, &target, true)?)?;
    assert_eq!(first["outputs"], repeated["outputs"]);
    assert_eq!(fs::read(project.join("oven.lock"))?, lock_after);
    assert_eq!(fs::read(&binary)?, bytes);
    assert_eq!(fs::read(&script)?, script_bytes);
    assert_eq!(fs::metadata(&binary)?.modified()?, modified);
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::MetadataExt::ino(&fs::metadata(&binary)?), inode);
    // An impossible target cannot rewrite the migrated lock or masquerade as host-target reuse.
    let refused = bake(project, "incan-invalid-target", false)?;
    assert!(!refused.status.success());
    let refusal = String::from_utf8_lossy(&refused.stderr);
    assert!(
        refusal.contains("local native preparation currently requires the explicit compiler host target"),
        "{refusal}"
    );
    assert_eq!(fs::read(project.join("oven.lock"))?, lock_after);
    fs::write(project.join("src/main.incn"), "def main() -> None:\n    println(43)\n")?;
    let refused = bake(project, &target, true)?;
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("completed project reuse missed before frontend preparation")
    );
    let changed = report(&bake(project, &target, false)?)?;
    assert_ne!(first["outputs"], changed["outputs"]);
    let run = std::process::Command::new(binary).output()?;
    assert!(run.status.success());
    assert_eq!(run.stdout, b"43\n");
    Ok(())
}

const DERIVE_CHILD: &str = "INCAN_EXECUTABLE_DERIVE_TEST_ROOT";
const DERIVE_SELECTOR: &str =
    "build::bake::executable_admission_tests::executable_publication_retains_declared_quoted_derive";

/// Execute the original publisher inside its real compiler layout with one declared local host macro added to the
/// original graph.
fn derive_child() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Real native family with a private, dependency-free host macro ----
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
        .push(serde_json::json!({
            "project":macros, "domain":"host", "features":[]
        }));
    let graph_path = root.path().join("graph.json");
    fs::write(&graph_path, serde_json::to_vec_pretty(&graph)?)?;

    // ---- A copied libtest retains compiler-owned source discovery without ambient overrides ----
    let compiler = std::path::PathBuf::from(std::env::var_os("CARGO_BIN_EXE_incan").ok_or("actual compiler required")?)
        .canonicalize()?;
    let copied = tempfile::Builder::new()
        .prefix("incan-executable-derive-libtest-")
        .tempfile_in(compiler.parent().ok_or("compiler owner missing")?)?
        .into_temp_path();
    fs::copy(std::env::current_exe()?, &copied)?;
    let output = std::process::Command::new(copied.as_os_str())
        .args([
            "--exact",
            DERIVE_SELECTOR,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(DERIVE_CHILD, root.path())
        .env("CARGO_BIN_EXE_incan", &compiler)
        .env("INCAN_SDK_NATIVE_COMPILER_GRAPH", &graph_path)
        .env("INCAN_OVEN_BAKE_PROFILES", "debug")
        .env_remove("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE")
        .env_remove("INCAN_TEST_REQUIRE_COMPLETED_PROJECT_REUSE")
        .env_remove("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE")
        .output()?;
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

/// The executable publisher must preserve a declared derive without an import, while an undeclared quote grants no
/// macro authority.
#[test]
#[ignore = "requires original native graph, prepared ordinary family and compiler-owned source layout"]
fn executable_publication_retains_declared_quoted_derive() -> Result<(), Box<dyn std::error::Error>> {
    let Some(root) = std::env::var_os(DERIVE_CHILD) else {
        return derive_child();
    };
    let root = std::path::PathBuf::from(root);
    let project = root.join("consumer");
    fs::create_dir_all(project.join("src"))?;
    let declaration = "[project]\nname='ordinary_derive_handoff'\nversion='0.1.0'\n[dependencies]\nderive_probe={loaf='derive_probe',path='../derive_probe',version='1.0.0'}\n";
    fs::write(project.join("loaf.toml"), declaration)?;
    let source = "@rust.derive(\"derive_probe::Generated\")\nmodel Item:\n    value: int\n\ndef main() -> None:\n    item = Item(value=42)\n    println(item.value)\n";
    fs::write(project.join("src/main.incn"), source)?;

    // ---- The actual source has derive demand and no item query that could mask the missing handoff ----
    let module = incan_provider::test_support::parsed_module_for_test(source)?;
    assert!(crate::rust_inspect_workspace::collect_rust_inspect_query_paths(std::slice::from_ref(&module)).is_empty());
    assert_eq!(
        crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths(&[module]),
        ["derive_probe::Generated"]
    );
    let bake = || super::bake_oven_project_targets(&project, &Default::default(), None);
    let report = bake()?;
    assert_eq!(report.profiles.len(), 1);
    assert_eq!(report.profiles[0].profile, "debug");
    let generated = project.join("target/incan/ordinary_derive_handoff/src/main.rs");
    let generated = fs::read_to_string(&generated)?;
    assert!(
        generated
            .split_whitespace()
            .collect::<String>()
            .contains("derive_probe::Generated"),
        "{generated}"
    );
    let binary = project.join("target/incan/ordinary_derive_handoff/oven/debug/ordinary_derive_handoff");
    let run = std::process::Command::new(&binary).output()?;
    assert!(run.status.success());
    assert_eq!(run.stdout, b"42\n");

    // ---- Presence in the original native graph does not authorize an undeclared macro alias ----
    fs::write(
        project.join("loaf.toml"),
        "[project]\nname='ordinary_derive_handoff'\nversion='0.1.0'\n",
    )?;
    let error = bake().err().ok_or("undeclared derive unexpectedly published")?;
    let message = error.to_string();
    assert!(
        message.contains("ordinary loaded source lacks complete native import or own-namespace coverage")
            && message.contains("derive_probe::Generated"),
        "{message}"
    );
    fs::write(project.join("loaf.toml"), declaration)?;
    let restored = bake()?;
    assert_eq!(restored.profiles.len(), 1);
    let run = std::process::Command::new(binary).output()?;
    assert!(run.status.success());
    assert_eq!(run.stdout, b"42\n");
    Ok(())
}
