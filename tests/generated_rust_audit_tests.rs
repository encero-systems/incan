use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn audit_script() -> PathBuf {
    repo_root().join("scripts/generated_rust_audit.py")
}

fn fixture_path(relative: &str) -> PathBuf {
    repo_root().join("tests/fixtures/generated_rust_audit").join(relative)
}

fn run_audit(args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new("python3")
        .arg(audit_script())
        .args(args)
        .current_dir(repo_root())
        .output()?)
}

fn path_string(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(path
        .to_str()
        .ok_or_else(|| format!("path was not valid UTF-8: {}", path.display()))?
        .to_owned())
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output, context: &str) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn artifact<'a>(
    json: &'a serde_json::Value,
    surface_class: &str,
) -> Result<&'a serde_json::Value, Box<dyn std::error::Error>> {
    json["artifacts"]
        .as_array()
        .ok_or("report artifacts field was not an array")?
        .iter()
        .find(|artifact| artifact["surface_class"].as_str() == Some(surface_class))
        .ok_or_else(|| format!("missing artifact for surface class `{surface_class}`").into())
}

#[test]
fn json_report_records_explicit_artifacts_and_marker_counts() -> Result<(), Box<dyn std::error::Error>> {
    let file_fixture = fixture_path("main.rs");
    let dir_fixture = fixture_path("nested");
    let file_spec = format!("program-main={}", path_string(&file_fixture)?);
    let dir_spec = format!("stdlib-copy={}", path_string(&dir_fixture)?);

    let output = run_audit(&["--format", "json", "--artifact", &file_spec, "--artifact", &dir_spec])?;
    assert_success(&output, "generated Rust audit JSON report");

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["report"], "generated-rust-strict-surface");
    assert!(json["generated_at"].as_str().is_some());

    let program = artifact(&json, "program-main")?;
    assert_eq!(program["artifact_path"], "tests/fixtures/generated_rust_audit/main.rs");
    assert_eq!(program["check_status"], "present");
    assert_eq!(program["strictness_status"], "available_for_review");
    assert_eq!(
        program["rust_files"],
        serde_json::json!(["tests/fixtures/generated_rust_audit/main.rs"])
    );
    assert_eq!(program["clone"]["status"], "pending_manual_review");
    assert_eq!(program["clone"]["marker_count"], 3);
    assert_eq!(program["allocation"]["marker_count"], 3);
    assert_eq!(program["eager_collection"]["marker_count"], 1);
    assert_eq!(program["clone"]["markers"][0]["line"], 3);
    assert_eq!(program["clone"]["markers"][0]["pattern"], ".clone(");

    let stdlib = artifact(&json, "stdlib-copy")?;
    assert_eq!(stdlib["artifact_path"], "tests/fixtures/generated_rust_audit/nested");
    assert_eq!(
        stdlib["rust_files"],
        serde_json::json!(["tests/fixtures/generated_rust_audit/nested/lib.rs"])
    );
    assert_eq!(stdlib["clone"]["marker_count"], 1);
    assert_eq!(stdlib["allocation"]["marker_count"], 2);
    assert_eq!(stdlib["eager_collection"]["marker_count"], 1);

    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        !stdout.contains(repo_root().to_str().ok_or("repo root was not valid UTF-8")?),
        "repo-contained artifact paths should be rendered relative to the repository root:\n{stdout}"
    );

    Ok(())
}

#[test]
fn markdown_report_contains_objective_rows_and_details() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = fixture_path("main.rs");
    let spec = format!("program-main={}", path_string(&fixture)?);
    let output = run_audit(&["--artifact", &spec])?;
    assert_success(&output, "generated Rust audit Markdown report");

    let markdown = String::from_utf8(output.stdout)?;
    assert!(markdown.contains("# Generated Rust Strict Surface Report"));
    assert!(markdown.contains("| `program-main` | `tests/fixtures/generated_rust_audit/main.rs` | present | available_for_review | 3 marker(s); notes: pending | 3 marker(s); notes: pending | 1 marker(s); notes: pending |"));
    assert!(markdown.contains("## Artifact Details"));
    assert!(markdown.contains("- Clone notes: status=`pending_manual_review`, markers=3, notes=pending"));
    assert!(!markdown.contains("score"));
    assert!(
        !markdown.contains(repo_root().to_str().ok_or("repo root was not valid UTF-8")?),
        "Markdown report should not leak absolute repo paths for repo-contained artifacts:\n{markdown}"
    );

    Ok(())
}

#[test]
fn missing_and_no_rust_artifacts_are_reported_without_failing_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let missing_spec = "missing-surface=tests/fixtures/generated_rust_audit/missing.rs";
    let no_rust_spec = "notes-only=tests/fixtures/generated_rust_audit/no_rust_dir";

    let output = run_audit(&[
        "--format",
        "json",
        "--artifact",
        missing_spec,
        "--artifact",
        no_rust_spec,
    ])?;
    assert_success(&output, "generated Rust audit missing artifact report");

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let missing = artifact(&json, "missing-surface")?;
    assert_eq!(
        missing["artifact_path"],
        "tests/fixtures/generated_rust_audit/missing.rs"
    );
    assert_eq!(missing["check_status"], "missing");
    assert_eq!(missing["strictness_status"], "not_evaluated");
    assert_eq!(missing["clone"]["status"], "not_available");
    assert_eq!(missing["clone"]["marker_count"], 0);
    assert_eq!(missing["message"], "artifact path does not exist");

    let notes_only = artifact(&json, "notes-only")?;
    assert_eq!(notes_only["check_status"], "no-rust-files");
    assert_eq!(notes_only["strictness_status"], "not_evaluated");
    assert_eq!(
        notes_only["message"],
        "artifact directory contains no Rust source files"
    );

    Ok(())
}

#[test]
fn fail_on_missing_exits_nonzero_after_emitting_report() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_audit(&[
        "--format",
        "json",
        "--fail-on-missing",
        "--artifact",
        "missing-surface=tests/fixtures/generated_rust_audit/missing.rs",
    ])?;
    assert_failure(&output, "generated Rust audit --fail-on-missing");

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let missing = artifact(&json, "missing-surface")?;
    assert_eq!(missing["check_status"], "missing");
    assert!(output.stderr.is_empty());

    Ok(())
}
