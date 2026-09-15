//! `inspect codegraph`, `inspect rust`, checked-binding facts, and the semantic projections that share a project
//! identity.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `tests/support/cli_project.rs`.

use std::fs;

mod support;

#[path = "support/cli_project.rs"]
mod cli_project;

use cli_project::*;

#[test]
fn semantic_inspection_surfaces_share_project_identity() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "semantic_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"pub model Widget:
    """A documented value that should appear in reports and graph facts."""
    pub value: int

pub def make_widget(value: int) -> Widget:
    """Create a value for semantic inspection smoke tests."""
    return Widget(value=value)
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from helpers import make_widget

pub def entrypoint() -> int:
    return make_widget(42).value

def main() -> None:
    println(f"semantic {entrypoint()}")
"#,
    )?;

    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let check = run_incan(tmp.path(), &["check", main_arg, "--format", "json"])?;
    assert_success(&check, "incan check --format json semantic inspection fixture");
    let check_json = parse_json_stdout(&check)?;
    assert_eq!(check_json["schema_version"], serde_json::json!(2));
    assert_eq!(check_json["ok"], serde_json::json!(true));
    assert_eq!(check_json["diagnostics"], serde_json::json!([]));

    let build = run_incan(tmp.path(), &["build", main_arg, "--offline", "--report", "json"])?;
    assert_success(&build, "incan build --report json semantic inspection fixture");
    let build_json = parse_json_stdout(&build)?;
    // Each report is independently versioned; this test asserts shared *project identity*, not a shared schema
    // number. The check report moved to v2 when it began carrying warnings, while build reports stayed at v1.
    assert_eq!(build_json["schema_version"], serde_json::json!(1));
    assert_eq!(build_json["project"]["name"], serde_json::json!("semantic_probe"));
    assert_source_files_include(&build_json, &["src/main.incn", "src/helpers.incn"])?;

    let inspect = run_incan(tmp.path(), &["inspect", "rust", main_arg, "--format", "json"])?;
    assert_success(&inspect, "incan inspect rust --format json semantic inspection fixture");
    let inspect_json = parse_json_stdout(&inspect)?;
    assert_eq!(inspect_json["schema_version"], build_json["schema_version"]);
    assert_eq!(inspect_json["compiler_version"], build_json["compiler_version"]);
    assert_eq!(
        inspect_json["generated"]["project_path"],
        build_json["generated"]["project_path"]
    );
    assert_eq!(
        inspect_json["generated"]["manifest_path"],
        build_json["generated"]["manifest_path"]
    );
    assert_eq!(
        inspect_json["generated"]["crate_root"],
        build_json["generated"]["crate_root"]
    );
    assert_source_files_include(&inspect_json, &["src/main.incn", "src/helpers.incn"])?;

    let codegraph = run_incan(tmp.path(), &["inspect", "codegraph", main_arg, "--format", "jsonl"])?;
    assert_success(
        &codegraph,
        "incan inspect codegraph --format jsonl semantic inspection fixture",
    );
    let records = parse_jsonl_stdout(&codegraph)?;
    assert_codegraph_record_contract(&records);
    // Codegraph is a separate versioned projection. RFC 113 adds checked registry records under codegraph schema v2,
    // while build reports retain their independently versioned schema.
    assert_eq!(build_json["schema_version"], serde_json::json!(1));
    assert_eq!(records[0]["compiler_version"], build_json["compiler_version"]);
    assert_eq!(records[0]["package"]["name"], serde_json::json!("semantic_probe"));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("file")
            && record["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("src/main.incn"))
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("file")
            && record["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("src/helpers.incn"))
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["kind"] == serde_json::json!("function")
            && record["name"] == serde_json::json!("entrypoint")
            && record["visibility"] == serde_json::json!("public")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("entrypoint")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("make_widget")
            && record["provenance"] == serde_json::json!("checked")
    }));

    Ok(())
}

#[test]
#[cfg_attr(
    not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64")
    )),
    ignore = "checked C integration requires a Linux x86-64 or macOS arm64 verifier"
)]
fn inspect_bindings_projects_checked_declaration_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "binding_inspection"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[interop.c]
schema = 1

[[interop.c.targets]]
target = "aarch64-apple-darwin"
headers = ["fixture.h"]

[[interop.c.targets.artifacts]]
name = "fixture"
kind = "system"
capability = "system.fixture"

[[interop.c.targets.bindings]]
module = ["fixture"]
name = "Fixture"
artifacts = ["fixture"]
"#,
    )?;
    fs::write(
        tmp.path().join("fixture.h"),
        "typedef struct fixture_handle fixture_handle;\ntypedef struct fixture_pair { int left; int right; } fixture_pair;\n#define FIXTURE_OK 0\nint fixture_add(int left, int right);\nvoid fixture_close(fixture_handle *handle);\nint fixture_inspect(fixture_handle *handle);\nint fixture_open(fixture_handle **output, int *attempts);\n",
    )?;
    fs::write(
        src_dir.join("fixture.incn"),
        r#"from std.interop import c

binding Fixture:
    header = "fixture.h"
    link = c.system_library("fixture")

    resource Handle:
        native = "fixture_handle"
        release = close

    symbol close(handle: c.Owned[Handle]) -> None:
        native = "fixture_close"

    symbol inspect(handle: c.Borrowed[Handle]) -> c.i32:
        native = "fixture_inspect"

    symbol add(left: c.i32, right: c.i32) -> c.i32:
        native = "fixture_add"

    enum Status:
        OK: c.i32 = FIXTURE_OK

    symbol open(output: c.Out[c.Owned[Handle]], attempts: c.InOut[c.i32]) -> c.i32:
        native = "fixture_open"

        outcome Status.OK:
            initializes = [output]
            updates = [attempts]

    struct Pair:
        native = "fixture_pair"
        left: c.i32 = left
        right: c.i32 = right

pub def fixture_name() -> str:
    return "fixture"

def private_bridge() -> c.i32:
    left: i32 = 2
    right: i32 = 3
    unsafe:
        return Fixture.add(left, right)

pub def checked_sum() -> c.i32:
    return private_bridge()
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from fixture import fixture_name

def main() -> None:
    println(fixture_name())
"#,
    )?;

    let project = tmp.path().to_str().ok_or("project path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["inspect", "bindings", project, "--format", "json"])?;
    assert_success(&output, "inspect checked C binding declarations as JSON");
    let report = parse_json_stdout(&output)?;
    assert_eq!(report["schema_version"], serde_json::json!(2));
    let bindings = report["bindings"]
        .as_array()
        .ok_or("binding report did not contain bindings")?;
    assert_eq!(bindings.len(), 1);
    let fixture = &bindings[0];
    assert_eq!(fixture["name"], serde_json::json!("Fixture"));
    assert_eq!(fixture["module"], serde_json::json!(["fixture"]));
    assert_eq!(fixture["header"], serde_json::json!("fixture.h"));
    assert_eq!(fixture["system_library"], serde_json::json!("fixture"));
    assert!(
        fixture["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "checked binding inspection must publish the compiler-owned portable descriptor identity: {fixture}"
    );
    assert!(
        fixture["source"]["file"]
            .as_str()
            .is_some_and(|path| path.ends_with("src/fixture.incn"))
    );
    assert_eq!(fixture["source"]["start_line"], serde_json::json!(3));
    assert_eq!(fixture["source"]["start_column"], serde_json::json!(1));
    assert!(fixture["source"]["end_column"].is_number());
    assert_eq!(fixture["resources"][0]["name"], serde_json::json!("Handle"));
    assert_eq!(fixture["resources"][0]["native"], serde_json::json!("fixture_handle"));
    assert_eq!(fixture["resources"][0]["release"], serde_json::json!("close"));
    assert_eq!(fixture["symbols"][0]["name"], serde_json::json!("close"));
    assert_eq!(fixture["symbols"][0]["native"], serde_json::json!("fixture_close"));
    assert_eq!(
        fixture["symbols"][0]["parameters"][0]["type"]["kind"],
        serde_json::json!("resource")
    );
    assert_eq!(
        fixture["symbols"][0]["parameters"][0]["type"]["access"],
        serde_json::json!("owned")
    );
    assert_eq!(
        fixture["symbols"][1]["parameters"][0]["type"]["access"],
        serde_json::json!("borrowed")
    );
    assert_eq!(
        fixture["symbols"][2]["parameters"][0]["type"]["spelling"],
        serde_json::json!("c.i32")
    );
    assert_eq!(
        fixture["symbols"][2]["return_type"]["spelling"],
        serde_json::json!("c.i32")
    );
    assert_eq!(fixture["symbols"][3]["name"], serde_json::json!("open"));
    assert_eq!(
        fixture["symbols"][3]["parameters"][0]["type"]["kind"],
        serde_json::json!("output")
    );
    assert_eq!(
        fixture["symbols"][3]["parameters"][0]["type"]["mode"],
        serde_json::json!("out")
    );
    assert_eq!(
        fixture["symbols"][3]["parameters"][0]["type"]["value"]["kind"],
        serde_json::json!("resource")
    );
    assert_eq!(
        fixture["symbols"][3]["parameters"][1]["type"]["mode"],
        serde_json::json!("in_out")
    );
    assert_eq!(
        fixture["symbols"][3]["outcomes"][0]["result"],
        serde_json::json!("Status.OK")
    );
    assert_eq!(
        fixture["symbols"][3]["outcomes"][0]["initializes"],
        serde_json::json!(["output"])
    );
    assert_eq!(
        fixture["symbols"][3]["outcomes"][0]["updates"],
        serde_json::json!(["attempts"])
    );
    assert_eq!(fixture["enums"][0]["name"], serde_json::json!("Status"));
    assert_eq!(fixture["enums"][0]["carrier"], serde_json::json!("c.i32"));
    assert_eq!(
        fixture["enums"][0]["variants"][0]["native"],
        serde_json::json!("FIXTURE_OK")
    );
    assert_eq!(fixture["structs"][0]["name"], serde_json::json!("Pair"));
    assert_eq!(fixture["structs"][0]["native"], serde_json::json!("fixture_pair"));
    assert_eq!(fixture["structs"][0]["fields"][1]["name"], serde_json::json!("right"));

    let text = run_incan(tmp.path(), &["inspect", "bindings", project, "--format", "text"])?;
    assert_success(&text, "inspect checked C binding declarations as text");
    let text = String::from_utf8(text.stdout)?;
    assert!(
        text.contains("Binding Fixture (fixture)"),
        "unexpected binding report:\n{text}"
    );
    assert!(text.contains("fixture.h"), "unexpected binding report:\n{text}");
    assert!(text.contains("identity: sha256:"), "unexpected binding report:\n{text}");
    assert!(text.contains("fixture_add"), "unexpected binding report:\n{text}");
    assert!(
        text.contains("resource Handle [native: fixture_handle, release: close]"),
        "unexpected binding report:\n{text}"
    );
    assert!(
        text.contains("outcome Status.OK [initializes: output, updates: attempts, invalidates: -]"),
        "unexpected binding report:\n{text}"
    );

    write_locked_oven_interop_plan(tmp.path())?;
    let receipt = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            project,
            "--format",
            "receipt",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_success(&receipt, "inspect redacted checked C binding usage receipt");
    let receipt_stdout = String::from_utf8(receipt.stdout)?;
    let receipt: serde_json::Value = serde_json::from_str(&receipt_stdout)?;
    assert_eq!(receipt["schema_version"], serde_json::json!(2));
    assert_eq!(
        receipt["compatibility"]["binding_contract"],
        serde_json::json!("exact_descriptor_identity")
    );
    assert_eq!(
        receipt["compatibility"]["target_contract"],
        serde_json::json!("exact_locked_target_identity")
    );
    assert_eq!(receipt["target"]["target"], serde_json::json!("aarch64-apple-darwin"));
    assert!(
        receipt["target"]["locked_target_identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "binding usage receipt must join the compiler-owned locked target identity: {receipt}"
    );
    assert!(
        receipt["target"].get("selected_execution_identity").is_none(),
        "a receipt must omit an execution identity when no selected interop execution receipt exists: {receipt}"
    );
    assert_eq!(receipt["bindings"].as_array().map(Vec::len), Some(1));
    assert_eq!(receipt["bindings"][0]["name"], serde_json::json!("Fixture"));
    assert_eq!(
        receipt["bindings"][0]["target_artifacts"],
        serde_json::json!(["fixture"])
    );
    assert!(
        receipt["bindings"][0]["identity"]
            .as_str()
            .is_some_and(|identity| identity.starts_with("sha256:")),
        "binding usage receipt must retain the checked descriptor identity: {receipt}"
    );
    assert_eq!(receipt["calls"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        receipt["calls"][0]["binding_identity"], receipt["bindings"][0]["identity"],
        "raw-call usage must join its checked binding by compiler-owned identity"
    );
    assert_eq!(receipt["calls"][0]["symbol"], serde_json::json!("add"));
    assert_eq!(
        receipt["calls"][0]["owner"]["name"],
        serde_json::json!("private_bridge")
    );
    assert_eq!(receipt["calls"][0]["owner"]["visibility"], serde_json::json!("private"));
    assert_eq!(receipt["facades"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        receipt["facades"][0]["facade"]["name"],
        serde_json::json!("checked_sum")
    );
    assert_eq!(
        receipt["facades"][0]["facade"]["visibility"],
        serde_json::json!("public")
    );
    assert_eq!(
        receipt["facades"][0]["bridge"]["name"],
        serde_json::json!("private_bridge")
    );
    assert_eq!(
        receipt["facades"][0]["bridge"]["visibility"],
        serde_json::json!("private")
    );
    assert_eq!(
        receipt["facades"][0]["calls"][0]["binding_identity"], receipt["bindings"][0]["identity"],
        "the facade receipt must link its bridge raw call through the exact descriptor identity"
    );
    assert_eq!(receipt["facades"][0]["calls"][0]["symbol"], serde_json::json!("add"));
    assert!(
        !receipt_stdout.contains(&tmp.path().to_string_lossy().to_string())
            && !receipt_stdout.contains("fixture.h")
            && !receipt_stdout.contains("source"),
        "binding usage receipt must not retain local paths, header declarations, or source spans: {receipt_stdout}"
    );

    let relocated_temp = tempfile::tempdir()?;
    let relocated_root = relocated_temp.path().join("binding-inspection-relocated");
    fs::create_dir_all(relocated_root.join("src"))?;
    for relative_path in [
        "loaf.toml",
        "oven.lock",
        "fixture.h",
        "src/fixture.incn",
        "src/main.incn",
    ] {
        fs::copy(tmp.path().join(relative_path), relocated_root.join(relative_path))?;
    }
    let relocated_project = relocated_root
        .to_str()
        .ok_or("relocated project path was not valid UTF-8")?;
    let relocated_receipt = run_incan(
        &relocated_root,
        &[
            "inspect",
            "bindings",
            relocated_project,
            "--format",
            "receipt",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_success(&relocated_receipt, "relocated redacted checked C binding usage receipt");
    assert_eq!(
        receipt_stdout.as_bytes(),
        relocated_receipt.stdout.as_slice(),
        "a relocated locked package changed its redacted binding receipt"
    );

    let manifest_path = tmp.path().join("loaf.toml");
    let manifest = fs::read_to_string(&manifest_path)?;
    let dangling_manifest = manifest.replacen(
        "module = [\"fixture\"]\nname = \"Fixture\"",
        "module = [\"fixture\"]\nname = \"MissingFixture\"",
        1,
    );
    assert_ne!(
        manifest, dangling_manifest,
        "fixture manifest must contain the declared binding relation"
    );
    fs::write(&manifest_path, dangling_manifest)?;
    write_locked_oven_interop_plan(tmp.path())?;
    let dangling = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            project,
            "--format",
            "receipt",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_failure(&dangling, "dangling target binding-artifact correspondence");
    assert!(
        String::from_utf8_lossy(&dangling.stderr).contains("did not produce that checked binding"),
        "a receipt must reject an authored correspondence without a compiler-produced binding:\n{}",
        String::from_utf8_lossy(&dangling.stderr)
    );

    let ignored_target = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            project,
            "--format",
            "json",
            "--target",
            "aarch64-apple-darwin",
        ],
    )?;
    assert_failure(&ignored_target, "binding target outside receipt mode");
    assert!(
        String::from_utf8_lossy(&ignored_target.stderr).contains("requires `--format receipt`"),
        "a target outside receipt mode must fail instead of being silently ignored:\n{}",
        String::from_utf8_lossy(&ignored_target.stderr)
    );

    let broken_path = src_dir.join("broken.incn");
    fs::write(
        &broken_path,
        r#"from std.interop import c

@c.binding(header="fixture.h", link=c.system_library("fixture"))
class Broken:
    value: int
"#,
    )?;
    let broken = run_incan(
        tmp.path(),
        &[
            "inspect",
            "bindings",
            broken_path.to_str().ok_or("broken path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&broken, "invalid checked C binding inspection");
    assert!(
        broken.stdout.is_empty(),
        "strict binding inspection must not emit partial JSON:\n{}",
        String::from_utf8_lossy(&broken.stdout)
    );
    assert!(
        String::from_utf8_lossy(&broken.stderr).contains("must extend BindingDeclaration"),
        "binding inspection should preserve the compiler diagnostic:\n{}",
        String::from_utf8_lossy(&broken.stderr)
    );

    Ok(())
}

#[test]
fn diagnostic_facts_keep_related_spans_and_type_payloads_across_cli_and_codegraph()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"model Wire:
    id [alias="wire"]: int
    label [alias="wire"]: str

def accept(value: int) -> None:
    pass

def main() -> None:
    accept(value=1, value=2)
    accept("text")
"#,
    )?;
    let source_arg = source_path.to_str().ok_or("source path was not valid UTF-8")?;

    let check = run_incan(tmp.path(), &["check", source_arg, "--format", "json"])?;
    assert_failure(&check, "incan check should emit diagnostic facts");
    let check_json = parse_json_stdout(&check)?;
    let cli_diagnostics = check_json["diagnostics"]
        .as_array()
        .ok_or("expected CLI diagnostics array")?;
    let cli_alias = cli_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Duplicate alias"))
        })
        .ok_or("expected duplicate alias diagnostic")?;
    let cli_duplicate_arg = cli_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Duplicate argument"))
        })
        .ok_or("expected duplicate argument diagnostic")?;
    let cli_mismatch = cli_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Argument 'value' of 'accept'"))
        })
        .ok_or("expected call argument type mismatch diagnostic")?;

    assert_eq!(cli_alias["origin"], serde_json::json!("typechecker"));
    assert_eq!(
        cli_alias["related_spans"][0]["label"],
        serde_json::json!("First field alias 'wire'")
    );
    assert_eq!(
        cli_duplicate_arg["related_spans"][0]["label"],
        serde_json::json!("First argument named 'value'")
    );
    assert_eq!(cli_mismatch["expected"], serde_json::json!("int"));
    assert_eq!(cli_mismatch["actual"], serde_json::json!("str"));

    let codegraph = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            source_arg,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(&codegraph, "tolerant codegraph should project diagnostic facts");
    let records = parse_jsonl_stdout(&codegraph)?;
    let graph_alias = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("diagnostic") && record["message"] == cli_alias["message"])
        .ok_or("expected duplicate alias codegraph diagnostic")?;
    let graph_duplicate_arg = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("diagnostic") && record["message"] == cli_duplicate_arg["message"]
        })
        .ok_or("expected duplicate argument codegraph diagnostic")?;
    let graph_mismatch = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("diagnostic") && record["message"] == cli_mismatch["message"]
        })
        .ok_or("expected type mismatch codegraph diagnostic")?;

    assert_eq!(graph_alias["origin"], cli_alias["origin"]);
    assert_eq!(
        graph_alias["related_spans"][0]["label"],
        cli_alias["related_spans"][0]["label"]
    );
    assert_eq!(
        graph_alias["related_spans"][0]["span"]["start"],
        cli_alias["related_spans"][0]["span"]["start"]["offset"]
    );
    assert_eq!(
        graph_duplicate_arg["related_spans"][0]["label"],
        cli_duplicate_arg["related_spans"][0]["label"]
    );
    assert_eq!(graph_mismatch["expected"], cli_mismatch["expected"]);
    assert_eq!(graph_mismatch["actual"], cli_mismatch["actual"]);

    Ok(())
}

#[test]
fn inspect_rust_reports_current_generated_rust_files() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"def main() -> None:
    println("inspect ok")
"#,
    )?;
    let executable = run_incan(
        tmp.path(),
        &[
            "inspect",
            "rust",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_success(&executable, "incan inspect rust executable");
    let executable_report = parse_json_stdout(&executable)?;
    assert_eq!(executable_report["mode"], serde_json::json!("executable"));
    assert!(executable_report["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with("main.incn")))
    }));
    assert!(
        executable_report["rust_files"]
            .as_array()
            .is_some_and(|files| { files.iter().any(|file| file["crate_root"] == serde_json::json!(true)) })
    );

    let project = tempfile::tempdir()?;
    let src_dir = project.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project.path().join("loaf.toml"),
        r#"[project]
name = "inspect_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub model Widget:
    """Widget docs survive into generated Rust."""
    pub value: int

pub def answer() -> int:
    """Answer docs survive into generated Rust."""
    return 42
"#,
    )?;
    let library = run_incan(
        project.path(),
        &[
            "inspect",
            "rust",
            project.path().to_str().ok_or("project path was not valid UTF-8")?,
            "--lib",
            "--format",
            "json",
        ],
    )?;
    assert_success(&library, "incan inspect rust --lib");
    let library_report = parse_json_stdout(&library)?;
    assert_eq!(library_report["mode"], serde_json::json!("library"));
    assert!(library_report["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with("src/lib.incn")))
    }));
    assert!(
        library_report["generated"]["project_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("target/lib"))
    );
    assert!(
        library_report["rust_files"]
            .as_array()
            .is_some_and(|files| { files.iter().any(|file| file["crate_root"] == serde_json::json!(true)) })
    );
    let crate_root_path = library_report["rust_files"]
        .as_array()
        .and_then(|files| files.iter().find(|file| file["crate_root"] == serde_json::json!(true)))
        .and_then(|file| file["path"].as_str())
        .ok_or("library inspection report did not include a crate root file")?;
    let crate_root = fs::read_to_string(crate_root_path)?;
    assert!(
        crate_root.contains(r#"#[doc = "Widget docs survive into generated Rust."]"#)
            || crate_root.contains("/// Widget docs survive into generated Rust."),
        "expected generated Rust to include public model docs, got:\n{crate_root}"
    );
    assert!(
        crate_root.contains(r#"#[doc = "Answer docs survive into generated Rust."]"#)
            || crate_root.contains("/// Answer docs survive into generated Rust."),
        "expected generated Rust to include public function docs, got:\n{crate_root}"
    );

    Ok(())
}

#[test]
fn inspect_codegraph_exports_multifile_imports_and_public_symbols() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "graph_demo"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"pub model Widget:
    pub value: int

pub def make_widget(value: int) -> Widget:
    return Widget(value=value)
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"import helpers
from helpers import make_widget

enum Signal:
    Ready

def local_value() -> int:
    return 3

def qualified_value() -> int:
    return helpers.make_widget(std.builtins.len([1, 2])).value

def ready() -> Signal:
    return Signal.Ready()

pub def entrypoint() -> int:
    return make_widget(local_value()).value
"#,
    )?;

    let first = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&first, "incan inspect codegraph");
    let second = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&second, "second incan inspect codegraph");
    assert_eq!(first.stdout, second.stdout, "codegraph JSONL should be deterministic");

    let records = parse_jsonl_stdout(&first)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["package"]["name"], serde_json::json!("graph_demo"));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("import")
            && record["path"] == serde_json::json!("helpers")
            && record["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|value| value == "make_widget"))
            })
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["kind"] == serde_json::json!("function")
            && record["name"] == serde_json::json!("entrypoint")
            && record["visibility"] == serde_json::json!("public")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("export")
            && record["name"] == serde_json::json!("entrypoint")
            && record["kind"] == serde_json::json!("declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("module_contains_declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("make_widget")
            && record["argument_count"] == serde_json::json!(1)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("helpers.make_widget")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("std.builtins.len")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("len")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("builtin")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("Signal.Ready")
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("Ready")
            && record["canonical_identity"]["declaration_kind"] == serde_json::json!("variant")
            && record["canonical_identity"]["origin"]["kind"] == serde_json::json!("module")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("local_value")
            && record["argument_count"] == serde_json::json!(0)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("local_value")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("make_widget")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("make_widget")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("local_value")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("field")
            && record["name"] == serde_json::json!("value")
            && record["target_id"] == serde_json::Value::Null
            && record["canonical_identity"]["declaration_name"] == serde_json::json!("value")
            && record["canonical_identity"]["namespace"] == serde_json::json!("member")
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("declaration_contains_call")
    }));

    let directory = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("src directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&directory, "directory incan inspect codegraph");
    let directory_records = parse_jsonl_stdout(&directory)?;
    assert!(directory_records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                directory_records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_keeps_one_identity_through_alias_reexport_and_without_a_local_record()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "identity_graph"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("provider.incn"),
        r#"pub def helper() -> int:
    return 7

pub run = alias helper
"#,
    )?;
    fs::write(
        src_dir.join("facade.incn"),
        r#"pub from provider import run as h
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from facade import h as run_helper

def entrypoint() -> int:
    print("identity")
    return (run_helper)()
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "identity-backed incan inspect codegraph");
    let records = parse_jsonl_stdout(&output)?;

    let provider = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!("helper")
        })
        .ok_or("provider declaration was absent")?;
    let provider_identity = &provider["canonical_identity"];
    assert_eq!(provider_identity["declaration_name"], serde_json::json!("helper"));
    assert_eq!(provider["provenance"], serde_json::json!("checked"));

    let declaration_alias = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!("run")
        })
        .ok_or("provider declaration alias was absent")?;
    assert_eq!(&declaration_alias["canonical_identity"], provider_identity);
    assert_ne!(declaration_alias["id"], provider["id"]);

    let reexport = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("export") && record["name"] == serde_json::json!("h"))
        .ok_or("facade re-export record was absent")?;
    assert_eq!(&reexport["canonical_identity"], provider_identity);
    assert_eq!(reexport["provenance"], serde_json::json!("checked"));

    let aliased_import = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("import")
                && record["bindings"].as_array().is_some_and(|bindings| {
                    bindings
                        .iter()
                        .any(|binding| binding["local_name"] == serde_json::json!("run_helper"))
                })
        })
        .ok_or("consumer alias import record was absent")?;
    let aliased_binding = aliased_import["bindings"]
        .as_array()
        .and_then(|bindings| {
            bindings
                .iter()
                .find(|binding| binding["local_name"] == serde_json::json!("run_helper"))
        })
        .ok_or("consumer alias binding was absent")?;
    assert_eq!(&aliased_binding["canonical_identity"], provider_identity);
    assert_eq!(aliased_import["provenance"], serde_json::json!("checked"));

    for record_kind in ["reference", "call"] {
        let aliased = records
            .iter()
            .find(|record| {
                record["record"] == serde_json::json!(record_kind)
                    && (record["name"] == serde_json::json!("run_helper")
                        || record["callee"] == serde_json::json!("run_helper"))
            })
            .ok_or_else(|| format!("aliased {record_kind} record was absent"))?;
        assert_eq!(
            &aliased["canonical_identity"], provider_identity,
            "every spelling must retain the original provider identity"
        );
        assert_eq!(
            aliased["target_id"], provider["id"],
            "graph-local linkage must select the canonical declaration rather than its alias binding"
        );
        assert_eq!(aliased["provenance"], serde_json::json!("checked"));
    }

    let builtin = records
        .iter()
        .find(|record| record["record"] == serde_json::json!("call") && record["callee"] == serde_json::json!("print"))
        .ok_or("builtin call record was absent")?;
    assert_eq!(builtin["target_id"], serde_json::Value::Null);
    assert_eq!(
        builtin["canonical_identity"]["origin"]["kind"],
        serde_json::json!("builtin")
    );
    assert_eq!(
        builtin["canonical_identity"]["declaration_name"],
        serde_json::json!("print")
    );
    assert_eq!(
        builtin["provenance"],
        serde_json::json!("checked"),
        "a missing graph-local declaration must not erase compiler-proven identity"
    );

    Ok(())
}

#[test]
fn inspect_codegraph_exports_checked_registry_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "registry_graph"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
    summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "registry codegraph export");
    let records = parse_jsonl_stdout(&output)?;
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("registry")
            && record["registry_identity"] == serde_json::json!("main::functions")
            && record["registry_public"] == serde_json::json!(true)
            && record["subject_kind"] == serde_json::json!("function")
            && record["subject_identity"] == serde_json::json!("main.normalize")
            && record["key"]["kind"] == serde_json::json!("newtype")
            && record["descriptor"]["kind"] == serde_json::json!("model")
            && record["registration_span"].is_object()
            && record["subject_span"].is_object()
            && record["provenance"] == serde_json::json!("checked")
    }));
    Ok(())
}

#[test]
fn inspect_codegraph_attaches_facade_paths_to_checked_registry_facts() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname = \"registry_graph_facade\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        src_dir.join("feature.incn"),
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"pub from crate.feature import functions as public_functions
pub from crate.feature import normalize as public_normalize
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir
                .join("main.incn")
                .to_str()
                .ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&output, "registry facade codegraph export");
    let records = parse_jsonl_stdout(&output)?;
    let registry = records
        .iter()
        .find(|record| {
            record["record"] == serde_json::json!("registry")
                && record["registry_identity"] == serde_json::json!("feature::functions")
        })
        .ok_or("missing checked feature registry record")?;
    assert_eq!(registry["subject_identity"], serde_json::json!("feature.normalize"));
    let reexport_paths = registry["reexport_paths"]
        .as_array()
        .ok_or("checked registry record must expose facade projections")?;
    assert_eq!(
        reexport_paths
            .iter()
            .map(|projection| projection["path"].clone())
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!(["main", "public_functions"]),
            serde_json::json!(["main", "public_normalize"]),
        ]
    );
    assert!(
        reexport_paths.iter().all(|path| path["span"].is_object()),
        "facade projections must retain their public-import anchors: {registry}"
    );
    Ok(())
}

#[test]
fn inspect_codegraph_projects_the_selected_incan_package_features() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "feature_graph_demo",
        r#"

[project.features]
alpha = []
beta = []
"#,
    )?;
    fs::write(
        &main_path,
        r#"when feature("alpha"):
    pub def alpha_entrypoint() -> str:
        return "alpha"

when feature("beta"):
    pub def beta_entrypoint() -> str:
        return "beta"
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    for (selected, expected, excluded) in [
        ("alpha", "alpha_entrypoint", "beta_entrypoint"),
        ("beta", "beta_entrypoint", "alpha_entrypoint"),
    ] {
        let output = run_incan(
            tmp.path(),
            &[
                "inspect",
                "codegraph",
                main_arg,
                "--format",
                "jsonl",
                "--no-default-features",
                "--features",
                selected,
            ],
        )?;
        assert_success(&output, &format!("codegraph projection for package feature {selected}"));
        let records = parse_jsonl_stdout(&output)?;
        let header = records.first().ok_or("codegraph did not emit a header")?;
        let semantic_context = header["semantic_contexts"]
            .as_array()
            .and_then(|contexts| contexts.first())
            .ok_or("codegraph header did not project semantic context")?;
        let package = semantic_context["packages"]
            .as_array()
            .and_then(|packages| packages.first())
            .ok_or("codegraph semantic context did not project package features")?;
        assert_eq!(package["active_features"], serde_json::json!([selected]));
        assert!(semantic_context["providers"].as_array().is_some_and(|providers| {
            providers.iter().any(|provider| {
                provider["provenance"]["kind"] == serde_json::json!("sdk")
                    && provider["enabled"] == serde_json::json!(true)
                    && provider["manifest_path"].as_str().is_some()
            })
        }));
        assert!(records.iter().any(|record| {
            record["record"] == serde_json::json!("declaration") && record["name"] == serde_json::json!(expected)
        }));
        assert!(
            records.iter().all(|record| {
                record["record"] != serde_json::json!("declaration") || record["name"] != serde_json::json!(excluded)
            }),
            "codegraph for `{selected}` retained inactive declaration `{excluded}`"
        );
    }

    Ok(())
}

#[test]
fn transient_sdk_profile_is_shared_by_check_and_provider_inspection() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "sdk_profile_override", "")?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let minimal_core = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "minimal"])?;
    assert_success(
        &minimal_core,
        "minimal SDK profile check using only core language surface",
    );
    let minimal_codegraph = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_arg,
            "--format",
            "jsonl",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(
        &minimal_codegraph,
        "minimal SDK profile codegraph using only core language surface",
    );

    fs::write(
        &main_path,
        r#"from std.fs.path import Path

def main() -> None:
    _ = Path("profile")
"#,
    )?;

    let minimal = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "minimal"])?;
    assert_failure(&minimal, "minimal SDK profile check using std.fs");
    let minimal_stderr = String::from_utf8_lossy(&minimal.stderr);
    assert!(
        minimal_stderr.contains("stdlib-system") && minimal_stderr.contains("disabled"),
        "minimal profile should diagnose the disabled std.fs component:\n{minimal_stderr}"
    );
    let minimal_json = run_incan(
        tmp.path(),
        &["check", main_arg, "--format", "json", "--sdk-profile", "minimal"],
    )?;
    assert_failure(&minimal_json, "minimal SDK profile JSON diagnostic using std.fs");
    let minimal_json = parse_json_stdout(&minimal_json)?;
    assert_eq!(minimal_json["diagnostics"][0]["code"], serde_json::json!("INCAN-I0101"));

    let default = run_incan(tmp.path(), &["check", main_arg, "--sdk-profile", "default"])?;
    assert_success(&default, "default SDK profile check using std.fs");

    for command in ["build", "run"] {
        let output = run_incan(tmp.path(), &[command, main_arg, "--sdk-profile", "minimal"])?;
        assert_failure(&output, &format!("{command} using disabled std.fs component"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("stdlib-system") && stderr.contains("disabled"),
            "{command} should use the transient provider projection:\n{stderr}"
        );
    }

    let inspection = run_incan(
        tmp.path(),
        &[
            "inspect",
            "providers",
            main_arg,
            "--format",
            "json",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(&inspection, "provider inspection with transient minimal SDK profile");
    let report = parse_json_stdout(&inspection)?;
    assert_eq!(report["sdk"]["profile"], serde_json::json!("minimal"));
    let components = report["sdk"]["components"]
        .as_array()
        .ok_or("provider report did not contain SDK components")?;
    let system = components
        .iter()
        .find(|component| component["id"] == serde_json::json!("stdlib-system"))
        .ok_or("provider report did not contain stdlib-system")?;
    assert_eq!(system["available"], serde_json::json!(true));
    assert_eq!(system["enabled"], serde_json::json!(false));
    let providers = report["providers"]
        .as_array()
        .ok_or("provider report did not contain providers")?;
    let system_provider = providers
        .iter()
        .find(|provider| provider["provenance"]["component_id"] == serde_json::json!("stdlib-system"))
        .ok_or("provider report did not contain the stdlib-system provider")?;
    assert_eq!(system_provider["available"], serde_json::json!(true));
    assert_eq!(system_provider["enabled"], serde_json::json!(false));
    assert_eq!(system_provider["used"], serde_json::json!(false));
    assert!(system_provider["provider_dependencies"].is_array());

    Ok(())
}

#[test]
fn lock_and_feature_inspection_record_transient_semantic_selections() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "semantic_selection_lock",
        r#"

[project.features]
default = ["alpha"]
alpha = []
beta = []
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let selection_args = [
        "--no-default-features",
        "--features",
        "beta",
        "--sdk-profile",
        "minimal",
    ];

    let inspection = run_incan(
        tmp.path(),
        &[
            "inspect",
            "features",
            main_arg,
            "--format",
            "json",
            selection_args[0],
            selection_args[1],
            selection_args[2],
            selection_args[3],
            selection_args[4],
        ],
    )?;
    assert_success(&inspection, "feature inspection with transient semantic selections");
    let report = parse_json_stdout(&inspection)?;
    let package = report["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .ok_or("feature report did not contain the root package")?;
    assert_eq!(package["active_features"], serde_json::json!(["beta"]));
    assert_eq!(package["reasons"]["beta"][0]["kind"], serde_json::json!("requested"));

    let lock = run_incan(
        tmp.path(),
        &[
            "lock",
            main_arg,
            selection_args[0],
            selection_args[1],
            selection_args[2],
            selection_args[3],
            selection_args[4],
        ],
    )?;
    assert_success(&lock, "semantic lock generation with transient selections");
    let lock: toml::Value = toml::from_str(&fs::read_to_string(tmp.path().join("oven.lock"))?)?;
    assert_eq!(lock["semantic"]["sdk"]["profile"].as_str(), Some("minimal"));
    let locked_package = lock["semantic"]["packages"]
        .as_array()
        .and_then(|packages| packages.first())
        .ok_or("semantic lock did not contain the root package")?;
    assert_eq!(
        locked_package["active_features"].as_array(),
        Some(&vec![toml::Value::String("beta".to_string())])
    );

    Ok(())
}

#[test]
fn locked_build_rejects_package_feature_or_sdk_projection_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "semantic_lock_drift",
        r#"

[project.features]
alpha = []
beta = []
"#,
    )?;
    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;

    let lock = run_incan(
        tmp.path(),
        &[
            "lock",
            main_arg,
            "--no-default-features",
            "--features",
            "alpha",
            "--sdk-profile",
            "minimal",
        ],
    )?;
    assert_success(&lock, "lock semantic alpha/minimal projection");

    for (feature, profile) in [("beta", "minimal"), ("alpha", "default")] {
        let build = run_incan(
            tmp.path(),
            &[
                "build",
                main_arg,
                "--locked",
                "--no-default-features",
                "--features",
                feature,
                "--sdk-profile",
                profile,
            ],
        )?;
        assert_failure(
            &build,
            &format!("locked build with drifted {feature}/{profile} projection"),
        );
        let stderr = String::from_utf8_lossy(&build.stderr);
        assert!(
            stderr.contains("oven.lock") && stderr.contains("out of date") && stderr.contains("Run `incan lock`"),
            "locked projection drift should fail as stale lock state:\n{stderr}"
        );
    }

    Ok(())
}

#[test]
fn codegraph_importer_example_consumes_compiler_jsonl_issue776() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_dir = tmp.path().join("source");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        source_dir.join("loaf.toml"),
        r#"[project]
name = "codegraph_importer_source"
version = "0.1.0"
"#,
    )?;
    let source_main = source_dir.join("main.incn");
    fs::write(
        &source_main,
        r#"from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
type FunctionId = newtype str

@derive(Descriptor)
model FunctionSpec:
    summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("greet"), FunctionSpec(summary="Greet a named user"))
pub def greet(name: str) -> str:
    return f"hello {name}"

def main() -> None:
    println(greet("Incan"))
"#,
    )?;

    let graph = run_incan(
        &source_dir,
        &[
            "inspect",
            "codegraph",
            source_main.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&graph, "compiler codegraph export for importer example");
    let graph_records = parse_jsonl_stdout(&graph)?;
    assert_codegraph_record_contract(&graph_records);

    let importer_dir = tmp.path().join("importer");
    let importer_src = importer_dir.join("src");
    fs::create_dir_all(&importer_src)?;
    fs::write(
        importer_dir.join("loaf.toml"),
        fs::read_to_string(support::repo_root().join("examples/pro/codegraph_importer/loaf.toml"))?,
    )?;
    fs::write(
        importer_src.join("importer.incn"),
        fs::read_to_string(support::repo_root().join("examples/pro/codegraph_importer/src/importer.incn"))?,
    )?;
    fs::write(
        importer_src.join("main.incn"),
        fs::read_to_string(support::repo_root().join("examples/pro/codegraph_importer/src/main.incn"))?,
    )?;
    fs::write(importer_dir.join("codegraph.jsonl"), &graph.stdout)?;

    let first = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&first, "Incan-authored codegraph importer example");
    let second = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&second, "second Incan-authored codegraph importer example");
    assert_eq!(first.stdout, second.stdout, "importer summary must be deterministic");

    let summary = parse_json_stdout(&first)?;
    assert_eq!(summary["schema_version"], serde_json::json!(7));
    assert_eq!(summary["mode"], serde_json::json!("strict"));
    assert_eq!(summary["metadata_record_count"], serde_json::json!(1));
    assert!(
        summary["fact_count"].as_i64().is_some_and(|count| count > 0),
        "importer must observe compiler-owned graph facts: {summary}"
    );
    assert!(
        summary["declaration_count"].as_i64().is_some_and(|count| count > 0),
        "importer must preserve declaration records without parsing source itself: {summary}"
    );
    assert!(
        summary["registry_count"].as_i64().is_some_and(|count| count > 0),
        "importer must preserve compiler-checked typed registry facts: {summary}"
    );

    fs::write(
        importer_dir.join("codegraph.jsonl"),
        concat!(
            r#"{"record":"header","schema_version":1,"mode":"strict","degraded":false}"#,
            "\n",
            r#"{"record":"file","degraded":false}"#,
            "\n",
        ),
    )?;
    let legacy = run_incan(&importer_dir, &["run", "src/main.incn"])?;
    assert_success(&legacy, "schema-v1 codegraph importer compatibility");
    let legacy_summary = parse_json_stdout(&legacy)?;
    assert_eq!(legacy_summary["schema_version"], serde_json::json!(1));
    assert_eq!(legacy_summary["file_count"], serde_json::json!(1));

    Ok(())
}

#[test]
fn inspect_codegraph_tolerant_directory_keeps_parseable_facts_and_diagnostics() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("ok.incn"),
        r#"pub def ok() -> int:
    return 1
"#,
    )?;
    let nested = tmp.path().join("nested");
    fs::create_dir_all(&nested)?;
    fs::write(
        nested.join("extra.incn"),
        r#"pub def extra() -> int:
    return 2
"#,
    )?;
    fs::write(tmp.path().join("broken.incn"), "def broken(:\n")?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(&strict, "strict incan inspect codegraph should reject broken source");

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(&tolerant, "tolerant incan inspect codegraph");
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("ok")
            && record["provenance"] == serde_json::json!("syntax")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("module")
            && record["module_path"] == serde_json::json!(["nested", "extra"])
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-P0001")
            && record["phase"] == serde_json::json!("parse")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_strict_directory_rejects_semantic_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("bad.incn"),
        r#"pub def bad() -> int:
    return missing()
"#,
    )?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(
        &strict,
        "strict incan inspect codegraph should reject directory typecheck diagnostics",
    );
    let strict_stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        strict_stderr.contains("Unknown symbol 'missing'"),
        "expected strict directory codegraph to report typecheck diagnostic, got:\n{strict_stderr}"
    );

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(
        &tolerant,
        "tolerant incan inspect codegraph should keep syntax facts for directory typecheck diagnostics",
    );
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("bad")
            && record["provenance"] == serde_json::json!("syntax")
            && record["canonical_identity"] == serde_json::Value::Null
            && record["degraded"] == serde_json::json!(true)
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("missing")
            && record["target_id"] == serde_json::Value::Null
            && record["canonical_identity"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("syntax")
            && record["degraded"] == serde_json::json!(true)
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-T0001")
            && record["phase"] == serde_json::json!("typecheck")
            && record["message"] == serde_json::json!("Unknown symbol 'missing'")
    }));

    Ok(())
}
