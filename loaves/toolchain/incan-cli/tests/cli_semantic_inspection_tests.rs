//! `inspect codegraph`, `inspect rust`, checked-binding facts, and the semantic projections that share a project
//! identity.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_codegraph_and_inspection_tests_root.rs");

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
    // number. The check report moved to v2 when it began carrying warnings; the build report carries its own line,
    // read from the constant so a bump there does not silently strand this pin.
    assert_eq!(
        build_json["schema_version"],
        serde_json::json!(BUILD_REPORT_SCHEMA_VERSION)
    );
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
    assert_eq!(
        build_json["schema_version"],
        serde_json::json!(BUILD_REPORT_SCHEMA_VERSION)
    );
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
