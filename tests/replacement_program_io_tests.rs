//! Program streams must remain observable independently of successful execution and evidence publication.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate the binary produced by Cargo, with an explicit override for a recorded baseline probe.
fn incan_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_incan")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_incan")))
}

/// Build a direct replacement command with source and mutable runtime state isolated to this fixture.
fn replacement_command(directory: &Path) -> Command {
    let mut command = Command::new(incan_binary());
    command
        .current_dir(directory)
        .env("INCAN_HOME", directory.join("incan-home"))
        .args([
            "build",
            "main.incn",
            "--backend",
            "replacement",
            "--backend-fallback",
            "refuse",
        ]);
    command
}

/// A runtime failure must not suppress a program line written before the failure.
#[test]
fn replacement_stdout_survives_a_later_runtime_failure() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> None:\n    println(\"before failure\")\n    assert false\n",
    )?;
    let output = replacement_command(temporary.path()).output()?;
    assert!(!output.status.success(), "the assertion must fail");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("runtime failure"),
        "the program must reach its runtime failure, not a setup refusal: {stderr}"
    );
    assert_eq!(output.stdout, b"before failure\n");
    assert!(
        !temporary.path().join(".incan/backend/receipt.json").exists(),
        "a failed execution must not publish a successful receipt"
    );
    Ok(())
}

/// Receipt persistence cannot retrospectively hide a successfully executed program's output.
#[test]
fn replacement_stdout_survives_receipt_persistence_failure() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> None:\n    println(\"before receipt failure\")\n",
    )?;
    let receipt_path = temporary.path().join(".incan/backend/receipt.json");
    fs::create_dir_all(&receipt_path)?;
    let output = replacement_command(temporary.path()).output()?;
    assert!(!output.status.success(), "a receipt cannot replace a directory");
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("failed to publish backend-selection receipt"),
        "the program must reach receipt publication: {stderr}"
    );
    assert_eq!(output.stdout, b"before receipt failure\n");
    assert!(
        receipt_path.is_dir(),
        "the failed publication must preserve the existing directory"
    );
    Ok(())
}

/// Successful ordinary execution leaves stdout and stderr for the program, not execution-result summaries.
#[test]
fn replacement_success_keeps_program_streams_separate_from_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> int:\n  println(\"program output\")\n  return 42\n",
    )?;
    let output = replacement_command(temporary.path()).output()?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.stdout, b"program output\n");
    assert!(
        output.stderr.is_empty(),
        "successful execution metadata belongs in a report, not stderr"
    );
    Ok(())
}

/// A report aimed at program stdout is refused before execution can mix either channel's bytes.
#[test]
fn replacement_requires_a_separate_json_report_destination() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> None:\n  println(\"must not run\")\n",
    )?;
    let output = replacement_command(temporary.path())
        .args(["--report", "json"])
        .output()?;
    assert!(
        !output.status.success(),
        "ambiguous program/report stdout must fail before execution"
    );
    assert!(
        output.stdout.is_empty(),
        "the program must not run before report-channel validation"
    );
    assert!(String::from_utf8(output.stderr)?.contains("--report-output"));
    assert!(!temporary.path().join(".incan/backend/receipt.json").exists());
    Ok(())
}

/// Selecting a file for JSON evidence must not redirect program stdout to stderr.
#[test]
fn replacement_json_report_file_preserves_program_stdout() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("main.incn"),
        "def main() -> int:\n    println(\"program output\")\n    return 42\n",
    )?;
    let output = replacement_command(temporary.path())
        .args(["--report", "json", "--report-output", "report.json"])
        .output()?;
    assert!(
        output.status.success(),
        "replacement execution must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"program output\n");
    assert!(
        !String::from_utf8(output.stderr)?.contains("program output"),
        "program stdout must not be copied to stderr"
    );
    let report: serde_json::Value = serde_json::from_slice(&fs::read(temporary.path().join("report.json"))?)?;
    assert_eq!(report["status"], "success");
    assert_eq!(report["backend"]["executed_backend"], "replacement");
    assert_eq!(report["replacement_execution"]["result"], "42");
    assert!(temporary.path().join(".incan/backend/receipt.json").is_file());
    Ok(())
}
