//! The emitter freeze gate from the crate's own test suite.
//!
//! `scripts/check_emitter_freeze.py` fingerprints every `.rs` file under `src/emit/` and compares it with the
//! manifest at `tests/fixtures/emitter_freeze/manifest.json`; `make pre-commit-fast` and CI run it directly. This root
//! runs the same script so `cargo test -p incan_emit` catches drift too, and proves on a scratch copy that a record
//! without its migration note is refused.

use incan_test_support as support;
use support::repo_root;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

const MANIFEST: &str = "loaves/compiler/incan_emit/tests/fixtures/emitter_freeze/manifest.json";

/// Run the gate from the checkout root with `args`, so relative paths in its output match the committed layout.
///
/// A spawn failure names the interpreter and the script, so a machine without `python3` on `PATH` reads the cause
/// rather than a bare `NotFound`.
fn run_gate(args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    let root = repo_root();
    let script = root.join("scripts/check_emitter_freeze.py");
    Ok(Command::new("python3")
        .arg(&script)
        .args(args)
        .current_dir(&root)
        .output()
        .map_err(|error| format!("could not run `python3 {}`: {error}", script.display()))?)
}

/// The script's stdout and stderr, for assertion messages that show what the gate said.
fn transcript(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A path as the script's command line needs it.
fn path_string(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    Ok(path
        .to_str()
        .ok_or_else(|| format!("path was not valid UTF-8: {}", path.display()))?
        .to_owned())
}

#[test]
fn emitter_tree_matches_the_freeze_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let output = run_gate(&[])?;
    assert!(
        output.status.success(),
        "the frozen emitter tree drifted from {MANIFEST}; the gate said:\n{}",
        transcript(&output)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("emitter freeze gate passed"),
        "unexpected gate output:\n{}",
        transcript(&output)
    );
    Ok(())
}

#[test]
fn record_without_a_migration_note_is_refused() -> Result<(), Box<dyn std::error::Error>> {
    // A scratch copy of the manifest: the committed one must never gain an entry from a test run.
    let scratch = tempfile::tempdir()?;
    let manifest = scratch.path().join("manifest.json");
    fs::copy(repo_root().join(MANIFEST), &manifest)?;
    let before = fs::read_to_string(&manifest)?;

    let output = run_gate(&[
        "--manifest",
        &path_string(&manifest)?,
        "--record",
        "--policy",
        "deletions-only",
    ])?;
    assert!(
        !output.status.success(),
        "a record without the four note fields must be refused; the gate said:\n{}",
        transcript(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--record refused, the migration note is incomplete"),
        "unexpected refusal text:\n{}",
        transcript(&output)
    );
    assert!(
        stdout.contains("Missing: --issue, --evidence, --owner, --retirement"),
        "the refusal must name every missing field:\n{}",
        transcript(&output)
    );
    assert_eq!(
        fs::read_to_string(&manifest)?,
        before,
        "a refused record must leave the manifest untouched"
    );
    Ok(())
}
