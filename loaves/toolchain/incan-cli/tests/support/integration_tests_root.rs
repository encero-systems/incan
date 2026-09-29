use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use incan_test_support as support;

use support::{incan_command, incan_debug_binary, repo_root, strip_ansi_escapes, unique_test_project_name};

use incan_test_support::canonical_projection;

/// Read generated Rust with RFC 120 projections decoded back to the spellings the source used.
///
/// Every linker-visible Incan-origin declaration reaches generated Rust as an encoded projection, so an assertion
/// written against a source spelling can only be evaluated after decoding. Decoding preserves the generated header
/// comment; the caller compares against this text rather than the raw file.
fn read_generated_rust(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    let decoded = canonical_projection::decoded_source_spellings(&fs::read_to_string(path)?);
    Ok(canonical_projection::reformatted_after_decode(&decoded).unwrap_or(decoded))
}

use incan_frontend::module::{ExportedTypeLikeDoc, ExportedTypeLikeKind, exported_type_like_docs};
use incan_frontend::{lexer, parser, typechecker};

/// The block-docstring fixture the checkout shares between roots, read from the harness crate's fixtures at test time;
/// `loaves/compiler/incan_frontend/src/module.rs` reads the same file for GitHub #247.
fn block_docstring_public_type_like() -> Result<String, Box<dyn std::error::Error>> {
    Ok(fs::read_to_string(incan_test_support::fixture(
        "block_docstring_public_type_like.incn",
    ))?)
}

/// Helper to run full pipeline on a source file
fn compile_file(path: &Path) -> Result<(), Vec<String>> {
    let source = fs::read_to_string(path).map_err(|e| vec![e.to_string()])?;
    compile_source(&source)
}

fn compile_source(source: &str) -> Result<(), Vec<String>> {
    let tokens = lexer::lex(source).map_err(|errs| errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;

    let ast = parser::parse(&tokens).map_err(|errs| errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;

    typechecker::check(&ast).map_err(|errs| errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>())?;

    Ok(())
}

/// Parse JSON log records from stdout that may also contain human logging or ordinary print lines.
fn parse_json_log_records(stdout: &str) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    stdout
        .lines()
        .filter(|line| line.trim_start().starts_with('{'))
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

/// Find a JSON logging record by its string body.
fn json_record_by_body<'a>(records: &'a [serde_json::Value], body: &str) -> Option<&'a serde_json::Value> {
    records
        .iter()
        .find(|record| record["Body"]["StringValue"] == serde_json::json!(body))
}

/// Create a minimal throwaway Incan project for end-to-end runtime error assertions.
fn write_runtime_error_project(source: &str) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("runtime_error_contract");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(&main_path, source)?;
    Ok((tmp, main_path))
}

/// Assert a runtime failure exposes a canonical Incan diagnostic without Rust panic leakage.
fn assert_runtime_error_output(run_output: &std::process::Output, kind: &str, detail_markers: &[&str]) {
    assert!(
        !run_output.status.success(),
        "expected runtime failure, stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );

    let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&run_output.stdout));
    let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&run_output.stderr));
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.contains(kind),
        "expected `{kind}` in runtime diagnostic, got:\n{combined}"
    );
    for marker in detail_markers {
        assert!(
            combined.contains(marker),
            "expected runtime diagnostic to contain `{marker}`, got:\n{combined}"
        );
    }
    for forbidden in ["panicked at", "thread 'main'", ".rs:"] {
        assert!(
            !combined.contains(forbidden),
            "expected runtime diagnostic to avoid raw Rust leakage `{forbidden}`, got:\n{combined}"
        );
    }
}

/// Assert that a program compiles successfully but fails at runtime with a canonical Incan diagnostic.
///
/// This helper intentionally checks the CLI surface rather than internal helper text so regressions in generated-main
/// panic formatting or subprocess execution still fail the contract.
fn assert_runtime_error_cli(
    source: &str,
    kind: &str,
    detail_markers: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let (_tmp, main_path) = write_runtime_error_project(source)?;

    let run_output = incan_command()
        .arg("run")
        .arg(&main_path)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert_runtime_error_output(&run_output, kind, detail_markers);

    Ok(())
}

/// Resolve one exact immutable compiled SDK provider selected for a generated consumer.
fn compiled_sdk_provider_artifact_root(
    generated_project: &Path,
    crate_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cargo_toml = fs::read_to_string(generated_project.join("Cargo.toml"))?;
    let manifest: toml::Value = toml::from_str(&cargo_toml)?;
    let path = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get(crate_name))
        .and_then(|dependency| dependency.get("path"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| format!("generated consumer did not select compiled SDK provider `{crate_name}`"))?;
    let path = PathBuf::from(path);
    Ok(if path.is_absolute() {
        path
    } else {
        generated_project.join(path)
    })
}

fn run_incan_command_with_timeout(
    mut command: Command,
    timeout: std::time::Duration,
) -> std::io::Result<(std::process::Output, bool)> {
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let start = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output().map(|output| (output, false));
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            return child.wait_with_output().map(|output| (output, true));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn is_incan_fixture(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("incn") | Some("incan"))
}

/// Make a temporary test directory to be able to run the CLI tests.
fn make_temp_test_dir() -> std::path::PathBuf {
    let mut dir = std::env::temp_dir();
    let uniq = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    dir.push(format!("incan_cli_test_{}", uniq));
    let Ok(()) = std::fs::create_dir_all(&dir) else {
        panic!("failed to create temp test dir");
    };
    dir
}

fn write_cycle_explicit_call_site_generics_project(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir)?;
    std::fs::write(
        dir.join("loaf.toml"),
        r#"[project]
name = "cycle_explicit_call_site_generics"
version = "0.1.0"
"#,
    )?;
    std::fs::write(
        src_dir.join("dataset.incn"),
        r#"from session import collect_with_active_session

pub model DataSet[T]:
  pub value: T

pub def collect_with_dataset[T](dataset: DataSet[T]) -> T:
  return collect_with_active_session[T](dataset)
"#,
    )?;
    std::fs::write(
        src_dir.join("session.incn"),
        r#"from dataset import DataSet

pub def collect_with_active_session[T](dataset: DataSet[T]) -> T:
  return dataset.value
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    std::fs::write(
        &main_path,
        r#"from dataset import DataSet, collect_with_dataset

def main() -> None:
  let ds = DataSet(value=1)
  println(collect_with_dataset[int](ds))
"#,
    )?;
    Ok(main_path)
}
