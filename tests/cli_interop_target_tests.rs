//! Declared C ABI interop targets: verified bindings, platform requirements, and the sealed interop plan.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `tests/support/cli_project.rs`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

mod support;

#[path = "support/cli_project.rs"]
mod cli_project;

use cli_project::*;

#[test]
#[cfg_attr(
    not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )),
    ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
)]
fn codegraph_projects_checked_c_bindings_and_explicit_unsafe_calls() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    let header_path = tmp.path().join("fixture.h");
    fs::write(
        &header_path,
        concat!(
            "typedef struct fixture_handle fixture_handle;\n",
            "#define FIXTURE_OK 0\n",
            "void fixture_close(fixture_handle *handle);\n",
            "int fixture_open(fixture_handle **output, int *attempts);\n",
            "unsigned int fixture_random(unsigned int *seed);\n",
        ),
    )?;
    fs::write(
        &source_path,
        format!(
            r#"from std.interop import c

binding Fixture:
    header = "{}"
    link = c.system_library("c")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]], attempts: c.InOut[c.i32]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
            updates = [attempts]

    symbol random(seed: c.InOut[c.u32]) -> c.u32:
        native = "fixture_random"

def inspect_contract() -> None:
    unsafe:
        handle = c.out[c.Owned[Handle]]()
        attempts_value: i32 = 0
        attempts = c.inout(attempts_value)
        status = Fixture.open(handle, attempts)
        if status == Fixture.Status.OK:
            resource = handle.take()
            Fixture.close(resource)

        seed_value: u32 = 7
        seed = c.inout(seed_value)
        Fixture.random(seed)
        seed.take()

pub def public_facade() -> None:
    inspect_contract()
"#,
            header_path.display()
        ),
    )?;

    let source = source_path.to_str().ok_or("source path was not valid UTF-8")?;
    let first = run_incan(tmp.path(), &["inspect", "codegraph", source, "--format", "jsonl"])?;
    assert_success(&first, "codegraph checked C binding projection");
    let second = run_incan(tmp.path(), &["inspect", "codegraph", source, "--format", "jsonl"])?;
    assert_success(&second, "second codegraph checked C binding projection");
    assert_eq!(
        first.stdout, second.stdout,
        "checked C codegraph records must be deterministic"
    );

    let records = parse_jsonl_stdout(&first)?;
    assert_codegraph_record_contract(&records);
    let binding = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("c_binding"))
        .ok_or("codegraph did not emit the checked C binding record")?;
    assert_eq!(binding["name"], serde_json::json!("Fixture"));
    assert_eq!(binding["header"], serde_json::json!(header_path.to_string_lossy()));
    assert_eq!(binding["system_library"], serde_json::json!("c"));
    assert!(
        binding["binding_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "checked C codegraph record must publish the portable descriptor identity: {binding}"
    );
    assert_eq!(binding["provenance"], serde_json::json!("checked"));
    let declaration_id = binding["declaration_id"]
        .as_str()
        .ok_or("checked C binding did not link to its class declaration record")?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["id"] == serde_json::json!(declaration_id)
            && record["name"] == serde_json::json!("Fixture")
    }));
    assert_eq!(binding["resources"][0]["name"], serde_json::json!("Handle"));
    assert_eq!(binding["resources"][0]["release"], serde_json::json!("close"));
    assert_eq!(
        binding["symbols"][1]["parameters"][0]["type"]["kind"],
        serde_json::json!("output")
    );
    assert_eq!(
        binding["symbols"][1]["parameters"][0]["type"]["mode"],
        serde_json::json!("out")
    );
    assert_eq!(
        binding["symbols"][1]["parameters"][1]["type"]["mode"],
        serde_json::json!("in_out")
    );
    assert_eq!(
        binding["symbols"][1]["outcomes"][0]["initializes"],
        serde_json::json!(["output"])
    );
    assert_eq!(binding["enums"][0]["carrier"], serde_json::json!("c.i32"));

    let raw_call = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("c_binding_call")
                && record["binding"] == serde_json::json!("Fixture")
                && record["symbol"] == serde_json::json!("open")
        })
        .ok_or("codegraph did not emit the checked raw C call record")?;
    assert_eq!(raw_call["binding_id"], binding["id"]);
    assert_eq!(raw_call["binding_identity"], binding["binding_identity"]);
    assert_eq!(raw_call["owner_visibility"], serde_json::json!("private"));
    let owner_declaration_id = raw_call["owner_declaration_id"]
        .as_str()
        .ok_or("checked raw C call did not retain its compiler-known owning bridge")?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["id"] == serde_json::json!(owner_declaration_id)
            && record["name"] == serde_json::json!("inspect_contract")
    }));
    let facade_declaration_id = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration")
                && record["kind"] == serde_json::json!("function")
                && record["name"] == serde_json::json!("public_facade")
                && record["visibility"] == serde_json::json!("public")
        })
        .and_then(|record| record["id"].as_str())
        .ok_or("codegraph did not emit the public C-ABI facade declaration")?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("inspect_contract")
            && record["owner_id"] == serde_json::json!(facade_declaration_id)
            && record["target_id"] == serde_json::json!(owner_declaration_id)
            && record["provenance"] == serde_json::json!("checked")
    }));
    let facade = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("c_binding_facade"))
        .ok_or("codegraph did not emit the compiler-proven C-ABI facade relation")?;
    assert_eq!(
        facade["facade_declaration_id"],
        serde_json::json!(facade_declaration_id)
    );
    assert_eq!(facade["bridge_declaration_id"], serde_json::json!(owner_declaration_id));
    assert_eq!(facade["provenance"], serde_json::json!("checked"));
    assert_eq!(facade["degraded"], serde_json::json!(false));
    assert!(
        facade["call_id"].is_string(),
        "facade relation must link to its compiler-proven ordinary call: {facade}"
    );
    assert!(
        facade["raw_call_ids"].as_array().is_some_and(|ids| !ids.is_empty()),
        "facade relation must link the private bridge to its direct raw calls: {facade}"
    );
    assert_eq!(raw_call["unsafe_acknowledged"], serde_json::json!(true));
    assert_eq!(raw_call["provenance"], serde_json::json!("checked"));
    let call_id = raw_call["call_id"]
        .as_str()
        .ok_or("checked raw C call did not link to its generic call record")?;
    assert!(
        records.iter().any(|record| {
            record["record"] == serde_json::json!("call") && record["id"] == serde_json::json!(call_id)
        })
    );

    let raw_call_count = records
        .iter()
        .filter(|record| record["record"] == serde_json::json!("c_binding_call"))
        .count();
    assert_eq!(
        raw_call_count, 3,
        "only direct native calls should receive C binding call records"
    );

    Ok(())
}

#[test]
fn lock_records_oven_interop_requirements_and_detects_input_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "oven_interop_lock",
        r#"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "x86_64-unknown-linux-gnu"
toolchain = { capability = "clang", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
definitions = ["FIXTURE=1"]

[[interop.c.targets.artifacts]]
name = "fixture"
kind = "static"
path = "interop/lib/libfixture.a"

[[interop.c.targets.shims]]
name = "fixture_bridge"
language = "c"
sources = ["interop/src/bridge.c"]
headers = ["interop/include/bridge.h"]
output = "fixture_bridge"
"#,
    )?;
    fs::create_dir_all(tmp.path().join("interop/include"))?;
    fs::create_dir_all(tmp.path().join("interop/src"))?;
    fs::create_dir_all(tmp.path().join("interop/lib"))?;
    fs::write(tmp.path().join("interop/include/bridge.h"), "int bridge(void);\n")?;
    fs::write(
        tmp.path().join("interop/src/bridge.c"),
        "int bridge(void) { return 7; }\n",
    )?;
    fs::write(tmp.path().join("interop/lib/libfixture.a"), b"fixture archive")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let incan_home = tmp.path().join(".incan-home");
    let incan_home = incan_home.to_str().ok_or("Incan home was not valid UTF-8")?;

    let lock_output = run_incan_with_env(tmp.path(), &["lock", main_arg], &[("INCAN_HOME", incan_home)])?;
    assert_success(&lock_output, "incan lock with declared Oven interop requirements");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("oven.lock"))?)?;
    let target = lock["semantic"]["oven"]["interop"]
        .as_array()
        .and_then(|targets| targets.first())
        .ok_or("lock did not contain Oven interop requirements")?;
    assert_eq!(target["target"].as_str(), Some("x86_64-unknown-linux-gnu"));
    assert_eq!(target["toolchain"]["capability"].as_str(), Some("clang"));
    assert_eq!(target["toolchain"]["version"].as_str(), Some(">=18, <19"));
    assert_eq!(
        target["headers"]
            .as_array()
            .and_then(|headers| headers.first())
            .and_then(|header| header["path"].as_str()),
        Some("interop/include/bridge.h")
    );
    assert_eq!(
        target["shims"]
            .as_array()
            .and_then(|shims| shims.first())
            .and_then(|shim| shim["sources"].as_array())
            .and_then(|sources| sources.first())
            .and_then(|source| source["path"].as_str()),
        Some("interop/src/bridge.c")
    );

    fs::write(
        tmp.path().join("interop/src/bridge.c"),
        "int bridge(void) { return 8; }\n",
    )?;
    let stale = run_incan_with_env(
        tmp.path(),
        &["build", main_arg, "--locked"],
        &[("INCAN_HOME", incan_home)],
    )?;
    assert_failure(&stale, "locked build after declared interop input drift");
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("oven.lock is out of date"),
        "declared interop input drift should invalidate the lock:\n{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    Ok(())
}

#[test]
fn lock_records_android_platform_requirements_without_selecting_a_local_sdk() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "oven_android_platform_lock",
        r#"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-linux-android"
toolchain = { capability = "android-ndk", version = ">=29, <30" }
sdk = { capability = "android", version = ">=36, <37" }

[interop.c.targets.platform]
kind = "android"
api-level = 34
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let lock_output = run_incan(tmp.path(), &["lock", main_arg])?;
    assert_success(&lock_output, "incan lock with Android platform requirements");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("oven.lock"))?)?;
    let target = lock["semantic"]["oven"]["interop"]
        .as_array()
        .and_then(|targets| targets.first())
        .ok_or("lock did not contain an Android Oven interop target")?;
    assert_eq!(target["target"].as_str(), Some("aarch64-linux-android"));
    assert_eq!(target["sdk"]["capability"].as_str(), Some("android"));
    assert_eq!(target["platform"]["kind"].as_str(), Some("android"));
    assert_eq!(target["platform"]["api-level"].as_integer(), Some(34));
    Ok(())
}

#[test]
fn inspect_interop_plan_is_locked_complete_and_relocatable() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Declare representative Android deployment requirements ----
    let tmp = tempfile::tempdir()?;
    let package = tmp.path().join("package");
    fs::create_dir_all(&package)?;
    let _main_path = write_minimal_project(
        &package,
        "interop_plan_handoff",
        r#"

[sdk]
profile = "minimal"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-linux-android"
toolchain = { capability = "android-ndk", version = ">=29, <30" }
sdk = { capability = "android", version = ">=36, <37" }
headers = ["interop/include/runtime.h"]
definitions = ["TFLITE_STATIC_MEMORY=1"]

[interop.c.targets.platform]
kind = "android"
api-level = 34

[[interop.c.targets.artifacts]]
name = "llama"
kind = "static"
path = "interop/lib/libllama.a"
dependencies = ["tflite"]

[[interop.c.targets.artifacts]]
name = "tflite"
kind = "bundled"
path = "interop/lib/libtensorflowlite_c.so"
runtime-name = "libtensorflowlite_c.so"
placement = "jniLibs/arm64-v8a"
minimum-platform = "21"
dependencies = ["log"]

[[interop.c.targets.artifacts]]
name = "log"
kind = "system"
capability = "android.library.log"

[[interop.c.targets.bindings]]
module = ["runtime"]
name = "Runtime"
artifacts = ["llama", "tflite", "log"]

[[interop.c.targets.shims]]
name = "llama_bridge"
language = "cxx"
sources = ["interop/src/llama_bridge.cc"]
headers = ["interop/include/runtime.h"]
output = "llama_bridge"
"#,
    )?;
    fs::create_dir_all(package.join("interop/include"))?;
    fs::create_dir_all(package.join("interop/src"))?;
    fs::create_dir_all(package.join("interop/lib"))?;
    fs::write(package.join("interop/include/runtime.h"), "int runtime(void);\n")?;
    fs::write(
        package.join("interop/src/llama_bridge.cc"),
        "extern \"C\" int runtime(void) { return 0; }\n",
    )?;
    fs::write(package.join("interop/lib/libllama.a"), b"llama archive")?;
    fs::write(
        package.join("interop/lib/libtensorflowlite_c.so"),
        b"tflite shared object",
    )?;
    // ---- Lock and inspect the complete structured requirement handoff ----
    write_locked_oven_interop_plan(&package)?;
    let output = run_incan(
        &package,
        &[
            "inspect",
            "interop-plan",
            "--target",
            "aarch64-linux-android",
            "--format",
            "json",
            ".",
        ],
    )?;
    assert_success(&output, "locked Android interop plan inspection");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(plan["schema_version"].as_u64(), Some(3));
    assert!(
        plan["locked_target_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "interop deployment plan must retain the portable locked-target join identity: {plan}"
    );
    assert_eq!(plan["target"].as_str(), Some("aarch64-linux-android"));
    assert_eq!(plan["toolchain"]["capability"].as_str(), Some("android-ndk"));
    assert_eq!(plan["sdk"]["capability"].as_str(), Some("android"));
    assert_eq!(plan["platform"]["kind"].as_str(), Some("android"));
    assert_eq!(plan["platform"]["api_level"].as_u64(), Some(34));
    assert_eq!(plan["include_roots"][0].as_str(), Some("interop/include"));
    assert_eq!(
        plan["artifacts"]
            .as_array()
            .ok_or("interop plan artifacts were not an array")?
            .iter()
            .filter_map(|artifact| artifact["name"].as_str())
            .collect::<Vec<_>>(),
        ["log", "tflite", "llama"]
    );
    assert_eq!(plan["artifacts"][0]["deployment"].as_str(), Some("system"));
    assert_eq!(plan["artifacts"][0]["capability"].as_str(), Some("android.library.log"));
    assert_eq!(plan["artifacts"][1]["deployment"].as_str(), Some("bundle"));
    assert_eq!(plan["artifacts"][1]["placement"].as_str(), Some("jniLibs/arm64-v8a"));
    assert_eq!(plan["artifacts"][2]["deployment"].as_str(), Some("static_link"));
    assert_eq!(plan["bindings"][0]["module"], serde_json::json!(["runtime"]));
    assert_eq!(plan["bindings"][0]["name"], serde_json::json!("Runtime"));
    assert_eq!(
        plan["bindings"][0]["artifacts"],
        serde_json::json!(["llama", "log", "tflite"])
    );
    assert_eq!(plan["shims"][0]["output"].as_str(), Some("llama_bridge"));
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains(&package.to_string_lossy().to_string()),
        "interop plan leaked its original package location"
    );

    let unknown = run_incan(
        &package,
        &["inspect", "interop-plan", "--target", "aarch64-apple-ios", "."],
    )?;
    assert_failure(&unknown, "undeclared interop plan target");
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("is not declared and locked by this package"),
        "unexpected undeclared interop-plan diagnostic:\n{}",
        String::from_utf8_lossy(&unknown.stderr)
    );

    // ---- Preserve the plan across relocation and reject stale locked bytes ----
    let relocated = tmp.path().join("relocated");
    fs::rename(&package, &relocated)?;
    let relocated_output = run_incan(
        &relocated,
        &[
            "inspect",
            "interop-plan",
            "--target",
            "aarch64-linux-android",
            "--format",
            "json",
            ".",
        ],
    )?;
    assert_success(&relocated_output, "relocated interop plan inspection");
    assert_eq!(
        output.stdout, relocated_output.stdout,
        "relocating a locked interop package changed its deployment handoff"
    );

    fs::write(
        relocated.join("interop/lib/libtensorflowlite_c.so"),
        b"changed tflite shared object",
    )?;
    let stale = run_incan(
        &relocated,
        &["inspect", "interop-plan", "--target", "aarch64-linux-android", "."],
    )?;
    assert_failure(&stale, "stale interop plan inspection");
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("oven.lock Oven interop requirements are out of date"),
        "unexpected stale interop-plan diagnostic:\n{}",
        String::from_utf8_lossy(&stale.stderr)
    );
    Ok(())
}

#[test]
fn inspect_interop_plan_uses_the_selected_workspace_member_lock_projection() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Declare one Oven interop workspace member ----
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("loaf.toml"),
        "[workspace]\nmembers = [\"packages/mobile\"]\n",
    )?;
    let member = root.path().join("packages/mobile");
    let _main_path = write_minimal_project(
        &member,
        "mobile",
        r#"

[sdk]
profile = "minimal"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
headers = ["interop/include/accelerate_bridge.h"]

[interop.c.targets.platform]
kind = "ios"
deployment-target = "13.0"

[[interop.c.targets.artifacts]]
name = "accelerate"
kind = "system"
capability = "apple.framework.Accelerate"
"#,
    )?;
    fs::create_dir_all(member.join("interop/include"))?;
    fs::write(
        member.join("interop/include/accelerate_bridge.h"),
        "float incan_dot(const float *left, const float *right, unsigned long count);\n",
    )?;
    // ---- Publish the one canonical workspace lock ----
    write_locked_workspace_oven_interop_plan(root.path(), &member)?;
    assert!(
        root.path().join("oven.lock").is_file() && !member.join("oven.lock").exists(),
        "interop workspace fixture did not publish exactly one canonical root lock"
    );

    // ---- Inspect the selected member through its root-lock projection ----
    let output = run_incan(
        root.path(),
        &[
            "inspect",
            "interop-plan",
            "packages/mobile",
            "--target",
            "aarch64-apple-ios",
            "--format",
            "json",
        ],
    )?;
    assert_success(&output, "workspace member interop plan inspection");
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(plan["target"].as_str(), Some("aarch64-apple-ios"));
    assert_eq!(plan["platform"]["kind"].as_str(), Some("ios"));
    assert_eq!(plan["artifacts"][0]["deployment"].as_str(), Some("system"));
    assert_eq!(
        plan["artifacts"][0]["capability"].as_str(),
        Some("apple.framework.Accelerate")
    );
    Ok(())
}

#[test]
fn check_verifies_c_bindings_against_a_declared_android_interop_target() -> Result<(), Box<dyn std::error::Error>> {
    let Some(clang) = c_abi_test_clang() else {
        return Ok(());
    };
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "declared_android_c_abi_check",
        r#"

[sdk]
profile = "minimal"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-linux-android"
toolchain = { capability = "android-ndk", version = ">=29, <30" }
sdk = { capability = "android", version = ">=36, <37" }
definitions = ["INCAN_ANDROID_FIXTURE=1"]

[interop.c.targets.platform]
kind = "android"
api-level = 34
"#,
    )?;
    let header = tmp.path().join("android_fixture.h");
    fs::write(
        &header,
        "#ifndef INCAN_ANDROID_FIXTURE\n#error expected Android target definition\n#endif\ntypedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_abs(int value);\n",
    )?;
    fs::write(
        &main_path,
        format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n        right: c.i32 = right\n\ndef main() -> None:\n    assert Fixture.Status.OK == 0\n",
            header.display()
        ),
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let output = run_incan_with_env(
        tmp.path(),
        &["check", "--interop-target", "aarch64-linux-android", main_arg],
        &[("INCAN_C_ABI_CLANG", clang.as_str())],
    )?;
    assert_success(&output, "declared Android C ABI verification");
    Ok(())
}

#[test]
fn check_rejects_an_undeclared_interop_target() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "undeclared_interop_target", "")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let output = run_incan(
        tmp.path(),
        &["check", "--interop-target", "aarch64-linux-android", main_arg],
    )?;
    assert_failure(&output, "undeclared Oven interop target selection");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("requires an [interop.c] declaration in loaf.toml"),
        "unexpected undeclared-target diagnostic:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// The only automatic Cargo use in the interop route is the explicit Oven bootstrap. Once Oven has sealed the
/// selected native plan, a locked runtime invocation must consume that receipt and never rediscover Cargo.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn oven_interop_bake_bootstraps_direct_c_then_locked_run_uses_the_sealed_plan() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "direct_c_interop_bootstrap",
        r#"

[sdk]
profile = "minimal"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-darwin"
headers = ["interop/include/fixture.h"]

[[interop.c.targets.artifacts]]
name = "fixture"
kind = "static"
path = "interop/lib/libfixture.a"

[[interop.c.targets.bindings]]
module = ["fixture"]
name = "Fixture"
artifacts = ["fixture"]
"#,
    )?;
    let include_dir = tmp.path().join("interop/include");
    let library_dir = tmp.path().join("interop/lib");
    fs::create_dir_all(&include_dir)?;
    fs::create_dir_all(&library_dir)?;
    fs::write(include_dir.join("fixture.h"), "int fixture_value(void);\n")?;
    let fixture_source = tmp.path().join("interop/fixture.c");
    fs::write(&fixture_source, "int fixture_value(void) { return 42; }\n")?;
    let clang = c_abi_test_clang().ok_or("the macOS direct-C fixture requires clang")?;
    let native_object = library_dir.join("fixture.o");
    let native_compile = Command::new(clang)
        .args(["-c", "-o"])
        .arg(&native_object)
        .arg(&fixture_source)
        .output()?;
    assert_success(&native_compile, "direct-C fixture object compilation");
    let native_archive = library_dir.join("libfixture.a");
    let native_archive_build = Command::new("ar")
        .args(["rcs"])
        .arg(&native_archive)
        .arg(&native_object)
        .output()?;
    assert_success(&native_archive_build, "direct-C fixture static archive creation");
    fs::write(
        tmp.path().join("src/fixture.incn"),
        r#"from std.interop import c


binding Fixture:
    header = "interop/include/fixture.h"
    link = c.system_library("fixture")

    symbol value() -> c.i32:
        native = "fixture_value"


pub def native_value() -> int:
    unsafe:
        return Fixture.value()
"#,
    )?;
    fs::write(
        &main_path,
        r#"from fixture import native_value


def main() -> None:
    println(native_value())
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let sdk_inventory = std::env::var_os("INCAN_SDK_INVENTORY")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            fs::read_dir(support::sdk_provider_store()).ok()?.find_map(|entry| {
                let inventory = entry.ok()?.path().join("sdk-inventory.json");
                inventory.is_file().then_some(inventory)
            })
        })
        .ok_or("direct-C interop regression requires the sealed SDK inventory from test prewarm")?;
    let sdk_inventory_text = sdk_inventory
        .to_str()
        .ok_or("sealed SDK inventory path was not valid UTF-8")?;

    let lock = run_incan_with_env(
        tmp.path(),
        &["lock", main_arg],
        &[("INCAN_SDK_INVENTORY", sdk_inventory_text)],
    )?;
    assert_success(&lock, "direct-C interop lock");
    let bake = run_incan_with_env(
        tmp.path(),
        &[
            "oven",
            "interop",
            "bake",
            "--project",
            ".",
            "--target",
            "aarch64-apple-darwin",
        ],
        &[("INCAN_SDK_INVENTORY", sdk_inventory_text)],
    )?;
    assert_success(&bake, "automatic direct-C interop bootstrap and bake");
    assert!(
        String::from_utf8_lossy(&bake.stdout).contains("named compatibility publisher"),
        "automatic interop bake did not disclose its bounded bootstrap:\n{}",
        String::from_utf8_lossy(&bake.stdout)
    );

    let cargo_marker = tmp.path().join("cargo-was-started");
    let guarded_run = run_incan_with_failing_cargo_guard_and_env(
        tmp.path(),
        &["run", "--locked", main_arg],
        &tmp.path().join("cargo-guard"),
        &cargo_marker,
        &[("INCAN_SDK_INVENTORY", sdk_inventory.as_path())],
    )?;
    assert_success(&guarded_run, "locked direct-C runtime after interop bake");
    assert_eq!(String::from_utf8_lossy(&guarded_run.stdout).trim(), "42");
    assert!(
        !cargo_marker.exists(),
        "locked direct-C runtime started Cargo after Oven sealed the native plan"
    );
    Ok(())
}

#[cfg(target_os = "macos")]
#[test]
fn check_verifies_c_bindings_against_a_declared_ios_interop_target() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "declared_ios_c_abi_check",
        r#"

[sdk]
profile = "minimal"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-ios"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "iphoneos", version = ">=18, <19" }
definitions = ["INCAN_IOS_FIXTURE=1"]

[interop.c.targets.platform]
kind = "ios"
deployment-target = "13.0"
"#,
    )?;
    let header = tmp.path().join("ios_fixture.h");
    fs::write(
        &header,
        "#include <stdint.h>\n#ifndef INCAN_IOS_FIXTURE\n#error expected iOS target definition\n#endif\ntypedef struct fixture_pair { int32_t left; int32_t right; } fixture_pair;\n#define FIXTURE_OK 0\nint32_t fixture_abs(int32_t value);\n",
    )?;
    fs::write(
        &main_path,
        format!(
            "from std.interop import c\n\nbinding Fixture:\n    header = \"{}\"\n    link = c.system_library(\"c\")\n\n    symbol absolute(value: c.i32) -> c.i32:\n        native = \"fixture_abs\"\n\n    enum Status:\n        OK: c.i32 = FIXTURE_OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n        right: c.i32 = right\n\ndef main() -> None:\n    assert Fixture.Status.OK == 0\n",
            header.display()
        ),
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let output = run_incan_with_env_and_removed(
        tmp.path(),
        &["check", "--interop-target", "aarch64-apple-ios", main_arg],
        &[],
        &["INCAN_C_ABI_CLANG"],
    )?;
    assert_success(&output, "declared iOS C ABI verification");
    Ok(())
}
