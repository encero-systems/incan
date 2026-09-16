//! The CLI's own reported surface: diagnostics JSON, `explain`, build reports, `tools`, `init`, `fmt`, and the version
//! gate.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `tests/support/cli_project.rs`.

use std::fs;
use std::process::Output;

use incan_driver::build_report::BUILD_REPORT_SCHEMA_VERSION;

mod support;

#[path = "support/cli_project.rs"]
mod cli_project;

use cli_project::*;

#[cfg(unix)]
#[test]
fn fixture_handoff_copy_rejects_symlinks() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let outside = temp.path().join("outside");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source)?;
    fs::create_dir_all(&outside)?;
    fs::write(outside.join("artifact"), "must not be copied")?;
    symlink(&outside, source.join("linked-handoff"))?;

    let result = copy_fixture_directory(&source, &destination);
    let Err(error) = result else {
        return Err("fixture handoff copy followed a symlink".into());
    };
    assert!(
        error.to_string().contains("fixture handoff contains symlink"),
        "unexpected symlink rejection: {error}"
    );
    assert!(
        !destination.join("linked-handoff").exists(),
        "fixture handoff copy materialized a symlink target"
    );
    Ok(())
}

#[test]
fn check_json_reports_parser_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("broken.incn");
    fs::write(&source_path, "def broken(:\n")?;

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json parser diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["schema_version"], serde_json::json!(2));
    assert_eq!(json["ok"], serde_json::json!(false));
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-P0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("parse"));
    assert_eq!(
        json["diagnostics"][0]["primary_span"]["start"]["line"],
        serde_json::json!(1)
    );

    Ok(())
}

#[test]
fn check_json_reports_typechecker_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"def main() -> None:
    missing()
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json typechecker diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-T0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("typecheck"));
    assert_eq!(
        json["diagnostics"][0]["message"],
        serde_json::json!("Unknown symbol 'missing'")
    );
    assert_eq!(
        json["diagnostics"][0]["explain"],
        serde_json::json!("incan explain INCAN-T0001")
    );

    let legacy_output = run_incan(
        tmp.path(),
        &[
            "--check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&legacy_output, "incan --check --format json typechecker diagnostic");
    let legacy_json = parse_json_stdout(&legacy_output)?;
    assert_eq!(legacy_json["diagnostics"][0]["code"], serde_json::json!("INCAN-T0001"));

    Ok(())
}

#[test]
fn check_json_reports_tooling_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let missing_path = tmp.path().join("missing.incn");

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            missing_path.to_str().ok_or("missing path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json tooling diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-C0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("tooling"));
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Cannot access file")),
        "expected missing file diagnostic, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    Ok(())
}

#[test]
fn check_json_reports_import_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "diag_import"
version = "0.1.0"
"#,
    )?;
    let source_path = src_dir.join("main.incn");
    fs::write(
        &source_path,
        r#"from pub::missinglib import Widget

def main() -> None:
    return
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "check",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan check --format json import diagnostic");
    let json = parse_json_stdout(&output)?;
    assert_eq!(json["diagnostics"][0]["code"], serde_json::json!("INCAN-I0001"));
    assert_eq!(json["diagnostics"][0]["phase"], serde_json::json!("import"));
    assert!(
        json["diagnostics"][0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Unknown `pub::` library")),
        "expected pub library import diagnostic, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    Ok(())
}

#[test]
fn explain_reports_known_and_unknown_diagnostic_codes() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;

    let known = run_incan(tmp.path(), &["explain", "INCAN-P0001", "--format", "json"])?;
    assert_success(&known, "incan explain known code json");
    let known_json = parse_json_stdout(&known)?;
    // Recoverable projections (#1293) widened the stable diagnostic payload, and the explain report carries the same
    // `DIAGNOSTIC_SCHEMA_VERSION` as every other stable diagnostic surface.
    assert_eq!(
        known_json["schema_version"],
        serde_json::json!(incan::frontend::diagnostics::DIAGNOSTIC_SCHEMA_VERSION)
    );
    assert_eq!(known_json["found"], serde_json::json!(true));
    assert_eq!(known_json["entry"]["code"], serde_json::json!("INCAN-P0001"));

    let unknown = run_incan(tmp.path(), &["explain", "INCAN-NOPE", "--format", "json"])?;
    assert_failure(&unknown, "incan explain unknown code json");
    let unknown_json = parse_json_stdout(&unknown)?;
    assert_eq!(unknown_json["found"], serde_json::json!(false));
    assert_eq!(unknown_json["entry"]["code"], serde_json::json!("INCAN-U0001"));

    Ok(())
}

#[test]
fn build_report_json_describes_executable_build() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let source_path = tmp.path().join("main.incn");
    fs::write(
        &source_path,
        r#"def main() -> None:
    println("report ok")
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "build",
            source_path.to_str().ok_or("source path was not valid UTF-8")?,
            "--offline",
            "--report",
            "json",
        ],
    )?;
    assert_success(&output, "incan build --report json executable");
    let report = parse_json_stdout(&output)?;
    assert_eq!(report["schema_version"], serde_json::json!(BUILD_REPORT_SCHEMA_VERSION));
    assert_eq!(report["status"], serde_json::json!("success"));
    assert_eq!(report["mode"], serde_json::json!("executable"));
    assert_eq!(report["profile"], serde_json::json!("release"));
    assert!(
        report["generated"]["project_path"]
            .as_str()
            .is_some_and(|path| path.contains("target/incan"))
    );
    assert!(report["generated"]["manifest_path"].is_null());
    assert!(
        report["generated"]["oven_output_dir"]
            .as_str()
            .is_some_and(|path| path.ends_with("target/incan/main/oven"))
    );
    assert!(report["source_files"].as_array().is_some_and(|files| {
        files.iter().any(|file| {
            file["path"].as_str().is_some_and(|path| path.ends_with("main.incn"))
                && file["module_path"]
                    .as_array()
                    .is_some_and(|segments| segments.as_slice() == [serde_json::json!("main")])
        })
    }));
    assert!(report["cargo"].is_null());
    assert!(report["oven"]["receipt_identity"].is_string());
    assert!(report["oven"]["build_unit_identity"].is_string());
    assert!(report["oven"]["plan_identity"].is_string());
    assert!(report["semantic"]["packages"].as_array().is_some());
    assert!(report["semantic"]["feature_edges"].as_array().is_some());
    assert!(report["semantic"]["providers"].as_array().is_some_and(|providers| {
        !providers.is_empty()
            && providers.iter().all(|provider| {
                provider["identity"].is_string()
                    && provider["participation"].is_string()
                    && provider["provenance"].is_object()
                    && provider["implementation_facets"].is_array()
                    && provider["backend_requirements"].is_array()
            })
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("binary") && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["timings_ms"]["total"].as_u64().is_some());
    assert!(report["notes"].as_array().is_some_and(|notes| {
        notes
            .iter()
            .any(|note| note.as_str().is_some_and(|text| text.contains("direct-rustc plan")))
    }));

    Ok(())
}

#[test]
fn build_report_output_file_describes_library_build() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "report_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub def answer() -> int:
    return 42
"#,
    )?;
    let report_path = tmp.path().join("target").join("build-report.json");
    let output = run_incan(
        tmp.path(),
        &[
            "build",
            "--lib",
            "--offline",
            "--report",
            "json",
            "--report-output",
            report_path.to_str().ok_or("report path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&output, "incan build --lib --report-output");
    assert!(
        output.stdout.is_empty(),
        "report-output should keep machine JSON out of stdout, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let report: serde_json::Value = serde_json::from_str(&fs::read_to_string(&report_path)?)?;
    assert_eq!(report["mode"], serde_json::json!("library"));
    assert_eq!(report["project"]["name"], serde_json::json!("report_lib"));
    assert_eq!(
        report["entrypoint"].as_str().map(|path| path.ends_with("src/lib.incn")),
        Some(true)
    );
    assert!(report["generated"]["cargo_target_dir"].is_null());
    assert!(
        report["generated"]["oven_output_dir"]
            .as_str()
            .is_some_and(|path| path.ends_with("target/lib/oven"))
    );
    assert!(report["cargo"].is_null());
    assert!(report["oven"]["plan_identity"].is_string());
    assert!(report["source_files"].as_array().is_some_and(|files| {
        files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with("src/lib.incn")))
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("incan_library_manifest")
                && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("rust_library_debug") && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("rust_library_release")
                && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["timings_ms"]["library_load_sources"].as_u64().is_some());
    assert!(
        report["timings_ms"]["library_collect_vocab_metadata"]
            .as_u64()
            .is_some()
    );
    assert!(report["timings_ms"]["library_prepare_total"].as_u64().is_some());
    assert!(report["timings_ms"]["oven_build"].as_u64().is_some());
    assert!(report["timings_ms"]["total"].as_u64().is_some());

    Ok(())
}

#[test]
fn hyphenated_library_package_preserves_identity_and_emits_a_valid_rust_target_issue995()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname = \"hyphenated-library\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        root.path().join("src/lib.incn"),
        "pub def answer() -> int:\n  return 42\n",
    )?;

    let output = run_incan(root.path(), &["build", "--lib"])?;
    assert_success(&output, "hyphenated library build");

    let cargo_toml = fs::read_to_string(root.path().join("target/lib/Cargo.toml"))?;
    let manifest: toml::Value = toml::from_str(&cargo_toml)?;
    assert_eq!(manifest["package"]["name"].as_str(), Some("hyphenated-library"));
    assert_eq!(manifest["lib"]["name"].as_str(), Some("hyphenated_library"));
    Ok(())
}

#[test]
fn requires_incan_allows_compatible_project_commands() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "compatible_toolchain_guard"
version = "0.1.0"
requires-incan = ">=0.6.0-0,<0.7.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan lock with compatible requires-incan");

    Ok(())
}

#[test]
fn requires_incan_rejects_project_aware_commands() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    let src_dir = project_root.join("src");
    let tests_dir = project_root.join("tests");
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        project_root.join("loaf.toml"),
        r#"[project]
name = "toolchain_guard"
version = "0.1.0"
requires-incan = ">999.0.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"def main() -> None:
  println("should not run")
"#,
    )?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import test

@test
def test_guard() -> None:
  assert True
"#,
    )?;

    let cases = vec![
        (vec!["lock"], "incan lock"),
        (vec!["build", "src/main.incn"], "incan build"),
        (vec!["run"], "incan run"),
        (vec!["test"], "incan test"),
    ];

    for (args, context) in cases {
        let output = run_incan(project_root, &args)?;
        assert_failure(&output, context);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("does not satisfy requires-incan"),
            "{context} should reject incompatible requires-incan, got:\n{stderr}"
        );
        assert!(
            stderr.contains("project.requires-incan"),
            "{context} should name the project constraint layer, got:\n{stderr}"
        );
    }

    Ok(())
}

#[test]
fn env_requires_incan_is_reported_and_enforced_for_env_run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("loaf.toml"),
        r#"[project]
name = "env_toolchain_guard"
version = "0.1.0"

[tool.incan.envs.release]
requires-incan = ">999.0.0"

[tool.incan.envs.release.scripts]
probe = ["incan", "--version"]
"#,
    )?;

    let show_output = run_incan(project_root, &["env", "show", "release"])?;
    assert_success(&show_output, "incan env show release");
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(
        show_stdout.contains("requires-incan: >999.0.0"),
        "env show should report effective constraint, got:\n{show_stdout}"
    );
    assert!(
        show_stdout.contains("unsatisfied"),
        "env show should report compatibility state, got:\n{show_stdout}"
    );

    let dry_run_output = run_incan(project_root, &["env", "run", "release", "probe", "--dry-run"])?;
    assert_success(&dry_run_output, "incan env run release probe --dry-run");
    let dry_run_stdout = String::from_utf8_lossy(&dry_run_output.stdout);
    assert!(
        dry_run_stdout.contains("active Incan:") && dry_run_stdout.contains("unsatisfied"),
        "env dry-run should surface unsatisfied compatibility without spawning, got:\n{dry_run_stdout}"
    );

    let run_output = run_incan(project_root, &["env", "run", "release", "probe"])?;
    assert_failure(&run_output, "incan env run release probe");
    let stderr = String::from_utf8_lossy(&run_output.stderr);
    assert!(
        stderr.contains("env.release.requires-incan"),
        "env run should name the env constraint layer, got:\n{stderr}"
    );

    Ok(())
}

#[test]
fn init_creates_project_scaffold_with_expected_content() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("generated_app");

    let output = run_incan(
        tmp.path(),
        &[
            "init",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--name",
            "cli_init_app",
            "--description",
            "Generated by CLI integration test",
            "--author",
            "CLI Tester <cli@example.com>",
            "--license",
            "MIT",
            "-y",
        ],
    )?;

    assert_success(&output, "incan init");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Created project 'cli_init_app'"),
        "init summary should name the created project, got:\n{stdout}"
    );

    let manifest = fs::read_to_string(project_dir.join("loaf.toml"))?;
    assert!(
        manifest.contains(r#"name = "cli_init_app""#),
        "manifest should include explicit project name"
    );
    assert!(
        manifest.contains(r#"version = "0.1.0""#),
        "manifest should include default version"
    );
    assert!(
        manifest.contains(r#"description = "Generated by CLI integration test""#),
        "manifest should include explicit description"
    );
    assert!(
        manifest.contains(r#"authors = ["CLI Tester <cli@example.com>"]"#),
        "manifest should include explicit author"
    );
    assert!(
        manifest.contains(r#"license = "MIT""#),
        "manifest should include explicit license"
    );
    assert!(
        manifest.contains(r#"main = "src/main.incn""#),
        "manifest should include main script"
    );

    let main = fs::read_to_string(project_dir.join("src").join("main.incn"))?;
    assert!(
        main.contains("Hello from cli_init_app!"),
        "starter main should use the project name"
    );
    assert!(project_dir.join("tests").join("test_main.incn").exists());
    assert!(project_dir.join("README.md").exists());
    assert!(project_dir.join(".gitignore").exists());
    Ok(())
}

#[test]
fn a_cargo_manifest_beside_a_loaf_manifest_warns_without_stopping_the_build() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_ignored_cargo_project", "")?;
    // Deliberately a Cargo manifest that would fail if anything tried to use it: rule 11 requires Oven to ignore the
    // file, not to parse it and find it acceptable.
    fs::write(
        tmp.path().join("Cargo.toml"),
        "[package]\nname = \"not-a-real-crate\"\nversion = \"0.0.0\"\n\n[dependencies]\nthis-crate-does-not-exist = \"9999\"\n",
    )?;

    // Rule 11's subject is Oven, so the warning is emitted where Oven takes authority over the project rather than at
    // manifest discovery. `incan check` never reaches that point and stays silent, which is why this drives a build.
    let assert_rule_11 = |output: &Output, context: &str| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Cargo.toml"),
            "{context}: rule 11 requires the diagnostic to name the ignored file, got:\n{stderr}"
        );
        assert!(
            stderr.contains("Cargo-compatibility mode"),
            "{context}: rule 11 requires the diagnostic to explain explicit Cargo-compatibility selection, got:\n{stderr}"
        );
        assert_eq!(
            stderr.matches("Cargo files here do not contribute").count(),
            1,
            "{context}: one command must warn once, got:\n{stderr}"
        );
    };

    let bake_output = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake_output, "incan oven bake with an ignored Cargo.toml");
    assert_rule_11(&bake_output, "oven bake");

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build with an ignored Cargo.toml");
    assert_rule_11(&build_output, "build");

    Ok(())
}

#[test]
fn tools_doctor_reports_text_and_json() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;

    let text_output = run_incan(tmp.path(), &["tools", "doctor"])?;
    assert_success(&text_output, "incan tools doctor");
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(
        text.contains("Incan tools doctor"),
        "text report should include command heading, got:\n{text}"
    );
    assert!(
        text.contains("PATH incan") && text.contains("PATH incan-lsp"),
        "text report should include PATH resolution sections, got:\n{text}"
    );
    assert!(
        text.contains("editor setup"),
        "text report should include editor recovery guidance, got:\n{text}"
    );
    assert!(
        text.contains("offline readiness"),
        "text report should include offline-readiness diagnostics, got:\n{text}"
    );
    assert!(
        text.contains("advisory local signals only"),
        "offline-readiness text should avoid guaranteeing offline success, got:\n{text}"
    );

    let json_output = run_incan(tmp.path(), &["tools", "doctor", "--format", "json"])?;
    assert_success(&json_output, "incan tools doctor --format json");
    let json: serde_json::Value = serde_json::from_slice(&json_output.stdout)?;
    assert_eq!(
        json.get("version").and_then(serde_json::Value::as_str),
        Some(env!("CARGO_PKG_VERSION"))
    );
    assert!(
        json.get("current_exe").and_then(serde_json::Value::as_str).is_some(),
        "doctor JSON should include current_exe: {json}"
    );
    assert!(
        json.pointer("/path/incan")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "doctor JSON should include path.incan: {json}"
    );
    assert!(
        json.pointer("/path/incan_lsp")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "doctor JSON should include path.incan_lsp: {json}"
    );
    assert!(
        json.pointer("/cargo_bin/incan")
            .and_then(serde_json::Value::as_object)
            .is_some(),
        "doctor JSON should include cargo_bin.incan: {json}"
    );
    assert_eq!(
        json.pointer("/editor_setup/literal_path_settings")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        json.pointer("/editor_setup/reload_after_rebuild")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        json.pointer("/offline_readiness/advisory_only")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        json.pointer("/offline_readiness/source_of_truth")
            .and_then(serde_json::Value::as_str),
        Some("Cargo and RFC 020 policy flags")
    );
    assert!(
        matches!(
            json.pointer("/offline_readiness/status")
                .and_then(serde_json::Value::as_str),
            Some("present" | "missing" | "unknown")
        ),
        "doctor JSON should include stable offline-readiness status: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/cargo/available")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        "doctor JSON should include cargo availability: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/cargo_home/source")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "doctor JSON should include effective Cargo home source: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/caches/registry_cache/exists")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        "doctor JSON should include registry cache hints: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/cargo_config/source_replacement_detected")
            .and_then(serde_json::Value::as_bool)
            .is_some(),
        "doctor JSON should include Cargo config source replacement hints: {json}"
    );
    assert!(
        json.pointer("/offline_readiness/next_steps")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|steps| !steps.is_empty()),
        "doctor JSON should include concrete next steps: {json}"
    );
    Ok(())
}

#[test]
fn tools_metadata_api_reports_checked_json() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("metadata_app");
    let main_path = write_minimal_project(&project_dir, "metadata_app", "")?;
    fs::write(
        &main_path,
        r#"
pub const LABEL = "metadata"

pub def label() -> str:
    """
    Return the label.

    Returns:
        str: Label text.
    """
    return LABEL
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "api",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_success(&output, "incan tools metadata api --format json");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(
        json.pointer("/schema_version").and_then(serde_json::Value::as_u64),
        Some(1)
    );
    assert_eq!(
        json.pointer("/package/name").and_then(serde_json::Value::as_str),
        Some("metadata_app")
    );
    assert_eq!(
        json.pointer("/package/version").and_then(serde_json::Value::as_str),
        Some("0.1.0")
    );
    assert_eq!(
        json.pointer("/modules/0/module_path/0")
            .and_then(serde_json::Value::as_str),
        Some("main")
    );
    assert!(
        json.pointer("/modules/0/declarations")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|decls| decls.len() == 2),
        "expected const and function declarations in metadata JSON: {json}"
    );
    assert_eq!(
        json.pointer("/modules/0/declarations/1/docstring_sections/summary")
            .and_then(serde_json::Value::as_str),
        Some("Return the label.")
    );
    assert_eq!(
        json.pointer("/modules/0/declarations/1/docstring_sections/returns/ty")
            .and_then(serde_json::Value::as_str),
        Some("str")
    );
    Ok(())
}

#[test]
fn tools_metadata_api_reports_docstring_drift() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("metadata_docstring_drift_app");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("loaf.toml"),
        r#"[project]
name = "metadata_docstring_drift_app"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("metrics.incn"),
        r#"
pub def avg(values: List[float]) -> float:
    """
    Return the arithmetic mean.

    Args:
        missing: Stale argument.

    Returns:
        str: Wrong return type.

    Aliases:
        MissingAvg: Stale public alias.
    """
    return 0.0
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub from crate.metrics import avg as PublicAvg
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "api",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_failure(&output, "incan tools metadata api with docstring drift");
    assert!(
        output.stdout.is_empty(),
        "metadata JSON should not be printed when docstring validation fails"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("API docstring drift for `avg`"),
        "expected docstring drift diagnostic heading, got:\n{stderr}"
    );
    assert!(
        stderr.contains("documented parameter `missing` does not exist"),
        "expected stale parameter diagnostic, got:\n{stderr}"
    );
    assert!(
        stderr.contains("documented return type `str` does not match checked return type `float`"),
        "expected return type diagnostic, got:\n{stderr}"
    );
    assert!(
        stderr.contains("documented alias `MissingAvg` does not exist"),
        "expected stale alias diagnostic, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn tools_metadata_api_reports_public_import_aliases() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("metadata_alias_app");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("loaf.toml"),
        r#"[project]
name = "metadata_alias_app"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("widgets.incn"),
        r#"
pub model Widget:
    """
    Widget contract.

    Aliases:
        PublicWidget: Re-exported package surface.
    """
    pub name: str
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub from crate.widgets import Widget as PublicWidget
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "api",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "--format",
            "json",
        ],
    )?;
    assert_success(&output, "incan tools metadata api --format json");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let declarations = json
        .pointer("/modules")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|module| module.pointer("/declarations").and_then(serde_json::Value::as_array))
        .flatten();
    let alias = declarations
        .filter(|declaration| declaration.pointer("/kind").and_then(serde_json::Value::as_str) == Some("alias"))
        .find(|declaration| declaration.pointer("/name").and_then(serde_json::Value::as_str) == Some("PublicWidget"))
        .ok_or_else(|| format!("expected PublicWidget alias declaration in metadata JSON: {json}"))?;
    assert_eq!(
        alias
            .pointer("/target_path")
            .and_then(serde_json::Value::as_array)
            .map(|segments| segments
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()),
        Some(vec!["crate", "widgets", "Widget"])
    );
    Ok(())
}

#[test]
fn tools_metadata_model_emits_project_contract_model() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_app");
    write_minimal_project(
        &project_dir,
        "contract_model_app",
        r#"
[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    write_order_summary_bundle(&project_dir)?;

    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "model",
            project_dir.to_str().ok_or("project path was not valid UTF-8")?,
            "OrderSummary",
            "--format",
            "incan",
        ],
    )?;
    assert_success(&output, "incan tools metadata model --format incan");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pub model OrderSummary:"),
        "expected emitted model, got:\n{stdout}"
    );
    assert!(
        stdout.contains("order_id [alias=\"orderId\", description=\"Stable order identifier\"]: str"),
        "expected field metadata in emitted model, got:\n{stdout}"
    );
    assert!(
        stdout.contains("coupon_code: Option[str]"),
        "expected nullable field projection, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn tools_metadata_model_materializes_project_bundle_for_run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_run_app");
    let main_path = write_minimal_project(
        &project_dir,
        "contract_model_run_app",
        r#"
[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    write_order_summary_bundle(&project_dir)?;
    fs::write(
        project_dir.join("src").join("orders.incn"),
        r#"
pub def make_order() -> OrderSummary:
    return OrderSummary(order_id="o-1", total_cents=1250, coupon_code=None)

pub def order_wire_name() -> str:
    let row = make_order()
    for info in row.__fields__():
        if info.name == "order_id":
            return str(info.wire_name)
    return ""

pub def order_description() -> str:
    let row = make_order()
    for info in row.__fields__():
        if info.name == "order_id":
            match info.description:
                Some(description) => return str(description)
                None => return ""
    return ""
"#,
    )?;
    fs::write(
        &main_path,
        r#"
from crate.orders import make_order, order_description, order_wire_name

def main() -> None:
    let row = make_order()
    println(row.order_id)
    println(order_wire_name())
    println(order_description())
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&output, "incan run with contract-backed model");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("o-1"),
        "expected materialized model value at runtime, got:\n{stdout}"
    );
    assert!(
        stdout.contains("orderId"),
        "expected RFC 021 alias reflection parity for materialized model, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Stable order identifier"),
        "expected RFC 021 description reflection parity for materialized model, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn tools_metadata_model_reads_built_library_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_lib");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("loaf.toml"),
        r#"[project]
name = "contract_model_lib"
version = "0.1.0"

[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub def ping() -> str:
    return "pong"
"#,
    )?;
    write_order_summary_bundle(&project_dir)?;

    let build_output = run_incan(&project_dir, &["build", "--lib"])?;
    assert_success(&build_output, "incan build --lib");

    let artifact_path = project_dir
        .join("target")
        .join("lib")
        .join("contract_model_lib.incnlib");
    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "model",
            artifact_path.to_str().ok_or("artifact path was not valid UTF-8")?,
            "orders.summary",
            "--format",
            "incan",
        ],
    )?;
    assert_success(&output, "incan tools metadata model from .incnlib");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pub model OrderSummary:"),
        "expected artifact-backed model, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn tools_metadata_model_reports_non_introspectable_artifact() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("contract_model_lib_without_models");
    let src_dir = project_dir.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        project_dir.join("loaf.toml"),
        r#"[project]
name = "contract_model_lib_without_models"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"
pub def ping() -> str:
    return "pong"
"#,
    )?;

    let build_output = run_incan(&project_dir, &["build", "--lib"])?;
    assert_success(&build_output, "incan build --lib without model metadata");

    let artifact_path = project_dir
        .join("target")
        .join("lib")
        .join("contract_model_lib_without_models.incnlib");
    let output = run_incan(
        tmp.path(),
        &[
            "tools",
            "metadata",
            "model",
            artifact_path.to_str().ok_or("artifact path was not valid UTF-8")?,
            "Missing",
            "--format",
            "incan",
        ],
    )?;
    assert_failure(&output, "incan tools metadata model from non-introspectable .incnlib");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not carry checked model metadata"),
        "expected non-introspectable artifact diagnostic, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn fmt_tuple_target_list_comprehension_remains_buildable() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "fmt_tuple_target_list_comp", "")?;
    fs::write(
        &main_path,
        r#"def main() -> None:
  values = ["alpha", "beta"]
  labels: list[str] = [f"{idx}:{value}" for idx, value in enumerate(values)]
"#,
    )?;

    let fmt_output = run_incan(
        tmp.path(),
        &["fmt", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&fmt_output, "incan fmt tuple-target list comprehension");

    let formatted = fs::read_to_string(&main_path)?;
    assert!(
        formatted.contains("for idx, value in enumerate(values)"),
        "formatter should keep tuple comprehension targets unparenthesized, got:\n{formatted}"
    );
    assert!(
        !formatted.contains("for (idx, value) in enumerate(values)"),
        "formatter emitted parser-invalid tuple target parentheses, got:\n{formatted}"
    );

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build after formatting tuple-target list comprehension",
    );
    Ok(())
}

/// Non-fatal warnings must reach `incan check --format json`, not only stderr (#1117).
///
/// Covers both warning classes deliberately: the parser's RFC 005 dot-notation nudge and the typechecker's
/// unreachable-code warning. A fix that threaded only typechecker warnings would leave the README's "stable
/// diagnostics" surface half-true, so the parser case is a first-class assertion here rather than an afterthought.
#[test]
fn check_json_reports_parser_and_typechecker_warnings_without_failing() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;

    // ---- Typechecker warning: unreachable code after `return` ----
    let typecheck_path = tmp.path().join("typecheck_warning.incn");
    fs::write(
        &typecheck_path,
        r#"def f() -> int:
    return 1
    println("dead code")

def main() -> None:
    println(f"{f()}")
"#,
    )?;
    let typecheck_arg = typecheck_path.to_str().ok_or("path was not valid UTF-8")?;
    let typecheck = run_incan(tmp.path(), &["check", typecheck_arg, "--format", "json"])?;
    assert_success(&typecheck, "a warning must not fail `incan check`");
    let typecheck_json = parse_json_stdout(&typecheck)?;

    assert_eq!(typecheck_json["schema_version"], serde_json::json!(2));
    assert_eq!(
        typecheck_json["ok"],
        serde_json::json!(true),
        "`ok` reports the absence of errors, so warnings must not clear it"
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["code"],
        serde_json::json!("INCAN-T0101")
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["severity"],
        serde_json::json!("warning")
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["phase"],
        serde_json::json!("typecheck")
    );
    assert_eq!(
        typecheck_json["diagnostics"][0]["origin"],
        serde_json::json!("typechecker")
    );

    // ---- Parser warning: RFC 005 `import rust.crate` dot-notation ----
    let parse_path = tmp.path().join("parse_warning.incn");
    fs::write(
        &parse_path,
        r#"import rust.chrono

def main() -> None:
    println("parser warning")
"#,
    )?;
    let parse_arg = parse_path.to_str().ok_or("path was not valid UTF-8")?;
    let parse = run_incan(tmp.path(), &["check", parse_arg, "--format", "json"])?;
    assert_success(&parse, "a parser warning must not fail `incan check`");
    let parse_json = parse_json_stdout(&parse)?;

    assert_eq!(parse_json["schema_version"], serde_json::json!(2));
    assert_eq!(parse_json["ok"], serde_json::json!(true));
    assert_eq!(parse_json["diagnostics"][0]["severity"], serde_json::json!("warning"));
    assert_eq!(parse_json["diagnostics"][0]["phase"], serde_json::json!("parse"));
    assert_eq!(parse_json["diagnostics"][0]["origin"], serde_json::json!("parser"));

    Ok(())
}

/// A file with both a warning and an error must report both in JSON, not just the error (#1117).
///
/// Warnings ride a separate field on the failure envelope precisely so this case works: folding them into the
/// error list would print them to stderr a second time, and dropping them would mean the same warning is visible
/// when a file compiles and invisible the moment anything else in it fails.
#[test]
fn check_json_reports_warnings_alongside_errors_when_typechecking_fails() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mixed.incn");
    fs::write(
        &path,
        r#"def f() -> int:
    return 1
    println("dead code")

def main() -> None:
    _ = undefined_symbol
"#,
    )?;

    let arg = path.to_str().ok_or("path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["check", arg, "--format", "json"])?;
    assert_failure(&output, "an undefined symbol must still fail `incan check`");
    let report = parse_json_stdout(&output)?;

    assert_eq!(report["schema_version"], serde_json::json!(2));
    assert_eq!(report["ok"], serde_json::json!(false), "an error must clear `ok`");

    let diagnostics = report["diagnostics"]
        .as_array()
        .ok_or("check report had no diagnostics array")?;
    let severities: Vec<&str> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic["severity"].as_str())
        .collect();
    assert!(
        severities.contains(&"error"),
        "expected the undefined-symbol error to be reported, got: {severities:?}"
    );
    assert!(
        severities.contains(&"warning"),
        "expected the unreachable-code warning to survive the failure, got: {severities:?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["code"] == serde_json::json!("INCAN-T0101")),
        "expected INCAN-T0101 in the failing report, got: {diagnostics:?}"
    );

    Ok(())
}

/// Statement tuple-unpack of a non-tuple must fail at the source language, not in generated Rust (#1132).
///
/// The regression is specifically about *where* the failure surfaces. Before this, `incan check` passed and the
/// program only failed while compiling emitted Rust, with `error[E0610]` pointing at a `__incan_tuple_unpack_*`
/// binding the user never wrote. Asserting the absence of both strings is the point: a diagnostic that merely
/// exists is not enough if the raw Rust error can still reach the user.
#[test]
fn check_rejects_statement_tuple_unpack_of_non_tuple_without_leaking_generated_rust()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("unpack.incn");
    fs::write(
        &path,
        r#"def main() -> None:
    a, b = 5
    println(f"{a} {b}")
"#,
    )?;

    let arg = path.to_str().ok_or("path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["check", arg, "--format", "json"])?;
    assert_failure(&output, "destructuring an `int` must fail `incan check`");
    let report = parse_json_stdout(&output)?;

    assert_eq!(report["ok"], serde_json::json!(false));
    let diagnostics = report["diagnostics"]
        .as_array()
        .ok_or("check report had no diagnostics array")?;
    let first = diagnostics.first().ok_or("expected at least one diagnostic")?;
    assert_eq!(first["severity"], serde_json::json!("error"));
    assert_eq!(first["phase"], serde_json::json!("typecheck"));
    assert!(
        first["message"]
            .as_str()
            .is_some_and(|message| message.contains("Cannot destructure 2 values from value of type 'int'")),
        "the diagnostic must name the resolved value type: {first}"
    );
    assert_eq!(
        first["primary_span"]["start"]["line"],
        serde_json::json!(2),
        "the span must point at the offending statement, not the file: {first}"
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("E0610"),
        "a raw rustc field-projection error must never reach the user:\n{combined}"
    );
    assert!(
        !combined.contains("__incan_tuple_unpack"),
        "a compiler-internal binding name must never reach the user:\n{combined}"
    );

    Ok(())
}
