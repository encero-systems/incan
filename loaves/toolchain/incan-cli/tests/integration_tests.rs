//! Integration tests for the Incan compiler frontend

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

/// Shared with `src/frontend/module.rs` tests (`exported_type_like_docs`) for GitHub #247.
/// The block-docstring fixture the checkout shares between roots, read from the harness crate's fixtures at test time.
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

#[test]
fn bare_incan_run_uses_project_main_script() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "bare_run_project"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"def main() -> None:
  println("bare run works")
"#,
    )?;

    let output = incan_command()
        .arg("run")
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected bare `incan run` to succeed from project root.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("bare run works"),
        "expected bare `incan run` to execute [project.scripts].main, got:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    Ok(())
}

/// Issue #1116: real module bindings, including explicit imports, win over ambient core builtin functions; authors
/// can still select a core builtin through `std.builtins`.
#[test]
fn builtin_function_shadowing_is_lexical_and_runtime_visible_issue1116() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("builtin_shadowing_contract");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    fs::write(
        src_dir.join("aggregates.incn"),
        r#"pub def sum(value: int) -> int:
  return value + 1
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from aggregates import sum

def len(value: int) -> int:
  return value + 1

def main() -> None:
  println(len(4))
  println(sum(41))
  println(std.builtins.len([10, 20, 30]))
"#,
    )?;

    let out_dir = tmp.path().join("out");
    let oven_home = tmp.path().join("incan-home");
    let mut bake_command = incan_command();
    bake_command
        .args(["oven", "bake", "--project", "."])
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", &oven_home);
    support::configure_explicit_oven_bake_command(&mut bake_command)?;
    let bake_output = bake_command.output()?;
    assert!(
        bake_output.status.success(),
        "expected explicit Oven bake to prepare the builtin-shadowing contract project.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bake_output.stdout),
        String::from_utf8_lossy(&bake_output.stderr)
    );

    let build_output = incan_command()
        .args([
            "build",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", &oven_home)
        .output()?;
    assert!(
        build_output.status.success(),
        "expected builtin-shadowing contract project to build.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    let binary = out_dir.join("oven/release").join(&project_name);
    assert!(
        binary.is_file(),
        "expected Oven to produce the builtin-shadowing executable at {}",
        binary.display()
    );
    let run_output = Command::new(&binary).output()?;
    assert!(
        run_output.status.success(),
        "expected builtin-shadowing executable to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    assert_eq!(
        String::from_utf8(run_output.stdout)?.lines().collect::<Vec<_>>(),
        vec!["5", "42", "3"],
        "module and imported bindings must win, while `std.builtins` must select the core builtin"
    );
    Ok(())
}

#[test]
fn build_explicit_mutable_rust_generic_reaches_codegen_through_normal_cli_path()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("explicit_mutable_rust_generic");
    let src_dir = tmp.path().join("src");
    let out_dir = tmp.path().join("out");
    let oven_home = tmp.path().join("oven-home");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!(
            r#"[project]
name = "{project_name}"
version = "0.1.0"
"#
        ),
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from rust::std::vec import Vec as ProviderHandle

def replace_items(mut items: ProviderHandle[tuple[&mut int, &mut int]]) -> None:
  pass

def main() -> None:
  replace_items(ProviderHandle.new())
  println("explicit mutable Rust generic CLI path")
"#,
    )?;

    let mut bake_command = incan_command();
    bake_command
        .args(["oven", "bake", "--project", "."])
        .current_dir(tmp.path())
        .env("INCAN_HOME", &oven_home);
    support::configure_explicit_oven_bake_command(&mut bake_command)?;
    let bake_output = bake_command.output()?;
    assert!(
        bake_output.status.success(),
        "explicit bake for the mutable Rust generic failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        bake_output.status,
        String::from_utf8_lossy(&bake_output.stdout),
        String::from_utf8_lossy(&bake_output.stderr)
    );

    let output = incan_command()
        .args([
            "build",
            "--locked",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(tmp.path())
        .env("INCAN_HOME", &oven_home)
        .output()?;
    assert!(
        output.status.success(),
        "normal build with explicit mutable Rust generic failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let generated = read_generated_rust(&out_dir.join("src/main.rs"))?;
    assert!(
        generated.contains("fn replace_items(_: ProviderHandle<(&mut i64, &mut i64)>)"),
        "normal CLI codegen must preserve explicit mutable Rust references through an alias, got:\n{generated}"
    );
    assert!(
        generated.contains("replace_items(ProviderHandle::new())"),
        "the caller must compile against the projected callee ABI, got:\n{generated}"
    );
    Ok(())
}

#[test]
fn decorated_method_explicit_mutable_rust_generic_keeps_static_and_wrapper_abi()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("decorated_explicit_mutable_rust_generic");
    let src_dir = tmp.path().join("src");
    let out_dir = tmp.path().join("out");
    let oven_home = tmp.path().join("oven-home");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!(
            r#"[project]
name = "{project_name}"
version = "0.1.0"
"#
        ),
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def preserve[F]() -> ((F) -> F):
  return (function) => function

from rust::std::vec import Vec as ProviderHandle

def adapted(value: int) -> int:
  return value

def to_int(function: (ProviderHandle[tuple[&mut int, &mut int]]) -> int) -> ((int) -> int):
  return adapted

class Container:
  @preserve()
  def replace(self, mut items: ProviderHandle[tuple[&mut int, &mut int]]) -> None:
    pass

@to_int
def transformed(mut items: ProviderHandle[tuple[&mut int, &mut int]]) -> int:
  return 0

def main() -> None:
  Container().replace(ProviderHandle.new())
  println(transformed(7))
  println("decorated explicit mutable Rust generic CLI path")
"#,
    )?;

    let mut bake_command = incan_command();
    bake_command
        .args(["oven", "bake", "--project", "."])
        .current_dir(tmp.path())
        .env("INCAN_HOME", &oven_home);
    support::configure_explicit_oven_bake_command(&mut bake_command)?;
    let bake_output = bake_command.output()?;
    assert!(
        bake_output.status.success(),
        "explicit bake for the decorated mutable Rust generic failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        bake_output.status,
        String::from_utf8_lossy(&bake_output.stdout),
        String::from_utf8_lossy(&bake_output.stderr)
    );

    let output = incan_command()
        .args([
            "build",
            "--locked",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(tmp.path())
        .env("INCAN_HOME", &oven_home)
        .output()?;
    assert!(
        output.status.success(),
        "normal decorated-method build with explicit mutable Rust generic failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let generated = read_generated_rust(&out_dir.join("src/main.rs"))?;
    let projected = "ProviderHandle<(&mut i64, &mut i64)>";
    assert!(
        generated.match_indices(projected).count() >= 3,
        "the decorated method, its static binding, and wrapper must share the explicit Rust-reference ABI, got:\n{generated}"
    );
    assert!(
        !generated.contains("ProviderHandle<(i64, i64)>"),
        "no decorated-method adapter may erase an explicit Rust-reference ABI, got:\n{generated}"
    );
    assert!(
        generated.contains("fn transformed(__incan_arg_0: i64) -> i64"),
        "a decorator-changed callable surface must remain checker-authoritative instead of inheriting the original Rust parameter, got:\n{generated}"
    );
    assert!(
        generated.contains("fn __incan_original_transformed(_: ProviderHandle<(&mut i64, &mut i64)>)"),
        "the original function must retain its explicit Rust-reference ABI, got:\n{generated}"
    );
    Ok(())
}

#[test]
fn build_rejects_unbound_type_annotation_before_generated_rust_issue902() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let cases = [("simple", "Count", "Count"), ("qualified", "missing::Count", "missing")];

    for (case_name, annotation, expected_symbol) in cases {
        let project_name = format!("unbound_type_annotation_{case_name}");
        let project_root = tmp.path().join(case_name);
        let src_dir = project_root.join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(
            project_root.join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        let main_path = src_dir.join("main.incn");
        fs::write(
            &main_path,
            format!(
                r#"def accepts(value: {annotation}) -> {annotation}:
  return value

def main() -> None:
  print(accepts(7))
"#
            ),
        )?;

        let output = incan_command()
            .args(["build", main_path.to_string_lossy().as_ref(), "--no-locked"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        let generated_project = project_root.join("target/incan").join(&project_name);

        assert!(
            !output.status.success(),
            "expected the {case_name} unbound annotation to fail"
        );
        assert!(
            stderr.contains(&format!("Unknown symbol '{expected_symbol}'")),
            "expected an Incan diagnostic for {case_name}, got:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            !stdout.contains("Generated Rust project in:")
                && !stdout.contains("Building...")
                && !stderr.contains("E0425")
                && !stderr.contains("E0433")
                && !generated_project.exists(),
            "the {case_name} annotation must fail before generated Rust is published:\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }

    Ok(())
}

#[test]
fn std_logging_runtime_surfaces_share_one_generated_run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("std_logging_runtime_surfaces");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    fs::write(
        src_dir.join("worker.incn"),
        r#"from std.logging import get_logger

pub def run_get_logger_worker() -> None:
  log = get_logger()
  log.info("worker ready")

pub def run_ambient_worker() -> None:
  log.info("worker ambient log ready")
"#,
    )?;
    let source = r#"from std.logging import ColorPolicy, Level, LogFormat, LogStyle, LoggerName, OutputTarget, basic_config, get_logger
from std.telemetry.core import TelemetryValue
from worker import run_ambient_worker, run_get_logger_worker

model LocalLog:
  def info(self, message: str) -> None:
    println(f"local:{message}")

def logger_context_case() -> None:
  basic_config(level=Level.WARNING, style=LogStyle.VERBOSE, color=ColorPolicy.NEVER, target="stdout")
  root = get_logger("app").bind({"shared": "root"})
  child = root.child("loader").bind({"component": "loader"})

  root.info("silent info")
  if root.is_enabled(Level.INFO):
    println("unexpected info enabled")
  if not child.is_enabled(Level.ERROR):
    println("unexpected error disabled")

  root.error("root event")
  child.warning("child event", fields={"shared": "event"})

def json_record_shape_case() -> None:
  basic_config(level=Level.DEBUG, format=LogFormat.JSON, target="stdout")
  log = get_logger()
  log.debug("json works", fields={"request_id": "abc", "component": "loader"})

def default_target_case() -> None:
  basic_config(level=Level.INFO)
  get_logger("app").info("stderr event")

def shadow_case() -> None:
  basic_config(level=Level.INFO, format=LogFormat.JSON, target="stdout")
  log = LocalLog()
  log.info("shadowed")

def ambient_root_case() -> None:
  basic_config(level=Level.INFO, format=LogFormat.JSON, target="stdout")
  log.info("snippet ambient")

def structured_fields_case() -> None:
  basic_config(level=Level.INFO, format=LogFormat.JSON, target="stdout")
  log.info("structured", fields={
    "rows": 42,
    "ok": true,
    "ratio": 1.5,
    "missing": None,
    "items": TelemetryValue.array([TelemetryValue.int(1), TelemetryValue.bool(false)]),
    "nested": TelemetryValue.map({"child": TelemetryValue.string("yes")}),
  })

def telemetry_constructor_case() -> None:
  text = TelemetryValue.string("alpha")
  payload = TelemetryValue.map({
    "items": TelemetryValue.array([TelemetryValue.int(42), TelemetryValue.bool(true)]),
    "empty": TelemetryValue.none(),
    "encoded": TelemetryValue.bytes("ff"),
    "ratio": TelemetryValue.float(1.5),
  })
  println(f"telemetry:{text.display_text()}")
  println(f"telemetry:{payload.display_text()}")

def validator_case() -> None:
  match LoggerName.from_underlying(""):
    Ok(_) => println("unexpected accepted empty logger name")
    Err(err) => println(f"validation:empty_logger:{err.to_string()}")
  match LoggerName.from_underlying(".app"):
    Ok(_) => println("unexpected accepted edge logger name")
    Err(err) => println(f"validation:edge_logger:{err.to_string()}")
  match LoggerName.from_underlying("app..db"):
    Ok(_) => println("unexpected accepted segmented logger name")
    Err(err) => println(f"validation:segmented_logger:{err.to_string()}")
  match OutputTarget.from_underlying("bogus"):
    Ok(_) => println("unexpected accepted output target")
    Err(err) => println(f"validation:output_target:{err.to_string()}")

def human_styles_case() -> None:
  basic_config(level=Level.INFO, style=LogStyle.MINIMAL, target="stdout")
  get_logger("app").info("minimal event")
  basic_config(level=Level.INFO, style=LogStyle.SHORT, target="stdout")
  get_logger("app").info("short event")
  basic_config(level=Level.INFO, style=LogStyle.COMPLETE, target="stdout")
  get_logger("app").info("complete event")
  basic_config(level=Level.INFO, style=LogStyle.VERBOSE, target="stdout")
  get_logger("app").info("verbose event")
  run_get_logger_worker()
  run_ambient_worker()

def main() -> None:
  logger_context_case()
  json_record_shape_case()
  default_target_case()
  shadow_case()
  ambient_root_case()
  structured_fields_case()
  telemetry_constructor_case()
  validator_case()
  human_styles_case()
"#;
    let main_path = src_dir.join("main.incn");
    fs::write(&main_path, source)?;

    let output = incan_command()
        .args(["run", main_path.to_string_lossy().as_ref()])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected combined std.logging source surface run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("silent info"),
        "expected INFO event to be filtered by source basic_config, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("unexpected"),
        "expected is_enabled filtering checks to pass, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[ERROR] root event") && stdout.contains(r#"shared="root""#) && stdout.contains("logger=app"),
        "expected root logger context to remain unmodified, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[WARNING] child event")
            && stdout.contains(r#"component="loader""#)
            && stdout.contains(r#"shared="event""#),
        "expected child logger bound fields and event override, got:\n{stdout}"
    );
    assert!(
        stdout.contains("logger=app.loader"),
        "expected child logger name, got:\n{stdout}"
    );
    assert!(
        !stdout.contains("stderr event") && stderr.contains("stderr event"),
        "expected default logging target to route the event to stderr.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("local:shadowed") && !stdout.contains(r#""Body":{"Type":"string","StringValue":"shadowed"}"#),
        "expected local log binding to remain ordinary source, got:\n{stdout}"
    );
    for expected in [
        "validation:empty_logger:std.logging logger names must not be empty",
        "validation:edge_logger:std.logging logger names must not start or end with '.'",
        "validation:segmented_logger:std.logging logger names must not contain empty segments",
        "validation:output_target:std.logging target must be 'stdout' or 'stderr'",
    ] {
        assert!(stdout.contains(expected), "expected `{expected}`, got:\n{stdout}");
    }
    assert!(
        !stdout.contains("unexpected accepted"),
        "expected std.logging validators to reject invalid values, got:\n{stdout}"
    );

    let records = parse_json_log_records(&stdout)?;
    let record = json_record_by_body(&records, "json works")
        .ok_or_else(|| std::io::Error::other(format!("missing `json works` record in:\n{stdout}")))?;
    assert_eq!(record["SeverityText"], serde_json::json!("DEBUG"));
    assert_eq!(record["SeverityNumber"], serde_json::json!(5));
    assert_eq!(record["InstrumentationScope"]["Name"], serde_json::json!("main"));
    assert_eq!(record["Body"]["Type"], serde_json::json!("string"));
    assert_eq!(record["Attributes"]["request_id"]["Type"], serde_json::json!("string"));
    assert_eq!(
        record["Attributes"]["request_id"]["StringValue"],
        serde_json::json!("abc")
    );
    assert_eq!(record["Attributes"]["component"]["Type"], serde_json::json!("string"));
    assert_eq!(
        record["Attributes"]["component"]["StringValue"],
        serde_json::json!("loader")
    );
    assert_eq!(record["Resource"]["Attributes"], serde_json::json!({}));
    assert!(
        record.get("request_id").is_none() && record.get("component").is_none(),
        "expected user fields to stay under Attributes, got:\n{record}"
    );

    let ambient = json_record_by_body(&records, "snippet ambient")
        .ok_or_else(|| std::io::Error::other(format!("missing `snippet ambient` record in:\n{stdout}")))?;
    assert_eq!(ambient["InstrumentationScope"]["Name"], serde_json::json!("main"));

    let structured = json_record_by_body(&records, "structured")
        .ok_or_else(|| std::io::Error::other(format!("missing `structured` record in:\n{stdout}")))?;
    let attributes = &structured["Attributes"];
    assert_eq!(attributes["rows"]["Type"], serde_json::json!("int"));
    assert_eq!(attributes["rows"]["IntValue"], serde_json::json!(42));
    assert_eq!(attributes["ok"]["Type"], serde_json::json!("bool"));
    assert_eq!(attributes["ok"]["BoolValue"], serde_json::json!(true));
    assert_eq!(attributes["ratio"]["Type"], serde_json::json!("float"));
    assert_eq!(attributes["ratio"]["FloatValue"], serde_json::json!(1.5));
    assert_eq!(attributes["missing"]["Type"], serde_json::json!("none"));
    assert_eq!(attributes["items"]["Type"], serde_json::json!("array"));
    assert_eq!(attributes["nested"]["Type"], serde_json::json!("map"));
    assert!(
        structured.get("rows").is_none() && structured.get("nested").is_none(),
        "expected structured fields to stay under Attributes, got:\n{structured}"
    );

    let log_lines: Vec<&str> = stdout.lines().filter(|line| line.contains("[INFO]")).collect();
    let short_line = log_lines
        .iter()
        .copied()
        .find(|line| line.contains("short event"))
        .unwrap_or("");
    let complete_line = log_lines
        .iter()
        .copied()
        .find(|line| line.contains("complete event"))
        .unwrap_or("");

    assert!(
        stdout.contains("[INFO] minimal event"),
        "expected minimal line, got:\n{stdout}"
    );
    assert_eq!(
        short_line.find(" [INFO] short event"),
        Some(8),
        "expected short style to use compact time-of-day timestamp, got:\n{stdout}"
    );
    assert!(
        complete_line.contains('T') && complete_line.contains("Z [INFO] complete event"),
        "expected complete style to use full datetime timestamp, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[INFO] verbose event\n  logger=app"),
        "expected verbose style to add logger metadata on a second line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("telemetry:alpha")
            && stdout.contains(r#""Type":"map""#)
            && stdout.contains(r#""items":{"Type":"array""#)
            && stdout.contains(r#""IntValue":42"#)
            && stdout.contains(r#""BoolValue":true"#)
            && stdout.contains(r#""BytesValue":"ff""#)
            && stdout.contains(r#""FloatValue":1.5"#),
        "expected telemetry value constructors to preserve structured values, got:\n{stdout}"
    );
    assert!(
        stdout.contains("worker ready")
            && stdout.contains("worker ambient log ready")
            && stdout.contains("logger=worker")
            && !stdout.contains("logger=std.logging"),
        "expected worker module logging to infer logger=worker, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn validated_newtype_runtime_scenarios() -> Result<(), Box<dyn std::error::Error>> {
    let output = incan_command()
        .args([
            "run",
            "-c",
            r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Self, ValidationError]:
    if n <= 0:
      return Err(ValidationError("attempts must be >= 1"))
    return Ok(Attempts(n))

def retry(attempts: Attempts) -> None:
  println(f"retry={attempts.0}")

def main() -> None:
  retry(3)
  attempts: Attempts = 4
  println(f"local={attempts.0}")
"#,
        ])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "validated-newtype success program failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("retry=3"), "unexpected stdout:\n{stdout}");
    assert!(stdout.contains("local=4"), "unexpected stdout:\n{stdout}");

    assert_runtime_error_cli(
        r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Self, ValidationError]:
    if n <= 0:
      return Err(ValidationError("attempts must be >= 1"))
    return Ok(Attempts(n))

def retry(attempts: Attempts) -> None:
  return

def read_attempts(attempts: Attempts) -> int:
  return attempts.0

def main() -> None:
  println(f"ok={read_attempts(Attempts(1))}")
  retry(0)
"#,
        "ValidationError",
        &["Attempts::from_underlying", "attempts must be >= 1"],
    )?;

    assert_runtime_error_cli(
        r#"
type PositiveInt = newtype int:
  def from_underlying(n: int) -> Result[Self, ValidationError]:
    if n <= 0:
      return Err(ValidationError("positive int must be greater than zero"))
    return Ok(PositiveInt(n))

model Bounds:
  low: PositiveInt
  high: PositiveInt

def width(bounds: Bounds) -> int:
  return bounds.high.0 - bounds.low.0

def main() -> None:
  println(f"width={width(Bounds(low=1, high=2))}")
  _ = Bounds(low=0, high=-1)
"#,
        "ValidationError",
        &[
            "Bounds validation failed with 2 error(s)",
            "low: positive int must be greater than zero",
            "high: positive int must be greater than zero",
        ],
    )?;

    Ok(())
}

#[test]
fn validated_newtype_json_deserialization_runtime() -> Result<(), Box<dyn std::error::Error>> {
    let output = incan_command()
        .args([
            "run",
            "-c",
            r#"
from std.serde import json
from std.serde.json import Deserialize

@derive(Clone, json)
type ShortId = newtype str:
  def from_underlying(value: str) -> Result[Self, ValidationError]:
    if len(value) > 8:
      return Err(ValidationError("identifier too long"))
    return Ok(ShortId(value))

type PositiveInt = newtype int[gt=0]

@derive(Clone, Deserialize)
type CheckedBox[D] = newtype D:
  def from_underlying(value: D) -> Result[Self, ValidationError]:
    return Ok(CheckedBox[D](value))

@derive(json)
model Envelope:
  id: ShortId

@derive(Deserialize)
model IdList:
  ids: list[ShortId]

@derive(Deserialize)
model OptionalId:
  id: Option[ShortId]

@derive(Deserialize)
model PositiveEnvelope:
  value: PositiveInt

@derive(Deserialize)
model GenericEnvelope:
  value: CheckedBox[str]

def main() -> None:
  match Envelope.from_json('{"id":"short"}'):
    case Ok(value):
      println(f"model_roundtrip:{value.to_json()}")
    case Err(_):
      println("model_valid_rejected")
  match Envelope.from_json('{"id":"identifier_too_long"}'):
    case Ok(_):
      println("model_invalid_accepted")
    case Err(_):
      println("model_invalid_rejected")
  match IdList.from_json('{"ids":["short","identifier_too_long"]}'):
    case Ok(_):
      println("list_invalid_accepted")
    case Err(_):
      println("list_invalid_rejected")
  match OptionalId.from_json('{"id":"identifier_too_long"}'):
    case Ok(_):
      println("optional_invalid_accepted")
    case Err(_):
      println("optional_invalid_rejected")
  match PositiveEnvelope.from_json('{"value":1}'):
    case Ok(value):
      println(f"constraint_valid:{value.value.0}")
    case Err(_):
      println("constraint_valid_rejected")
  match PositiveEnvelope.from_json('{"value":0}'):
    case Ok(_):
      println("constraint_invalid_accepted")
    case Err(_):
      println("constraint_invalid_rejected")
  match GenericEnvelope.from_json('{"value":"generic"}'):
    case Ok(value):
      println(f"generic_valid:{value.value.0}")
    case Err(_):
      println("generic_valid_rejected")
"#,
        ])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "validated-newtype JSON program failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        vec![
            "model_roundtrip:{\"id\":\"short\"}",
            "model_invalid_rejected",
            "list_invalid_rejected",
            "optional_invalid_rejected",
            "constraint_valid:1",
            "constraint_invalid_rejected",
            "generic_valid:generic",
        ],
        "expected every JSON ingress to preserve newtype validation, got:\n{stdout}"
    );

    Ok(())
}

/// Issue #914: derived traits on a nested newtype must survive frontend bound checking and generated-Rust compilation.
#[test]
fn derived_nested_newtype_generic_bounds_build_and_run_issue914() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let source_path = temp.path().join("nested_newtype_generic_bounds.incn");
    let output_dir = temp.path().join("generated");
    fs::write(
        &source_path,
        r#"@derive(Clone, Eq)
type BaseId = newtype str

@derive(Clone, Eq)
type ChildId = newtype BaseId

def has_duplicates[T with (Clone, Eq)](values: list[T]) -> bool:
    for left in range(len(values)):
        for right in range(len(values)):
            if right > left and values[left] == values[right]:
                return true
    return false

def main() -> None:
    identifiers = [ChildId(BaseId("one")), ChildId(BaseId("one"))]
    println(has_duplicates(identifiers))
"#,
    )?;

    let build = incan_command()
        .args([
            "build",
            source_path.to_string_lossy().as_ref(),
            output_dir.to_string_lossy().as_ref(),
        ])
        .output()?;
    assert!(
        build.status.success(),
        "nested newtype generic-bound build failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    // The normal build above already verifies the command path and its generated Rust. Execute that exact Oven
    // artifact for the runtime assertion instead of compiling the same source again through `incan run`.
    let binary = output_dir.join("oven/release/nested_newtype_generic_bounds");
    assert!(
        binary.is_file(),
        "expected Oven to produce the nested-newtype executable at {}",
        binary.display()
    );
    let run = Command::new(&binary).output()?;
    assert!(
        run.status.success(),
        "nested newtype generic-bound run failed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8(run.stdout)?, "true\n");
    Ok(())
}

#[test]
fn rfc028_user_defined_operators_run_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        r#"[project]
name = "rfc028_user_defined_operators"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"model Money:
  cents: int

  def __add__(self, other: Money) -> Money:
    return Money(cents=self.cents + other.cents)

  def __lt__(self, other: Money) -> bool:
    return self.cents < other.cents


model Row:
  value: int

  def __getitem__(self, index: int) -> int:
    return self.value + index

  def __setitem__(self, index: int, value: int) -> None:
    pass


model OpBox:
  value: int

  def __matmul__(self, other: OpBox) -> OpBox:
    return OpBox(value=self.value + other.value)

  def __invert__(self) -> OpBox:
    return OpBox(value=0 - self.value)


def main() -> None:
  total = Money(cents=100) + Money(cents=25)
  println(total.cents)
  println(Money(cents=25) < Money(cents=100))
  row = Row(value=4)
  row[3] = 9
  println(row[3])
  mat = OpBox(value=2) @ OpBox(value=3)
  println(mat.value)
  inverted = ~OpBox(value=8)
  println(inverted.value)
"#,
    )?;

    let output = incan_command()
        .arg("run")
        .arg("src/main.incn")
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        output.status.success(),
        "expected RFC 028 operator program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("125") && stdout.contains("true") && stdout.contains("7") && stdout.contains("5"),
        "unexpected RFC 028 operator output:\n{stdout}"
    );

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

#[test]
fn binary_read_result_context_crosses_boundaries_issue955() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("typed_binary_read_result");
    let src_dir = tmp.path().join("src");
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    fs::write(
        src_dir.join("io_facade.incn"),
        "pub from std.io import BytesIO, Endian, IoError\n",
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from io_facade import BytesIO, Endian, IoError

def read_u8() -> Result[u8, IoError]:
  result: Result[u8, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_i8() -> Result[i8, IoError]:
  result: Result[i8, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_u16() -> Result[u16, IoError]:
  result: Result[u16, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_i16() -> Result[i16, IoError]:
  result: Result[i16, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_u32() -> Result[u32, IoError]:
  result: Result[u32, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_i32() -> Result[i32, IoError]:
  result: Result[i32, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_u64() -> Result[u64, IoError]:
  result: Result[u64, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_i64() -> Result[i64, IoError]:
  result: Result[i64, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_u128() -> Result[u128, IoError]:
  result: Result[u128, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_i128() -> Result[i128, IoError]:
  result: Result[i128, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_f32() -> Result[f32, IoError]:
  result: Result[f32, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def read_f64() -> Result[f64, IoError]:
  result: Result[f64, IoError] = BytesIO(b"").read(Endian.Big)
  return result

def main() -> None:
  read_u8()
  read_i8()
  read_u16()
  read_i16()
  read_u32()
  read_i32()
  read_u64()
  read_i64()
  read_u128()
  read_i128()
  read_f32()
  read_f64()
"#,
    )?;
    fs::write(
        tests_dir.join("test_binary_read.incn"),
        r#"from io_facade import BytesIO, Endian, IoError
from std.testing import assert_eq, fail

def test_typed_binary_read_results() -> None:
  u16_result: Result[u16, IoError] = BytesIO(b"\x00\x01").read(Endian.Big)
  match u16_result:
    Ok(value) => assert_eq(value, 1)
    Err(error) => fail(error.message())

  u32_result: Result[u32, IoError] = BytesIO(b"\x00\x00\x00\x02").read(Endian.Big)
  match u32_result:
    Ok(value) => assert_eq(value, 2)
    Err(error) => fail(error.message())

  f64_result: Result[f64, IoError] = BytesIO(b"\x00\x00\x00\x00\x00\x00\x00\x00").read(Endian.Big)
  match f64_result:
    Ok(value) => assert_eq(value, 0.0)
    Err(error) => fail(error.message())
"#,
    )?;

    let out_dir = tmp.path().join("out");
    let build_output = incan_command()
        .args([
            "build",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        build_output.status.success(),
        "expected every BinaryRead width to compile from its typed Result context.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    let generated_main = fs::read_to_string(out_dir.join("src/main.rs"))?;
    let normalized = generated_main
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    for rust_type in [
        "u8", "i8", "u16", "i16", "u32", "i32", "u64", "i64", "u128", "i128", "f32", "f64",
    ] {
        assert!(
            normalized.contains(&format!("BinaryRead::<{rust_type},>::read")),
            "expected exact BinaryRead::<{rust_type}> dispatch in generated Rust:\n{generated_main}"
        );
    }

    let test_output = incan_command()
        .args(["test", tests_dir.to_string_lossy().as_ref()])
        .current_dir(tmp.path())
        .env(
            "INCAN_TEST_SHARED_TARGET_DIR",
            repo_root().join("target").join("incan_e2e_shared_target"),
        )
        .output()?;
    assert!(
        test_output.status.success(),
        "expected package test batch to preserve typed BinaryRead dispatch.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&test_output.stdout),
        String::from_utf8_lossy(&test_output.stderr)
    );
    Ok(())
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

/// Regression (GitHub #247): `incan fmt` on disk must preserve body docstrings for all public block-like type
/// declarations, and [`exported_type_like_docs`] must still see them after the CLI round-trip.
///
/// `format_files` delegates to [`incan_format::format_source`]; this still covers subprocess + I/O if those paths
/// diverge from in-process formatting.
#[test]
fn test_cli_fmt_preserves_block_decl_docstrings_and_export_doc_surface() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("block_docstrings_cli.incn");
    fs::write(&path, block_docstring_public_type_like()?)?;
    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    let ast = parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    fn assert_markers(doc: Option<&str>, ctx: &str) -> Result<(), Box<dyn std::error::Error>> {
        let Some(doc) = doc else {
            return Err(std::io::Error::other(format!("{ctx}: missing docstring after CLI fmt")).into());
        };
        let t = doc.trim();
        if !t.contains("Line A documents the class API.") {
            return Err(std::io::Error::other(format!("{ctx}: missing marker A in {t:?}")).into());
        }
        if !t.contains("Line B keeps interior newlines after trim().") {
            return Err(std::io::Error::other(format!("{ctx}: missing marker B in {t:?}")).into());
        }
        Ok(())
    }

    let docs = exported_type_like_docs(&ast);
    assert_eq!(docs.len(), 5, "expected five public type-like exports with docs");
    let mut by_name: std::collections::HashMap<String, ExportedTypeLikeDoc> = std::collections::HashMap::new();
    for d in docs {
        by_name.insert(d.name.clone(), d);
    }

    let m = by_name
        .get("CliModelProbe")
        .ok_or_else(|| std::io::Error::other("missing CliModelProbe"))?;
    assert_eq!(m.kind, ExportedTypeLikeKind::Model);
    assert_markers(m.docstring.as_deref(), "model")?;

    let c = by_name
        .get("CliClassProbe")
        .ok_or_else(|| std::io::Error::other("missing CliClassProbe"))?;
    assert_eq!(c.kind, ExportedTypeLikeKind::Class);
    assert_markers(c.docstring.as_deref(), "class")?;

    let e = by_name
        .get("CliEnumProbe")
        .ok_or_else(|| std::io::Error::other("missing CliEnumProbe"))?;
    assert_eq!(e.kind, ExportedTypeLikeKind::Enum);
    assert_markers(e.docstring.as_deref(), "enum")?;

    let t = by_name
        .get("CliTraitProbe")
        .ok_or_else(|| std::io::Error::other("missing CliTraitProbe"))?;
    assert_eq!(t.kind, ExportedTypeLikeKind::Trait);
    assert_markers(t.docstring.as_deref(), "trait")?;

    let n = by_name
        .get("CliNewtypeProbe")
        .ok_or_else(|| std::io::Error::other("missing CliNewtypeProbe"))?;
    assert_eq!(n.kind, ExportedTypeLikeKind::Newtype);
    assert_markers(n.docstring.as_deref(), "newtype")?;

    Ok(())
}

#[test]
fn test_cli_fmt_accepts_assert_identity_bool_literals() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("assert_identity_bool_literals.incn");
    fs::write(
        &path,
        r#"
def check_flags(ready: bool, done: bool) -> None:
    assert ready is true, "ready should be true"
    assert done is false
"#,
    )?;

    let output = incan_command().arg("fmt").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected `incan fmt` to accept assert identity checks against bool literals.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

/// Regression (GitHub #484): parenthesized logical chains should wrap at obvious boolean breakpoints.
#[test]
fn test_cli_fmt_wraps_long_parenthesized_logical_expression_chain() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("long_logical_chain.incn");
    fs::write(
        &path,
        r#"model Item:
    kind_name: str
    predicate_kind_name: str
    source_name: str


def matches(item: Item) -> bool:
    return (item.kind_name == "filter" and item.predicate_kind_name == "bool_literal" and item.source_name == "rewritten_prism_node")
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"model Item:
    kind_name: str
    predicate_kind_name: str
    source_name: str


def matches(item: Item) -> bool:
    return (
        item.kind_name == "filter"
        and item.predicate_kind_name == "bool_literal"
        and item.source_name == "rewritten_prism_node"
    )
"#;
    assert_eq!(formatted, expected);
    assert!(
        formatted.lines().all(|line| line.len() <= 120),
        "expected formatted output to stay within 120 columns:\n{formatted}"
    );

    let output = incan_command().arg("--check").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected wrapped expression to parse/typecheck after CLI fmt; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

/// Regression (GitHub #289): `incan fmt` must preserve escaped newlines in f-strings as textual `\\n`.
#[test]
fn test_cli_fmt_preserves_fstring_escaped_newline_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("fstring_escaped_newline.incn");
    fs::write(
        &path,
        r#"def main() -> str:
    return f"a\n{1}"
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    assert!(
        formatted.contains(r#"f"a\n{1}""#),
        "expected formatted output to preserve escaped newline text, got:\n{}",
        formatted
    );

    let output = incan_command().arg("--check").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected formatted file to parse/typecheck after CLI fmt; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

/// Regression (GitHub #336 / RFC 053): the CLI formatter must apply the vertical-spacing contract on disk.
#[test]
fn test_cli_fmt_applies_rfc053_vertical_spacing_contract() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("rfc053_vertical_spacing.incn");
    fs::write(
        &path,
        r#"type UserId = str
# comment about the alias

model User:
  """
  First paragraph.


  Second paragraph.
  """
  id: UserId

trait Service:
  def connect(self) -> None: ...
  def reset(self) -> None:
    pass
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"type UserId = str
# comment about the alias


model User:
    """
    First paragraph.

    Second paragraph.
    """

    id: UserId


trait Service:
    def connect(self) -> None

    def reset(self) -> None:
        pass
"#;
    assert_eq!(formatted, expected);

    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    Ok(())
}

/// Regression (GitHub #336 / RFC 053): top-level type/function-shaped declarations keep two blank lines even when
/// adjacent to module statics.
#[test]
fn test_cli_fmt_keeps_two_blank_lines_between_static_and_function() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("rfc053_static_function_spacing.incn");
    fs::write(
        &path,
        r#"static prism_store_node_counts: list[int] = []
pub def allocate_prism_store_id() -> int:
  return len(prism_store_node_counts)
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"static prism_store_node_counts: list[int] = []


pub def allocate_prism_store_id() -> int:
    return len(prism_store_node_counts)
"#;
    assert_eq!(formatted, expected);

    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    Ok(())
}

/// Regression (GitHub #336 / RFC 053): a trailing own-line comment after a multi-line construct must stay after the
/// full suite, not get reinserted after the construct header.
#[test]
fn test_cli_fmt_keeps_trailing_comment_after_multiline_function() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("rfc053_trailing_comment_after_function.incn");
    fs::write(
        &path,
        r#"def load_user(id: str) -> str:
    return id

# TODO: split retries
"#,
    )?;

    let status = incan_command().arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    let expected = r#"def load_user(id: str) -> str:
    return id
# TODO: split retries
"#;
    assert_eq!(formatted, expected);

    let tokens = lexer::lex(&formatted)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;
    parser::parse(&tokens)
        .map_err(|errs| std::io::Error::other(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))?;

    Ok(())
}

/// Regression (GitHub #394): multiline function parameter lists must accept a trailing comma.
#[test]
fn test_cli_check_accepts_trailing_comma_in_multiline_function_params() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("trailing_param_comma.incn");
    fs::write(
        &path,
        r#"def identity(
    value: int,
) -> int:
    return value


def main() -> None:
    println(identity(1))
"#,
    )?;

    let output = incan_command().arg("--check").arg(&path).output()?;
    assert!(
        output.status.success(),
        "expected multiline trailing parameter comma to parse/typecheck; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}

/// Regression: float compound-assign with int RHS should typecheck (Python-like / promotion).
#[test]
fn test_compound_assign_float_with_int_rhs() {
    let program = r#"
def main() -> None:
    mut y: float = 100.0
    y /= 3
    y %= 7
    println(y)
"#;

    let result = compile_source(program);
    assert!(result.is_ok(), "Expected program to typecheck, got {:?}", result.err());
}

/// Test that all valid fixtures compile successfully
#[test]
fn test_valid_fixtures() {
    let fixtures_dir = incan_test_support::fixture("valid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(&fixtures_dir) else {
        panic!("failed to read directory {}", fixtures_dir.display());
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if is_incan_fixture(&path) {
            matched += 1;
            let result = compile_file(&path);
            if let Err(errs) = result {
                panic!(
                    "Expected {} to compile successfully, got errors: {:?}",
                    path.display(),
                    errs
                );
            }
        }
    }
    assert!(matched > 0, "No .incn fixtures found in {}", fixtures_dir.display());
}

/// Test that invalid fixtures produce errors
#[test]
fn test_invalid_fixtures() {
    let fixtures_dir = incan_test_support::fixture("invalid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(&fixtures_dir) else {
        panic!("failed to read directory {}", fixtures_dir.display());
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if is_incan_fixture(&path) {
            matched += 1;
            let result = compile_file(&path);
            assert!(
                result.is_err(),
                "Expected {} to fail compilation, but it succeeded",
                path.display()
            );
        }
    }
    assert!(matched > 0, "No .incn fixtures found in {}", fixtures_dir.display());
}

#[test]
fn test_help_is_banner_free() -> Result<(), Box<dyn std::error::Error>> {
    let output = incan_command().arg("--help").output()?;
    assert!(
        output.status.success(),
        "incan --help failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("░░███") && !stderr.contains("░░███"),
        "logo leaked into help output"
    );
    Ok(())
}

#[test]
fn test_version_is_single_line_and_banner_free() -> Result<(), Box<dyn std::error::Error>> {
    let output = incan_command().arg("--version").output()?;
    assert!(
        output.status.success(),
        "incan --version failed: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("░░███") && !stderr.contains("░░███"),
        "logo leaked into version output"
    );
    assert_eq!(stdout.lines().count(), 1, "expected single-line version output");
    Ok(())
}

#[test]
fn lifecycle_new_version_and_env_commands_work() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_dir = tmp.path().join("greeter");

    let new_output = incan_command()
        .args(["new", "greeter", "--yes", "--dir"])
        .arg(&project_dir)
        .args([
            "--description",
            "A generated greeting app",
            "--author",
            "Danny <danny@example.com>",
            "--license",
            "MIT",
        ])
        .output()?;
    assert!(
        new_output.status.success(),
        "incan new failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&new_output.stdout),
        String::from_utf8_lossy(&new_output.stderr)
    );

    let manifest_path = project_dir.join("loaf.toml");
    let initial_manifest = fs::read_to_string(&manifest_path)?;
    assert!(initial_manifest.contains(r#"name = "greeter""#));
    assert!(initial_manifest.contains(r#"description = "A generated greeting app""#));
    assert!(initial_manifest.contains(r#"authors = ["Danny <danny@example.com>"]"#));
    assert!(initial_manifest.contains(r#"license = "MIT""#));
    // Derived rather than written out: a prerelease compiler emits a `-0` lower bound so rc builds accept rc
    // projects, and a final release omits it so a released project does not silently accept prereleases. Pinning
    // one spelling makes this test fail on every transition between the two, which it did on the 0.5.0 bump.
    let expected_requires_incan = {
        let version = semver::Version::parse(incan_core::version::INCAN_VERSION)?;
        let lower = if version.pre.is_empty() {
            format!(">={}.{}.0", version.major, version.minor)
        } else {
            format!(">={}.{}.0-0", version.major, version.minor)
        };
        format!("{lower},<{}.{}.0", version.major, version.minor + 1)
    };
    assert!(
        initial_manifest.contains(&format!(r#"requires-incan = "{expected_requires_incan}""#)),
        "generated manifest should require {expected_requires_incan}, got:\n{initial_manifest}"
    );
    assert!(project_dir.join("src/main.incn").exists());
    assert!(project_dir.join("tests/test_main.incn").exists());

    let default_show_output = incan_command()
        .args(["env", "show", "default"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        default_show_output.status.success(),
        "env show default on fresh project failed: {}",
        String::from_utf8_lossy(&default_show_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&default_show_output.stdout).contains("overlay chain: project -> default"),
        "unexpected env show default output:\n{}",
        String::from_utf8_lossy(&default_show_output.stdout)
    );

    let dry_run = incan_command()
        .args(["version", "patch", "--dry-run"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        dry_run.status.success(),
        "dry-run failed: {}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dry_run.stdout).contains("new version: 0.1.1"),
        "unexpected dry-run output:\n{}",
        String::from_utf8_lossy(&dry_run.stdout)
    );
    assert_eq!(
        fs::read_to_string(&manifest_path)?,
        initial_manifest,
        "dry-run must not modify loaf.toml"
    );

    let version_output = incan_command()
        .args(["version", "patch"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        version_output.status.success(),
        "version bump failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&version_output.stdout),
        String::from_utf8_lossy(&version_output.stderr)
    );
    assert!(fs::read_to_string(&manifest_path)?.contains(r#"version = "0.1.1""#));

    let set_output = incan_command()
        .args([
            "version",
            "--set",
            "2.0.0-rc.1",
            "--project",
            manifest_path.to_str().ok_or("manifest path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        set_output.status.success(),
        "version set failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&set_output.stdout),
        String::from_utf8_lossy(&set_output.stderr)
    );
    assert!(fs::read_to_string(&manifest_path)?.contains(r#"version = "2.0.0-rc.1""#));

    let keep_prerelease_output = incan_command()
        .args([
            "version",
            "patch",
            "--keep-prerelease",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        keep_prerelease_output.status.success(),
        "version keep-prerelease failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&keep_prerelease_output.stdout),
        String::from_utf8_lossy(&keep_prerelease_output.stderr)
    );
    assert!(fs::read_to_string(&manifest_path)?.contains(r#"version = "2.0.1-rc.1""#));

    let missing_request_output = incan_command()
        .args([
            "version",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(!missing_request_output.status.success());
    assert!(
        String::from_utf8_lossy(&missing_request_output.stderr).contains("requires a bump name or `--set <version>`"),
        "unexpected missing-request stderr:\n{}",
        String::from_utf8_lossy(&missing_request_output.stderr)
    );

    let conflicting_request_output = incan_command()
        .args([
            "version",
            "patch",
            "--set",
            "3.0.0",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(!conflicting_request_output.status.success());
    assert!(
        String::from_utf8_lossy(&conflicting_request_output.stderr)
            .contains("accepts either a bump name or `--set <version>`, not both"),
        "unexpected conflicting-request stderr:\n{}",
        String::from_utf8_lossy(&conflicting_request_output.stderr)
    );

    fs::write(
        &manifest_path,
        format!(
            "{}\n[rust-dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"derive\"]\n\n[tool.incan.envs.default]\nenv-vars = {{ INCAN_NO_BANNER = \"1\" }}\n\n[tool.incan.envs.unit]\ncwd = \".\"\n\n[tool.incan.envs.unit.rust-dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"alloc\"]\n\n[tool.incan.envs.unit.scripts]\nprobe = [\"{}\", \"--version\"]\n",
            fs::read_to_string(&manifest_path)?,
            incan_debug_binary().display()
        ),
    )?;

    let list_output = incan_command()
        .args(["env", "list"])
        .current_dir(project_dir.join("src"))
        .output()?;
    assert!(
        list_output.status.success(),
        "env list failed: {}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let list_stdout = String::from_utf8_lossy(&list_output.stdout);
    assert!(list_stdout.contains("default"));
    assert!(list_stdout.contains("unit"));

    let list_json_output = incan_command()
        .args([
            "env",
            "list",
            "--format",
            "json",
            "--project",
            project_dir.to_str().ok_or("project path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        list_json_output.status.success(),
        "env list json failed: {}",
        String::from_utf8_lossy(&list_json_output.stderr)
    );
    let list_json: serde_json::Value = serde_json::from_slice(&list_json_output.stdout)?;
    assert_eq!(list_json, serde_json::json!(["default", "unit"]));

    let show_output = incan_command()
        .args(["env", "show", "unit"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        show_output.status.success(),
        "env show failed: {}",
        String::from_utf8_lossy(&show_output.stderr)
    );
    let show_stdout = String::from_utf8_lossy(&show_output.stdout);
    assert!(show_stdout.contains("overlay chain: project -> default -> unit"));
    assert!(show_stdout.contains("INCAN_NO_BANNER=1"));
    assert!(show_stdout.contains("Dependencies"));
    assert!(show_stdout.contains("serde"));
    assert!(show_stdout.contains("alloc"));
    assert!(show_stdout.contains("derive"));

    let show_overview_output = incan_command()
        .args(["env", "show"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        show_overview_output.status.success(),
        "env show overview failed: {}",
        String::from_utf8_lossy(&show_overview_output.stderr)
    );
    let show_overview_stdout = String::from_utf8_lossy(&show_overview_output.stdout);
    assert!(show_overview_stdout.contains("default"));
    assert!(show_overview_stdout.contains("unit"));
    assert!(show_overview_stdout.contains("Scripts"));

    let show_overview_json_output = incan_command()
        .args([
            "env",
            "show",
            "--format",
            "json",
            "--project",
            manifest_path.to_str().ok_or("manifest path is not valid UTF-8")?,
        ])
        .current_dir(tmp.path())
        .output()?;
    assert!(
        show_overview_json_output.status.success(),
        "env show overview json failed: {}",
        String::from_utf8_lossy(&show_overview_json_output.stderr)
    );
    let show_overview_json: serde_json::Value = serde_json::from_slice(&show_overview_json_output.stdout)?;
    let show_overview_array = show_overview_json.as_array().ok_or("expected array json output")?;
    assert_eq!(show_overview_array.len(), 2);
    assert!(show_overview_array.iter().any(|entry| entry["name"] == "default"));
    assert!(show_overview_array.iter().any(|entry| entry["name"] == "unit"));

    let show_json_output = incan_command()
        .args(["env", "show", "unit", "--format", "json"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        show_json_output.status.success(),
        "env show json failed: {}",
        String::from_utf8_lossy(&show_json_output.stderr)
    );
    let show_json: serde_json::Value = serde_json::from_slice(&show_json_output.stdout)?;
    assert_eq!(show_json["env"], "unit");
    assert_eq!(show_json["dependencies"]["serde"]["version"], "1.0");

    let dry_run_env = incan_command()
        .args(["env", "run", "unit", "probe", "--dry-run"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        dry_run_env.status.success(),
        "env dry-run failed: {}",
        String::from_utf8_lossy(&dry_run_env.stderr)
    );
    assert!(
        String::from_utf8_lossy(&dry_run_env.stdout).contains("--version"),
        "unexpected env dry-run output:\n{}",
        String::from_utf8_lossy(&dry_run_env.stdout)
    );

    let run_env = incan_command()
        .args(["env", "run", "unit", "probe"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        run_env.status.success(),
        "env run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_env.stdout),
        String::from_utf8_lossy(&run_env.stderr)
    );
    assert!(String::from_utf8_lossy(&run_env.stdout).starts_with("incan "));
    Ok(())
}

#[test]
fn zero_clone_starter_project_runs_tests_and_release_builds() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("starter");
    let project_dir = tmp.path().join(&project_name);

    let new_output = incan_command()
        .args(["new", &project_name, "--yes", "--dir"])
        .arg(&project_dir)
        .output()?;
    assert!(
        new_output.status.success(),
        "incan new failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&new_output.stdout),
        String::from_utf8_lossy(&new_output.stderr)
    );
    let new_stdout = String::from_utf8_lossy(&new_output.stdout);
    assert!(new_stdout.contains("Run it:     incan run"));
    assert!(new_stdout.contains("Test it:    incan test"));
    assert!(new_stdout.contains("Release it: incan build --release"));

    let main_source = fs::read_to_string(project_dir.join("src/main.incn"))?;
    assert!(
        main_source.contains("pub def greeting() -> str:"),
        "starter source should expose a small testable function, got:\n{main_source}"
    );
    let test_source = fs::read_to_string(project_dir.join("tests/test_main.incn"))?;
    assert!(
        test_source.contains("assert_eq(greeting()"),
        "starter test should assert generated behavior, got:\n{test_source}"
    );

    let run_output = incan_command()
        .arg("run")
        .current_dir(&project_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        run_output.status.success(),
        "starter incan run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run_output.stdout).contains(&format!("Hello from {project_name}!")),
        "unexpected starter run output:\n{}",
        String::from_utf8_lossy(&run_output.stdout)
    );

    let test_output = incan_command()
        .arg("test")
        .current_dir(&project_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        test_output.status.success(),
        "starter incan test failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&test_output.stdout),
        String::from_utf8_lossy(&test_output.stderr)
    );

    let build_output = incan_command()
        .args(["build", "--release"])
        .current_dir(&project_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        build_output.status.success(),
        "starter incan build --release failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    Ok(())
}

#[test]
fn env_run_nested_incan_run_uses_dependency_overlay_override() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::create_dir_all(project_root.join("src"))?;
    fs::write(
        project_root.join("loaf.toml"),
        format!(
            r#"[project]
name = "env_overlay_exec"
version = "0.1.0"

[rust-dependencies.serde_json]
version = "999.0.0"

[tool.incan.envs.unit.scripts]
run = ["{}", "run", "src/main.incn"]

[tool.incan.envs.unit.rust-dependencies.serde_json]
version = "1.0"
"#,
            incan_debug_binary().display()
        ),
    )?;
    fs::write(
        project_root.join("src/main.incn"),
        r#"import rust::serde_json as json

def main() -> None:
  pass
"#,
    )?;

    let bare_run = incan_command()
        .args(["run", "src/main.incn"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        !bare_run.status.success(),
        "plain run unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bare_run.stdout),
        String::from_utf8_lossy(&bare_run.stderr)
    );
    let bare_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&bare_run.stderr));
    assert!(
        bare_stderr.contains("serde_json") && bare_stderr.contains("999.0.0"),
        "expected invalid pinned dependency diagnostic, got:\n{}",
        bare_stderr
    );

    let env_run = incan_command()
        .args(["env", "run", "unit", "run"])
        .current_dir(project_root)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        env_run.status.success(),
        "env-backed nested run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&env_run.stdout),
        String::from_utf8_lossy(&env_run.stderr)
    );
    let env_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&env_run.stderr));
    assert!(
        !env_stderr.contains("999.0.0"),
        "nested env-backed run should use the overlay manifest instead of the broken base pin, got:\n{}",
        env_stderr
    );
    Ok(())
}

#[test]
fn env_run_nested_incan_env_show_prefers_parent_project_override() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::create_dir_all(project_root.join("child"))?;
    fs::write(
        project_root.join("loaf.toml"),
        format!(
            r#"[project]
name = "parent_project"
version = "0.1.0"

[tool.incan.envs.unit]
cwd = "child"
env-vars = {{ PARENT = "1" }}

[tool.incan.envs.unit.scripts]
inspect = ["{}", "env", "show", "unit", "--format", "json"]
"#,
            incan_debug_binary().display()
        ),
    )?;
    fs::write(
        project_root.join("child/loaf.toml"),
        r#"[project]
name = "child_project"
version = "0.1.0"

[tool.incan.envs.unit]
env-vars = { CHILD = "1" }
"#,
    )?;

    let bare_show = incan_command()
        .args(["env", "show", "unit", "--format", "json"])
        .current_dir(project_root.join("child"))
        .output()?;
    assert!(
        bare_show.status.success(),
        "bare child env show failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bare_show.stdout),
        String::from_utf8_lossy(&bare_show.stderr)
    );
    let bare_json: serde_json::Value = serde_json::from_slice(&bare_show.stdout)?;
    assert_eq!(bare_json["env_vars"]["CHILD"], "1");
    assert!(bare_json["env_vars"].get("PARENT").is_none());

    let env_show = incan_command()
        .args(["env", "run", "unit", "inspect"])
        .current_dir(project_root)
        .output()?;
    assert!(
        env_show.status.success(),
        "env-backed nested env show failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&env_show.stdout),
        String::from_utf8_lossy(&env_show.stderr)
    );
    let nested_json: serde_json::Value = serde_json::from_slice(&env_show.stdout)?;
    assert_eq!(nested_json["env_vars"]["PARENT"], "1");
    assert!(nested_json["env_vars"].get("CHILD").is_none());
    Ok(())
}

#[test]
fn test_parse_error_is_banner_free() {
    let Ok(output) = incan_command().arg("--definitely-not-a-flag").output() else {
        panic!("failed to run incan with invalid args");
    };
    assert!(
        !output.status.success(),
        "expected invalid args to fail, status={:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("░░███") && !stderr.contains("░░███"),
        "logo leaked into parse error output"
    );
}

#[test]
fn test_fstring_unknown_symbol_cli_caret_points_to_interpolation() {
    let source = "def main() -> str:\n  return f\"value: {unknown_var}\"\n";
    let Ok(output) = incan_command().args(["run", "-c", source]).output() else {
        panic!("failed to run incan with f-string source");
    };

    assert!(
        !output.status.success(),
        "expected unknown symbol compilation failure, status={:?}",
        output.status
    );

    let stderr_colored = String::from_utf8_lossy(&output.stderr);
    let stderr = strip_ansi_escapes(&stderr_colored);
    assert!(
        stderr.contains("Unknown symbol 'unknown_var'"),
        "expected unknown symbol diagnostic in stderr, got:\n{}",
        stderr
    );
    assert!(
        stderr.contains("return f\"value: {unknown_var}\""),
        "expected source line in diagnostic, got:\n{}",
        stderr
    );

    let caret_line = match stderr.lines().find(|line| line.contains('^')) {
        Some(line) => line,
        None => panic!("expected caret line in diagnostic, got:\n{}", stderr),
    };

    let mut max_caret_run = 0usize;
    let mut current_run = 0usize;
    for c in caret_line.chars() {
        if c == '^' {
            current_run += 1;
            if current_run > max_caret_run {
                max_caret_run = current_run;
            }
        } else {
            current_run = 0;
        }
    }

    assert_eq!(
        max_caret_run,
        "{unknown_var}".len(),
        "expected caret width to match interpolation span; stderr:\n{}",
        stderr
    );
}

#[test]
fn test_fstring_list_interpolation_uses_structured_formatting() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def debug_values[T](values: list[T]) -> str:
  return f"{values:?}"

def display_values[T](values: list[T]) -> str:
  return f"{values}"

def main() -> None:
  columns: list[str] = ["id", "amount"]
  println(f"debug: {columns:?}")
  println(f"display: {columns}")
  println(debug_values[str](["id", "amount"]))
  println(display_values[str](["id", "amount"]))
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        output.status.success(),
        "expected list f-string interpolation to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("debug: [\"id\", \"amount\"]"),
        "expected debug list output, got:\n{stdout}"
    );
    assert!(
        stdout.contains("display: [\"id\", \"amount\"]"),
        "expected default list f-string output to use structured formatting, got:\n{stdout}"
    );
    assert!(
        stdout.lines().filter(|line| *line == "[\"id\", \"amount\"]").count() == 2,
        "expected both generic list helpers to render, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn fixed_call_unpack_runs_for_positional_and_keyword_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def total(a: int, b: int, *rest: int, **labels: str) -> int:
  println(labels["city"])
  return a + b + rest[0]

def route(path: str, method: str) -> str:
  return method + " " + path

class Counter:
  def add(self, left: int, right: int) -> int:
    return left + right

def main() -> None:
  xy: tuple[int, int] = (2, 3)
  counter = Counter()
  println(total(*xy, *[4], **{"city": "London"}))
  println(route(**{"path": "/status", "method": "GET"}))
  println(counter.add(*(5, 6)))
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected fixed call unpack program to run, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["London", "9", "GET /status", "11"],
        "unexpected fixed unpack runtime output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn rfc046_computed_properties_run_as_getters() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"trait Named:
  property label -> str

model Money with Named:
  cents: int

  pub property adjusted -> int:
    return self.cents + 1

  property label -> str:
    return "money"

def main() -> None:
  value = Money(cents=250)
  println(value.adjusted)
  println(value.label)
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected computed property program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "251\nmoney\n");
    Ok(())
}

#[test]
fn exact_f32_arithmetic_and_mixed_f64_operands_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("exact_float_arithmetic");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    let source = r#"
model ExactSamples:
  narrow: f32
  wide: f64


def main() -> None:
  narrow: f32 = 9.0
  peer: f32 = 4.0
  wide: f64 = 4.0
  ieee: float = 0.5
  count: int = 1
  samples = ExactSamples(narrow=narrow, wide=wide)
  exact_values: list[f64] = [wide]
  println(narrow + peer)
  println(narrow - peer)
  println(narrow * peer)
  println(narrow / peer)
  println(narrow // peer)
  println(narrow % peer)
  println(narrow ** peer)
  println(narrow + wide)
  println(narrow / wide)
  println(narrow // wide)
  println(narrow % wide)
  println(narrow ** wide)
  println(narrow + ieee)
  println(narrow + count)
  println(samples.narrow.is_finite())
  println(samples.wide.is_nan())
  println(exact_values[0].is_finite())
"#;
    let main_path = src_dir.join("main.incn");
    fs::write(&main_path, source)?;

    let oven_home = tmp.path().join("incan-home");
    let mut bake_command = incan_command();
    bake_command
        .args(["oven", "bake", "--project", "."])
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", &oven_home);
    support::configure_explicit_oven_bake_command(&mut bake_command)?;
    let bake_output = bake_command.output()?;
    assert!(
        bake_output.status.success(),
        "expected explicit Oven bake to prepare exact-float arithmetic.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&bake_output.stdout),
        String::from_utf8_lossy(&bake_output.stderr)
    );

    let out_dir = tmp.path().join("out");
    let build_output = incan_command()
        .args([
            "build",
            "--locked",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_HOME", &oven_home)
        .output()?;

    assert!(
        build_output.status.success(),
        "expected mixed exact-f32 arithmetic to compile and run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );

    let binary = out_dir.join("oven/release").join(&project_name);
    assert!(
        binary.is_file(),
        "expected exact-float executable at {}",
        binary.display()
    );
    let output = Command::new(&binary).output()?;
    assert!(
        output.status.success(),
        "expected exact-float arithmetic executable to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "13\n5\n36\n2.25\n2\n1\n6561\n13\n2.25\n2\n1\n6561\n9.5\n10\ntrue\nfalse\ntrue\n"
    );
    Ok(())
}

#[test]
fn runtime_error_canonicalization_cases() -> Result<(), Box<dyn std::error::Error>> {
    // One CLI journey proves generated-main subprocess diagnostics. The same compiled program then exercises the
    // remaining independent runtime failures by selector, avoiding seven rebuilds of an otherwise identical project.
    let cases: &[(&str, &str, &[&str])] = &[
        ("key", "KeyError", &["not found in dict"]),
        ("index", "IndexError", &["out of range for list"]),
        ("find", "ValueError", &["value not found in list"]),
        ("int", "ValueError", &["cannot convert 'abc' to int"]),
        ("float", "ValueError", &["cannot convert 'abc' to float"]),
        ("remove", "IndexError", &["out of range for list"]),
        ("swap", "IndexError", &["out of range for list"]),
        ("assert-int", "AssertionError", &["boom"]),
        ("assert-generic", "AssertionError", &["boom"]),
        (
            "exact-f32-overflow",
            "ValueError",
            &["non-finite float cannot initialize exact f32"],
        ),
        (
            "exact-f64-overflow",
            "ValueError",
            &["non-finite float cannot initialize exact f64"],
        ),
    ];
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("runtime_error_matrix");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from std.environ import get_or


def fail_int(message: str) -> int:
  assert false, message


def fail_as[T](message: str) -> T:
  assert false, message


def main() -> None:
  scenario = get_or("INCAN_RUNTIME_ERROR_CASE", "key")
  if scenario == "key":
    let values = {"a": 1}
    println(values["b"])
  elif scenario == "index":
    let values = [1, 2, 3]
    println(values[99])
  elif scenario == "find":
    let values = [1, 2, 3]
    println(values.index(99))
  elif scenario == "int":
    println(int("abc"))
  elif scenario == "float":
    println(float("abc"))
  elif scenario == "remove":
    mut values = [1, 2, 3]
    values.remove(99)
  elif scenario == "swap":
    mut values = [1, 2, 3]
    values.swap(0, 99)
  elif scenario == "assert-int":
    _ = fail_int("boom")
  elif scenario == "assert-generic":
    _ = fail_as[int]("boom")
  elif scenario == "exact-f32-overflow":
    let value: f32 = 3.4e38
    println(value * value)
  elif scenario == "exact-f64-overflow":
    let value: f64 = 1.7e308
    println(value * value)
"#,
    )?;

    let (cli_case, cli_kind, cli_markers) = cases[0];
    let cli_output = incan_command()
        .args(["run", main_path.to_string_lossy().as_ref()])
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_RUNTIME_ERROR_CASE", cli_case)
        .output()?;
    assert_runtime_error_output(&cli_output, cli_kind, cli_markers);

    let out_dir = tmp.path().join("out");
    let build_output = incan_command()
        .args([
            "build",
            main_path.to_string_lossy().as_ref(),
            out_dir.to_string_lossy().as_ref(),
        ])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        build_output.status.success(),
        "expected the shared runtime-error matrix program to build.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_output.stdout),
        String::from_utf8_lossy(&build_output.stderr)
    );
    let binary = out_dir.join("oven").join("release").join(project_name);
    assert!(
        binary.is_file(),
        "expected Oven to produce the runtime-error matrix binary at {}",
        binary.display()
    );

    for (case, expected_type, expected_substrings) in &cases[1..] {
        let output = Command::new(&binary).env("INCAN_RUNTIME_ERROR_CASE", case).output()?;
        assert_runtime_error_output(&output, expected_type, expected_substrings);
    }
    Ok(())
}

#[test]
fn test_fail_on_empty_collection() {
    let dir = make_temp_test_dir();
    let test_file = dir.join("test_empty.incn");
    let Ok(()) = std::fs::write(
        &test_file,
        r#"
def helper() -> Unit:
  pass
"#,
    ) else {
        panic!("failed to write test file");
    };

    let Ok(output) = incan_command().args(["test", dir.to_string_lossy().as_ref()]).output() else {
        panic!("failed to run incan test");
    };
    assert!(
        output.status.success(),
        "expected empty collection to succeed by default: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let Ok(output) = incan_command()
        .args(["test", "--fail-on-empty", dir.to_string_lossy().as_ref()])
        .output()
    else {
        panic!("failed to run incan test --fail-on-empty");
    };
    assert!(
        !output.status.success(),
        "expected empty collection to fail with --fail-on-empty: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_rfc052_module_static_counter_runs() {
    let source = r#"
static counter: int = 0

def main() -> None:
  counter = counter + 1
  counter += 2
  println(counter)
"#;
    let Ok(output) = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
    else {
        panic!("failed to run incan with static counter source");
    };

    assert!(
        output.status.success(),
        "expected static counter program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains('3'),
        "expected static counter output to contain 3.\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn test_rfc052_static_initializer_runs_before_main_without_static_reads() {
    let source = r#"
def init_counter() -> int:
  println("init")
  return 1

static counter: int = init_counter()

def main() -> None:
  println("main")
"#;
    let Ok(output) = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
    else {
        panic!("failed to run incan with eager static initializer source");
    };

    assert!(
        output.status.success(),
        "expected eager static initializer program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert!(
        lines.len() >= 2 && lines[0] == "init" && lines[1] == "main",
        "expected initializer output before main output.\nstdout:\n{}",
        stdout
    );
}

#[test]
fn test_rfc052_static_alias_mutation_runs() {
    let source = r#"
static items: list[int] = []

def main() -> None:
  let live = items
  live.append(1)
  live.append(2)
  println(len(items))
  println(len(live))
"#;
    let Ok(output) = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
    else {
        panic!("failed to run incan with static alias source");
    };

    assert!(
        output.status.success(),
        "expected static alias program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().filter(|line| line.trim() == "2").count() >= 2,
        "expected static alias output to print 2 twice.\nstdout:\n{stdout}"
    );
}

#[test]
fn test_const_model_constructor_compile_and_run_issue658() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Version:
  pub major: int
  pub minor: int

model Change:
  pub version: Version
  note [alias="message"]: FrozenStr

model Lifecycle:
  pub since: Version
  pub changed: FrozenList[Change]
  pub deprecated: Option[Version]

pub const V0_1: Version = Version(major=0, minor=1)
pub const V0_3: Version = Version(major=0, minor=3)
pub const LIFECYCLE: Lifecycle = Lifecycle(
  since=V0_1,
  changed=[Change(version=V0_3, message="metadata")],
  deprecated=None,
)

def main() -> None:
  println(f"{V0_1.major}.{V0_1.minor}")
  println(f"{LIFECYCLE.changed[0].version.major}.{LIFECYCLE.changed[0].version.minor}")
  println(LIFECYCLE.changed[0].note)
  match LIFECYCLE.deprecated:
    None => println("active")
    Some(version) => println(f"{version.major}.{version.minor}")
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected const model constructor program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line.trim() == "0.1"),
        "expected const model constructor output 0.1.\nstdout:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim() == "0.3"),
        "expected nested const model constructor output 0.3.\nstdout:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim() == "metadata"),
        "expected nested const model constructor output metadata.\nstdout:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.trim() == "active"),
        "expected const model option metadata output active.\nstdout:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_lowercase_imported_pub_static_compile_and_run_issue659() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let versions = dir.join("versions.incn");
    let main = dir.join("main.incn");
    std::fs::write(
        &versions,
        r#"
pub static v0_1: int = 1
pub static v0_2: int = 2
"#,
    )?;
    std::fs::write(
        &main,
        r#"
from versions import v0_1
from versions import v0_2 as current_version

def main() -> None:
  println(v0_1)
  println(current_version)
"#,
    )?;

    let output = incan_command()
        .args(["run", main.to_string_lossy().as_ref()])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected lowercase imported pub static program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(lines, ["1", "2"], "unexpected lowercase static output");
    Ok(())
}

#[test]
fn test_imported_static_initializer_does_not_deadlock_issue680() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let project_name = unique_test_project_name("imported_static_deadlock");
    std::fs::write(
        dir.join("loaf.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir)?;
    let state = src_dir.join("state.incn");
    let facade = src_dir.join("facade.incn");
    let direct_user = src_dir.join("direct_user.incn");
    let reexport_user = src_dir.join("reexport_user.incn");
    let main = src_dir.join("main.incn");
    std::fs::write(
        &state,
        r#"
pub class Registry:
  pub entries: list[int]

  @staticmethod
  def new() -> Self:
    return Registry(entries=[])


pub static registry: Registry = Registry.new()


pub def registry_len() -> int:
  return len(registry.entries)
"#,
    )?;
    std::fs::write(&facade, "pub from state import registry\n")?;
    std::fs::write(
        &direct_user,
        r#"
from state import registry


pub def add_direct() -> None:
  registry.entries.append(1)
"#,
    )?;
    std::fs::write(
        &reexport_user,
        r#"
from facade import registry


pub def add_reexport() -> None:
  registry.entries.append(1)
"#,
    )?;
    std::fs::write(
        &main,
        r#"
from direct_user import add_direct
from reexport_user import add_reexport
from state import registry_len


def main() -> None:
  add_direct()
  add_reexport()
  assert registry_len() == 2
  println("ok")
"#,
    )?;

    let mut command = incan_command();
    command
        .arg("run")
        .arg(main.strip_prefix(&dir)?)
        .current_dir(&dir)
        .env("CARGO_NET_OFFLINE", "true");
    let (output, timed_out) = run_incan_command_with_timeout(command, std::time::Duration::from_secs(30))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !timed_out,
        "imported static init repro timed out; likely deadlocked.\nstdout:\n{}\nstderr:\n{}",
        stdout, stderr
    );
    assert!(
        output.status.success(),
        "expected imported static init repro to run.\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(
        stdout.lines().any(|line| line.trim() == "ok"),
        "expected imported static init repro to print ok.\nstdout:\n{stdout}"
    );

    let generated_src_dir = dir.join("target/incan").join(project_name).join("src");
    let generated_direct_user = std::fs::read_to_string(generated_src_dir.join("direct_user.rs"))?;
    assert!(
        generated_direct_user
            .contains("use crate::state::__incan_init_module_statics as __incan_init_imported_static_registry;")
            && generated_direct_user.contains("__incan_init_imported_static_registry();"),
        "direct imported static access should call the defining module init guard before forcing REGISTRY:\n{}",
        generated_direct_user
    );
    let generated_facade = std::fs::read_to_string(generated_src_dir.join("facade.rs"))?;
    assert!(
        generated_facade
            .contains("use crate::state::__incan_init_module_statics as __incan_init_imported_static_registry;")
            && generated_facade.contains("pub(crate) fn __incan_init_module_statics()")
            && generated_facade.contains("__incan_init_imported_static_registry();"),
        "static re-export modules should chain the defining module init guard:\n{}",
        generated_facade
    );
    Ok(())
}

#[test]
fn test_static_list_index_assignment_and_remove_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
static entries: list[int] = []

def main() -> None:
  entries.append(1)
  entries[0] = 2
  println(entries[0])
  entries.remove(0)
  entries.append(3)
  println(entries[0])
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected static list index mutation program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(lines, ["2", "3"], "unexpected static list mutation output");
    Ok(())
}

#[test]
fn test_list_concatenation_plus_operator_runs() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> None:
  a: List[int] = [1, 2]
  b: List[int] = [3, 4]
  c: List[int] = a + b
  println(len(a))
  println(len(b))
  println(len(c))
  println(c[0])
  println(c[3])
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected list concat program to run.\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["2", "2", "4", "1", "4"],
        "expected concatenated list output 2/2/4/1/4.\nstdout:\n{}",
        stdout
    );

    Ok(())
}

#[test]
fn test_rfc016_loop_expression_runs() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def find_value(flag: bool) -> int:
  return loop:
    if flag:
      break 42
    break 7

def main() -> None:
  println(find_value(True))
  println(find_value(False))
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected loop expression program to run.\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["42", "7"],
        "unexpected loop expression output.\nstdout:\n{stdout}"
    );

    Ok(())
}

#[test]
fn test_rfc032_value_enums_run() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Environment(str):
  Development = "development"
  Production = "production"

enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def main() -> None:
  env = Environment.Production
  status = HttpStatus.NotFound
  println(env.value())
  println(status.value())
  match Environment.from_value("development"):
    Some(parsed_env) => println(parsed_env.value())
    None => println("missing env")
  match HttpStatus.from_value(404):
    Some(parsed_status) => println(parsed_status.value())
    None => println(0)
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected value enum program to run.\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["production", "404", "development", "404"],
        "unexpected value enum output.\nstdout:\n{stdout}"
    );

    Ok(())
}

#[test]
fn test_list_extend_method_runs_without_consuming_source() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def main() -> None:
  mut a: List[int] = [1, 2]
  b: List[int] = [3, 4]
  a.extend(b)
  println(len(a))
  println(a[3])
  println(len(b))
  println(b[0])
"#;
    let output = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "expected list extend program to run.\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        vec!["4", "4", "2", "3"],
        "expected extended list output 4/4/2/3.\nstdout:\n{}",
        stdout
    );

    Ok(())
}

#[test]
fn test_rfc052_static_self_referential_method_arg_runs() {
    let source = r#"
static items: list[int] = []

def main() -> None:
  items.append(len(items))
  items.append(len(items))
  println(items[0])
  println(items[1])
"#;
    let Ok(output) = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
    else {
        panic!("failed to run incan with static self-referential source");
    };

    assert!(
        output.status.success(),
        "expected static self-referential append program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert!(
        lines.len() >= 2 && lines[0] == "0" && lines[1] == "1",
        "expected first two output lines to be 0 and 1.\nstdout:\n{stdout}"
    );
}

#[test]
fn test_rfc052_static_init_is_eager_and_declaration_ordered() {
    let source = r#"
static init_order: list[int] = []

def mark(value: int) -> int:
  init_order.append(value)
  return value

static first: int = mark(1)
static second: int = mark(2)

def main() -> None:
  println(len(init_order))
  println(init_order[0])
  println(init_order[1])
"#;
    let Ok(output) = incan_command()
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()
    else {
        panic!("failed to run incan with static init-order source");
    };

    assert!(
        output.status.success(),
        "expected static init-order program to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
    assert!(
        lines.len() >= 3 && lines[0] == "2" && lines[1] == "1" && lines[2] == "2",
        "expected eager declaration-order static init output 2, 1, 2.\nstdout:\n{stdout}"
    );
}

/// Test specific lexer behavior
mod lexer_tests {
    use incan_core::lang::keywords::KeywordId;
    use incan_core::lang::operators::OperatorId;
    use incan_core::lang::punctuation::PunctuationId;
    use incan_frontend::lexer::{TokenKind, lex};

    #[test]
    fn lexer_token_surface_cases() {
        let Ok(tokens) = lex("a //= b\nc // d") else {
            panic!("lex failed");
        };
        let has_floor_div_eq = tokens.iter().any(|t| t.kind.is_operator(OperatorId::SlashSlashEq));
        let has_floor_div = tokens.iter().any(|t| t.kind.is_operator(OperatorId::SlashSlash));
        assert!(has_floor_div_eq, "expected to see //= token");
        assert!(has_floor_div, "expected to see // token");

        let Ok(tokens) = lex("import foo::bar::baz as fb") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Import));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "foo"));
        assert!(tokens[2].kind.is_punctuation(PunctuationId::ColonColon));
        assert!(matches!(&tokens[3].kind, TokenKind::Ident(s) if s == "bar"));
        assert!(tokens[4].kind.is_punctuation(PunctuationId::ColonColon));
        assert!(matches!(&tokens[5].kind, TokenKind::Ident(s) if s == "baz"));
        assert!(tokens[6].kind.is_keyword(KeywordId::As));
        assert!(matches!(&tokens[7].kind, TokenKind::Ident(s) if s == "fb"));

        let Ok(tokens) = lex("result?") else {
            panic!("lex failed");
        };
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "result"));
        assert!(tokens[1].kind.is_punctuation(PunctuationId::Question));

        let Ok(tokens) = lex("x => y") else {
            panic!("lex failed");
        };
        assert!(tokens[1].kind.is_punctuation(PunctuationId::FatArrow));

        let Ok(tokens) = lex("case Some(x):") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Case));

        let Ok(tokens) = lex("pass") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Pass));

        let Ok(tokens) = lex("mut self") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Mut));
        assert!(tokens[1].kind.is_keyword(KeywordId::SelfKw));

        let Ok(tokens) = lex(r#"f"Hello {name}""#) else {
            panic!("lex failed");
        };
        assert!(matches!(&tokens[0].kind, TokenKind::FString(_)));

        let Ok(tokens) = lex("yield value") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Yield));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "value"));

        let Ok(tokens) = lex("import rust::serde_json") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Import));
        assert!(tokens[1].kind.is_keyword(KeywordId::Rust));
        assert!(tokens[2].kind.is_punctuation(PunctuationId::ColonColon));
        assert!(matches!(&tokens[3].kind, TokenKind::Ident(s) if s == "serde_json"));
    }
}

mod numeric_semantics_tests {
    use incan_frontend::{lexer, parser, typechecker};

    #[test]
    fn test_python_like_numeric_ops_compile() {
        let source = r#"
def main() -> None:
  a: int = 7
  b: int = -3
  x = a / b       # float
  y = a // b      # floor div
  z = a % b       # python remainder
  f: float = 7.0
  g = f % 2.0
  h = f // 2.0
"#;
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
    }
}

/// End-to-end codegen tests
mod codegen_tests {
    use crate::support::repo_root;

    use super::{
        compiled_sdk_provider_artifact_root, incan_command, strip_ansi_escapes, support, unique_test_project_name,
    };
    use incan_driver::backend::IrCodegen;
    use incan_frontend::{lexer, parser, typechecker};
    use incan_semantics_core::{SemanticSourceTargetKind, decode_incan_symbol_identity, encode_incan_symbol_identity};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn run_incan_source(source: &str) -> std::process::Output {
        incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap_or_else(|e| panic!("failed to run incan source: {e}"))
    }

    /// Build an Incan source fixture and return its compiler-reported executable artifact.
    fn build_incan_source_binary(source: &Path, output_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let source_arg = source.to_str().ok_or("source path was not valid UTF-8")?;
        let output_arg = output_dir.to_str().ok_or("output path was not valid UTF-8")?;
        let output = incan_command()
            .args(["build", source_arg, output_arg, "--report", "json"])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "Incan build failed for {}. stdout:\n{}\nstderr:\n{}",
                source.display(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            )
            .into());
        }
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let binary = report["artifacts"]
            .as_array()
            .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "binary"))
            .and_then(|artifact| artifact["path"].as_str())
            .ok_or("build report did not contain a binary artifact")?;
        let binary = PathBuf::from(binary);
        if !binary.is_file() {
            return Err(format!("reported binary does not exist: {}", binary.display()).into());
        }
        Ok(binary)
    }

    fn rustc_compile_ok(source: &str) -> Result<(), String> {
        let mut dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        let uniq = duration.as_nanos();
        dir.push(format!("incan_bench_smoke_{}", uniq));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let rs_path = dir.join("main.rs");
        let bin_path = dir.join("bin");
        std::fs::write(&rs_path, source).map_err(|e| e.to_string())?;

        let out = Command::new("rustc")
            .arg("--edition=2021")
            .arg(&rs_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .map_err(|e| e.to_string())?;

        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        }
    }

    fn make_temp_dir(prefix: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        let uniq = duration.as_nanos();
        dir.push(format!("{}_{}", prefix, uniq));
        let Ok(()) = std::fs::create_dir_all(&dir) else {
            panic!("failed to create temp dir");
        };
        dir
    }

    #[test]
    fn test_hello_world_codegen() {
        let path = repo_root().join("examples/hello.incn");
        if !path.exists() {
            return; // Skip if example not present
        }

        let Ok(source) = fs::read_to_string(&path) else {
            panic!("failed to read {}", path.display());
        };
        let Ok(tokens) = lexer::lex(&source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };

        // Verify the generated code contains expected elements
        assert!(rust_code.contains("fn main()"), "Should have main function");
        assert!(rust_code.contains("println!"), "Should have println macro");
        assert!(rust_code.contains("Hello from Incan!"), "Should have the message");
    }

    #[test]
    fn test_string_literal_match_patterns_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def describe(value: str) -> str:
    match value:
        case "star":
            return "literal"
        case other:
            return other.upper()

def describe_alt(value: str) -> str:
    mut out = ""
    match value:
        "star" | "sun" => out += "literal"
        other => out += other.upper()
    return out

def main() -> None:
    println(describe("star"))
    println(describe("fallback"))
    println(describe_alt("sun"))
    println(describe_alt("fallback"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "string literal match pattern regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["literal", "FALLBACK", "literal", "FALLBACK"],
            "unexpected string match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_payload_enum_without_equality_payload_compiles() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
model Payload:
    value: str

enum Token:
    Item(Payload)
    Empty

enum Mode:
    Fast
    Slow

def describe(token: Token) -> str:
    match token:
        case Token.Item(payload):
            return payload.value
        case Token.Empty:
            return "empty"

def main() -> None:
    if Mode.Fast == Mode.Fast:
        println(describe(Token.Item(Payload(value="ok"))))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "payload enum derive regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["ok"], "unexpected payload enum output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_method_alias_codegen_rewrites_to_target_method() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
model Stats:
  value: int
  mean = avg

  def avg(self) -> int:
    return self.value

def main() -> None:
  let stats = Stats(value=10)
  println(stats.mean())
"#;
        let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
        let ast =
            parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
        let rust_code = IrCodegen::new()
            .try_generate(&ast)
            .map_err(|error| std::io::Error::other(format!("codegen failed: {error:?}")))?;
        let avg_identity = rust_code
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .filter_map(|token| decode_incan_symbol_identity(token).ok().flatten())
            .find(|identity| identity.kind == SemanticSourceTargetKind::Method && identity.declaration_name == "avg")
            .ok_or_else(|| std::io::Error::other("generated Rust did not carry the target method identity"))?;
        let projection = encode_incan_symbol_identity(&avg_identity);
        assert!(
            rust_code.contains(&format!(".{projection}(")),
            "expected method alias call to lower to the target declaration's canonical projection, got:\n{rust_code}"
        );
        assert!(
            !rust_code.contains(".mean("),
            "method alias must not emit an independent wrapper call, got:\n{rust_code}"
        );
        Ok(())
    }

    #[test]
    fn test_run_c_import_this() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args(["run", "-c", "import this"])
            // This test should not require network access. We expect the workspace dependencies to already be available
            // (the test suite built them)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c import this failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("The Zen of Incan") && stdout.contains("Readability counts"),
            "stdout missing zen line; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_run_c_import_this_release_flag() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args(["run", "--release", "-c", "import this"])
            // This test should not require network access. We expect the workspace dependencies to already be available
            // (the test suite built them)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run --release -c import this failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("The Zen of Incan") && stdout.contains("Readability counts"),
            "stdout missing zen line; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_variadic_rest_calls_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def collect(prefix: str, *items: int, **labels: str) -> int:
    mut total: int = 0
    for item in items:
        total = total + item
    if labels["name"] == "direct":
        return total
    if labels["name"] == "callable":
        return total
    return total

class Collector:
    def collect(self, *items: int, **labels: str) -> int:
        mut total: int = 0
        for item in items:
            total = total + item
        if labels["name"] == "method":
            return total
        return -100

def main() -> None:
    f = collect
    collector = Collector()
    println(collect("x", 1, 2, name="direct") + f("x", 4, 5, name="callable") + collector.collect(6, 7, name="method"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "variadic rest run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["25"], "unexpected variadic rest output:\n{stdout}");
        Ok(())
    }

    /// Compile and run one standalone local-partial program through the normal Oven-backed generated-Rust path.
    fn compile_and_run_local_partial_project(
        source: &str,
        project_stem: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_name = unique_test_project_name(project_stem);
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(
            tmp.path().join("loaf.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        let main_path = src_dir.join("main.incn");
        fs::write(&main_path, source)?;

        let oven_home = tmp.path().join("incan-home");
        let mut bake_command = incan_command();
        bake_command
            .args(["oven", "bake", "--project", "."])
            .current_dir(tmp.path())
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", &oven_home);
        support::configure_explicit_oven_bake_command(&mut bake_command)?;
        let bake_output = bake_command.output()?;
        assert!(
            bake_output.status.success(),
            "expected explicit Oven bake to prepare local-partial regression.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bake_output.stdout),
            String::from_utf8_lossy(&bake_output.stderr)
        );

        let out_dir = tmp.path().join("out");
        let build_output = incan_command()
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .current_dir(tmp.path())
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_HOME", &oven_home)
            .output()?;
        assert!(
            build_output.status.success(),
            "local partial generated-Rust build regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            build_output.status,
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let binary = out_dir.join("oven/release").join(&project_name);
        assert!(
            binary.is_file(),
            "expected generated Rust executable at {}",
            binary.display()
        );
        let run_output = Command::new(&binary).output()?;
        assert!(
            run_output.status.success(),
            "local partial generated Rust executable failed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(strip_ansi_escapes(&String::from_utf8_lossy(&run_output.stdout)))
    }

    #[test]
    fn local_partial_default_and_override_compile_and_run_issue1124() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
def route(method: str, path: str, content_type: str = "text") -> str:
  return method + path + content_type

def main() -> None:
  get = partial route(method="GET")
  alias = get
  println(alias("/health") + "|" + alias(method="HEAD", path="/head"))
"#,
            "local_partial_default_contract",
        )?;
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["GET/healthtext|HEAD/headtext"],
            "unexpected local partial output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn generic_caller_inherits_selected_implementation_bounds_issue1280() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
from std.derives.collection import FallibleIterator

model Stream[R] with FallibleIterator[int, str]:
    value: R

    def __next__(mut self) -> Result[Option[int], str]:
        return Ok(None)

pub def drain[R](value: R) -> Result[int, str]:
    mut count = 0
    for _item in Stream[R](value=value)?:
        count += 1
    return Ok(count)

pub def relay[R](value: R) -> Result[int, str]:
    return drain(value)

def main() -> Result[None, str]:
    println(relay("ready")?)
    return Ok(None)
"#,
            "generic_implementation_bound_contract",
        )?;
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["0"],
            "unexpected generic implementation-bound output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn local_partial_runtime_preset_captures_at_construction_issue1124() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
def route(method: int, path: int, content_type: int = 3) -> int:
  return method + path + content_type

def main() -> None:
  mut method = 1
  get = partial route(method=method)
  method = 2
  println(get(4))
  println(get(method=7, path=4))
"#,
            "local_partial_capture_contract",
        )?;
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["8", "14"],
            "the omitted preset must retain its construction-time value while a named call overrides it:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_decorated_variadic_callables_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def preserve[F]() -> ((F) -> F):
    return (func) => func

@preserve()
pub def decorated_total(first: int, second: int, *rest: int, **labels: str) -> int:
    mut total: int = first + second
    for value in rest:
        total = total + value
    if labels["mode"] == "sum":
        return total
    return -1

class Box:
    base: int

    @preserve()
    def total(self, first: int, *rest: int, **labels: str) -> int:
        mut total: int = self.base + first
        for value in rest:
            total = total + value
        if labels["mode"] == "sum":
            return total
        return -1

def main() -> None:
    box = Box(base=5)
    println(decorated_total(1, 2, 3, 4, mode="sum") + box.total(6, 7, 8, mode="sum"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "decorated variadic callable regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["36"], "unexpected decorated variadic output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_decorated_variadic_library_builds() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"decorated_rest_lib\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            root.join("src/lib.incn"),
            r#"
def preserve[F]() -> ((F) -> F):
    return (func) => func

@preserve()
pub def decorated_total(first: int, second: int, *rest: int, **labels: str) -> int:
    mut total: int = first + second
    for value in rest:
        total = total + value
    if labels["mode"] == "sum":
        return total
    return -1

pub class Box:
    base: int

    @preserve()
    def total(self, first: int, *rest: int, **labels: str) -> int:
        mut total: int = self.base + first
        for value in rest:
            total = total + value
        if labels["mode"] == "sum":
            return total
        return -1
"#,
        )?;

        let mut command = incan_command();
        let output = command
            .args(["build", "--lib"])
            .current_dir(root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "decorated variadic library build failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_string_and_bytes_iteration_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = run_incan_source(
            "def main() -> None:\n  mut out = \"\"\n  for ch in \"Az\":\n    out += ch\n  for index, ch in enumerate(\"xy\"):\n    out += f\"{index}{ch}\"\n  mut total = 0\n  for byte in b\"Az\":\n    total += byte\n  for index, byte in enumerate(b\"\\x01\\x02\"):\n    total += index + byte\n  println(out)\n  println(total)\n",
        );

        assert!(
            output.status.success(),
            "incan run string/bytes iteration regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(lines, vec!["Az0x1y", "191"]);

        Ok(())
    }

    #[test]
    fn test_std_fs_compile_and_run_path_file_and_tree_operations() -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("incan_std_fs_integration_{}", std::process::id()));
        let root = base.join("root");
        let copied = base.join("copy");
        let moved = base.join("moved");
        let source = format!(
            r#"
from std.fs import IoError, OpenOptions, Path
from std.tempfile import NamedTemporaryFile, SpooledTemporaryFile, TemporaryDirectory
from rust::std::thread import sleep
from rust::std::time import Duration

def run() -> Result[None, IoError]:
    root = Path("{root}")
    copied = Path("{copied}")
    moved = Path("{moved}")
    if moved.exists():
        moved.remove_tree()?
    if copied.exists():
        copied.remove_tree()?
    if root.exists():
        root.remove_tree()?
    root.mkdir(true, true)?
    root.joinpath("a.txt").write_text("alpha", "utf-8", "strict", None)?
    root.joinpath("c.md").write_text("charlie", "utf-8", "strict", None)?
    root.joinpath("sub").mkdir(true, true)?
    root.joinpath("sub").joinpath("b.txt").write_text("bravo", "utf-8", "strict", None)?
    println(len(root.glob("*.txt")?))
    println(len(root.rglob("*.txt")?))
    println(len(root.rglob("sub/[ab].txt")?))
    match root.joinpath("a.txt").open("r", -1, Some("definitely-not-an-encoding"), None, None):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match root.joinpath("a.txt").open("rbb+", -1, None, None, None):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    default_reader = root.joinpath("a.txt").open()?
    println(default_reader.read(-1)?)
    default_out = root.joinpath("default-open.txt")
    default_writer = default_out.open("w")?
    default_writer.write("delta")?
    default_writer.flush()?
    println(default_out.read_text("utf-8", "strict")?)
    latin = root.joinpath("latin.txt")
    latin.write_bytes(b"\xff")?
    println(len(latin.read_text("windows-1252", "strict")?) > 0)
    match latin.read_text("utf-8", "strict"):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    println(latin.read_text("utf-8", "replace")? != "")
    latin_out = root.joinpath("latin-out.txt")
    latin_out.write_text("€", "windows-1252", "strict", None)?
    println(latin_out.read_text("windows-1252", "strict")? == "€")
    latin_handle_out = root.joinpath("latin-handle-out.txt")
    latin_handle = latin_handle_out.open("w", -1, Some("windows-1252"), Some("strict"), None)?
    latin_handle.write("€")?
    latin_handle.flush()?
    println(latin_handle_out.read_text("windows-1252", "strict")? == "€")
    text_handle = latin.open("r", -1, Some("windows-1252"), Some("strict"), None)?
    println(len(text_handle.read(-1)?) > 0)
    options_file = OpenOptions().write(true).create(true).truncate(true).open(root.joinpath("options.txt"))?
    options_file.write_bytes(b"opts")?
    options_file.flush()?
    println(root.joinpath("options.txt").read_text("utf-8", "strict")?)
    handle = root.joinpath("a.txt").open("rb", 0, None, None, None)?
    chunk = handle.read_exact(2)?
    println(len(chunk))
    source_modified = root.joinpath("a.txt").stat()?.modified_unix()?
    root.copy(copied, true, true)?
    copied_text = copied.joinpath("sub").joinpath("b.txt").read_text("utf-8", "strict")?
    println(copied_text)
    copied_modified = copied.joinpath("a.txt").stat()?.modified_unix()?
    println(copied_modified == source_modified)
    sleep(Duration.from_secs(1))
    copied.joinpath("a.txt").touch(true)?
    touched_modified = copied.joinpath("a.txt").stat()?.modified_unix()?
    println(touched_modified > copied_modified)
    copied.move(moved)?
    println(moved.joinpath("a.txt").exists())
    stat = moved.joinpath("a.txt").stat()?
    println(stat.modified_unix()? > 0)
    usage = moved.disk_usage()?
    println(usage.total > 0 and usage.free > 0)

    staged = root.joinpath("published.next")
    published = root.joinpath("published.txt")
    staged.write_text("published", "utf-8", "strict", None)?
    staged.replace(published)?
    root.sync_directory()?
    println(published.read_text("utf-8", "strict")?)
    match published.sync_directory():
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    rejected_staged = root.joinpath("rejected.next")
    rejected_target = root.joinpath("rejected-directory")
    rejected_staged.write_text("new", "utf-8", "strict", None)?
    rejected_target.mkdir(false, false)?
    match rejected_staged.replace(rejected_target):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    println(rejected_target.is_dir())
    println(rejected_staged.exists())
    held_lock = published.lock_exclusive()?
    match published.try_lock_exclusive()?:
        Some(_) => println("bad")
        None => println("contended")

    file = NamedTemporaryFile.try_new_with("incan-", ".txt", None)?
    path = file.path()
    path.write_text("hello", "utf-8", "strict", None)?
    println(path.read_text("utf-8", "strict")?)

    directory = TemporaryDirectory.try_new_with("incan-dir-", "", None)?
    child = directory.path() / "child.txt"
    child.write_text("world", "utf-8", "strict", None)?
    println(child.read_text("utf-8", "strict")?)

    mut memory = SpooledTemporaryFile(max_size=64)
    memory.write(b"memory")?
    println(memory.rolled_to_disk())
    memory.seek(0, 0)?
    println(len(memory.read(-1)?))

    mut spool = SpooledTemporaryFile(max_size=4)
    spool.write(b"rolled")?
    println(spool.rolled_to_disk())
    println(spool.path()?.exists())
    spool.seek(0, 0)?
    println(len(spool.read(-1)?))
    kept_spool = spool.persist()?
    println(kept_spool.exists())
    kept_spool.unlink()?

    kept_file = file.persist()?
    println(kept_file.exists())
    kept_file.unlink()?

    kept_directory = directory.persist()?
    println(kept_directory.exists())
    kept_directory.remove_tree()?

    moved.remove_tree()?
    root.remove_tree()?
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            root = root.display(),
            copied = copied.display(),
            moved = moved.display()
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.fs smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "1",
                "2",
                "1",
                "invalid_input",
                "invalid_input",
                "alpha",
                "delta",
                "true",
                "invalid_data",
                "true",
                "true",
                "true",
                "true",
                "opts",
                "2",
                "bravo",
                "true",
                "true",
                "true",
                "true",
                "true",
                "published",
                "invalid_input",
                "invalid_input",
                "true",
                "true",
                "contended",
                "hello",
                "world",
                "false",
                "6",
                "true",
                "true",
                "6",
                "true",
                "true",
                "true"
            ],
            "unexpected std.fs output:\n{stdout}"
        );
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_std_fs_replace_rejects_cross_device_without_losing_target() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::MetadataExt;

        let source_root = tempfile::tempdir()?;
        let shared_memory = Path::new("/dev/shm");
        if !shared_memory.is_dir() || fs::metadata(source_root.path())?.dev() == fs::metadata(shared_memory)?.dev() {
            return Ok(());
        }
        let target_root = shared_memory.join(format!(
            "incan_std_fs_cross_device_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir_all(&target_root)?;
        let staged = source_root.path().join("staged.txt");
        let target = target_root.join("published.txt");
        let source = format!(
            r#"
from std.fs import IoError, Path

def run() -> Result[None, IoError]:
    staged = Path("{staged}")
    target = Path("{target}")
    staged.write_text("new", "utf-8", "strict", None)?
    target.write_text("old", "utf-8", "strict", None)?
    match staged.replace(target):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    println(target.read_text("utf-8", "strict")?)
    println(staged.exists())
    return Ok(None)

def main() -> Result[None, IoError]:
    run()?
    return Ok(None)
"#,
            staged = staged.display(),
            target = target.display(),
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let _ = fs::remove_dir_all(&target_root);
        assert!(
            output.status.success(),
            "cross-device replace smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).lines().collect::<Vec<_>>(),
            vec!["cross_device", "old", "true"],
            "cross-device replacement must preserve the target and staged source"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_std_fs_locks_coordinate_between_incan_processes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("published.lock");
        let ready = root.path().join("holder-ready");
        let holder_path = root.path().join("holder.incn");
        let holder_source = format!(
            r#"
from std.fs import IoError, Path
from rust::std::thread import sleep
from rust::std::time import Duration

def hold() -> Result[None, IoError]:
    guard = Path("{target}").lock_shared()?
    Path("{ready}").write_text("ready", "utf-8", "strict", None)?
    # The Rust harness terminates this generated program after the direct probe finishes, so this
    # is a liveness ceiling rather than test latency.
    sleep(Duration.from_secs(300))
    return Ok(None)

def main() -> None:
    match hold():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            target = target.display(),
            ready = ready.display(),
        );
        fs::write(&holder_path, holder_source)?;
        let holder_binary = build_incan_source_binary(&holder_path, &root.path().join("holder-build"))?;

        let probe_path = root.path().join("probe.incn");
        let probe_source = format!(
            r#"
from std.fs import IoError, Path

def main() -> None:
    match Path("{target}").try_lock_shared():
        Ok(Some(_)) => println("shared")
        Ok(None) => println("blocked")
        Err(err) => println(err.message())
    match Path("{target}").try_lock_exclusive():
        Ok(Some(_)) => println("acquired")
        Ok(None) => println("contended")
        Err(err) => println(err.message())
"#,
            target = target.display(),
        );
        fs::write(&probe_path, probe_source)?;
        let probe_binary = build_incan_source_binary(&probe_path, &root.path().join("probe-build"))?;

        let mut holder = Command::new(&holder_binary)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // A cold CI runner must compile the generated holder project before it can create the readiness file. Keep
        // this deadline comfortably above observed cold pinned-toolchain compilation time while retaining a finite
        // failure bound.
        let holder_ready_started = std::time::Instant::now();
        let holder_ready_timeout = Duration::from_secs(120);
        let mut holder_ready = false;
        while holder_ready_started.elapsed() < holder_ready_timeout {
            if ready.exists() {
                holder_ready = true;
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        if !holder_ready {
            let _ = holder.kill();
            let output = holder.wait_with_output()?;
            return Err(format!(
                "Incan lock holder did not become ready. stdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        let probes = Command::new(&probe_binary).output();
        let _ = holder.kill();
        let holder_output = holder.wait_with_output()?;
        let probes = probes?;

        assert!(
            probes.status.success(),
            "lock probes failed. stdout:\n{}\nstderr:\n{}\nholder stdout:\n{}\nholder stderr:\n{}",
            String::from_utf8_lossy(&probes.stdout),
            String::from_utf8_lossy(&probes.stderr),
            String::from_utf8_lossy(&holder_output.stdout),
            String::from_utf8_lossy(&holder_output.stderr),
        );
        assert_eq!(String::from_utf8_lossy(&probes.stdout).trim(), "shared\ncontended");
        Ok(())
    }

    /// Verify that source programs can retain the public SHA-256 hasher across method calls.
    #[test]
    fn test_std_hash_storable_sha256_hasher_issue969() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.hash import Sha256Hasher, sha256

model StructuralSink:
    hasher: Sha256Hasher

    def append(mut self, chunk: bytes) -> None:
        self.hasher.update(chunk)

    def finalize(mut self) -> bytes:
        return self.hasher.finalize_bytes()

def main() -> None:
    mut sink = StructuralSink(hasher=sha256.new())
    sink.append(b"a")
    sink.append(b"bc")
    println(sink.finalize() == sha256.digest(b"abc"))
"#;
        let output = incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "storable SHA-256 hasher program failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");
        Ok(())
    }

    #[test]
    fn test_std_hash_compile_and_run_digest_file_and_error_paths() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.hash's generated-project dependencies in the root Cargo graph so CI fetches them before this smoke
        // runs the generated project under CARGO_NET_OFFLINE.
        use blake2::Digest as _;
        assert_eq!(blake2::Blake2s256::digest(b"abc").len(), 32);
        assert_eq!(blake3::hash(b"abc").as_bytes().len(), 32);
        assert_eq!(md5_010::Md5::digest(b"abc").len(), 16);
        assert_eq!(sha1::Sha1::digest(b"abc").len(), 20);
        assert_eq!(sha2::Sha256::digest(b"abc").len(), 32);
        assert_eq!(sha3::Sha3_256::digest(b"abc").len(), 32);
        let mut xxh32 = xxhash_rust::xxh32::Xxh32::default();
        xxh32.update(b"abc");
        assert_ne!(xxh32.digest(), 0);
        let mut xxh64 = xxhash_rust::xxh64::Xxh64::default();
        xxh64.update(b"abc");
        assert_ne!(xxh64.digest(), 0);
        let mut xxh3 = xxhash_rust::xxh3::Xxh3Default::new();
        xxh3.update(b"abc");
        assert_ne!(xxh3.digest(), 0);

        let payload = std::env::temp_dir().join(format!("incan_std_hash_integration_{}.txt", std::process::id()));
        std::fs::write(&payload, b"abc")?;

        let source = format!(
            r#"
from std.hash import (
    blake2b,
    blake2s,
    blake3,
    HashError,
    file_digest,
    file_hash_u32,
    file_hash_u64,
    file_hash_u128,
    md5,
    reader_digest,
    reader_hash_u32,
    reader_hash_u64,
    reader_hash_u128,
    sha1,
    sha224,
    sha256,
    sha384,
    sha512,
    sha3_224,
    sha3_256,
    sha3_384,
    sha3_512,
    shake128,
    shake256,
    xxh32,
    xxh64,
    xxh3_64,
    xxh3_128,
)
from std.fs import Path
from std.io import BytesIO

def run() -> Result[None, HashError]:
    sha1_digest = sha1.digest(b"abc")
    println(len(sha1_digest))
    println(sha1_digest == b"\xa9\x99\x3e\x36\x47\x06\x81\x6a\xba\x3e\x25\x71\x78\x50\xc2\x6c\x9c\xd0\xd8\x9d")
    println(len(md5.digest(b"abc")))
    println(md5.digest(b"abc") == b"\x90\x01\x50\x98\x3c\xd2\x4f\xb0\xd6\x96\x3f\x7d\x28\xe1\x7f\x72")
    println(len(sha224.digest(b"abc")))
    println(len(sha384.digest(b"abc")))
    println(len(sha512.digest(b"abc")))
    println(len(sha3_224.digest(b"abc")))
    println(len(sha3_256.digest(b"abc")))
    println(len(sha3_384.digest(b"abc")))
    println(len(sha3_512.digest(b"abc")))
    println(len(blake2b.digest(b"abc")))
    println(len(blake2s.digest(b"abc")))
    println(len(blake3.digest(b"abc")))

    mut legacy = sha1.new()
    legacy.update(b"a")
    legacy.update(b"bc")
    println(legacy.finalize_bytes() == sha1_digest)

    digest = sha256.digest(b"abc")
    println(len(digest))

    mut h = sha256.new()
    h.update(b"a")
    h.update(b"bc")
    println(h.finalize_bytes() == digest)

    mut fast = xxh3_64.new()
    fast.update(b"a")
    fast.update(b"bc")
    println(fast.finalize_u64() == xxh3_64.hash_u64(b"abc"))

    println(len(shake128.digest(b"abc", 8)?))
    println(len(shake256.digest(b"abc", 8)?))
    match shake128.digest(b"abc", 0):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)

    path = Path("{payload}")
    missing_path = Path("{missing_payload}")
    match path.open("rb"):
        Ok(file) => println(file_digest(file, "sha256", 1)? == digest)
        Err(err) => return Err(HashError(kind=err.kind, algorithm="open", detail=err.detail))
    println(file_digest(path, "sha1", 1)? == sha1_digest)
    println(file_digest(path, "sha256", 1)? == digest)
    println(len(file_digest(path, "shake128", 1, 8)?))
    println(len(file_digest(path, "shake256", 2, 8)?))
    println(file_hash_u32(path, "xxh32", 1)? == xxh32.hash_u32(b"abc"))
    println(file_hash_u64(path, "xxh3_64", 1)? == xxh3_64.hash_u64(b"abc"))
    println(file_hash_u64(path, "xxh64", 2)? == xxh64.hash_u64(b"abc"))
    println(file_hash_u128(path, "xxh3_128", 2)? == xxh3_128.hash_u128(b"abc"))
    println(reader_digest(BytesIO(b"abc"), "sha256", 1)? == digest)
    println(len(reader_digest(BytesIO(b"abc"), "shake256", 2, 8)?))
    println(reader_hash_u32(BytesIO(b"abc"), "xxh32", 2)? == xxh32.hash_u32(b"abc"))
    println(reader_hash_u64(BytesIO(b"abc"), "xxh3_64", 2)? == xxh3_64.hash_u64(b"abc"))
    println(reader_hash_u64(BytesIO(b"abc"), "xxh64", 2)? == xxh64.hash_u64(b"abc"))
    println(reader_hash_u128(BytesIO(b"abc"), "xxh3_128", 2)? == xxh3_128.hash_u128(b"abc"))

    match file_hash_u64(path, "sha256", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_hash_u64(path, "unknown", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match reader_hash_u64(BytesIO(b"abc"), "sha256", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match reader_hash_u64(BytesIO(b"abc"), "unknown", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_digest(path, "shake128", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_digest(path, "sha256", 0):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match reader_digest(BytesIO(b"abc"), "sha256", 0):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match file_digest(missing_path, "sha256", 1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            payload = payload.display(),
            missing_payload = payload.with_extension("missing").display(),
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let _ = std::fs::remove_file(&payload);
        assert!(
            output.status.success(),
            "incan run std.hash smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "20",
                "true",
                "16",
                "true",
                "28",
                "48",
                "64",
                "28",
                "32",
                "48",
                "64",
                "64",
                "32",
                "32",
                "true",
                "32",
                "true",
                "true",
                "8",
                "8",
                "invalid_length",
                "true",
                "true",
                "true",
                "8",
                "8",
                "true",
                "true",
                "true",
                "true",
                "true",
                "8",
                "true",
                "true",
                "true",
                "true",
                "unsupported_width",
                "unknown_algorithm",
                "unsupported_width",
                "unknown_algorithm",
                "invalid_length",
                "invalid_chunk_size",
                "invalid_chunk_size",
                "not_found"
            ],
            "unexpected std.hash output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_checksum_compile_and_run_crc32_vectors() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.checksum's generated-project dependency in the root Cargo graph so CI fetches it before this smoke
        // runs the generated project under CARGO_NET_OFFLINE.
        assert_eq!(crc32fast::hash(b"abc"), 891568578);

        let source = r#"
from std.checksum import crc32

def main() -> None:
    println(crc32.value(b"abc") == 891568578)
    println(crc32.digest(b"abc") == b"\x35\x24\x41\xc2")

    mut h = crc32.new()
    h.update(b"a")
    h.update(b"bc")
    println(h.finalize_u32() == crc32.value(b"abc"))

    mut bytes = crc32.new()
    bytes.update(b"abc")
    println(bytes.finalize_bytes() == crc32.digest(b"abc"))
"#;
        let output = incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.checksum smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["true", "true", "true", "true"],
            "unexpected std.checksum output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_hash_hmac_compile_and_run_rfc4231_vectors() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.hash's keyed-MAC dependency in the root Cargo graph so CI fetches it before this smoke runs the
        // generated project under CARGO_NET_OFFLINE.
        use hmac::Mac as _;
        let mut probe = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"key")?;
        probe.update(b"abc");
        assert_eq!(probe.finalize_reset().into_bytes().len(), 32);
        probe.update(b"def");
        assert_eq!(probe.finalize().into_bytes().len(), 32);

        // Vectors are RFC 4231's published values for HMAC-SHA256. They are the point of this test: round-tripping
        // our own output against our own input would prove only self-consistency, which is exactly the weakness a
        // keyed MAC exists to fix.
        let source = r#"
from std.hash import hmac_sha256

def main() -> None:
    # RFC 4231 test case 1
    println(hmac_sha256.digest(b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", b"\x48\x69\x20\x54\x68\x65\x72\x65") == b"\xb0\x34\x4c\x61\xd8\xdb\x38\x53\x5c\xa8\xaf\xce\xaf\x0b\xf1\x2b\x88\x1d\xc2\x00\xc9\x83\x3d\xa7\x26\xe9\x37\x6c\x2e\x32\xcf\xf7")
    println(hmac_sha256.verify(b"\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b\x0b", b"\x48\x69\x20\x54\x68\x65\x72\x65", b"\xb0\x34\x4c\x61\xd8\xdb\x38\x53\x5c\xa8\xaf\xce\xaf\x0b\xf1\x2b\x88\x1d\xc2\x00\xc9\x83\x3d\xa7\x26\xe9\x37\x6c\x2e\x32\xcf\xf7"))

    # test case 2, short key
    println(hmac_sha256.digest(b"\x4a\x65\x66\x65", b"\x77\x68\x61\x74\x20\x64\x6f\x20\x79\x61\x20\x77\x61\x6e\x74\x20\x66\x6f\x72\x20\x6e\x6f\x74\x68\x69\x6e\x67\x3f") == b"\x5b\xdc\xc1\x46\xbf\x60\x75\x4e\x6a\x04\x24\x26\x08\x95\x75\xc7\x5a\x00\x3f\x08\x9d\x27\x39\x83\x9d\xec\x58\xb9\x64\xec\x38\x43")
    println(hmac_sha256.verify(b"\x4a\x65\x66\x65", b"\x77\x68\x61\x74\x20\x64\x6f\x20\x79\x61\x20\x77\x61\x6e\x74\x20\x66\x6f\x72\x20\x6e\x6f\x74\x68\x69\x6e\x67\x3f", b"\x5b\xdc\xc1\x46\xbf\x60\x75\x4e\x6a\x04\x24\x26\x08\x95\x75\xc7\x5a\x00\x3f\x08\x9d\x27\x39\x83\x9d\xec\x58\xb9\x64\xec\x38\x43"))

    # test case 3, 0xdd payload
    println(hmac_sha256.digest(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd") == b"\x77\x3e\xa9\x1e\x36\x80\x0e\x46\x85\x4d\xb8\xeb\xd0\x91\x81\xa7\x29\x59\x09\x8b\x3e\xf8\xc1\x22\xd9\x63\x55\x14\xce\xd5\x65\xfe")
    println(hmac_sha256.verify(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd\xdd", b"\x77\x3e\xa9\x1e\x36\x80\x0e\x46\x85\x4d\xb8\xeb\xd0\x91\x81\xa7\x29\x59\x09\x8b\x3e\xf8\xc1\x22\xd9\x63\x55\x14\xce\xd5\x65\xfe"))

    # test case 4, incrementing key
    println(hmac_sha256.digest(b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19", b"\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd") == b"\x82\x55\x8a\x38\x9a\x44\x3c\x0e\xa4\xcc\x81\x98\x99\xf2\x08\x3a\x85\xf0\xfa\xa3\xe5\x78\xf8\x07\x7a\x2e\x3f\xf4\x67\x29\x66\x5b")
    println(hmac_sha256.verify(b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19", b"\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd\xcd", b"\x82\x55\x8a\x38\x9a\x44\x3c\x0e\xa4\xcc\x81\x98\x99\xf2\x08\x3a\x85\xf0\xfa\xa3\xe5\x78\xf8\x07\x7a\x2e\x3f\xf4\x67\x29\x66\x5b"))

    # test case 6, key longer than the block size
    println(hmac_sha256.digest(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\x54\x65\x73\x74\x20\x55\x73\x69\x6e\x67\x20\x4c\x61\x72\x67\x65\x72\x20\x54\x68\x61\x6e\x20\x42\x6c\x6f\x63\x6b\x2d\x53\x69\x7a\x65\x20\x4b\x65\x79\x20\x2d\x20\x48\x61\x73\x68\x20\x4b\x65\x79\x20\x46\x69\x72\x73\x74") == b"\x60\xe4\x31\x59\x1e\xe0\xb6\x7f\x0d\x8a\x26\xaa\xcb\xf5\xb7\x7f\x8e\x0b\xc6\x21\x37\x28\xc5\x14\x05\x46\x04\x0f\x0e\xe3\x7f\x54")
    println(hmac_sha256.verify(b"\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa", b"\x54\x65\x73\x74\x20\x55\x73\x69\x6e\x67\x20\x4c\x61\x72\x67\x65\x72\x20\x54\x68\x61\x6e\x20\x42\x6c\x6f\x63\x6b\x2d\x53\x69\x7a\x65\x20\x4b\x65\x79\x20\x2d\x20\x48\x61\x73\x68\x20\x4b\x65\x79\x20\x46\x69\x72\x73\x74", b"\x60\xe4\x31\x59\x1e\xe0\xb6\x7f\x0d\x8a\x26\xaa\xcb\xf5\xb7\x7f\x8e\x0b\xc6\x21\x37\x28\xc5\x14\x05\x46\x04\x0f\x0e\xe3\x7f\x54"))

    # A tag from the right data under the wrong key must not verify.
    println(hmac_sha256.verify(b"wrong-key", b"\x48\x69\x20\x54\x68\x65\x72\x65", b"\xb0\x34\x4c\x61\xd8\xdb\x38\x53\x5c\xa8\xaf\xce\xaf\x0b\xf1\x2b\x88\x1d\xc2\x00\xc9\x83\x3d\xa7\x26\xe9\x37\x6c\x2e\x32\xcf\xf7"))

    # Streaming the message in pieces must agree with the one-shot tag.
    mut signer = hmac_sha256.new(b"Jefe")
    signer.update(b"what do ya want ")
    signer.update(b"for nothing?")
    println(signer.finalize_bytes() == b"\x5b\xdc\xc1\x46\xbf\x60\x75\x4e\x6a\x04\x24\x26\x08\x95\x75\xc7\x5a\x00\x3f\x08\x9d\x27\x39\x83\x9d\xec\x58\xb9\x64\xec\x38\x43")
"#;
        let output = incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.hash hmac smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "true", "true", "true", "true", "true", "true", "true", "true", "true", "true", "false", "true"
            ],
            "unexpected std.hash hmac output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_io_compile_and_run_bytesio_core_and_numeric_helpers() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.io's generated-project dependency in the root Cargo graph so CI fetches it before this smoke runs
        // the generated project under CARGO_NET_OFFLINE.
        let mut cache_anchor = [0u8; 4];
        <byteorder::LittleEndian as byteorder::ByteOrder>::write_u32(&mut cache_anchor, 258);
        assert_eq!(cache_anchor, [2, 1, 0, 0]);

        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.derives.collection import FallibleIterator
from std.io import BinaryReader, BytesIO, Endian, IoError
from std.traits.callable import Callable1, Callable2

model NumberStream with FallibleIterator[int, str]:
    items: list[int]
    index: int
    fail_at: Option[int]

    def __next__(mut self) -> Result[Option[int], str]:
        match self.fail_at:
            Some(index) =>
                if self.index == index:
                    return Err("boom")
            None => pass
        if self.index >= len(self.items):
            return Ok(None)
        item = self.items[self.index]
        self.index += 1
        return Ok(Some(item))

def double(value: int) -> int:
    return value * 2

def is_even(value: int) -> bool:
    return value % 2 == 0

def expand(value: int) -> list[int]:
    return [value, value + 10]

def expand_empty(value: int) -> list[int]:
    return []

def observe_item(value: int) -> None:
    println(f"seen:{value}")

def observe_error(error: str) -> None:
    println(f"source-error:{error}")

def prefix_error(error: str) -> str:
    return f"mapped:{error}"

def add(left: int, right: int) -> int:
    return left + right

@derive(Clone)
model Multiplier with Callable1[int, int]:
    factor: int

    def __call__(self, value: int) -> int:
        return value * self.factor

@derive(Clone)
model ScaledAdd with Callable2[int, int, int]:
    factor: int

    def __call__(self, left: int, right: int) -> int:
        return left + right * self.factor

model TracingStream with FallibleIterator[int, str]:
    items: list[int]
    index: int
    fail_at: Option[int]

    def __next__(mut self) -> Result[Option[int], str]:
        println(f"poll:{self.index}")
        match self.fail_at:
            Some(index) =>
                if self.index == index:
                    return Err("trace-boom")
            None => pass
        if self.index >= len(self.items):
            return Ok(None)
        item = self.items[self.index]
        self.index += 1
        return Ok(Some(item))

def trace_map(value: int) -> int:
    println(f"map-callback:{value}")
    return value * 2

def trace_filter(value: int) -> bool:
    println(f"filter-callback:{value}")
    return true

def trace_expand(value: int) -> list[int]:
    println(f"flat-map-callback:{value}")
    return [value, value + 10]

def trace_inspect(value: int) -> None:
    println(f"inspect-callback:{value}")

def trace_inspect_error(error: str) -> None:
    println(f"inspect-error-callback:{error}")

def trace_map_error(error: str) -> str:
    println(f"map-error-callback:{error}")
    return f"mapped:{error}"

def exercise_fallible_adapters() -> None:
    traced = TracingStream(items=[1, 2], index=0, fail_at=Some(2)).map(trace_map).filter(trace_filter).flat_map(trace_expand).take(10).inspect(trace_inspect).inspect_err(trace_inspect_error).map_err(trace_map_error)
    println("pipeline:constructed")
    match traced.collect():
        Ok(_) => println("bad")
        Err(error) => println(f"pipeline-error:{error}")

    match NumberStream(items=[1, 2, 3], index=0, fail_at=None).map(double).collect():
        Ok(values) => println(f"map:{values[0]}:{values[1]}:{values[2]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=None).map(Multiplier(factor=3)).collect():
        Ok(values) => println(f"model-map:{values[0]}:{values[1]}")
        Err(error) => println(error)

    offset = 4
    match NumberStream(items=[1, 2], index=0, fail_at=None).map((value) => value + offset).collect():
        Ok(values) => println(f"closure-map:{values[0]}:{values[1]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2, 3, 4], index=0, fail_at=None).filter(is_even).collect():
        Ok(values) => println(f"filter:{values[0]}:{values[1]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=None).flat_map(expand).collect():
        Ok(values) => println(f"flat:{values[0]}:{values[1]}:{values[2]}:{values[3]}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=None).flat_map(expand_empty).collect():
        Ok(values) => println(f"flat-empty:{len(values)}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2], index=0, fail_at=Some(1)).take(1).collect():
        Ok(values) => println(f"take:{values[0]}")
        Err(error) => println(error)

    match NumberStream(items=[3, 4], index=0, fail_at=None).inspect(observe_item).collect():
        Ok(values) => println(f"inspect:{len(values)}")
        Err(error) => println(error)

    errors = NumberStream(items=[7, 8], index=0, fail_at=Some(1)).inspect_err(observe_error).map_err(prefix_error)
    match errors.collect():
        Ok(_) => println("bad")
        Err(error) => println(f"error:{error}")

    match NumberStream(items=[1, 2, 3], index=0, fail_at=None).fold(0, add):
        Ok(value) => println(f"fold:{value}")
        Err(error) => println(error)

    match NumberStream(items=[1, 2, 3], index=0, fail_at=None).fold(0, ScaledAdd(factor=2)):
        Ok(value) => println(f"model-fold:{value}")
        Err(error) => println(error)

    match NumberStream(items=[4, 5], index=0, fail_at=Some(1)).inspect(observe_item).fold(0, add):
        Ok(_) => println("bad")
        Err(error) => println(f"fold-error:{error}")

    println("take-zero:start")
    match TracingStream(items=[9], index=0, fail_at=Some(0)).take(0).collect():
        Ok(values) => println(f"take-zero:{len(values)}")
        Err(error) => println(error)
    println("take-zero:end")

model FailingReader with BinaryReader:
    def read_bytes(self, size: int) -> Result[bytes, IoError]:
        return Err(IoError(kind="other", detail="read failed", operation="read_bytes"))

def propagate_reader_error() -> Result[None, IoError]:
    for _ in FailingReader().chunks(2)?:
        pass
    return Ok(None)

def error_kind(err: IoError) -> str:
    return err.kind

def map_reader_error() -> Result[None, str]:
    for _ in FailingReader().chunks(2).map_err(error_kind)?:
        pass
    return Ok(None)

def run() -> Result[None, IoError]:
    exercise_fallible_adapters()
    buf = BytesIO(b"abc\0rest")
    first = buf.read(2)?
    println(len(first))
    println(buf.tell())
    buf.rewind()?
    nul: u8 = 0
    letter_t: u8 = 116
    until = buf.read_until(nul)?
    println(len(until))
    println(buf.remaining())
    println(buf.skip_until(letter_t)?)
    println(buf.remaining())
    match buf.read_exact(1):
        Ok(_) => println("bad")
        Err(err) => println(err.kind)

    for chunk in BytesIO(b"abcde").chunks(2)?:
        println(len(chunk))
    for _ in BytesIO(b"").chunks(2)?:
        println("bad")
    match FailingReader().chunks(0).__next__():
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match propagate_reader_error():
        Ok(_) => println("bad")
        Err(err) => println(err.kind)
    match map_reader_error():
        Ok(_) => println("bad")
        Err(kind) => println(kind)

    out = BytesIO()
    u32_value: u32 = 258
    i16_value: i16 = -2
    u128_value: u128 = 42
    f64_value: f64 = 1.5
    out.write(u32_value, Endian.Little)?
    out.write(i16_value, Endian.Big)?
    out.write(u128_value, Endian.Big)?
    out.write(f64_value, Endian.Little)?
    println(len(out.getvalue()))
    out.rewind()?
    read_u32: u32 = out.read(Endian.Little)?
    read_i16: i16 = out.read(Endian.Big)?
    read_u128: u128 = out.read(Endian.Big)?
    read_f64: f64 = out.read(Endian.Little)?
    println(read_u32)
    println(read_i16)
    println(read_u128)
    println(read_f64 == f64_value)

    rewrite = BytesIO(b"abcd")
    rewrite.seek(1, 0)?
    xy: bytes = b"XY"
    rewrite.write(xy)?
    rewrite.truncate(Some(3))?
    println(len(rewrite.getvalue()))
    println(rewrite.remaining())
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .env(
                "INCAN_STDLIB",
                repo_root().join("loaves/stdlib"),
            )
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.io smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "pipeline:constructed",
                "poll:0",
                "map-callback:1",
                "filter-callback:2",
                "flat-map-callback:2",
                "inspect-callback:2",
                "inspect-callback:12",
                "poll:1",
                "map-callback:2",
                "filter-callback:4",
                "flat-map-callback:4",
                "inspect-callback:4",
                "inspect-callback:14",
                "poll:2",
                "inspect-error-callback:trace-boom",
                "map-error-callback:trace-boom",
                "pipeline-error:mapped:trace-boom",
                "map:2:4:6",
                "model-map:3:6",
                "closure-map:5:6",
                "filter:2:4",
                "flat:1:11:2:12",
                "flat-empty:0",
                "take:1",
                "seen:3",
                "seen:4",
                "inspect:2",
                "source-error:boom",
                "error:mapped:boom",
                "fold:6",
                "model-fold:12",
                "seen:4",
                "fold-error:boom",
                "take-zero:start",
                "take-zero:0",
                "take-zero:end",
                "2",
                "2",
                "4",
                "4",
                "4",
                "0",
                "unexpected_eof",
                "2",
                "2",
                "1",
                "invalid_input",
                "other",
                "other",
                "30",
                "258",
                "-2",
                "42",
                "true",
                "3",
                "0"
            ],
            "unexpected std.io output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_encoding_hex_compile_and_run_strict_surface() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/std_encoding_hex_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.encoding.hex smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "417a00",
                "3",
                "417a00",
                "417a00",
                "FF",
                "10",
                "00",
                "7f",
                "invalid_length",
                "invalid_character"
            ],
            "unexpected std.encoding.hex output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_fs_glob_string_api_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.fs.glob import filter_matches, matches

def main() -> None:
    println(matches("routes/users.incn", "routes/*.incn"))
    println(matches("routes/users.incn", "routes/[a-z]*.incn"))
    println(matches("routes/users.incn", "routes/[!0-9]*.incn"))
    println(matches("routes/users.incn", "routes/?.incn"))
    hits = filter_matches(["api/users", "docs/readme", "api/orders"], "api/*")
    println(len(hits))
    println(hits[0])
    println(hits[1])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "std.fs.glob string API failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["true", "true", "true", "false", "2", "api/users", "api/orders"],
            "unexpected std.fs.glob output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_imported_default_constructor_fields_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_defaults");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("config.incn"),
            r#"
pub model Config:
    pub enabled: bool = false
    pub retries: int = 3
"#,
        )?;
        let main_path = root.join("default_ctor.incn");
        fs::write(
            &main_path,
            r#"
from pkg.config import Config

def main() -> None:
    cfg = Config()
    println(cfg.enabled)
    println(cfg.retries)
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported default constructor regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "false\n3");
        Ok(())
    }

    #[test]
    fn test_imported_private_class_constructor_compile_and_run_issue886() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_private_class_constructor");
        let package = root.join("src").join("pkg");
        fs::create_dir_all(&package)?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"imported_private_class_constructor\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            package.join("text_vaults.incn"),
            r#"
pub class Vault:
    secret: str = "sealed"
    pub label: str
    revision: int = 7

    def private_value(self) -> str:
        return self.secret

    def revision_value(self) -> int:
        return self.revision
"#,
        )?;
        fs::write(
            package.join("number_vaults.incn"),
            r#"
pub class Vault:
    secret: int = 41
    pub label: int
    revision: str = "r2"

    def private_value(self) -> int:
        return self.secret

    def revision_value(self) -> str:
        return self.revision
"#,
        )?;
        fs::write(
            package.join("vault_facade.incn"),
            "pub from text_vaults import Vault as FacadeVault\n",
        )?;
        fs::write(
            package.join("public_api.incn"),
            "pub from vault_facade import FacadeVault as ExportedVault\n",
        )?;
        let main_path = package.join("consumer.incn");
        // Direct, same-leaf alias, and multi-hop facade construction share one
        // source-resolution path, so one executable proves their coexistence.
        fs::write(
            &main_path,
            r#"
from text_vaults import Vault
from text_vaults import Vault as TextVault
from number_vaults import Vault as NumberVault
from public_api import ExportedVault as ConsumerVault

def main() -> None:
    direct = Vault(label="direct")
    text = TextVault(label="visible", revision=9)
    number = NumberVault(label=5)
    facade = ConsumerVault(label="facade", revision=11)
    println(direct.label)
    println(direct.private_value())
    println(direct.revision_value())
    println(text.label)
    println(text.private_value())
    println(text.revision_value())
    println(number.label)
    println(number.private_value())
    println(number.revision_value())
    println(facade.label)
    println(facade.private_value())
    println(facade.revision_value())
"#,
        )?;

        let direct_output = incan_command()
            .current_dir(&root)
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            direct_output.status.success(),
            "direct imported private class constructor failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            direct_output.status,
            String::from_utf8_lossy(&direct_output.stdout),
            String::from_utf8_lossy(&direct_output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&direct_output.stdout).trim(),
            "direct\nsealed\n7\nvisible\nsealed\n9\n5\n41\nr2\nfacade\nsealed\n11"
        );

        let tests_dir = root.join("tests");
        fs::create_dir_all(&tests_dir)?;
        fs::write(
            tests_dir.join("test_private_class_facade.incn"),
            r#"
from pkg.public_api import ExportedVault as TestVault

def test_private_class_facade_constructor() -> None:
    value = TestVault(label="test-batch", revision=13)
    assert value.label == "test-batch"
    assert value.private_value() == "sealed"
    assert value.revision_value() == 13
"#,
        )?;
        let test_output = incan_command()
            .current_dir(&root)
            .args(["test", tests_dir.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            test_output.status.success(),
            "multi-hop source facade constructor failed in a generated test batch: status={:?}\nstdout:\n{}\nstderr:\n{}",
            test_output.status,
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&test_output.stdout).contains("test_private_class_facade_constructor"),
            "expected generated test batch to execute the facade constructor regression:\n{}",
            String::from_utf8_lossy(&test_output.stdout)
        );

        fs::write(
            &main_path,
            r#"
from text_vaults import Vault as PrivateVault

def main() -> None:
    value = PrivateVault(label="visible")
    println(value.secret)
"#,
        )?;
        let private_check = incan_command()
            .current_dir(&root)
            .args(["--check", main_path.to_string_lossy().as_ref()])
            .output()?;
        assert!(
            !private_check.status.success(),
            "expected imported private-field access to fail Incan typechecking"
        );
        let private_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            private_stderr.contains("Field 'secret' on 'PrivateVault' is private"),
            "expected source-level private-field diagnostic, got:\n{private_stderr}"
        );
        Ok(())
    }

    #[test]
    fn test_imported_value_enum_ordinal_map_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_ordinal_enum");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("status.incn"),
            r#"
pub enum Status(str):
    Open = "open"
    Paid = "paid"
    Cancelled = "cancelled"
"#,
        )?;
        let main_path = root.join("ordinal_enum.incn");
        fs::write(
            &main_path,
            r#"
from std.collections import OrdinalMap
from pkg.status import Status

def main() -> None:
    statuses: list[Status] = [Status.Open, Status.Paid, Status.Cancelled]
    match OrdinalMap.from_keys(statuses):
        Ok(columns) => match columns.require(Status.Paid):
            Ok(value) => println(value)
            Err(err) => println(err.message())
        Err(err) => println(err.message())
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported value-enum OrdinalMap regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
        Ok(())
    }

    #[test]
    fn test_imported_pascal_case_function_is_not_constructor() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_pascal_case_function");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("factory.incn"),
            r#"
pub def BytesIO(initial: int = 7) -> int:
    return initial

pub class FactoryValue:
    secret: str = "sealed"
    pub label: str

pub def MakeValue(label: str) -> FactoryValue:
    return FactoryValue(label=label)
"#,
        )?;
        let main_path = root.join("factory_call.incn");
        fs::write(
            &main_path,
            r#"
from pkg.factory import BytesIO, MakeValue

def main() -> None:
    println(BytesIO())
    println(BytesIO(3))
    value = MakeValue(label="factory")
    println(value.label)
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported PascalCase function regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "7\n3\nfactory");
        Ok(())
    }

    #[test]
    fn test_imported_method_union_arg_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let root = make_temp_dir("incan_imported_method_union_arg");
        fs::create_dir_all(root.join("pkg"))?;
        fs::write(
            root.join("pkg").join("ops.incn"),
            r#"
pub model LocalPath:
    pub raw: str

pub class Opener:
    def accept(self, path: Union[LocalPath, str]) -> str:
        return "ok"
"#,
        )?;
        let main_path = root.join("union_arg.incn");
        fs::write(
            &main_path,
            r#"
from pkg.ops import LocalPath, Opener

def main() -> None:
    println(Opener().accept(LocalPath(raw="a")))
    println(Opener().accept("b"))
"#,
        )?;
        let output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "imported method union argument regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "ok\nok");
        Ok(())
    }

    #[test]
    fn test_std_fs_preserves_legacy_file_builtins() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("incan_std_fs_legacy_builtin_{}.txt", std::process::id()));
        let source = format!(
            r#"
def main() -> None:
    match write_file("{path}", "legacy"):
        Ok(_) => pass
        Err(err) => println(err.to_string())
    match read_file("{path}"):
        Ok(data) => println(data)
        Err(err) => println(err.to_string())
"#,
            path = path.display()
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "legacy file builtins failed after std.fs registration: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(stdout.trim(), "legacy", "unexpected legacy builtin output:\n{stdout}");
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn legacy_file_builtin_errors_honor_declared_string_type_issue874() -> Result<(), Box<dyn std::error::Error>> {
        let missing_root = std::env::temp_dir().join(format!(
            "incan_legacy_file_error_type_issue874_{}_missing",
            std::process::id()
        ));
        let read_path = missing_root.join("read.txt");
        let write_path = missing_root.join("write.txt");
        let source = format!(
            r#"
def normalize(error: str) -> str:
    return error.upper()

def main() -> None:
    match read_file("{read_path}").map_err((error) => normalize(error)):
        Ok(_) => println("unexpected read success")
        Err(error) => println(error)
    match write_file("{write_path}", "data").map_err((error) => normalize(error)):
        Ok(_) => println("unexpected write success")
        Err(error) => println(error)
    match read_file().map_err((error) => normalize(error)):
        Ok(_) => println("unexpected missing read argument success")
        Err(error) => println(error)
    match write_file().map_err((error) => normalize(error)):
        Ok(_) => println("unexpected missing write arguments success")
        Err(error) => println(error)
"#,
            read_path = read_path.display(),
            write_path = write_path.display(),
        );
        let output = incan_command()
            .args(["run", "-c", source.as_str()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "legacy file builtin string error regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout)?;
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines.len(),
            4,
            "expected valid- and missing-argument errors for read and write:\n{stdout}"
        );
        assert!(
            lines
                .iter()
                .all(|line| !line.is_empty() && *line == line.to_uppercase())
        );
        Ok(())
    }

    #[test]
    fn rust_result_non_clone_payload_routes_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::fs import read_dir
from rust::std::fs import ReadDir
from rust::std::path import Path as RustPath

def observe_entries(_entries: ReadDir) -> None:
    pass

def tap[T, E](result: Result[T, E], f: Callable[T, None]) -> Result[T, E]:
    match result:
        Ok(value) =>
            f(value)
            return Ok(value)
        Err(error) => return Err(error)

def main() -> None:
    match read_dir(RustPath.new(".")):
        Ok(entries) =>
            mut seen = False
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
            println(seen)
        Err(err) => println(err.to_string())
    inspected = read_dir(RustPath.new(".")).inspect(observe_entries)
    match inspected:
        Ok(entries) =>
            mut seen = False
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
            println(seen)
        Err(err) => println(err.to_string())
    tapped = tap(read_dir(RustPath.new(".")), observe_entries)
    match tapped:
        Ok(entries) =>
            mut seen = False
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
            println(seen)
        Err(err) => println(err.to_string())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "non-Clone Rust Result route regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["true", "true", "true"],
            "expected direct match, Result.inspect, and user-authored tap routes to consume the non-Clone payload:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_result_execution_matrix() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.result import map as result_map, map_err as result_map_err
from std.result import and_then as result_and_then, or_else as result_or_else
from std.traits.callable import Callable1

def result_double(value: int) -> int:
    return value * 2

def result_prefix(error: str) -> str:
    return f"error: {error}"

def result_keep_even(value: int) -> Result[int, str]:
    if value % 2 == 0:
        return Ok(value)
    return Err("odd")

def result_recover(_error: str) -> Result[int, str]:
    return Ok(7)

model ResultPrefixer with Callable1[str, str]:
    prefix: str

    def __call__(self, error: str) -> str:
        return f"{self.prefix}: {error}"

def result_from_return() -> Result[str, str]:
    return Ok("from_return")

def std_result_free_helpers() -> None:
    ok_value: Result[int, str] = Ok(2)
    err_value: Result[int, str] = Err("bad")
    even_value: Result[int, str] = Ok(4)
    missing_value: Result[int, str] = Err("missing")
    match result_map(ok_value, result_double):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_map_err(err_value, result_prefix):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_and_then(even_value, result_keep_even):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_or_else(missing_value, result_recover):
        Ok(value) => println(value)
        Err(error) => println(error)

def result_methods() -> None:
    ok_value: Result[int, str] = Ok(2)
    err_value: Result[int, str] = Err("bad")
    missing_value: Result[int, str] = Err("missing")
    match ok_value.map(result_double).and_then(result_keep_even):
        Ok(value) => println(value)
        Err(error) => println(error)
    match err_value.map_err(result_prefix):
        Ok(value) => println(value)
        Err(error) => println(error)
    match missing_value.or_else(result_recover).map(result_double):
        Ok(value) => println(value)
        Err(error) => println(error)

def result_callable_object() -> None:
    value: Result[int, str] = Err("bad")
    match value.map_err(ResultPrefixer(prefix="error")):
        Ok(value) => println(value)
        Err(error) => println(error)

def result_capturing_closure() -> None:
    prefix = "uuid"
    value: Result[int, str] = Err("bad")
    mapped = value.map_err((err) => f"{prefix}: {err}")
    match mapped:
        Ok(number) => println(number)
        Err(error) => println(error)

def result_string_literals() -> None:
    direct: Result[str, str] = Ok("from_call")
    match direct:
        case Ok(msg):
            println(msg)
        case Err(err):
            println(err)
    match result_from_return():
        case Ok(msg):
            println(msg)
        case Err(err):
            println(err)

def main() -> None:
    std_result_free_helpers()
    result_methods()
    result_callable_object()
    result_capturing_closure()
    result_string_literals()
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Result execution matrix failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "4",
                "error: bad",
                "4",
                "7",
                "4",
                "error: bad",
                "14",
                "error: bad",
                "uuid: bad",
                "from_call",
                "from_return",
            ],
            "unexpected Result execution matrix output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_question_mark_comprehensions_propagate_results_issue633() -> Result<(), Box<dyn std::error::Error>> {
        let output = run_incan_source(
            r#"
def parse_value(value: int) -> Result[int, str]:
    if value == 2:
        return Err("bad value")
    return Ok(value)


def parse_all(values: list[int]) -> Result[list[int], str]:
    return Ok([parse_value(value)? for value in values])


def parse_key(value: int) -> Result[str, str]:
    if value == 2:
        return Err("bad key")
    return Ok(str(value))


def parse_map(values: list[int]) -> Result[dict[str, int], str]:
    return Ok({parse_key(value)?: value for value in values})


def main() -> None:
    match parse_all([1, 2, 3]):
        Ok(values) => println(values[0])
        Err(err) => println(err)
    match parse_map([1, 2, 3]):
        Ok(values) => println(values["1"])
        Err(err) => println(err)
"#,
        );
        assert!(
            output.status.success(),
            "question-mark comprehension regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["bad value", "bad key"],
            "unexpected issue633 output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_static_str_index_and_slice_use_string_helpers() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
const ALPHABET: str = "abcdef"

def main() -> None:
    println(ALPHABET[1])
    println(ALPHABET[2:5])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "static str index/slice regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["b", "cde"], "unexpected static str output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_collection_literal_spreads_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    tail: tuple[int, int] = (4, 5)
    values = [1, *[2, 3], *tail]
    defaults = {"trace": "disabled", "accept": "json"}
    merged = {**defaults, "trace": "enabled"}
    println(values[0] + values[1] + values[2] + values[3] + values[4])
    println(merged["trace"])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "collection literal spread run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["15", "enabled"],
            "unexpected collection spread output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_enum_methods_and_trait_adoption_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
trait Labelled:
    def label(self) -> str: ...

enum Signal with Labelled:
    Start
    Stop

    def label(self) -> str:
        match self:
            Signal.Start => return "start"
            Signal.Stop => return "stop"

    def default() -> Self:
        return Signal.Start

def keep_labelled[T with Labelled](value: T) -> T:
    return value

def main() -> None:
    signal = keep_labelled(Signal.default())
    println(signal.label())
    println(Signal.Stop.label())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "enum methods and trait adoption run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["start", "stop"], "unexpected enum method output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_union_types_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
type LocalPath = newtype str

def normalize_path_like(value: LocalPath | str) -> LocalPath:
    if isinstance(value, str):
        return LocalPath(value)
    elif isinstance(value, LocalPath):
        return value

def parse_value(flag: bool) -> int | str:
    if flag:
        return 42
    return "fallback"

def normalize(value: int | str) -> str:
    if isinstance(value, int):
        return "number"
    else:
        return value.upper()

def describe(value: int | str) -> str:
    match value:
        int(n) =>
            return str(n)
        str(s) =>
            return s.upper()

def label(value: str | None) -> str:
    if value is not None:
        return value.upper()
    return "missing"

def describe_optional(value: int | str | None) -> str:
    match value:
        int(n) =>
            return str(n)
        str(s) =>
            return s.upper()
        None =>
            return "missing"

def describe_wide(value: int | str | bool) -> str:
    if isinstance(value, int):
        return "number"
    else:
        match value:
            bool(flag) =>
                if flag:
                    return "true"
                return "false"
            str(text) =>
                return text.upper()

def describe_chain(value: int | str | bool) -> str:
    if isinstance(value, int):
        return "number"
    elif isinstance(value, str):
        return value.upper()
    else:
        if value:
            return "true"
        return "false"

def describe_wide_chain(value: int | float | str | bool) -> str:
    if isinstance(value, bool):
        return "bool"
    elif isinstance(value, int):
        return "int"
    elif isinstance(value, float):
        return "float"
    elif isinstance(value, str):
        return value.upper()
    return "unknown"

def describe_wide_match(value: int | float | str | bool) -> str:
    match value:
        bool(flag) =>
            if flag:
                return "bool:true"
            return "bool:false"
        int(n) =>
            return str(n)
        float(f) =>
            return str(f)
        str(s) =>
            return s.upper()

def describe_optional_narrow(value: int | str | None) -> str:
    if isinstance(value, int):
        return "number"
    else:
        if value is None:
            return "missing"
        else:
            return value.upper()

def main() -> None:
    println(normalize(parse_value(False)))
    println(normalize(parse_value(True)))
    println(describe(parse_value(False)))
    println(label("present"))
    println(label(None))
    println(describe_optional(parse_value(True)))
    println(describe_optional(None))
    println(describe_wide("wide"))
    println(describe_wide(True))
    println(describe_chain("chain"))
    println(describe_chain(False))
    println(describe_wide_chain("wide-chain"))
    println(describe_wide_chain(1.25))
    println(describe_wide_match(True))
    println(describe_wide_match(7))
    println(describe_wide_match(2.5))
    println(describe_wide_match("match"))
    println(describe_optional_narrow("optional"))
    println(describe_optional_narrow(None))
    println(normalize_path_like("from-string").0)
    println(normalize_path_like(LocalPath("from-path")).0)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "union type run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "FALLBACK",
                "number",
                "FALLBACK",
                "PRESENT",
                "missing",
                "42",
                "missing",
                "WIDE",
                "true",
                "CHAIN",
                "false",
                "WIDE-CHAIN",
                "float",
                "bool:true",
                "7",
                "2.5",
                "MATCH",
                "OPTIONAL",
                "missing",
                "from-string",
                "from-path"
            ],
            "unexpected union output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_union_model_variants_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model Leaf:
    value: int

@derive(Clone)
model Pair:
    args: list[Expr]

type Expr = Union[Leaf, Pair]

def pair() -> Expr:
    return Pair(args=[Leaf(value=1), Leaf(value=2)])

def clone_expr(expr: Expr) -> Expr:
    return expr.clone()

def sum_expr(expr: Expr) -> int:
    match expr:
        Leaf(leaf) =>
            return leaf.value
        Pair(pair) =>
            return sum_expr(pair.args[0])

def main() -> None:
    println(sum_expr(clone_expr(pair())))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "union model variant run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1"], "unexpected union model variant output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_imported_union_alias_list_field_compiles_issue622() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("union_list_cross_module_alias_repro");
        fs::create_dir_all(project_root.join("src"))?;
        fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"union_list_cross_module_alias_repro\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            project_root.join("src/exprs.incn"),
            r#"
@derive(Clone)
pub model Leaf:
    pub value: int

@derive(Clone)
pub model Pair:
    pub args: list[Expr]

pub type Expr = Union[Leaf, Pair]

pub def pair() -> Expr:
    return Pair(args=[Leaf(value=1), Leaf(value=2)])
"#,
        )?;
        fs::write(
            project_root.join("src/lib.incn"),
            r#"
from exprs import Expr, Leaf, Pair, pair

def sum_expr(expr: Expr) -> int:
    match expr:
        Leaf(leaf) => return leaf.value
        Pair(pair_expr) => return sum_expr(pair_expr.args[0])

pub def main_value() -> int:
    return sum_expr(pair())
"#,
        )?;

        let output = incan_command()
            .args(["build", "--lib"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "expected imported union alias list-field project to build for #622.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_keyword_named_public_alias_compiles_issue669() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("keyword_named_public_alias_repro");
        fs::create_dir_all(&project_root)?;
        fs::write(
            project_root.join("test_keyword_alias_probe.incn"),
            r#"
pub def modulo_value(value: int) -> int:
    return value

pub mod = alias modulo_value


def test_keyword_alias_probe__can_call_alias() -> None:
    assert mod(7) == 7, "keyword alias should call the implementation"
"#,
        )?;

        let output = incan_command()
            .args(["test", "test_keyword_alias_probe.incn"])
            .current_dir(&project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "expected keyword-named public alias test project to pass for #669.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_issue562_type_alias_dict_and_union_surfaces_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
type FieldValue = str | bool | int | float | None
type Fields = Dict[str, FieldValue]

model Logger:
    fields: Fields = {}

    def copy_fields(self, extra: Fields) -> Fields:
        mut merged: Fields = {}
        for key in self.fields.keys():
            merged[key] = self.fields[key]
        for key in extra.keys():
            merged[key] = extra[key]
        return merged

def to_text(value: FieldValue) -> str:
    match value:
        str(text) =>
            return text
        bool(flag) =>
            if flag:
                return "true"
            return "false"
        int(number) =>
            return str(number)
        float(number) =>
            return str(number)
        None =>
            return "none"

def main() -> None:
    logger = Logger(fields={"base": "one"})
    merged = logger.copy_fields({"count": 7, "flag": True, "ratio": 2.5, "none": None})
    println(to_text(merged["base"]))
    println(to_text(merged["count"]))
    println(to_text(merged["flag"]))
    println(to_text(merged["ratio"]))
    println(to_text(merged["none"]))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "issue #562 alias transparency run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["one", "7", "true", "2.5", "none"],
            "unexpected issue #562 alias transparency output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_issue502_independent_union_narrowing_branches_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
type LocalPath = newtype str

def normalize_path_like(value: LocalPath | str) -> LocalPath:
    if isinstance(value, str):
        return LocalPath(value)
    if isinstance(value, LocalPath):
        return value

def main() -> None:
    println(normalize_path_like("from-string").0)
    println(normalize_path_like(LocalPath("from-path")).0)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "independent union narrowing branch regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["from-string", "from-path"],
            "unexpected independent union narrowing output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_issue501_option_union_isinstance_narrowing_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
type LocalPath = newtype str

def describe(value: Option[LocalPath | str]) -> str:
    if value is not None:
        if isinstance(value, str):
            return value.upper()
        elif isinstance(value, LocalPath):
            return value.0
    return "missing"

def main() -> None:
    println(describe("from-string"))
    println(describe(LocalPath("from-path")))
    println(describe(None))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Option[Union] isinstance narrowing regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["FROM-STRING", "from-path", "missing"],
            "unexpected Option[Union] narrowing output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn mixed_string_storage_isinstance_union_variants_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let stdout = compile_and_run_local_partial_project(
            r#"
const FROZEN_TEXT: FrozenStr = "frozen"

def is_string(value: FrozenStr | str | int) -> bool:
    return std.builtins.isinstance(value, str)

def optional_is_string(value: Option[FrozenStr | str | int]) -> bool:
    return std.builtins.isinstance(value, str)

def render_string(value: FrozenStr | str | int) -> str:
    if std.builtins.isinstance(value, str):
        return str(value)
    return "number"

def render_optional_string(value: Option[FrozenStr | str | int]) -> str:
    if std.builtins.isinstance(value, str):
        return str(value)
    return "other"

def main() -> None:
    println(is_string(FROZEN_TEXT))
    println(is_string("runtime"))
    println(is_string(7))
    println(optional_is_string(FROZEN_TEXT))
    println(optional_is_string("runtime"))
    println(optional_is_string(7))
    println(optional_is_string(None))
    println(render_string(FROZEN_TEXT))
    println(render_string("runtime"))
    println(render_string(7))
    println(render_optional_string(FROZEN_TEXT))
    println(render_optional_string("runtime"))
    println(render_optional_string(7))
    println(render_optional_string(None))
"#,
            "mixed_string_storage_isinstance_contract",
        )?;

        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "true", "true", "false", "true", "true", "false", "false", "frozen", "runtime", "number", "frozen",
                "runtime", "other", "other",
            ],
            "unexpected mixed string-storage isinstance output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_comprehension_and_generator_execution_matrix() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model StoredNode:
    store_id_raw: int
    node: str


def filtered_comprehension() -> None:
    nodes: list[StoredNode] = [
        StoredNode(store_id_raw=1, node="a"),
        StoredNode(store_id_raw=2, node="b"),
    ]
    filtered = [stored.node for stored in nodes if stored.store_id_raw == 1]
    scores = [1, 2, 3, 4]
    squared_evens = {x: x * x for x in scores if x % 2 == 0}
    println(filtered[0])
    println(squared_evens[2])


def source_ordered_generator_expression() -> None:
    xs = [1, 2, 3]
    ys = [2, 3, 4]
    values = (x * y for x in xs if x > 1 for y in ys if y > x).collect()
    println(values[0])
    println(values[1])
    println(values[2])


def triple(x: int) -> int:
    return x * 3

def big(x: int) -> bool:
    return x > 6


def generator_helper_chain() -> None:
    xs = [1, 2, 3, 4, 5]
    values = (x for x in xs).map(triple).filter(big).take(2).collect()
    println(values[0])
    println(values[1])


def numbers() -> Generator[int]:
    yield 1
    yield 2


def concrete_generator_yield() -> None:
    values = numbers().collect()
    println(values[0])
    println(values[1])


def lazy_numbers() -> Generator[int]:
    println("started")
    yield 1


def lazy_generator_body() -> None:
    values = lazy_numbers()
    println("after construction")
    items = values.collect()
    println(items[0])


def singleton[T](value: T) -> Generator[T]:
    yield value


def generic_generator_yield() -> None:
    values = singleton[int](3).collect()
    println(values[0])


def main() -> None:
    filtered_comprehension()
    source_ordered_generator_expression()
    generator_helper_chain()
    concrete_generator_yield()
    lazy_generator_body()
    generic_generator_yield()
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "comprehension and generator execution matrix failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "a",
                "4",
                "6",
                "8",
                "12",
                "9",
                "12",
                "1",
                "2",
                "after construction",
                "started",
                "1",
                "3",
            ],
            "unexpected comprehension/generator matrix output:\n{stdout}"
        );
        Ok(())
    }

    /// #1123 regression: generator-expression construction evaluates the outer source once, but every filter and
    /// element evaluation remains deferred until a consumer polls the generator. This is a generated-Rust
    /// compile-and-run oracle for the established source contract; it does not claim replacement-executor support.
    #[test]
    fn test_generator_expression_defers_filter_and_element_evaluation() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
def source() -> list[int]:
    println("outer source")
    return [1, 2, 3]


def keep(value: int) -> bool:
    println("filter")
    return value > 1


def project(value: int) -> int:
    println("element")
    return value * 10


def main() -> None:
    values = (project(value) for value in source() if keep(value))
    println("after construction")
    collected = values.collect()
    println(collected[0])
    println(collected[1])
"#;
        let output = run_incan_source(source);
        assert!(
            output.status.success(),
            "generator-expression laziness regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "outer source",
                "after construction",
                "filter",
                "filter",
                "element",
                "filter",
                "element",
                "20",
                "30",
            ],
            "generator-expression filters/elements must not run before construction completes:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_clone_self_struct_field_reads_do_not_move_out_of_borrowed_self() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
pub class ActiveRegistration:
    pub logical_name: str
    pub rank: int

    def clone(self) -> Self:
        return ActiveRegistration(logical_name=self.logical_name, rank=self.rank)

def main() -> None:
    reg = ActiveRegistration(logical_name="orders", rank=1)
    copied = reg.clone()
    println(copied.logical_name)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c clone(self)->Self field regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["orders"], "unexpected clone(self)->Self output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_loop_item_field_index_assignment_materializes_owned_value_issue616()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model Assignment:
    output_name: str

def names(assignments: list[Assignment]) -> list[str]:
    mut output_names: list[str] = []
    for assignment in assignments:
        existing_idx = index_of_name(output_names, assignment.output_name)
        if existing_idx >= 0:
            output_names[existing_idx] = assignment.output_name
        else:
            output_names.append(assignment.output_name)
    return output_names

def index_of_name(names: list[str], name: str) -> int:
    for idx, current in enumerate(names):
        if current == name:
            return idx
    return -1

def main() -> None:
    result = names([Assignment(output_name="amount"), Assignment(output_name="amount")])
    println(result[0])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "loop item field index-assignment regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["amount"],
            "unexpected loop item field index-assignment output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_field_backed_by_value_method_args_do_not_require_user_clone_issue241()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Cursor:
    def join(self, other: Self, on: bool) -> Self:
        return Cursor()

@derive(Clone)
class Wrapper:
    _cursor: Cursor

    def merge(self, other: Self) -> Self:
        return Wrapper(_cursor=self._cursor.join(other._cursor, true))

def main() -> None:
    left = Wrapper(_cursor=Cursor())
    right = Wrapper(_cursor=Cursor())
    _ = left.merge(right)
    println("ok")
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "field-backed by-value method arg regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["ok"], "unexpected issue241 output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_issue241_generic_field_backed_method_args_infer_clone_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Cursor[T]:
    pub value: T

    def join(self, other: Self, on: bool) -> Self:
        return self

@derive(Clone)
class Wrapper[T]:
    pub _cursor: Cursor[T]

    def merge(self, other: Self) -> Self:
        return Wrapper(_cursor=self._cursor.join(other._cursor, true))

def main() -> None:
    left = Wrapper(_cursor=Cursor(value=1))
    right = Wrapper(_cursor=Cursor(value=2))
    println(left.merge(right)._cursor.value)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic issue241 regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1"], "unexpected generic issue241 output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_returning_tuple_with_reused_field_materializes_owned_items() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Pred:
    pub name: str

@derive(Clone)
class Node:
    pub filter_predicate: Pred

def pair(node: Node) -> tuple[Pred, Pred]:
    return (node.filter_predicate, node.filter_predicate)

def main() -> None:
    left, right = pair(Node(filter_predicate=Pred(name="x")))
    println(left.name)
    println(right.name)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "tuple field reuse ownership regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["x", "x"], "unexpected tuple field reuse output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generic_tuple_return_with_reused_field_infers_clone_bound() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
class Node[T]:
    pub value: T

def pair[T](node: Node[T]) -> tuple[T, T]:
    return (node.value, node.value)

def main() -> None:
    left, right = pair(Node(value=1))
    println(left)
    println(right)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic tuple field reuse regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["1", "1"],
            "unexpected generic tuple field reuse output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_incan_call_materializes_owned_value_from_box_as_ref() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::boxed import Box

@derive(Clone)
class Node:
    pub value: int

def take(node: Node) -> int:
    return node.value

def from_box(child: Box[Node]) -> int:
    return take(child.as_ref())

def main() -> None:
    println(from_box(Box.new(Node(value=4))))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "borrowed box as_ref call regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["4"], "unexpected box as_ref output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generic_incan_call_materializes_owned_value_from_box_as_ref() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::boxed import Box

@derive(Clone)
class Node[T]:
    pub value: T

def take[T](node: Node[T]) -> T:
    return node.value

def from_box[T](child: Box[Node[T]]) -> T:
    return take(child.as_ref())

def main() -> None:
    println(from_box(Box.new(Node(value=4))))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic borrowed box as_ref call regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["4"], "unexpected generic box as_ref output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_match_on_shared_self_option_field_materializes_owned_scrutinee() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
pub class Node:
    pub value: int

@derive(Clone)
pub class Wrapper:
    child: Option[Node]

    def read(self) -> int:
        match self.child:
            Some(child) => return child.value
            None => return 0

def main() -> None:
    println(Wrapper(child=Some(Node(value=4))).read())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "shared self option-field match regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["4"],
            "unexpected shared self option-field match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_match_on_shared_self_option_box_field_materializes_owned_scrutinee()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::boxed import Box

@derive(Clone)
pub class Node:
    pub value: int

@derive(Clone)
pub class Wrapper:
    child: Option[Box[Node]]

    def read(self) -> int:
        match self.child:
            Some(child) => return child.as_ref().value
            None => return 0

def main() -> None:
    println(Wrapper(child=Some(Box.new(Node(value=4)))).read())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "shared self option-box-field match regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["4"],
            "unexpected shared self option-box-field match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_generic_match_on_shared_self_option_field_infers_clone_bound() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
pub class Wrapper[T]:
    child: Option[T]

    def read_or(self, fallback: T) -> T:
        match self.child:
            Some(child) => return child
            None => return fallback

def main() -> None:
    println(Wrapper(child=Some(4)).read_or(0))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "generic shared self option-field match regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["4"],
            "unexpected generic shared self option-field match output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_trait_supertraits_runtime_with_backend_clone_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
trait Collection[T]:
    def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
    def sorted(self) -> Self: ...

model BoxedValue[T] with OrderedCollection:
    value: T

    def first(self) -> T:
        return self.value

    def sorted(self) -> Self:
        return self

def take_first(values: Collection[int]) -> int:
    return values.first()

def take_sorted(values: OrderedCollection[int]) -> OrderedCollection[int]:
    return values.sorted()

def main() -> None:
    println(take_first(BoxedValue(value=1)))
    println(take_sorted(BoxedValue(value=2)).first())
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "trait-supertrait ownership regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1", "2"], "unexpected trait-supertrait output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_run_file_release_flag() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_run_release_file");
        let source_path = project_dir.join("main.incn");
        std::fs::write(
            &source_path,
            r#"def main() -> None:
  println("release file path works")
"#,
        )?;

        let output = incan_command()
            .args(["run", "--release", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run --release <file> failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("release file path works"),
            "stdout missing expected output; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_check_web_route_uses_proc_macro_passthrough() {
        let project_dir = make_temp_dir("incan_web_proc_macro_test");
        let source_path = project_dir.join("main.incn");
        let source = r#"
import std.async
from std.web import route

@route("/health")
async def health() -> str:
    return "ok"

def main() -> None:
    pass
"#;
        let Ok(()) = std::fs::write(&source_path, source) else {
            panic!("failed to write source file");
        };

        let Ok(output) = incan_command()
            .args(["--check", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan check");
        };

        assert!(
            output.status.success(),
            "incan check web route failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_run_async_channel_facade() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_async_channel_facade_test");
        let source_path = project_dir.join("async_channel.incn");
        let source = r#"
import std.async
from std.async.channel import channel, unbounded_channel, oneshot

async def main() -> None:
    tx, rx = channel(4)
    cloned = tx.clone()

    match await cloned.send(1):
        Ok(_) => println("sent")
        Err(err) => println(err.message())

    match await rx.recv():
        Some(value) => println(value)
        None => println("closed")

    match await tx.reserve():
        Ok(permit) =>
            match permit.send(4):
                Ok(_) => println("reserved")
                Err(err) => println(err.message())
        Err(err) => println(err.message())

    match await rx.recv():
        Some(value) => println(value)
        None => println("closed")

    tx2, rx2 = unbounded_channel()
    match await tx2.send(2):
        Ok(_) => println("sent")
        Err(err) => println(err.message())

    match rx2.try_recv():
        Some(value) => println(value)
        None => println("empty")

    match await tx2.reserve():
        Ok(permit) =>
            match permit.send(5):
                Ok(_) => println("unbounded reserved")
                Err(err) => println(err.message())
        Err(err) => println(err.message())

    match rx2.try_recv():
        Some(value) => println(value)
        None => println("empty")

    println(f"close:{rx2.close()}")
    println(tx2.is_closed())

    otx, orx = oneshot()
    match otx.send(3):
        Ok(_) => println("delivered")
        Err(value) => println(value)

    match await orx.recv():
        Ok(value) => println(value)
        Err(err) => println(err.message())
"#;
        std::fs::write(&source_path, source)?;

        let output = incan_command()
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run async channel facade failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("sent"), "expected send output; got:\n{}", stdout);
        assert!(
            stdout.contains("1"),
            "expected bounded receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("2"),
            "expected unbounded receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("reserved"),
            "expected bounded reserve output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("4"),
            "expected bounded permit receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("unbounded reserved"),
            "expected unbounded reserve output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("5"),
            "expected unbounded permit receive output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("close:true"),
            "expected receiver close output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("true"),
            "expected closed-state output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("delivered"),
            "expected oneshot send output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("3"),
            "expected oneshot receive output; got:\n{}",
            stdout
        );
        Ok(())
    }

    /// Regression (GitHub #289): `await expr?` must emit `.await?` (not `?.await`) in generated Rust.
    #[test]
    fn test_build_async_await_try_ordering_emits_await_before_try() {
        let project_dir = make_temp_dir("incan_async_await_try_ordering");
        let source_path = project_dir.join("async_await_try_ordering.incn");
        let out_dir = project_dir.join("out");
        let source = r#"
import std.async

async def register_sources() -> Result[None, str]:
    return Ok(None)

async def main() -> Result[None, str]:
    await register_sources()?
    return Ok(None)
"#;
        let Ok(()) = std::fs::write(&source_path, source) else {
            panic!("failed to write source file");
        };

        let Ok(output) = incan_command()
            .args([
                "build",
                source_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan build");
        };

        assert!(
            output.status.success(),
            "incan build await/try ordering regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let generated_main = out_dir.join("src/main.rs");
        let Ok(main_rs) = std::fs::read_to_string(&generated_main) else {
            panic!("failed to read generated Rust source");
        };
        let normalized: String = main_rs.chars().filter(|c| !c.is_whitespace()).collect();
        // Assert the ordering, not the callee's spelling. RFC 120 projections emit a linker-visible
        // `__incan_v1_...` name for `register_sources`, so pinning the source spelling tested the projection rather
        // than the await/try ordering this case exists for.
        assert!(
            normalized.contains(").await?;"),
            "expected awaited-then-try ordering in generated Rust, got:\n{}",
            main_rs
        );
        assert!(
            !normalized.contains(")?.await"),
            "generated Rust must not apply `?` before `.await`, got:\n{}",
            main_rs
        );
    }

    #[test]
    fn test_build_and_run_keyword_named_modules_escape_consistently() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_keyword_module_paths");
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(src_dir.join("api"))?;
        std::fs::write(
            project_dir.join("loaf.toml"),
            "[project]\nname = \"keyword_module_paths\"\nversion = \"0.1.0\"\n",
        )?;

        let main_path = src_dir.join("main.incn");
        // Use a Rust keyword that remains a legal Incan module spelling. `type` is a separate Incan keyword, so
        // parser work to allow `from type import ...` would be a different issue than Rust-side module escaping.
        std::fs::write(
            &main_path,
            r#"from extern import root_value
from api.extern import nested_value

def main() -> None:
  println(root_value())
  println(nested_value())
"#,
        )?;
        std::fs::write(
            src_dir.join("extern.incn"),
            r#"pub def root_value() -> str:
  return "root-keyword"
"#,
        )?;
        std::fs::write(
            src_dir.join("api").join("extern.incn"),
            r#"pub def nested_value() -> str:
  return "nested-keyword"
"#,
        )?;

        let out_dir = project_dir.join("out");
        let build_output = incan_command()
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            build_output.status.success(),
            "incan build keyword-module project failed: status={:?} stderr={}",
            build_output.status,
            String::from_utf8_lossy(&build_output.stderr)
        );

        let main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let api_mod_rs = std::fs::read_to_string(out_dir.join("src/api/mod.rs"))?;
        let normalized_main: String = main_rs.chars().filter(|c| !c.is_whitespace()).collect();
        let normalized_api_mod: String = api_mod_rs.chars().filter(|c| !c.is_whitespace()).collect();

        assert!(
            normalized_main.contains("#[path=\"extern.rs\"]modr#extern;"),
            "expected top-level keyword module path attr in generated main.rs, got:\n{main_rs}"
        );
        // The escape is `crate::r#extern::`; what follows it is the callee's emitted name, which RFC 120 projections
        // now own. Asserting the raw `root_value` spelling tested the projection instead of the keyword escaping.
        assert!(
            normalized_main.contains("crate::r#extern::"),
            "expected generated use path to escape top-level keyword module, got:\n{main_rs}"
        );
        assert!(
            normalized_main.contains("crate::api::r#extern::"),
            "expected generated use path to escape nested keyword module, got:\n{main_rs}"
        );
        assert!(
            normalized_api_mod.contains("#[path=\"extern.rs\"]pubmodr#extern;"),
            "expected nested keyword module path attr in api/mod.rs, got:\n{api_mod_rs}"
        );

        // The normal build above has already exercised the compiler command and produced this exact executable.
        // Reuse it for the runtime assertion rather than taking the same source through a second compilation path.
        let binary = out_dir.join("oven/release/keyword_module_paths");
        assert!(
            binary.is_file(),
            "expected Oven to produce the keyword-module executable at {}",
            binary.display()
        );
        let run_output = Command::new(&binary).output()?;
        assert!(
            run_output.status.success(),
            "incan run keyword-module project failed: status={:?} stderr={}",
            run_output.status,
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert!(
            stdout.contains("root-keyword"),
            "expected top-level keyword module output, got:\n{stdout}"
        );
        assert!(
            stdout.contains("nested-keyword"),
            "expected nested keyword module output, got:\n{stdout}"
        );

        Ok(())
    }

    /// Regression (GitHub #976): an explicit `crate::` import must select the root module even when a nested module
    /// has the same leaf name.
    #[test]
    fn test_run_explicit_root_import_over_same_leaf_nested_module() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_explicit_root_import");
        let src_dir = project_dir.join("src");
        let nested_dir = src_dir.join("substrait");
        std::fs::create_dir_all(&nested_dir)?;
        std::fs::write(
            project_dir.join("loaf.toml"),
            "[project]\nname = \"explicit_root_import\"\nversion = \"0.1.0\"\n",
        )?;

        let main_path = src_dir.join("main.incn");
        std::fs::write(
            &main_path,
            r#"from substrait.schema_registry import selected_value

def main() -> None:
  println(selected_value())
"#,
        )?;
        std::fs::write(
            src_dir.join("schema_registry.incn"),
            r#"pub def root_value() -> str:
  return "root registry"
"#,
        )?;
        std::fs::write(
            nested_dir.join("schema_registry.incn"),
            r#"from crate::schema_registry import root_value

pub def selected_value() -> str:
  return root_value()
"#,
        )?;

        let run_output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "explicit root import project failed: status={:?} stderr={}",
            run_output.status,
            String::from_utf8_lossy(&run_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&run_output.stdout).contains("root registry"),
            "explicit root import did not run the root module: stdout={} stderr={}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        Ok(())
    }

    /// Regression (GitHub #976): a bare import that resolves to its own nested source file must give root guidance.
    #[test]
    fn test_build_rejects_same_leaf_self_import_with_root_guidance() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_same_leaf_self_import");
        let src_dir = project_dir.join("src");
        let nested_dir = src_dir.join("substrait");
        std::fs::create_dir_all(&nested_dir)?;
        std::fs::write(
            project_dir.join("loaf.toml"),
            "[project]\nname = \"same_leaf_self_import\"\nversion = \"0.1.0\"\n",
        )?;

        let main_path = src_dir.join("main.incn");
        std::fs::write(
            &main_path,
            r#"from substrait.schema_registry import registered_columns

def main() -> None:
  println(registered_columns())
"#,
        )?;
        std::fs::write(
            src_dir.join("schema_registry.incn"),
            r#"pub def registered_columns() -> str:
  return "root registry"
"#,
        )?;
        std::fs::write(
            nested_dir.join("schema_registry.incn"),
            r#"from schema_registry import registered_columns

pub def registered_columns() -> str:
  return registered_columns()
"#,
        )?;

        let output = incan_command()
            .args(["build", main_path.to_string_lossy().as_ref()])
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            !output.status.success(),
            "self-importing source module unexpectedly built successfully:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("imports itself"),
            "expected a self-import diagnostic, got:\n{stderr}"
        );
        assert!(
            stderr.contains("crate::schema_registry"),
            "expected root-import guidance, got:\n{stderr}"
        );

        Ok(())
    }

    #[test]
    fn test_run_async_task_and_time_facade() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_async_task_time_facade_test");
        let source_path = project_dir.join("async_task_time.incn");
        let source = r#"
import std.async
from std.async.task import spawn, spawn_blocking
from std.async.time import sleep, timeout, timeout_ms, timeout_join, timeout_join_ms, TimeoutJoinOutcome

async def quick_value() -> int:
    await sleep(0.01)
    return 7

async def slow_value() -> int:
    await sleep(0.05)
    return 99

def blocking_value() -> int:
    return 42

async def main() -> None:
    match await spawn(quick_value()):
        Ok(value) => println(f"spawn_ok:{value}")
        Err(err) => println(f"spawn_err:{err.message()}")

    match await spawn_blocking(blocking_value):
        Ok(value) => println(f"spawn_blocking_ok:{value}")
        Err(err) => println(f"spawn_blocking_err:{err.message()}")

    match await timeout(0.25, quick_value()):
        Ok(value) => println(f"timeout_ok:{value}")
        Err(err) => println(f"timeout_err:{err.message()}")

    match await timeout(0.001, slow_value()):
        Ok(value) => println(f"timeout_unexpected_ok:{value}")
        Err(err) => println(f"timeout_expired:{err.message()}")

    match await timeout_ms(250, quick_value()):
        Ok(value) => println(f"timeout_ms_ok:{value}")
        Err(err) => println(f"timeout_ms_err:{err.message()}")

    match await timeout_ms(1, slow_value()):
        Ok(value) => println(f"timeout_ms_unexpected_ok:{value}")
        Err(err) => println(f"timeout_ms_expired:{err.message()}")

    durable = spawn(slow_value())
    match await timeout_join(0.001, durable):
        TimeoutJoinOutcome.Completed(value) => println(f"timeout_join_unexpected_ok:{value}")
        TimeoutJoinOutcome.JoinFailed(err) => println(f"timeout_join_err:{err.message()}")
        TimeoutJoinOutcome.TimedOut(handle) =>
            println("task still running after timeout")
            match await handle:
                Ok(value) => println(f"timeout_join_later:{value}")
                Err(err) => println(f"timeout_join_later_err:{err.message()}")

    durable_ms = spawn(slow_value())
    match await timeout_join_ms(1, durable_ms):
        TimeoutJoinOutcome.Completed(value) => println(f"timeout_join_ms_unexpected_ok:{value}")
        TimeoutJoinOutcome.JoinFailed(err) => println(f"timeout_join_ms_err:{err.message()}")
        TimeoutJoinOutcome.TimedOut(handle) =>
            match await handle:
                Ok(value) => println(f"timeout_join_ms_later:{value}")
                Err(err) => println(f"timeout_join_ms_later_err:{err.message()}")
"#;
        std::fs::write(&source_path, source)?;

        let output = incan_command()
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run async task/time facade failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("spawn_ok:7"),
            "expected spawn success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("spawn_blocking_ok:42"),
            "expected spawn_blocking success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_ok:7"),
            "expected timeout success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_expired:operation timed out"),
            "expected timeout expiry output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_ms_ok:7"),
            "expected timeout_ms success output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_ms_expired:operation timed out"),
            "expected timeout_ms expiry output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("task still running after timeout"),
            "expected durable timeout message; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_join_later:99"),
            "expected timeout_join preserved handle output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("timeout_join_ms_later:99"),
            "expected timeout_join_ms preserved handle output; got:\n{}",
            stdout
        );
        assert!(
            !stdout.contains("timeout_unexpected_ok")
                && !stdout.contains("timeout_ms_unexpected_ok")
                && !stdout.contains("timeout_join_unexpected_ok")
                && !stdout.contains("timeout_join_ms_unexpected_ok")
                && !stdout.contains("spawn_err:")
                && !stdout.contains("spawn_blocking_err:")
                && !stdout.contains("timeout_err:")
                && !stdout.contains("timeout_ms_err:"),
            "unexpected error/success fallback branch output; got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_run_async_barrier_cancellation_withdraws_waiter() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_async_barrier_cancel_test");
        let source_path = project_dir.join("async_barrier_cancel.incn");
        let source = r#"
import std.async
from std.async.sync import Barrier, Mutex
from std.async.task import spawn, yield_now
from std.async.time import timeout_join_ms, TimeoutJoinOutcome

async def mark_ready(ready: Mutex[int]) -> None:
    guard = await ready.lock()
    guard.set(1)

async def is_ready(ready: Mutex[int]) -> bool:
    guard = await ready.lock()
    return guard.get() == 1

async def wait_until_ready(ready: Mutex[int]) -> None:
    while True:
        if await is_ready(ready):
            return
        await yield_now()

async def wait_barrier(barrier: Barrier, ready: Mutex[int]) -> int:
    await mark_ready(ready)
    return await barrier.wait()

async def main() -> None:
    barrier = Barrier.new(2)

    cancelled_ready = Mutex.new(0)
    cancelled = spawn(wait_barrier(barrier, cancelled_ready))
    await wait_until_ready(cancelled_ready)
    cancelled.abort()
    match await cancelled:
        Ok(slot) => println(f"unexpected_cancelled_slot:{slot}")
        Err(err) => println(f"cancelled:{err.message()}")

    replacement_ready = Mutex.new(0)
    replacement = spawn(wait_barrier(barrier, replacement_ready))
    await wait_until_ready(replacement_ready)
    match await timeout_join_ms(5, replacement):
        TimeoutJoinOutcome.Completed(slot) => println(f"unexpected_replacement_completed:{slot}")
        TimeoutJoinOutcome.JoinFailed(err) => println(f"unexpected_replacement_failed:{err.message()}")
        TimeoutJoinOutcome.TimedOut(handle) =>
            println("replacement_waiting")
            current = await barrier.wait()
            match await handle:
                Ok(slot) => println(f"replacement_slot:{slot}")
                Err(err) => println(f"unexpected_replacement_join_failed:{err.message()}")
            println(f"current_slot:{current}")
"#;
        std::fs::write(&source_path, source)?;

        let output = incan_command()
            .args(["run", source_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run async barrier cancellation failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("cancelled:task") && stdout.contains("was cancelled"),
            "expected cancelled join output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("replacement_waiting"),
            "expected replacement to keep waiting until another active participant arrived; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("replacement_slot:") && stdout.contains("current_slot:"),
            "expected both active participants to complete after the second arrival; got:\n{}",
            stdout
        );
        assert!(
            !stdout.contains("unexpected_"),
            "unexpected fallback branch output; got:\n{}",
            stdout
        );

        Ok(())
    }

    #[test]
    fn test_run_repro_model_traits() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("repro_model_traits.incn"))
            // This should not require network access (workspace deps should already be available).
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run repro_model_traits failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("[Ada] hello"),
            "expected repro output; got:\n{}",
            stdout
        );
    }

    /// RFC 021: Runtime verification that __fields__() returns correct FieldInfo values
    #[test]
    fn test_run_field_info_reflection() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("field_info_reflection.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run field_info_reflection failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Verify __class_name__
        assert!(
            stdout.contains("Account"),
            "expected __class_name__ to return 'Account'; got:\n{}",
            stdout
        );

        // Verify field info for type_ (has alias)
        assert!(
            stdout.contains("field:type_|wire:type|type:str|default:false"),
            "expected type_ field info with alias='type'; got:\n{}",
            stdout
        );

        // Verify field info for balance (has default)
        assert!(
            stdout.contains("field:balance|wire:balance|type:int|default:true"),
            "expected balance field info with default=true; got:\n{}",
            stdout
        );

        // Verify field info for name (no alias, no default)
        assert!(
            stdout.contains("field:name|wire:name|type:str|default:false"),
            "expected name field info; got:\n{}",
            stdout
        );

        // Empty models should produce no FieldInfo entries
        assert!(
            stdout.contains("empty_fields:0"),
            "expected empty model to return 0 fields; got:\n{}",
            stdout
        );

        // Nested generics should use Incan type formatting
        assert!(
            stdout.contains("settings_field:complex|type:list[dict[str, int]]"),
            "expected nested generic type name; got:\n{}",
            stdout
        );

        // User-defined field types should use their Incan type name
        assert!(
            stdout.contains("user_field:address|type:Address"),
            "expected user-defined field type name; got:\n{}",
            stdout
        );

        // Public inherited class fields should appear in __fields__().
        assert!(
            stdout.contains("child_field:base_id|type:int"),
            "expected inherited base field in __fields__; got:\n{}",
            stdout
        );
        assert!(
            !stdout.contains("child_field:name|type:str"),
            "private child fields must not appear in __fields__; got:\n{}",
            stdout
        );
    }

    /// RFC 023: Runtime parity check for source-defined stdlib surfaces migrated off helper stubs.
    #[test]
    fn test_run_rfc023_stdlib_behavior_parity() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc023_stdlib_behavior_parity.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc023_stdlib_behavior_parity failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("{\"value\":1,\"player\":\"Ada\"}"),
            "expected explicit Serialize adoption to preserve JSON output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("Score"),
            "expected reflection class name output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("true\ntrue"),
            "expected clone/equality and ordering behavior from derive-backed traits; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("{\"value\":0,\"player\":\"\"}"),
            "expected Default derive to preserve zero-value JSON output; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("field:value|wire:value|type:int|default:true"),
            "expected reflection metadata for value field; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("field:player|wire:player|type:str|default:true"),
            "expected reflection metadata for player field; got:\n{}",
            stdout
        );
    }

    #[test]
    fn test_run_rfc030_std_collections_behavior() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc030_std_collections_behavior.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc030_std_collections_behavior failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_run_rfc088_source_owned_iterator_sum() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/rfc088_iterator_adapters.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc088_iterator_adapters failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "2\n3\n15\n15\n15\n3\n3\n",
            "source-owned Iterator.sum() must preserve adapter, primitive, and checked-newtype behavior"
        );
    }

    #[test]
    fn test_run_iterator_adapters_as_loop_and_comprehension_sources_issue950_953()
    -> Result<(), Box<dyn std::error::Error>> {
        let output =
            incan_command()
                .arg("run")
                .arg(repo_root().join(
                    "loaves/compiler/incan_emit/tests/codegen_snapshots/issue950_953_iterator_adapter_sources.incn",
                ))
                .env("CARGO_NET_OFFLINE", "true")
                .output()?;

        assert!(
            output.status.success(),
            "incan run issue950_953_iterator_adapter_sources failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "11\n11\n11\n0:beta\n1:alpha\n2\nalpha\n2\n",
            "iterator adapters and builtin zip must preserve item types and source-owned polling across loops and comprehensions"
        );
        Ok(())
    }

    #[test]
    fn test_run_builtin_zip_only_keeps_generated_iterator_support_issue950() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue950_builtin_zip_only.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue950_builtin_zip_only failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout), "11\nalpha:1\n");
        Ok(())
    }

    #[test]
    fn test_run_set_constructor_from_values_issue951() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue951_set_constructor.incn"))
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue951_set_constructor failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "source:3\ngeneric-source:3\nset-source:2\n2:2:2:2:2:2:2:1:2:0\n",
            "set constructors must deduplicate values without consuming a source collection that is used later"
        );
        Ok(())
    }

    #[test]
    fn test_run_set_add_issue963() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue963_set_add.incn"))
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue963_set_add failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "1\n",
            "Set.add must mutate the set and preserve HashSet deduplication"
        );
        Ok(())
    }

    #[test]
    fn test_run_user_defined_set_shadowing_issue951() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(repo_root().join("loaves/compiler/incan_emit/tests/codegen_snapshots/issue951_set_shadowing.incn"))
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run issue951_set_shadowing failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "3\n",
            "a source-defined set function must remain an ordinary call through generated Rust"
        );
        Ok(())
    }

    #[test]
    fn test_run_rfc088_iterator_sum_float_and_newtype_matrix() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc088_iterator_sum_runtime.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc088_iterator_sum_runtime failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "6\n3.75\n3.75\n3\n",
            "source-owned Iterator.sum() must support int, float, and checked/unchecked newtypes"
        );
    }

    #[test]
    fn test_run_rfc064_std_encoding_behavior() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc064_std_encoding_behavior.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc064_std_encoding_behavior failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("strict-padding-error")
                && stdout.contains("bech32-checksum-error")
                && stdout.contains("rfc064-encoding-ok"),
            "expected strict error markers and success marker; got:\n{}",
            stdout
        );
    }

    #[test]
    fn test_std_uuid_surface_runs_as_native_test_issue1051() -> Result<(), Box<dyn std::error::Error>> {
        // Anchor `std.uuid`'s generated native-test dependency in the root test build so offline CI shards do not
        // depend on cache order.
        let mut rng = rand::thread_rng();
        let _ = rand::Rng::gen_range(&mut rng, 0..1);

        let output = incan_command()
            .arg("test")
            .arg(incan_test_support::fixture("valid/test_std_uuid_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan test std_uuid_surface failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("test_std_uuid_surface"),
            "the UUID native-test regression must execute its declared test. stdout:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(())
    }

    #[test]
    fn test_run_std_ordinal_map_surface() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/std_ordinal_map_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .map_err(|error| format!("failed to run std.ordinal_map fixture: {error}"))?;

        assert!(
            output.status.success(),
            "incan run std_ordinal_map_surface failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "std.ordinal_map ok");

        // Normal Oven output is project-local. This fixture is its own project root, so do not accidentally assert
        // the old Cargo-era process-working-directory target path when the direct-rustc consumer is correct.
        let generated_project = incan_test_support::fixture("valid/target/incan/std_ordinal_map_surface");
        let generated_main = fs::read_to_string(generated_project.join("src/main.rs"))
            .map_err(|error| format!("failed to read generated std.ordinal_map consumer: {error}"))?;
        assert!(
            generated_main.contains("__incan_ordinal_require_str("),
            "OrdinalMap[str] literal lookup should lower through the borrowed string fast path:\n{generated_main}"
        );
        assert!(
            generated_main.contains("pub use crate::__incan_std::collections::OrdinalMap;"),
            "OrdinalMap should be supplied through the stable compiled-provider facade:\n{generated_main}"
        );
        assert!(
            !generated_project.join("src/__incan_std/collections.rs").exists(),
            "a compiled std.collections module must not be materialized in the consumer"
        );
        let artifact_root = compiled_sdk_provider_artifact_root(&generated_project, "incan_stdlib_data")?;
        let generated_collections = fs::read_to_string(artifact_root.join("src/collections.rs"))
            .map_err(|error| format!("failed to read compiled std.collections artifact: {error}"))?;
        // The splice now passes the two support functions it needs, and RFC 120 projects their names, so the
        // invocation reads `!(<projection>, <projection>);` rather than the argument-free form this assertion was
        // written against. Require the splice and both arguments without pinning either spelling or a line break.
        let compact_collections: String = generated_collections.split_whitespace().collect();
        let spliced_with_support_functions = compact_collections
            .split_once("incan_std_data::__incan_ordinal_map_string_fast_impls!(")
            .and_then(|(_, rest)| rest.split_once(");"))
            .is_some_and(|(arguments, _)| {
                arguments.matches(',').count() == 1
                    && arguments
                        .split(',')
                        .all(|argument| argument.starts_with(incan_semantics_core::INCAN_SYMBOL_RUST_PREFIX))
            });
        assert!(
            spliced_with_support_functions,
            "the compiled std.collections artifact should splice in the stdlib-owned OrdinalMap string support:\n{generated_collections}"
        );
        Ok(())
    }

    /// Exercise `std.regex` through the immutable SDK inventory injected for the Oven suite.
    ///
    /// The provider inventory is the normal-command authority here. A former “fresh Cargo home” lane re-published
    /// the provider while testing the consumer, which both weakened this boundary and contradicted Oven Alpha's
    /// no-Cargo normal-command contract.
    fn assert_std_regex_surface_from_sealed_sdk_inventory() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = tmp.path().join("std_regex_surface.incn");
        fs::copy(incan_test_support::fixture("valid/std_regex_surface.incn"), &source)?;
        let generated_project = tmp.path().join("target/incan/std_regex_surface");
        let oven_home = tmp.path().join("oven-home");
        let inventory = std::env::var_os("INCAN_SDK_INVENTORY")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .ok_or("std.regex Oven regression requires the sealed SDK inventory from test prewarm")?;

        let mut command = incan_command();
        command
            .current_dir(tmp.path())
            .env("INCAN_SOURCE_ROOT", repo_root())
            .env("INCAN_STDLIB", repo_root().join("loaves/stdlib"))
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_HOME", &oven_home)
            .env("INCAN_SDK_INVENTORY", &inventory)
            .env_remove("INCAN_INTERNAL_SDK_PROVIDER_STORE")
            .arg("run")
            .arg(&source);
        let output = command.output()?;

        assert!(
            output.status.success(),
            "incan run std_regex_surface failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "true",
                "xx@0:2",
                "ALPHA-12",
                "beta",
                "beta",
                "0:4",
                "<none>",
                "<none>",
                "beta|<none>",
                "one,two",
                "a:1,b:2",
                "a|b|c",
                "a|b,c",
                "a|b,c",
                "a|b|c",
                "Lovelace, Ada",
                "Lovelace/Ada",
                "Lovelace, Ada",
                "$2, $1",
                "x x three",
                "$1 two",
                "é@6:8|6:8",
                "unicode-full",
                ";31m",
                ";31m",
                ";31m",
                ";31m",
                ";31m",
                ";31m",
                ";31m",
                ";31m",
                ";31m",
                "; !",
                "; !",
                "é|a",
                "before ;31mafter",
                "before|;31m",
                "before|;31m",
                "|;|!",
            ],
            "unexpected std.regex output:\n{stdout}"
        );
        let consumer_core = generated_project.join("src/__incan_std/regex/_core.rs");
        assert!(
            !consumer_core.exists(),
            "a compiled std.regex module must not be materialized in the consumer: {}",
            consumer_core.display()
        );
        let artifact_root = fs::canonicalize(compiled_sdk_provider_artifact_root(
            &generated_project,
            "incan_stdlib_data",
        )?)?;
        let provider_root = inventory.parent().ok_or("SDK inventory has no provider root")?;
        assert!(
            artifact_root.starts_with(provider_root),
            "Oven std.regex child must use the scheduler-injected immutable SDK inventory rather than an ambient provider store: {}",
            artifact_root.display()
        );
        let generated_core = fs::read_to_string(artifact_root.join("src/regex/_core.rs"))?;
        for unexpected in [
            "RegexBuilder::new(&(pattern).to_string())",
            "raw.find(&(text).to_string())",
            "raw.find_iter(&(text).to_string())",
            "raw.captures(&(text).to_string())",
            "raw.captures_iter(&(text).to_string())",
            "found.as_str().to_string()",
            "raw_name.to_string()",
        ] {
            assert!(
                !generated_core.contains(unexpected),
                "std.regex should let the compiler borrow Incan strings for Rust regex APIs instead of cloning them:\n{generated_core}"
            );
        }
        for expected in [
            "RegexBuilder::new(&pattern)",
            "raw.find(&text)",
            "raw.find_iter(&text)",
            "raw.captures(&text)",
            "raw.captures_iter(&text)",
        ] {
            assert!(
                generated_core.contains(expected),
                "std.regex should preserve compiler-managed Rust borrow boundaries; missing `{expected}`:\n{generated_core}"
            );
        }
        assert_eq!(
            generated_core.matches("RustString::from(found.as_str())").count(),
            6,
            "every borrowed regex match view should be copied into owned public match text:\n{generated_core}"
        );
        assert_eq!(
            generated_core.matches("RustString::from(raw_name)").count(),
            2,
            "every borrowed regex capture name should be copied into an owned dictionary key:\n{generated_core}"
        );
        let compact_generated_core = generated_core
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        assert_eq!(
            compact_generated_core
                .matches("incan_std_core::strings::str_concat(&out,&str_slice_byte_range(&text,last_end,start),)")
                .count(),
            3,
            "every replacement loop should pass the owned Rust byte-range result directly to str_concat:\n{generated_core}"
        );
        assert_eq!(
            compact_generated_core
                .matches("incan_std_core::strings::str_concat(&out,&str_slice_from_byte_offset(&text,last_end),)")
                .count(),
            3,
            "every replacement loop should pass the owned Rust suffix directly to str_concat:\n{generated_core}"
        );
        assert!(
            !generated_core.contains("out = out + str_slice"),
            "compiled std.regex providers must not lower owned Rust String results through infix String + String:\n{generated_core}"
        );
        Ok(())
    }

    #[test]
    fn test_run_std_regex_rfc059_surface_from_sealed_sdk_inventory() -> Result<(), Box<dyn std::error::Error>> {
        assert_std_regex_surface_from_sealed_sdk_inventory()
    }

    #[test]
    fn explicit_stale_sdk_inventory_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = tmp.path().join("std_regex_surface.incn");
        fs::copy(incan_test_support::fixture("valid/std_regex_surface.incn"), &source)?;
        let stale_inventory = tmp.path().join("stale-sdk-inventory.json");
        fs::write(
            &stale_inventory,
            r#"{
  "schema_version": 2,
  "sdk_id": "incan",
  "sdk_version": "0.6.0",
  "compiler_requirement": ">=0.6.0-dev.0,<0.7.0",
  "provider_codegen_revision": 4,
  "components": {},
  "profiles": {"default": []}
}"#,
        )?;
        let provider_store = tmp.path().join("unused-sdk-provider-store");
        let generated_cargo_target = tmp.path().join("generated-cargo-target");

        let output = incan_command()
            .current_dir(tmp.path())
            .env_remove("INCAN_STDLIB")
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_SDK_INVENTORY", &stale_inventory)
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .arg("run")
            .arg(&source)
            .output()?;
        assert!(
            !output.status.success(),
            "an explicit SDK inventory is authoritative and must not be replaced from nearby source: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("generated with provider codegen revision 4")
                && stderr.contains(&format!(
                    "requires revision {}",
                    incan_core::version::SDK_PROVIDER_CODEGEN_REVISION
                )),
            "the failure should report the exact incompatible provider revision:\n{stderr}"
        );
        assert!(
            !provider_store.exists(),
            "rejecting an explicit incompatible SDK inventory must not publish replacement providers"
        );
        Ok(())
    }

    #[test]
    fn explicit_legacy_sdk_inventory_blocks_lock_preheat() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("provider_backed_library");
        fs::create_dir_all(project.join("src"))?;
        fs::write(
            project.join("loaf.toml"),
            "[project]\nname = \"provider_backed_library\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
        let stale_inventory = tmp.path().join("stale-sdk-inventory.json");
        fs::write(
            &stale_inventory,
            r#"{
  "schema_version": 1,
  "sdk_id": "incan",
  "sdk_version": "0.5.0",
  "compiler_requirement": ">=0.5.0-dev.6,<0.6.0",
  "components": {},
  "profiles": {"default": []}
}"#,
        )?;
        let provider_store = tmp.path().join("unused-sdk-provider-store");
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let configure = |command: &mut Command| {
            command
                .current_dir(&project)
                .env_remove("INCAN_STDLIB")
                .env_remove("INCAN_STDLIB_DIR")
                .env("INCAN_SDK_INVENTORY", &stale_inventory)
                .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
                .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target);
        };

        let mut lock = incan_command();
        configure(&mut lock);
        let lock = lock.arg("lock").output()?;
        assert!(
            !lock.status.success(),
            "an explicit legacy SDK inventory must fail before lock preheat can replace it: status={:?}\nstdout:\n{}\nstderr:\n{}",
            lock.status,
            String::from_utf8_lossy(&lock.stdout),
            String::from_utf8_lossy(&lock.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&lock.stderr));
        assert!(
            stderr.contains("unsupported SDK inventory schema 1") && stderr.contains("expected 2"),
            "the failure should report the exact unsupported inventory schema:\n{stderr}"
        );
        assert!(
            !provider_store.exists(),
            "rejecting an explicit legacy SDK inventory must not publish replacement providers"
        );
        Ok(())
    }

    #[test]
    fn test_run_std_regex_unsupported_safe_engine_pattern_reports_error() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.regex import Regex

def main() -> None:
    match Regex("(?<=prefix)\\w+"):
        Ok(_) => println("unexpected-ok")
        Err(err) =>
            println("unsupported")
            println(err.kind())
            println(err.message())
"#,
            ])
            .output()?;

        assert!(
            output.status.success(),
            "std.regex unsupported-pattern program should report RegexError without failing the process: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert!(
            stdout.contains("unsupported") && !stdout.contains("unexpected-ok"),
            "expected safe-engine rejection branch, got:\n{stdout}"
        );
        assert!(
            stdout.contains("compile_error"),
            "expected stable RegexError kind, got:\n{stdout}"
        );
        assert!(
            stdout.to_ascii_lowercase().contains("look"),
            "expected diagnostic to identify the unsupported lookaround boundary, got:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_run_u128_modulo_floor_div() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/u128_modulo_floor_div.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run u128_modulo_floor_div failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "u128 modulo ok");
        Ok(())
    }

    #[test]
    fn test_run_rfc030_field_overlay_reflection() {
        let Ok(output) = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("rfc030_field_overlay_reflection.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "incan run rfc030_field_overlay_reflection failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_check_cyclic_explicit_call_site_generics_cross_module_succeeds() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_cycle_explicit_call_site_check");
        let main_path = super::write_cycle_explicit_call_site_generics_project(&project_dir)?;

        let output = incan_command()
            .arg("--check")
            .arg(main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan --check cyclic explicit call-site generics failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn test_run_cyclic_explicit_call_site_generics_cross_module_succeeds() -> Result<(), Box<dyn std::error::Error>> {
        let project_dir = make_temp_dir("incan_cycle_explicit_call_site_run");
        let main_path = super::write_cycle_explicit_call_site_generics_project(&project_dir)?;

        let output = incan_command()
            .arg("run")
            .arg(main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run cyclic explicit call-site generics failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains('1'),
            "expected runtime output to contain 1, got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn test_benchmark_quicksort_codegen_compiles() {
        let path = repo_root().join("workspaces/benchmarks/sorting/quicksort/quicksort.incn");
        if !path.exists() {
            return;
        }

        let Ok(source) = fs::read_to_string(&path) else {
            panic!("failed to read {}", path.display());
        };
        let Ok(tokens) = lexer::lex(&source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };

        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };

        // Regression: Vec::swap indices must be cast to usize.
        let mut ok = true;
        let mut search_from = 0usize;
        while let Some(pos) = rust_code[search_from..].find(".swap(") {
            let abs = search_from + pos;
            let window_end = (abs + 120).min(rust_code.len());
            let window = &rust_code[abs..window_end];
            if !window.contains("as usize") {
                ok = false;
                break;
            }
            search_from = abs + 5;
        }
        assert!(
            ok,
            "expected quicksort to cast swap indices to usize; generated:\n{}",
            rust_code
        );

        // Note: This test uses standalone rustc compilation, which can't access the stdlib facets or incan_derive.
        // Skip the compilation check if generated Rust references external Incan crates.
        if rust_code.contains("incan_std_") || rust_code.contains("incan_derive::") {
            // Skip rustc compilation test for code that requires Incan support crates.
            return;
        }

        let Ok(()) = rustc_compile_ok(&rust_code) else {
            panic!("generated quicksort Rust failed to compile");
        };
    }

    #[test]
    fn test_const_declarations_compile_and_run() {
        let Ok(output) = incan_command()
            .args([
                "run",
                "-c",
                r#"
const PI: float = 3.14159
const APP_NAME: str = "Incan"
const MAGIC: int = 42
const ENABLED: bool = true
const RAW_DATA: bytes = b"\x00\x01\x02\x03"
const FROZEN_TEXT: FrozenStr = "frozen"
const NUMBERS: FrozenList[int] = [1, 2, 3, 4, 5]
const GREETING: str = "Hello World"

def main() -> None:
    print(PI)
    print(APP_NAME)
    print(MAGIC)
    print(ENABLED)
    print(RAW_DATA.len())
    print(FROZEN_TEXT.len())
    print(NUMBERS.len())
    print(GREETING)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "const declarations test failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("3.14159"), "PI const not emitted correctly");
        assert!(stdout.contains("Incan"), "APP_NAME const not emitted correctly");
        assert!(stdout.contains("42"), "MAGIC const not emitted correctly");
        assert!(stdout.contains("true"), "ENABLED const not emitted correctly");
        assert!(stdout.contains("4"), "RAW_DATA length incorrect");
        assert!(stdout.contains("6"), "FROZEN_TEXT length incorrect");
        assert!(stdout.contains("5"), "NUMBERS length incorrect");
        assert!(stdout.contains("Hello World"), "GREETING concat not working");
    }

    #[test]
    fn descriptor_const_with_empty_frozen_list_builds_and_runs() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone, Eq, Descriptor)
model Version:
    major: int
    minor: int

@derive(Clone, Eq, Descriptor)
model Change:
    version: Version
    note: str

@derive(Clone, Eq, Descriptor)
model Deprecation:
    since: Version
    note: str

@derive(Clone, Eq, Descriptor)
model Lifecycle:
    since: Version
    changed: FrozenList[Change]
    deprecated: Option[Deprecation]

const INITIAL_VERSION: Version = Version(major=0, minor=1)
const NO_CHANGES: FrozenList[Change] = []
const INITIAL_LIFECYCLE: Lifecycle = Lifecycle(
    since=INITIAL_VERSION,
    changed=NO_CHANGES,
    deprecated=None,
)

@derive(Clone, Descriptor)
model FunctionDescriptor:
    lifecycle: Lifecycle = INITIAL_LIFECYCLE

def main() -> None:
    descriptor = FunctionDescriptor()
    assert descriptor.lifecycle == INITIAL_LIFECYCLE
    println(descriptor.lifecycle.since.minor)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "descriptor FrozenList const program failed: status={:?} stdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1");
        Ok(())
    }

    #[test]
    fn test_const_str_materializes_to_owned_str_at_runtime_sites() {
        let Ok(output) = incan_command()
            .args([
                "run",
                "-c",
                r#"
const PREFIX: str = "target/"

def echo(value: str) -> str:
    return value

def direct() -> str:
    return PREFIX

def join(name: str) -> str:
    return PREFIX + name

def main() -> None:
    local = PREFIX
    println(direct())
    println(echo(PREFIX))
    println(echo(local))
    println(join("orders.csv"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "const str materialization test failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["target/", "target/", "target/", "target/orders.csv"]);
    }

    #[test]
    fn test_rfc041_rusttype_interop_typechecks_end_to_end() {
        let source = r#"
from rust::std::string import String as RustString

type Name = rusttype RustString:
  def parse(raw: str) -> Result[Name, str]:
    ...

  def as_str(self) -> str:
    ...

  interop:
    from str try Name.parse
    into str via Name.as_str

def main() -> None:
  pass
"#;
        let Ok(()) = super::compile_source(source) else {
            panic!("expected RFC 041 rusttype/interop source to typecheck");
        };
    }

    #[test]
    fn test_rfc041_rusttype_with_methods_typechecks() {
        let source = r#"
from rust::mail import Sender as RustSender

type Sender = rusttype RustSender:
  send_now = try_send

  def try_send(self, value: int) -> Result[None, str]:
    ...

def push(sender: Sender, value: int) -> Result[None, str]:
  return sender.send_now(value)

def main() -> None:
  pass
"#;
        let Ok(()) = super::compile_source(source) else {
            panic!("expected RFC 041 rusttype method surface to typecheck");
        };
    }

    #[test]
    fn test_rfc041_rust_coercion_codegen_smoke() {
        let source = r#"
from rust::std::time import Duration

def main() -> None:
  _ = Duration.from_secs_f32(1.5)
"#;
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };
        assert!(
            rust_code.contains("Duration::from_secs_f32"),
            "expected RFC 041 coercion fixture to lower to Duration::from_secs_f32 call, got:\n{rust_code}"
        );
    }

    #[test]
    fn test_rfc041_structural_coercion_codegen_smoke() {
        let source = r#"
def main() -> None:
  maybe: Option[int] = Some(1)
  names: List[str] = ["a", "b"]
  scores: Dict[str, float] = {"latency": 1.5}
"#;
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };
        assert!(
            rust_code.contains("let _maybe: Option<i64> = Some(1);"),
            "expected Option[int] smoke value to lower to a Rust Option expression; got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("let _names: Vec<String> = vec![\"a\".to_string(), \"b\".to_string()];"),
            "expected List[str] smoke value to lower to an owned Rust string vec; got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("collect::<std::collections::HashMap<_, _>>()"),
            "expected Dict[str, float] smoke value to lower to a Rust HashMap collect; got:\n{rust_code}"
        );
    }

    #[test]
    fn test_rfc009_numeric_resize_and_decimal_codegen_smoke() {
        let source = r#"
def main() -> None:
  small: i8 = 120
  wide: int = small.resize()
  maybe: Option[i8] = wide.try_resize()
  wrapped: i8 = wide.wrapping_resize()
  capped: i8 = wide.saturating_resize()
  price: decimal[5, 2] = 19.99d
"#;
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lexing failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(()) = typechecker::check(&ast) else {
            panic!("typecheck failed");
        };
        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };
        assert!(
            rust_code.contains("let wide: i64 = (small) as i64;"),
            "expected lossless resize to emit a Rust cast, got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("incan_std_core::num::try_resize::<_, i8>(wide)"),
            "expected try_resize to call stdlib checked resize helper, got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("incan_std_core::num::saturating_resize::<_, i8>(wide)"),
            "expected saturating_resize to call stdlib saturating helper, got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("let _price: incan_std_core::num::Decimal128")
                && rust_code.contains("Decimal128::from_literal")
                && rust_code.contains("\"19.99d\""),
            "expected decimal annotation/literal to lower to Decimal128, got:\n{rust_code}"
        );
    }

    #[test]
    fn test_mixed_numeric_codegen_runs() {
        let Ok(output) = incan_command()
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    size: int = 2
    x: float = 3.0
    result = 2.0 * x / size
    println(result)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "mixed numeric run failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains('3'),
            "mixed numeric output missing expected result; stdout={}",
            stdout
        );
    }

    #[test]
    fn test_std_async_race_and_race_for_surfaces_share_one_run() {
        let output = run_incan_source(
            r#"
import std.async
from std.async.race import arm, race
from std.async.time import sleep

def label(value: int) -> str:
    return f"win:{value}"

async def fast() -> int:
    return 1

async def slow() -> int:
    await sleep(0.01)
    return 2

async def first() -> int:
    return 1

async def second() -> int:
    return 2

async def run_race_for_first() -> str:
    prefix = "win"
    return race for value:
        await slow() => f"{prefix}:{value}"
        await fast() => f"{prefix}:{value}"

async def run_race_for_tie() -> int:
    return race for value:
        await first() => value
        await second() => value

async def main() -> None:
    println(await race(arm(slow(), label), arm(fast(), label)))
    println(await race(arm(first(), label), arm(second(), label)))
    println(await run_race_for_first())
    println(await run_race_for_tie())
"#,
        );
        assert!(
            output.status.success(),
            "std.async race surface batch failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout.lines().map(str::trim).collect::<Vec<_>>(),
            vec!["win:1", "win:1", "win:1", "1"],
            "unexpected stdout:\n{stdout}"
        );
    }

    #[test]
    fn test_std_math_surface_runs() {
        let Ok(output) = incan_command()
            .args([
                "run",
                "-c",
                r#"
import std.math

def main() -> None:
    println(math.PI)
    println(math.round(1.6))
    println(math.log2(8.0))
    println(math.atan2(1.0, 1.0))
    println(math.hypot(3.0, 4.0))
    println(math.gcd(54, 24))
    println(math.lcm(6, 8))

    assert math.is_int_like("0")
    assert math.is_int_like("-123")
    assert not math.is_int_like("1e3")
    assert not math.is_int_like("01")

    assert math.is_float_like("0.0")
    assert math.is_float_like("-0.5")
    assert math.is_float_like("1e3")
    assert math.is_float_like("1.25E+10")
    assert not math.is_float_like("1")
    assert not math.is_float_like("+1")
    assert not math.is_float_like("1e+")
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "std.math module run failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines.len(),
            7,
            "expected 7 output lines (PI/round/log2/atan2/hypot/gcd/lcm); got: {stdout}"
        );

        let Ok(pi) = lines[0].parse::<f64>() else {
            panic!("PI output was not a float: `{}`", lines[0]);
        };
        let Ok(round) = lines[1].parse::<f64>() else {
            panic!("round output was not a float: `{}`", lines[1]);
        };
        let Ok(log2) = lines[2].parse::<f64>() else {
            panic!("log2 output was not a float: `{}`", lines[2]);
        };
        let Ok(atan2) = lines[3].parse::<f64>() else {
            panic!("atan2 output was not a float: `{}`", lines[3]);
        };
        let Ok(hypot) = lines[4].parse::<f64>() else {
            panic!("hypot output was not a float: `{}`", lines[4]);
        };
        let Ok(gcd) = lines[5].parse::<i64>() else {
            panic!("gcd output was not an int: `{}`", lines[5]);
        };
        let Ok(lcm) = lines[6].parse::<i64>() else {
            panic!("lcm output was not an int: `{}`", lines[6]);
        };

        assert!((pi - std::f64::consts::PI).abs() < 1e-12, "unexpected PI value: {pi}");
        assert!((round - 2.0).abs() < 1e-12, "unexpected round value: {round}");
        assert!((log2 - 3.0).abs() < 1e-12, "unexpected log2 value: {log2}");
        assert!(
            (atan2 - std::f64::consts::FRAC_PI_4).abs() < 1e-12,
            "unexpected atan2 value: {atan2}"
        );
        assert!((hypot - 5.0).abs() < 1e-12, "unexpected hypot value: {hypot}");
        assert_eq!(gcd, 6, "unexpected gcd value: {gcd}");
        assert_eq!(lcm, 24, "unexpected lcm value: {lcm}");
    }

    #[test]
    fn test_std_datetime_surface_runs_with_std_time_runtime_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let runtime_source = std::fs::read_to_string(repo_root().join("loaves/stdlib/data/src/datetime/runtime.incn"))?;
        let mut civil_sources = Vec::new();
        civil_sources.push(std::fs::read_to_string(
            repo_root().join("loaves/stdlib/data/src/datetime/civil.incn"),
        )?);
        for entry in std::fs::read_dir(repo_root().join("loaves/stdlib/data/src/datetime/civil"))? {
            let entry = entry?;
            if entry.path().extension().is_some_and(|extension| extension == "incn") {
                civil_sources.push(std::fs::read_to_string(entry.path())?);
            }
        }
        let civil_source = civil_sources.join("\n");
        assert!(
            runtime_source.contains("from rust::std::time import") && !runtime_source.contains("@rust"),
            "std.datetime runtime must use the Rust std::time boundary without raw @rust bodies"
        );
        assert!(
            !civil_source.contains("from rust::") && !civil_source.contains("@rust"),
            "std.datetime civil calendar code must remain source-defined Incan"
        );

        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/std_datetime_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "std.datetime surface run failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "500",
                "2",
                "9",
                "true",
                "true",
                "true",
                "2026-04-21",
                "2026-07-14",
                "true",
                "2026-04-15T00:34:56.123456789",
                "Tue Apr 14 2026",
                "12:34:56.123456789",
                "07:08:09.123456789",
                "2026-04-14",
                "2026-04-14T07:08:09.123456789",
                "2026-04-14",
                "53",
                "bad-week",
                "2026-04-15T12:34:56",
                "true",
                "1800",
                "+01:00",
                "Z",
                "2026-04-14T12:34:56.123456789+01:00",
                "2026-04-14T12:34:56.123456789+0100",
                "2026-04-14 12:34:56.123456789+01:00",
                "2026-04-14T12:34:56.123456789+01:00",
                "2026-04-14T12:34:56Z",
                "bad-offset",
                "long-nanos",
                "bad-date-digits",
                "bad-time-digits",
                "named-timezone",
            ],
            "unexpected std.datetime output: {stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_compression_surface_runs_generated_project() -> Result<(), Box<dyn std::error::Error>> {
        // Keep std.compression's generated-project dependencies in the root Cargo graph so CI fetches them before this
        // smoke runs the generated project under CARGO_NET_OFFLINE.
        use std::io::{Cursor, Read as _};

        let sample = b"abc";
        let mut gzip = flate2::read::GzEncoder::new(Cursor::new(sample), flate2::Compression::new(6));
        let mut gzip_out = Vec::new();
        gzip.read_to_end(&mut gzip_out)?;
        assert!(!gzip_out.is_empty());

        let zstd_out = zstd::stream::encode_all(Cursor::new(sample), 0)?;
        assert!(!zstd_out.is_empty());

        let mut bz2 = bzip2::read::BzEncoder::new(Cursor::new(sample), bzip2::Compression::new(6));
        let mut bz2_out = Vec::new();
        bz2.read_to_end(&mut bz2_out)?;
        assert!(!bz2_out.is_empty());

        let mut lzma = xz2::read::XzEncoder::new(Cursor::new(sample), 6);
        let mut lzma_out = Vec::new();
        lzma.read_to_end(&mut lzma_out)?;
        assert!(!lzma_out.is_empty());

        let mut snappy = snap::raw::Encoder::new();
        assert!(!snappy.compress_vec(sample)?.is_empty());

        let output = incan_command()
            .arg("run")
            .arg(incan_test_support::fixture("valid/std_compression_surface.incn"))
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "std.compression surface run failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec![
                "gzip round trip ok",
                "zlib round trip ok",
                "deflate round trip ok",
                "zstd round trip ok",
                "bz2 round trip ok",
                "lzma round trip ok",
                "snappy round trip ok",
                "snappy.raw round trip ok",
                "autodetection ok",
                "stream round trips ok",
                "file stream round trip ok",
                "option and chunk errors ok",
            ],
            "unexpected std.compression output: {stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_rust_associated_call_in_elif_branch_uses_path_syntax() {
        let Ok(output) = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::path import Path

def f(kind: str, output_uri: str) -> bool:
    if kind == "a":
        return Path.new(output_uri).exists()
    elif kind == "b":
        return Path.new(output_uri).exists()
    else:
        return false

def main() -> None:
    println(f("a", "missing-a"))
    println(f("b", "missing-b"))
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
        else {
            panic!("failed to run incan");
        };

        assert!(
            output.status.success(),
            "rust associated call in elif branch failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_rust_path_new_borrows_owned_string_argument() {
        let output = run_incan_source(
            r#"
from rust::std::path import Path as RustPath

def main() -> None:
    path = "example.txt"
    println(RustPath.new(path).exists())
"#,
        );

        assert!(
            output.status.success(),
            "Path.new should borrow an owned Incan string at the Rust boundary. stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// End-to-end integration tests for `incan test`.
///
/// These tests exercise the full pipeline: write an Incan test file → run `incan test` via the CLI → verify
/// stdout/stderr/exit code. They catch integration bugs like broken Oven native-harness wiring or parametrize
/// expansion that unit tests cannot detect.
mod test_runner_e2e {
    use super::incan_command;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestProject {
        dir: tempfile::TempDir,
    }

    impl std::ops::Deref for TestProject {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            self.dir.path()
        }
    }

    /// Create a temp directory with a single test file and keep it alive for the test duration.
    fn write_test_project(filename: &str, source: &str) -> TestProject {
        let seq = TEST_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix = format!("incan_e2e_test_{}_{}_", std::process::id(), seq);
        let Ok(dir) = tempfile::Builder::new().prefix(&prefix).tempdir() else {
            panic!("failed to create temp dir");
        };
        let Ok(()) = std::fs::write(dir.path().join(filename), source) else {
            panic!("failed to write test file");
        };
        TestProject { dir }
    }

    /// Run `incan test` for the given path argument (file or directory).
    fn run_incan_test_path(path: &Path) -> std::process::Output {
        incan_command()
            .args(["test", path.to_string_lossy().as_ref()])
            .output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    /// Run `incan test` on a directory and return the combined output.
    fn run_incan_test(dir: &Path) -> std::process::Output {
        run_incan_test_path(dir)
    }

    /// Run `incan test` with extra flags.
    fn run_incan_test_with_args(dir: &Path, extra: &[&str]) -> std::process::Output {
        let mut cmd = incan_command();
        cmd.arg("test");
        for arg in extra {
            cmd.arg(arg);
        }
        cmd.arg(dir.to_string_lossy().as_ref());
        cmd.output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    /// Run `incan test` with `cwd` and a relative path argument.
    fn run_incan_test_relative(cwd: &Path, relative_path: &str) -> std::process::Output {
        incan_command()
            .arg("test")
            .arg(relative_path)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("failed to run `incan test {relative_path}`: {}", e))
    }

    /// Run `incan build <entry> <out_dir>` for an inline-test production source.
    fn run_incan_build(entry: &Path, out_dir: &Path) -> std::process::Output {
        let output = incan_command()
            .args([
                "build",
                entry.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output();
        let Ok(output) = output else {
            panic!("failed to run `incan build`");
        };
        output
    }

    // ---- Passing test ----

    /// Issue #815: generic and `Self`-returning Index adoptions must compile in the generated test package.
    #[test]
    fn e2e_issue815_generic_index_trait_adoptions_compile() {
        let dir = write_test_project(
            "test_issue815_generic_index.incn",
            r#"
from std.testing import assert_eq
from std.traits.indexing import Index

class GenericBox[T with Clone] with Index[str, str]:
    pub label: str
    pub witness: list[T]

    def __getitem__(self, key: str) for Index[str, str] -> str:
        return key

class PlainBox with Index[list[str], Self]:
    pub label: str

    def __getitem__(self, key: list[str]) for Index[list[str], Self] -> Self:
        return self

class GenericSelfBox[T with Clone] with Index[list[str], Self]:
    pub label: str
    pub witness: list[T]

    def __getitem__(self, key: list[str]) for Index[list[str], Self] -> Self:
        return self

def test_generic_index() -> None:
    box = GenericBox[int](label="orders", witness=[1])
    assert_eq(box["amount"], "amount")

def test_self_returning_index() -> None:
    box = PlainBox(label="nested")
    assert_eq(box[["name"]].label, "nested")

def test_generic_self_returning_index() -> None:
    box = GenericSelfBox[int](label="nested-generic", witness=[1])
    assert_eq(box[["name"]].label, "nested-generic")
"#,
        );

        let output = run_incan_test(&dir);
        assert!(
            output.status.success(),
            "expected issue #815 test package to compile and pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn e2e_basic_reporting_decorator_filter_and_capture_share_one_project() {
        let dir = write_test_project(
            "test_runner_surface.incn",
            r#"
from std.testing import assert_eq, test

def test_addition() -> None:
    assert_eq(1 + 1, 2)

def test_one() -> None:
    assert_eq(1, 1)

def test_two() -> None:
    assert_eq(2, 2)

@test
def verifies_total() -> None:
    assert_eq(40 + 2, 42)

def test_alpha() -> None:
    assert_eq(1, 1)

def test_beta() -> None:
    assert_eq(2, 2)

def test_prints() -> None:
    print("VISIBLE_CAPTURE")
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected both tests to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("PASSED") || stdout.contains("passed"),
            "expected PASSED in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_runner_surface.incn::test_one")
                && stdout.contains("test_runner_surface.incn::test_two")
                && stdout.contains("test_runner_surface.incn::verifies_total"),
            "expected basic and decorated test names in reporter output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.match_indices("PASSED").count() >= 6,
            "expected passing result lines for all basic surface tests.\nstdout:\n{}",
            stdout,
        );

        // Stable root-relative IDs and keyword selection are pure runner rules.
        // The marker collection matrix retains the single CLI-level list path.
        let captured = run_incan_test_with_args(&dir, &["--nocapture", "-k", "test_prints"]);
        let captured_stdout = String::from_utf8_lossy(&captured.stdout);
        let captured_stderr = String::from_utf8_lossy(&captured.stderr);
        assert!(
            captured.status.success(),
            "expected nocapture run to succeed.\nstdout:\n{}\nstderr:\n{}",
            captured_stdout,
            captured_stderr,
        );
        assert!(captured_stdout.contains("VISIBLE_CAPTURE"));
    }

    #[test]
    fn e2e_generated_harness_success_reports_the_incan_test_identity_issue996() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = write_test_project(
            "loaf.toml",
            "[project]\nname = \"test_runner_repro\"\nversion = \"0.1.0\"\n",
        );
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::create_dir_all(dir.join("tests"))?;
        std::fs::write(dir.join("src/lib.incn"), "pub def answer() -> int:\n  return 42\n")?;
        std::fs::write(
            dir.join("tests/test_smoke.incn"),
            "def test_smoke__reports_pass() -> None:\n  assert 42 == 42\n",
        )?;

        let output = run_incan_test_relative(&dir, "tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected the passing generated harness to preserve its Incan result.\nstdout:\n{stdout}\nstderr:\n{stderr}",
        );
        assert!(
            stdout.contains("test_smoke.incn::test_smoke__reports_pass") && stdout.contains("PASSED"),
            "expected the runner to report the collected Incan identity as passing.\nstdout:\n{stdout}",
        );
        assert!(
            !stdout.contains("Test runner did not report outcome"),
            "a successful native harness must not become a missing-outcome failure.\nstdout:\n{stdout}",
        );
        Ok(())
    }

    #[test]
    fn e2e_generated_harness_oven_bake_is_reused() {
        let dir = write_test_project(
            "test_oven_bake_reuse.incn",
            r#"
from std.testing import assert_eq

def test_oven_bake_reuse() -> None:
    assert_eq(1, 1)
"#,
        );

        let first = run_incan_test_with_args(&dir, &["-v"]);
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first Oven run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );
        assert!(
            first_stdout.contains("Oven test phases"),
            "expected verbose first run to report Oven phases.\nstdout:\n{}",
            first_stdout,
        );
        assert!(
            first_stdout.contains("planned 1 generated harness unit(s)"),
            "expected verbose run to report generated harness planning.\nstdout:\n{}",
            first_stdout,
        );
        assert!(
            first_stdout.contains("native bake"),
            "expected first run to bake its caller-owned native output.\nstdout:\n{}",
            first_stdout,
        );

        let second = run_incan_test_with_args(&dir, &["-v"]);
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        assert!(
            second.status.success(),
            "expected second Oven run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_stdout.contains("native reuse"),
            "expected second run to reuse its stored native output.\nstdout:\n{}",
            second_stdout,
        );
    }

    #[test]
    fn e2e_cross_file_batch_falls_back_when_top_level_names_collide() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_a.incn",
            r#"
from std.testing import assert_eq

model Order:
    id: int

def test_a() -> None:
    order = Order(id=1)
    assert_eq(order.id, 1)
"#,
        );
        std::fs::write(
            dir.join("test_b.incn"),
            r#"
from std.testing import assert_eq

model Order:
    id: int

def test_b() -> None:
    order = Order(id=2)
    assert_eq(order.id, 2)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected same-named top-level declarations in different files to run in isolated harnesses.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_a.incn::test_a") && stdout.contains("test_b.incn::test_b"),
            "expected both tests in reporter output.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_cross_file_batch_rebases_spans_for_type_info_issue692() -> Result<(), Box<dyn std::error::Error>> {
        fn source_with_call_offset(header: &str, call_prefix: &str, call_and_tail: &str, offset: usize) -> String {
            let fixed_len = header.len() + call_prefix.len();
            assert!(
                offset >= fixed_len + 6,
                "test fixture offset leaves no room for padding"
            );
            let padding = format!("    #{}\n", "x".repeat(offset - fixed_len - 6));
            format!("{header}{padding}{call_prefix}{call_and_tail}")
        }

        let target_offset = 320;
        let dir = write_test_project(
            "test_constructor_marker.incn",
            &source_with_call_offset(
                "model Box:\n    value: int\n\ndef test_type_constructor() -> None:\n",
                "    item = ",
                "Box(value=1)\n    assert item.value == 1\n",
                target_offset,
            ),
        );
        std::fs::write(
            dir.join("test_zero_arg_call.incn"),
            source_with_call_offset(
                "def tap() -> str:\n    return \"ok\"\n\ndef test_zero_arg_call_in_list() -> None:\n",
                "    values = [",
                "tap()]\n    assert values[0] == \"ok\"\n",
                target_offset,
            ),
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected same-span constructor and zero-argument calls from different files not to share type-info facts.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_constructor_marker.incn::test_type_constructor")
                && stdout.contains("test_zero_arg_call.incn::test_zero_arg_call_in_list"),
            "expected both files to run in one directory test batch.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_imported_default_expression_expands_with_required_scope_issue395() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "default_expr_import_test_repro"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            src_dir.join("defaults.incn"),
            r#"
pub def fallback() -> int:
    return 2
"#,
        )?;
        std::fs::write(
            src_dir.join("helper.incn"),
            r#"
from defaults import fallback

pub def combine(left: int, middle: int = fallback(), right: int = 3) -> int:
    return left + middle + right
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_default_expr_import.incn"),
            r#"
from std.testing import assert_eq
from helper import combine

def test_imported_default_expression_expands_with_required_imports() -> None:
    assert_eq(combine(left=1, right=4), 7, "default expression helper should be available after expansion")
"#,
        )?;

        let output = run_incan_test_relative(&dir, "tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected imported default expression test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains(
                "test_default_expr_import.incn::test_imported_default_expression_expands_with_required_imports"
            ),
            "expected issue 395 test name in reporter output.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_report_formats_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_report_formats.incn",
            r#"
from std.testing import assert_eq

def test_report_one() -> None:
    assert_eq(1, 1)
"#,
        );

        let json_output = run_incan_test_with_args(&dir, &["--format", "json", "--shuffle", "--seed", "7"]);
        let json_stdout = String::from_utf8_lossy(&json_output.stdout);
        let json_stderr = String::from_utf8_lossy(&json_output.stderr);
        assert!(
            json_output.status.success(),
            "expected JSON-format run to succeed.\nstdout:\n{}\nstderr:\n{}",
            json_stdout,
            json_stderr,
        );

        let mut saw_result = false;
        let mut saw_summary = false;
        for line in json_stdout.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line)?;
            if value.get("test_id").is_some() {
                saw_result = true;
                assert_eq!(
                    value.get("schema_version").and_then(|v| v.as_str()),
                    Some("incan.test.v1")
                );
                assert_eq!(
                    value.get("test_id").and_then(|v| v.as_str()),
                    Some("test_report_formats.incn::test_report_one")
                );
                assert_eq!(value.get("status").and_then(|v| v.as_str()), Some("passed"));
            }
            if value.get("summary").is_some() {
                saw_summary = true;
                assert_eq!(
                    value
                        .get("summary")
                        .and_then(|summary| summary.get("shuffle_seed"))
                        .and_then(|v| v.as_u64()),
                    Some(7)
                );
            }
        }
        assert!(
            saw_result,
            "expected at least one JSON result record.\nstdout:\n{}",
            json_stdout
        );
        assert!(saw_summary, "expected a JSON summary record.\nstdout:\n{}", json_stdout);

        let report = dir.join("reports").join("junit.xml");
        let report_arg = report.to_string_lossy().to_string();
        let junit_output = run_incan_test_with_args(&dir, &["--junit", report_arg.as_str()]);
        let junit_stdout = String::from_utf8_lossy(&junit_output.stdout);
        let junit_stderr = String::from_utf8_lossy(&junit_output.stderr);
        assert!(
            junit_output.status.success(),
            "expected JUnit report run to succeed.\nstdout:\n{}\nstderr:\n{}",
            junit_stdout,
            junit_stderr,
        );
        let xml = std::fs::read_to_string(&report)?;
        assert!(
            xml.contains("<testsuite") && xml.contains("test_report_one"),
            "expected JUnit XML with test case, got:\n{}",
            xml,
        );
        Ok(())
    }

    #[test]
    fn e2e_run_xfail_treats_xfail_as_ordinary_test() {
        let dir = write_test_project(
            "test_run_xfail.incn",
            r#"
from std.testing import assert_eq, xfail

@xfail("currently passes")
def test_xpass() -> None:
    assert_eq(1, 1)
"#,
        );

        let default = run_incan_test(&dir);
        let default_stdout = String::from_utf8_lossy(&default.stdout);
        let default_stderr = String::from_utf8_lossy(&default.stderr);
        assert!(
            !default.status.success(),
            "expected default xpass to fail.\nstdout:\n{}\nstderr:\n{}",
            default_stdout,
            default_stderr,
        );

        let run_xfail = run_incan_test_with_args(&dir, &["--run-xfail"]);
        let stdout = String::from_utf8_lossy(&run_xfail.stdout);
        let stderr = String::from_utf8_lossy(&run_xfail.stderr);
        assert!(
            run_xfail.status.success(),
            "expected --run-xfail to treat xfail marker as ordinary.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_run_xfail.incn::test_xpass") && stdout.contains("PASSED"),
            "expected ordinary passing output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_conftest_nearest_fixture_override_project() {
        let override_dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "nested_conftest_precedence"
version = "0.1.0"
"#,
        );
        let override_tests_dir = override_dir.join("tests");
        let override_unit_dir = override_tests_dir.join("unit");
        if let Err(err) = std::fs::create_dir_all(&override_unit_dir) {
            panic!("failed to create nested tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            override_tests_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def shared() -> str:
    return "parent"
"#,
        ) {
            panic!("failed to write parent conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            override_unit_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def shared() -> str:
    return "child"
"#,
        ) {
            panic!("failed to write nested conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            override_unit_dir.join("test_precedence.incn"),
            r#"
from std.testing import assert_eq

def test_uses_nearest_fixture(shared: str) -> None:
    assert_eq(shared, "child")
"#,
        ) {
            panic!("failed to write nested conftest test: {}", err);
        }

        let output = run_incan_test(&override_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected nearest conftest fixture to override parent fixture without duplicate generated functions.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );
        assert!(stdout.contains("test_uses_nearest_fixture"));
    }

    #[test]
    fn e2e_builtin_fixture_and_assert_helper_share_one_project() {
        let dir = write_test_project(
            "test_builtin_fixture_and_assert_helper.incn",
            r#"
from std.testing import assert_eq
import std.testing as testing
from rust::std::path import PathBuf

def test_tmp_path_fixture(tmp_path: PathBuf) -> None:
    assert_eq(tmp_path.exists(), true)

def test_assert_helper() -> None:
    testing.assert(True)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected built-in tmp_path fixture to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_assert_helper"));
    }

    #[test]
    fn e2e_markers_parametrize_timeout_and_collection_errors_share_projects() {
        let platform = std::env::consts::OS;
        let dir = write_test_project(
            "test_runner_collection_surface.incn",
            &format!(
                r#"
from rust::std::thread import sleep
from rust::std::time import Duration
from std.testing import assert_eq, feature, mark, param_case, parametrize, platform, skipif, slow, timeout, xfail, xfailif

const TEST_MARKERS: List[str] = ["api", "db", "smoke"]
const TEST_MARKS: List[str] = ["smoke"]

def test_inherited_smoke() -> None:
    assert_eq(1, 1)

@mark("api")
def test_api() -> None:
    assert_eq(1, 1)

@mark("api")
@slow
def test_api_slow() -> None:
    assert_eq(1, 1)

@mark("db")
def test_db() -> None:
    assert_eq(1, 1)

def test_fast() -> None:
    assert_eq(1, 1)

@slow
def test_slow_case() -> None:
    assert_eq(1, 1)

@parametrize("x, expected", [
    param_case((1, 3), marks=[xfail("known")], id="one-three"),
    (2, 4),
], ids=["ignored", "two-four"])
def test_marked_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)

@parametrize("x", [1, 2], ids=["one", "two"])
@parametrize("y", [10, 20], ids=["ten", "twenty"])
def test_pair(x: int, y: int) -> None:
    assert_eq(x < y, true)

@parametrize("a, b, expected", [(1, 2, 3), (10, 20, 30), (0, 0, 0)])
def test_add(a: int, b: int, expected: int) -> None:
    assert_eq(a + b, expected)

@parametrize("x, expected", [(2, 4), (3, 7)])
def test_double_failure(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)

@skipif(platform() == "{platform}", reason="host platform")
def test_skip_on_platform_probe() -> None:
    assert_eq(1, 0)

@xfailif(feature("known_bug"), reason="feature-gated known issue")
def test_feature_xfail() -> None:
    assert_eq(1, 0)

@timeout("1ms")
def test_timeout_marker() -> None:
    sleep(Duration.from_millis(100))
"#
            ),
        );

        // One list invocation exercises CLI-to-runner marker wiring. Pure parser,
        // strict-registration, keyword, and slow-selection combinations are unit
        // tested below the process boundary rather than rebuilding this project.
        let marker_list = run_incan_test_with_args(
            &dir,
            &["--list", "-m", "api and not slow", "--strict-markers", "--slow"],
        );
        let marker_stdout = String::from_utf8_lossy(&marker_list.stdout);
        let marker_stderr = String::from_utf8_lossy(&marker_list.stderr);
        assert!(
            marker_list.status.success(),
            "expected boolean marker expression to collect.\nstdout:\n{}\nstderr:\n{}",
            marker_stdout,
            marker_stderr,
        );
        assert!(marker_stdout.contains("test_runner_collection_surface.incn::test_api"));
        assert!(!marker_stdout.contains("test_runner_collection_surface.incn::test_api_slow"));
        assert!(!marker_stdout.contains("test_runner_collection_surface.incn::test_db"));

        // This one ordinary execution covers the outcome matrix below. Its
        // constituent cases previously rebuilt the same generated test project
        // five times with different keyword filters, despite no filter-specific
        // behavior being under test here.
        let ordinary = run_incan_test_with_args(&dir, &["--verbose"]);
        let ordinary_stdout = String::from_utf8_lossy(&ordinary.stdout);
        let ordinary_stderr = String::from_utf8_lossy(&ordinary.stderr);
        let ordinary_combined = format!("{ordinary_stdout}\n{ordinary_stderr}");
        assert!(
            !ordinary.status.success(),
            "expected the ordinary matrix to report its failing parameter, feature-disabled xfail, and timeout cases.\n{}",
            ordinary_combined,
        );
        assert!(
            ordinary_combined.contains("xfailed") || ordinary_combined.contains("XFAIL"),
            "expected the marked parametrized case to remain xfailed.\n{}",
            ordinary_combined,
        );
        for expected in [
            "test_add[1-2-3]",
            "test_add[10-20-30]",
            "test_add[0-0-0]",
            "test_double_failure[3-7]",
            "test_skip_on_platform_probe",
            "test_feature_xfail",
            "test_timeout_marker",
            "timed out after",
        ] {
            assert!(
                ordinary_combined.contains(expected),
                "expected ordinary outcome matrix to report `{expected}`.\n{}",
                ordinary_combined,
            );
        }
        assert!(
            ordinary_combined.contains("SKIPPED") || ordinary_combined.contains("skipped"),
            "expected the host-platform skip to remain reported.\n{}",
            ordinary_combined,
        );

        // Parsing `--feature` and turning a true `xfailif(feature(...))` into
        // the runner's XFail marker are direct CLI/discovery contracts. The
        // ordinary execution above already proves that an XFail marker renders
        // correctly in the generated runner, so it need not rebuild this same
        // project with a one-test keyword filter.
    }

    #[test]
    fn e2e_jobs_fail_fast_stops_launching_pending_units() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_a_fail.incn",
            r#"
def test_a_fail() -> None:
    assert 1 == 2

def test_c_pending() -> None:
    pass
"#,
        );
        std::fs::write(
            dir.join("test_b_slow.incn"),
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_b_slow() -> None:
    sleep(Duration.from_millis(800))
"#,
        )?;
        let output = run_incan_test_with_args(&dir, &["--jobs", "2", "-x"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected fail-fast run to fail.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_a_fail"),
            "expected failing test to be reported.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains("test_c_pending"),
            "expected fail-fast scheduler not to launch pending units after the first completed failure.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_jobs_run_independent_files_concurrently() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_sleep_a.incn",
            "from rust::std::thread import sleep\nfrom rust::std::time import Duration\n\ndef test_sleep_a() -> None:\n    sleep(Duration.from_millis(600))\n",
        );
        std::fs::write(
            dir.join("test_sleep_b.incn"),
            "from rust::std::thread import sleep\nfrom rust::std::time import Duration\n\ndef test_sleep_b() -> None:\n    sleep(Duration.from_millis(600))\n",
        )?;
        let output = run_incan_test_with_args(&dir, &["--jobs", "2"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "parallel run failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let running_a = stdout
            .find("test_sleep_a.incn (1 item(s))")
            .ok_or("missing sleep_a start")?;
        let running_b = stdout
            .find("test_sleep_b.incn (1 item(s))")
            .ok_or("missing sleep_b start")?;
        let passed_a = stdout
            .find("test_sleep_a.incn::test_sleep_a PASSED")
            .ok_or("missing sleep_a pass")?;
        let passed_b = stdout
            .find("test_sleep_b.incn::test_sleep_b PASSED")
            .ok_or("missing sleep_b pass")?;
        assert!(
            running_a < passed_a.min(passed_b) && running_b < passed_a.min(passed_b),
            "--jobs 2 did not start both independent batches before either completed:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn e2e_sequential_single_file_runs_do_not_cross_wire_paths() {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "session_isolation_relative"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_alpha.incn"),
            r#"
from std.testing import assert_eq

def test_alpha_one() -> None:
    assert_eq(1, 1)

def test_alpha_two() -> None:
    assert_eq(2, 2)
"#,
        ) {
            panic!("failed to write test_alpha.incn: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_beta.incn"),
            r#"
from std.testing import assert_eq

def test_beta_only() -> None:
    assert_eq(3, 3)
"#,
        ) {
            panic!("failed to write test_beta.incn: {}", err);
        }

        let first = run_incan_test_relative(&dir, "tests/test_alpha.incn");
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first single-file run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );

        let second = run_incan_test_relative(&dir, "tests/test_beta.incn");
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        let second_combined = format!("{second_stdout}\n{second_stderr}");
        assert!(
            second.status.success(),
            "expected second single-file run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_combined.contains("test_beta.incn::test_beta_only"),
            "expected the requested beta test to run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("test_alpha.incn::test_alpha_one")
                && !second_combined.contains("test_alpha.incn::test_alpha_two"),
            "expected no alpha tests in second single-file run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("Test runner did not report outcome"),
            "expected no missing-outcome diagnostic in second run.\noutput:\n{}",
            second_combined,
        );

        let alpha_absolute_path = tests_dir.join("test_alpha_abs.incn");
        let beta_absolute_path = tests_dir.join("test_beta_abs.incn");
        if let Err(err) = std::fs::write(
            &alpha_absolute_path,
            r#"
from std.testing import assert_eq

def test_alpha_abs_one() -> None:
    assert_eq(10, 10)
"#,
        ) {
            panic!("failed to write test_alpha_abs.incn: {}", err);
        }
        if let Err(err) = std::fs::write(
            &beta_absolute_path,
            r#"
from std.testing import assert_eq

def test_beta_abs_only() -> None:
    assert_eq(20, 20)
"#,
        ) {
            panic!("failed to write test_beta_abs.incn: {}", err);
        }

        let first = run_incan_test_path(&alpha_absolute_path);
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first absolute-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );

        let second = run_incan_test_path(&beta_absolute_path);
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        let second_combined = format!("{second_stdout}\n{second_stderr}");
        assert!(
            second.status.success(),
            "expected second absolute-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_combined.contains("test_beta_abs.incn::test_beta_abs_only"),
            "expected the requested absolute-path beta test to run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("test_alpha_abs.incn::test_alpha_abs_one"),
            "expected no alpha absolute-path tests in second run.\noutput:\n{}",
            second_combined,
        );
        assert!(
            !second_combined.contains("Test runner did not report outcome"),
            "expected no missing-outcome diagnostic in second absolute-path run.\noutput:\n{}",
            second_combined,
        );
    }

    #[test]
    fn e2e_nested_package_modules_in_tests_succeed() {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "nested_test"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");

        if let Err(err) = std::fs::create_dir_all(src_dir.join("dataset")) {
            panic!("failed to create nested src dirs: {}", err);
        }
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub const DATASET_VERSION: int = 1\n",
        ) {
            panic!("failed to write dataset mod source: {}", err);
        }
        if let Err(err) = std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset import DATASET_VERSION\npub def filter_ds(value: int) -> int:\n    return value + DATASET_VERSION\n",
        ) {
            panic!("failed to write dataset ops source: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_dataset.incn"),
            r#"
from std.testing import assert_eq
from dataset import DATASET_VERSION
from dataset.ops import filter_ds

def test_nested_dataset_modules() -> None:
    assert_eq(DATASET_VERSION, 1)
    assert_eq(filter_ds(41), 42)
"#,
        ) {
            panic!("failed to write nested dataset test: {}", err);
        }

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected nested package module test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            !stderr.contains("file for module `dataset` found at both"),
            "expected no stale flat-vs-nested module collision.\nstderr:\n{}",
            stderr,
        );
    }

    #[test]
    fn e2e_test_runner_preserves_fixture_cwd_for_file_and_batch_runs() {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "fixture_cwd_parity"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        let fixtures_dir = tests_dir.join("fixtures");

        if let Err(err) = std::fs::create_dir_all(&fixtures_dir) {
            panic!("failed to create fixture dir: {}", err);
        }
        if let Err(err) = std::fs::write(fixtures_dir.join("orders.csv"), "id\n1\n") {
            panic!("failed to write fixture file: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_fixture_path.incn"),
            r#"
from std.testing import assert_eq
from rust::std::path import Path

const FIXTURE: str = "tests/fixtures/orders.csv"

def test_fixture_path_exists() -> None:
    assert_eq(Path.new(FIXTURE).exists(), true)
"#,
        ) {
            panic!("failed to write fixture path test: {}", err);
        }

        let single = run_incan_test_relative(&dir, "tests/test_fixture_path.incn");
        let single_stdout = String::from_utf8_lossy(&single.stdout);
        let single_stderr = String::from_utf8_lossy(&single.stderr);
        assert!(
            single.status.success(),
            "expected single-file fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            single_stdout,
            single_stderr,
        );

        let batch = run_incan_test_relative(&dir, "tests");
        let batch_stdout = String::from_utf8_lossy(&batch.stdout);
        let batch_stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected batched fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            batch_stdout,
            batch_stderr,
        );

        use std::time::{SystemTime, UNIX_EPOCH};

        let mut bare_dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        bare_dir.push(format!("incan_e2e_test_nomani_{}", duration.as_nanos()));
        if let Err(err) = std::fs::create_dir_all(&bare_dir) {
            panic!("failed to create temp dir: {}", err);
        }
        let tests_dir = bare_dir.join("tests");
        let fixtures_dir = tests_dir.join("fixtures");

        if let Err(err) = std::fs::create_dir_all(&fixtures_dir) {
            panic!("failed to create fixture dir: {}", err);
        }
        if let Err(err) = std::fs::write(fixtures_dir.join("ok.txt"), "ok\n") {
            panic!("failed to write fixture file: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_cwd.incn"),
            r#"
from std.testing import assert_eq
from rust::std::path import Path

def test_cwd__fixture_path_is_repo_relative() -> None:
    assert_eq(
        Path.new("tests/fixtures/ok.txt").exists(),
        true,
        "fixture path should resolve from the project root in both per-file and batched test runs",
    )
"#,
        ) {
            panic!("failed to write fixture path test: {}", err);
        }

        let single = run_incan_test_relative(&bare_dir, "tests/test_cwd.incn");
        let single_stdout = String::from_utf8_lossy(&single.stdout);
        let single_stderr = String::from_utf8_lossy(&single.stderr);
        assert!(
            single.status.success(),
            "expected manifest-less single-file fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            single_stdout,
            single_stderr,
        );

        let batch = run_incan_test_relative(&bare_dir, "tests");
        let batch_stdout = String::from_utf8_lossy(&batch.stdout);
        let batch_stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected manifest-less batched fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            batch_stdout,
            batch_stderr,
        );
    }

    #[test]
    fn e2e_inline_and_imported_surfaces_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_and_imported_surface_batch"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(src_dir.join("widgets.incn"), "pub static MARKER: int = 41\n")?;
        std::fs::write(
            src_dir.join("defaults.incn"),
            r#"
pub def fallback() -> int:
    return 2
"#,
        )?;
        std::fs::write(
            src_dir.join("helper.incn"),
            r#"
from defaults import fallback

pub def combine(left: int, middle: int = fallback(), right: int = 3) -> int:
    return left + middle + right
"#,
        )?;
        std::fs::write(
            src_dir.join("helpers.incn"),
            r#"
pub def count_names(names: List[str]) -> int:
    return len(names)
"#,
        )?;
        std::fs::write(
            src_dir.join("registry.incn"),
            r#"
pub const TOKEN: str = "token"
pub const DECORATOR_TOKEN: str = "probe.value"

def keep_int(func: (int) -> int) -> (int) -> int:
    return func

pub def registered(_name: str) -> Callable[(int) -> int, (int) -> int]:
    return keep_int
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"
def add(a: int, b: int) -> int:
    return a + b

def secret() -> str:
    return "private"

def main() -> None:
    println("production")

module tests:
    from rust::incan_std_testing import TestEnv
    from rust::std::path import PathBuf
    import std.testing as testing
    from std.testing import assert_eq, assert_is_some, fixture, test

    @fixture(autouse=true)
    def seed() -> int:
        return 40

    @fixture
    def answer(seed: int) -> int:
        return seed + 2

    @fixture(autouse=true)
    def isolate_env(mut env: TestEnv) -> None:
        env.unset("INCAN_INLINE_ENV_FIXTURE")

    def test_inline_addition(seed: int) -> None:
        assert_eq(seed, 40)
        assert_eq(add(2, 3), 5)

    def test_inline_private_access(seed: int) -> None:
        assert_eq(seed, 40)
        assert_eq(secret(), "private")

    def test_inline_assert_helper(seed: int) -> None:
        assert_eq(seed, 40)
        testing.assert(True)

    @test
    def decorated_inline_case(seed: int) -> None:
        assert_eq(seed, 40)
        assert_eq(add(20, 22), 42)

    def test_inline_fixture_and_tmp_path(answer: int, tmp_path: PathBuf) -> None:
        assert_eq(answer, 42)
        assert_eq(tmp_path.exists(), true)

    def test_inline_tmp_workdir(tmp_workdir: PathBuf) -> None:
        assert_eq(tmp_workdir.exists(), true)

    def test_inline_env_fixture(mut env: TestEnv) -> None:
        env.set("INCAN_INLINE_ENV_FIXTURE", "set")
        assert_eq(assert_is_some(env.get("INCAN_INLINE_ENV_FIXTURE")), "set")
        env.unset("INCAN_INLINE_ENV_FIXTURE")
        assert_eq(env.get("INCAN_INLINE_ENV_FIXTURE"), None)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_imported_surface_batch.incn"),
            r#"
from std.testing import assert_eq
from helper import combine
from helpers import count_names
from registry import DECORATOR_TOKEN, TOKEN, registered
from widgets import MARKER

def identity(value: str) -> str:
    return value

@registered(DECORATOR_TOKEN)
def increment(value: int) -> int:
    return value + 1

def test_imported_const_str_call_arguments_materialize() -> None:
    local: str = TOKEN
    assert_eq(identity(TOKEN), "token")
    assert_eq(identity(TOKEN.to_string()), "token")
    assert_eq(identity(local), "token")
    assert_eq(TOKEN.upper(), "TOKEN")

def test_imported_decorator_factory_const_str_argument_materializes() -> None:
    assert_eq(increment(1), 2)

def test_imported_pub_static_scalar_read() -> None:
    assert_eq(MARKER, 41)

def test_empty_names() -> None:
    assert_eq(count_names([]), 0)

def test_assert_statement_sugar() -> None:
    assert 1 + 1 == 2
    assert 3 != 4
    assert not False
    assert True

def test_imported_default_expression_expands_with_required_imports() -> None:
    assert_eq(combine(left=1, right=4), 7, "default expression helper should be available after expansion")
"#,
        )?;
        let production_entry = src_dir.join("production_only.incn");
        std::fs::write(
            &production_entry,
            r#"
def main() -> None:
    println("production")

module tests:
    from std.testing import assert_eq

    def test_production() -> None:
        assert_eq(1 + 1, 2)
"#,
        )?;

        let mut bake_command = incan_command();
        bake_command
            .args(["oven", "bake", "--project", "."])
            .current_dir(&*dir)
            .env("CARGO_NET_OFFLINE", "true");
        super::support::configure_explicit_oven_bake_command(&mut bake_command)?;
        let bake_output = bake_command.output()?;
        assert!(
            bake_output.status.success(),
            "expected one explicit Oven bake to prepare the complete inline/imported test project.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&bake_output.stdout),
            String::from_utf8_lossy(&bake_output.stderr),
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected batched inline/imported test-runner surfaces to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("main.incn::test_inline_addition")
                && stdout.contains("main.incn::test_inline_private_access")
                && stdout.contains("main.incn::decorated_inline_case")
                && stdout.contains("main.incn::test_inline_fixture_and_tmp_path")
                && stdout.contains("test_imported_surface_batch.incn::test_imported_pub_static_scalar_read")
                && stdout.contains(
                    "test_imported_surface_batch.incn::test_imported_default_expression_expands_with_required_imports"
                ),
            "expected representative batched inline/imported test names.\nstdout:\n{}",
            stdout
        );
        assert!(
            !stderr.contains("str_as_str") && !stderr.contains("expected `String`, found `&str`"),
            "imported const str call and decorator arguments should materialize as owned strings.\nstderr:\n{}",
            stderr,
        );
        assert!(
            !stderr.contains("type annotations needed"),
            "expected no Rust inference failure for empty string list.\nstderr:\n{}",
            stderr,
        );
        assert!(
            !stderr.contains("vec![].into_iter().map(|s| s.to_string()).collect()"),
            "expected no untyped empty string-list conversion in generated Rust.\nstderr:\n{}",
            stderr,
        );

        let out_dir = dir.join("out");
        let build_output = run_incan_build(&production_entry, &out_dir);
        let build_stderr = String::from_utf8_lossy(&build_output.stderr);

        assert!(
            build_output.status.success(),
            "expected production build to ignore inline test imports.\nstderr:\n{}",
            build_stderr,
        );
        let main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            !main_rs.contains("__incan_std::testing"),
            "inline test import should not leak into generated production code:\n{}",
            main_rs,
        );
        assert!(
            !main_rs.contains("test_inline_addition"),
            "inline test function should not leak into generated production code:\n{}",
            main_rs,
        );
        Ok(())
    }

    #[test]
    fn e2e_imported_generic_decorator_factory_preserves_function_signatures() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "generic_decorator_factory"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            src_dir.join("registry.incn"),
            r#"
pub def registered[F](name: str) -> ((F) -> F):
    return (func) => func
"#,
        )?;
        std::fs::write(
            src_dir.join("columns.incn"),
            r#"
from registry import registered

pub model ColumnExpr:
    pub name: str

@registered[(str) -> ColumnExpr]("incql.functions.col")
pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)

@registered("incql.functions.literal")
pub def literal() -> ColumnExpr:
    return ColumnExpr(name="literal")
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_generic_decorator_factory.incn"),
            r#"
from std.testing import assert_eq
from columns import col, literal

def test_explicit_generic_decorator_factory_signature() -> None:
    assert_eq(col("id").name, "id")

def test_inferred_generic_decorator_factory_signature() -> None:
    assert_eq(literal().name, "literal")
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected imported generic decorator factory project to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_decorated_sum_shadows_builtin_sum_issue677() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "decorated_sum_inline"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        let source_path = src_dir.join("functions.incn");
        std::fs::write(
            &source_path,
            r#"
pub model IntExpr:
    pub value: int

pub model TextExpr:
    pub value: str

pub type Expr = IntExpr | TextExpr

pub model Measure:
    pub kind: str

pub def registered[F](function_ref: str) -> ((F) -> F):
    return (func) => func

pub def expr(value: int) -> Expr:
    return IntExpr(value=value)

@registered("demo.sum")
pub def sum(value: Expr) -> Measure:
    return Measure(kind="local")

module tests:
    def test_inline_test_resolves_decorated_sum_before_builtin_sum() -> None:
        measure = sum(expr(1))
        assert measure.kind == "local"
"#,
        )?;

        let output = run_incan_test_path(&source_path);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected decorated inline sum test to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("functions.incn::test_inline_test_resolves_decorated_sum_before_builtin_sum"),
            "expected the #677 inline test to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_conventional_test_batches_split_import_declaration_collisions_issue676()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "import_collision_batch"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            src_dir.join("helpers.incn"),
            r#"
pub def col() -> int:
    return 1
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_imports_col.incn"),
            r#"
from helpers import col

def test_imported_col() -> None:
    assert col() == 1
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_declares_col.incn"),
            r#"
def col() -> int:
    return 2

def test_local_col() -> None:
    assert col() == 2
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected import/local declaration collision batch to split and pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_imported_col") && stdout.contains("test_local_col"),
            "expected both split test files to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_method_call_decorator_factories_use_checked_receiver_lowering() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "method_call_decorator_factories"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
class Registry:
    pub names: list[str]

    @staticmethod
    def new() -> Self:
        return Registry(names=[])

    @staticmethod
    def add_static[F](name: str) -> (F) -> F:
        FUNCTIONS.names.append(name)
        return (func) => func

    def add[F](mut self, name: str) -> (F) -> F:
        self.names.append(name)
        return (func) => func


static FUNCTIONS: Registry = Registry.new()


@Registry::add_static("static")
def static_col(name: str) -> str:
    return name


@FUNCTIONS.add("instance")
def instance_col(name: str) -> str:
    return name


def main() -> None:
    println(static_col("amount"))
    println(instance_col("price"))
    println(len(FUNCTIONS.names))
"#,
        )?;

        let out_dir = dir.join("out");
        let output = run_incan_build(&src_dir.join("main.incn"), &out_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected method-call decorator factories to build.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let generated = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        // Assert the syntax, not the method's spelling. RFC 120 projects `add_static`, so pinning the source name
        // tested the projection rather than the associated-function lowering this case exists for. Requiring the
        // decorator's own argument to arrive at a `Registry::`-qualified callee keeps it specific to this decorator.
        let normalized: String = generated
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let lowered_as_associated_function = normalized.split("Registry::").skip(1).any(|tail| {
            tail.split_once('(').is_some_and(|(callee, arguments)| {
                callee.starts_with(incan_semantics_core::INCAN_SYMBOL_RUST_PREFIX)
                    && arguments.starts_with("\"static\".to_string()")
            })
        });
        assert!(
            lowered_as_associated_function,
            "class static method decorator should lower as associated function syntax:\n{}",
            generated,
        );
        // Same again for the instance half: `add` is projected, and the storage access is what this case pins.
        // Requiring the materialized argument to arrive at a method on the borrowed static keeps that specific.
        let reaches_receiver_through_static_storage = normalized.split("__incan_static_value.").skip(1).any(|tail| {
            tail.split_once('(').is_some_and(|(method, arguments)| {
                method.starts_with(incan_semantics_core::INCAN_SYMBOL_RUST_PREFIX)
                    && arguments.starts_with("__incan_static_arg_0")
            })
        });
        assert!(
            normalized.contains(".with_mut(|__incan_static_value|")
                && (normalized.contains("let__incan_static_arg_0=\"instance\".to_string();")
                    || normalized.contains("let__incan_static_arg_0=\"instance\".into();"))
                && reaches_receiver_through_static_storage,
            "static registry receiver should lower through static storage access:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn build_lib_imported_static_decorator_receiver_materializes_string_arg_issue671()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "imported_static_decorator_receiver"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("probe_registry.incn"),
            r#"
@derive(Clone)
pub class ProbeRegistry:
    @staticmethod
    def new() -> Self:
        return ProbeRegistry()

    def add[F](mut self, name: str, value: int) -> (F) -> F:
        return (func) => func


pub static PROBE_REGISTRY: ProbeRegistry = ProbeRegistry.new()
"#,
        )?;
        std::fs::write(
            src_dir.join("probe_decorated.incn"),
            r#"
from probe_registry import PROBE_REGISTRY

@PROBE_REGISTRY.add("decorated", 1)
pub def decorated(value: int) -> int:
    return value
"#,
        )?;
        std::fs::write(src_dir.join("lib.incn"), "pub from probe_decorated import decorated\n")?;

        let output = incan_command()
            .args(["build", "--lib"])
            .current_dir(&*dir)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected imported static decorator receiver project to build for #671.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let generated = std::fs::read_to_string(dir.join("target/lib/src/probe_decorated.rs"))?;
        assert!(
            (generated.contains("let __incan_static_arg_0 = \"decorated\".into();")
                || generated.contains("let __incan_static_arg_0 = \"decorated\".to_string();"))
                && !generated.contains("__incan_static_arg_0.clone()"),
            "imported static decorator string argument should materialize as owned String:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn build_static_receiver_option_model_lookup_issue674() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "main.incn",
            r#"
@derive(Clone)
model Entry:
    value: int


@derive(Clone)
class Registry:
    entries: list[Entry]

    @staticmethod
    def new() -> Self:
        return Registry(entries=[Entry(value=1)])

    def entry(self, name: str) -> Option[Entry]:
        if len(self.entries) == 0:
            return None
        return Some(self.entries[0])


static REGISTRY: Registry = Registry.new()


pub def lookup() -> int:
    match REGISTRY.entry("decorated"):
        Some(entry) => return entry.value
        None => return 0


def main() -> None:
    println(lookup())
"#,
        );

        let out_dir = dir.join("out");
        let output = run_incan_build(&dir.join("main.incn"), &out_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected static receiver Option model lookup to build for #674.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let generated = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated.contains("match {\n        let __incan_static_arg_0 = \"decorated\".to_string();")
                || generated.contains("match {\n        let __incan_static_arg_0 = \"decorated\".into();"),
            "static receiver match scrutinee should materialize args inside an expression block:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn e2e_directory_run_preserves_per_file_inline_test_modules_issue676() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_directory_batch"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("alpha.incn"),
            r#"
const ALPHA_OFFSET: int = 10
static alpha_runs: int = 0

model AlphaRecord:
    value: int
    label: str

def alpha_value() -> int:
    return 1

def alpha_record() -> AlphaRecord:
    return AlphaRecord(value=alpha_value() + ALPHA_OFFSET, label="alpha")


module tests:
    def test_alpha_value() -> None:
        alpha_runs += 1
        record = alpha_record()
        assert alpha_value() == 1
        assert record.value == 11
        assert record.label == "alpha"
        assert alpha_runs == 1
"#,
        )?;
        std::fs::write(
            src_dir.join("beta.incn"),
            r#"
const BETA_OFFSET: int = 20
static beta_runs: int = 0

model BetaRecord:
    value: int
    label: str

def beta_value() -> int:
    return 2

def beta_record() -> BetaRecord:
    return BetaRecord(value=beta_value() + BETA_OFFSET, label="beta")


module tests:
    def test_beta_value() -> None:
        beta_runs += 1
        record = beta_record()
        assert beta_value() == 2
        assert record.value == 22
        assert record.label == "beta"
        assert beta_runs == 1
"#,
        )?;
        let functions_dir = src_dir.join("functions");
        std::fs::create_dir_all(&functions_dir)?;
        std::fs::write(
            functions_dir.join("columns.incn"),
            r#"
const COLUMN_OFFSET: int = 30
static column_runs: int = 0

model Column:
    value: int
    label: str

pub def col() -> int:
    return 3

def column() -> Column:
    return Column(value=col() + COLUMN_OFFSET, label="column")


module tests:
    def test_col() -> None:
        column_runs += 1
        item = column()
        assert col() == 3
        assert item.value == 33
        assert item.label == "column"
        assert column_runs == 1
"#,
        )?;
        std::fs::write(
            functions_dir.join("uses_columns.incn"),
            r#"
from functions.columns import col

const USES_COLUMN_OFFSET: int = 40
static uses_column_runs: int = 0

model UsesColumn:
    value: int
    label: str

def uses_col() -> int:
    return col() + 1

def uses_column() -> UsesColumn:
    return UsesColumn(value=uses_col() + USES_COLUMN_OFFSET, label="uses-column")


module tests:
    def test_uses_col() -> None:
        uses_column_runs += 1
        item = uses_column()
        assert uses_col() == 4
        assert item.value == 44
        assert item.label == "uses-column"
        assert uses_column_runs == 1
"#,
        )?;

        let uses_columns = run_incan_test_path(&functions_dir.join("uses_columns.incn"));
        assert!(
            uses_columns.status.success(),
            "expected direct imported inline test run to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&uses_columns.stdout),
            String::from_utf8_lossy(&uses_columns.stderr),
        );

        let directory = run_incan_test_path(&src_dir);
        let stdout = String::from_utf8_lossy(&directory.stdout);
        let stderr = String::from_utf8_lossy(&directory.stderr);
        assert!(
            directory.status.success(),
            "expected directory inline test run to keep per-file parser context.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("alpha.incn::test_alpha_value")
                && stdout.contains("beta.incn::test_beta_value")
                && stdout.contains("columns.incn::test_col")
                && stdout.contains("uses_columns.incn::test_uses_col"),
            "expected every inline source file to run from directory discovery.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains("Only one `module tests:` block") && !stderr.contains("Only one `module tests:` block"),
            "directory batching should not report duplicate inline modules across files.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            !stderr.contains("the name `col` is defined multiple times"),
            "directory batching should keep imported names inside their source module scope.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        Ok(())
    }

    #[test]
    fn e2e_inline_module_parametrize_markers_strict_and_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_parametrize_markers"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("math.incn"),
            r#"
module tests:
    from rust::std::thread import sleep
    from rust::std::time import Duration
    from std.testing import assert_eq, mark, param_case, parametrize, timeout, xfail

    const TEST_MARKERS: List[str] = ["smoke"]
    const TEST_MARKS: List[str] = ["smoke"]

    @parametrize("x, expected", [
        param_case((1, 3), marks=[xfail("known")], id="one-three"),
        (2, 4),
    ], ids=["ignored", "two-four"])
    def test_double(x: int, expected: int) -> None:
        assert_eq(x * 2, expected)

    @mark("smoke")
    @timeout("1ms")
    def test_timeout_marker() -> None:
        sleep(Duration.from_millis(100))
"#,
        )?;

        // Discovery-level coverage proves inline marker registration and default
        // marks. One execution preserves the distinct generated-harness outcome
        // contract without rebuilding this project for a list-only query.
        let run = run_incan_test_with_args(&dir, &["--verbose"]);
        let run_stdout = String::from_utf8_lossy(&run.stdout);
        let run_stderr = String::from_utf8_lossy(&run.stderr);
        let run_combined = format!("{run_stdout}\n{run_stderr}");
        assert!(
            !run.status.success(),
            "expected the inline timeout marker to make the ordinary run fail.\n{}",
            run_combined,
        );
        assert!(
            run_combined.contains("XFAIL") || run_combined.contains("xfailed"),
            "expected the inline parametrized xfail/pass cases to be reported.\n{}",
            run_combined,
        );
        assert!(
            run_combined.contains("test_timeout_marker") && run_combined.contains("timed out after"),
            "expected the inline timeout marker to be reported.\n{}",
            run_combined,
        );
        Ok(())
    }

    #[test]
    fn e2e_fixture_lifetime_success_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "fixture_lifetime_success_batch"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("conftest.incn"),
            r#"
from rust::std::path import Path
from std.testing import fixture

@fixture(scope="session")
def session_value() -> int:
    marker = Path.new("session-marker.txt")
    if marker.exists():
        return 2
    write_file("session-marker.txt", "created")
    return 1
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_a.incn"),
            r#"
from std.testing import assert_eq

def test_a(session_value: int) -> None:
    assert_eq(session_value, 1)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_b.incn"),
            r#"
from std.testing import assert_eq

def test_b(session_value: int) -> None:
    assert_eq(session_value, 1)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_fixture_lifetimes.incn"),
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture, parametrize

static module_scope_calls: int = 0
static yield_observed: int = 0
static module_yield_calls: int = 0
static teardown_order: int = 0
static async_order: int = 0
static async_reverse_order: str = ""
static async_param_setups: int = 0

@fixture(scope="module")
def once() -> int:
    module_scope_calls += 1
    return module_scope_calls

def test_module_scope_first(once: int) -> None:
    assert_eq(once, 1)

def test_module_scope_second(once: int) -> None:
    assert_eq(once, 1)

@fixture
def captured_resource() -> int:
    value: int = 41
    yield value + 1
    yield_observed += value

def test_yield_capture_body(captured_resource: int) -> None:
    assert_eq(captured_resource, 42)

def test_yield_capture_after_teardown() -> None:
    assert_eq(yield_observed, 41)

@fixture(scope="module")
def module_shared() -> int:
    yield 10
    assert_eq(module_yield_calls, 2)

def test_module_yield_first(module_shared: int) -> None:
    module_yield_calls += 1
    assert_eq(module_shared, 10)

def test_module_yield_second(module_shared: int) -> None:
    module_yield_calls += 1
    assert_eq(module_shared, 10)

@fixture
def outer() -> int:
    yield 1
    assert_eq(teardown_order, 1)
    teardown_order += 1

@fixture
def inner(outer: int) -> int:
    yield outer + 1
    assert_eq(teardown_order, 0)
    teardown_order += 1

def test_reverse_teardown_body(inner: int) -> None:
    assert_eq(inner, 2)

def test_reverse_teardown_after() -> None:
    assert_eq(teardown_order, 2)

@fixture
def seed() -> int:
    async_order += 1
    return 40

@fixture
async def resource(seed: int) -> int:
    await sleep_ms(1)
    async_order += 1
    yield seed + 2
    await sleep_ms(1)
    async_order += 10

def test_1_uses_async_fixture(resource: int) -> None:
    assert_eq(resource, 42)
    assert_eq(async_order, 2)

def test_2_observes_async_teardown() -> None:
    assert_eq(async_order, 12)

@fixture
async def parent() -> int:
    async_reverse_order += "setup-parent;"
    await sleep_ms(1)
    yield 1
    await sleep_ms(1)
    async_reverse_order += "teardown-parent;"

@fixture
async def child(parent: int) -> int:
    async_reverse_order += "setup-child;"
    await sleep_ms(1)
    yield parent + 1
    await sleep_ms(1)
    async_reverse_order += "teardown-child;"

def test_1_uses_child(child: int) -> None:
    assert_eq(child, 2)
    assert_eq(async_reverse_order, "setup-parent;setup-child;")

def test_2_observes_reverse_teardown() -> None:
    assert_eq(async_reverse_order, "setup-parent;setup-child;teardown-child;teardown-parent;")

@fixture
async def base() -> int:
    async_param_setups += 1
    await sleep_ms(1)
    yield 10

@parametrize("value", [1, 2])
async def test_param_async_fixture(value: int, base: int) -> None:
    await sleep_ms(1)
    assert_eq(base, 10)
    assert_eq(value > 0, true)

def test_after_param_cases() -> None:
    assert_eq(async_param_setups, 2)
"#,
        )?;

        let output = run_incan_test_with_args(&dir, &["--jobs", "1"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected fixture lifetime success batch to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_module_scope_first") && stdout.contains("test_module_scope_second"));
        assert!(stdout.contains("test_param_async_fixture[1]") && stdout.contains("test_param_async_fixture[2]"));
        Ok(())
    }

    #[test]
    fn e2e_fixture_teardown_failure_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_yield_fixture_teardown.incn",
            r#"
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture
def resource() -> int:
    calls += 1
    yield calls
    calls += 10

def test_1_fails(resource: int) -> None:
    assert_eq(resource, 99)

def test_2_observes_teardown() -> None:
    assert_eq(calls, 11)
"#,
        );
        std::fs::write(
            dir.join("test_yield_fixture_teardown_failure.incn"),
            r#"
from std.testing import assert_eq, fixture

@fixture
def resource() -> int:
    yield 42
    assert_eq(1, 2)

def test_body_passes(resource: int) -> None:
    assert_eq(resource, 42)
"#,
        )?;
        std::fs::write(
            dir.join("test_yield_fixture_teardown_aggregate.incn"),
            r#"
from std.testing import assert_eq, fixture

@fixture
def parent() -> int:
    yield 1
    assert_eq(1, 2, "parent teardown failed")

@fixture
def child(parent: int) -> int:
    yield parent + 1
    assert_eq(3, 4, "child teardown failed")

def test_body_passes(child: int) -> None:
    assert_eq(child, 2)
"#,
        )?;
        std::fs::write(
            dir.join("test_async_yield_fixture_failure.incn"),
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture
async def resource() -> int:
    calls += 1
    await sleep_ms(1)
    yield calls
    await sleep_ms(1)
    calls += 10

def test_1_fails(resource: int) -> None:
    assert_eq(resource, 99)

def test_2_observes_async_teardown() -> None:
    assert_eq(calls, 11)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        assert!(
            !output.status.success(),
            "expected fixture teardown failure batch to fail.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            combined.contains("test_2_observes_teardown PASSED")
                && combined.contains("test_2_observes_async_teardown PASSED")
                && combined.contains("test_body_passes")
                && combined.contains("fixture teardown failed")
                && combined.contains("child teardown failed")
                && combined.contains("parent teardown failed"),
            "expected teardown diagnostics and observer tests in failure batch.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_module_missing_fixture_is_collection_error() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_missing_fixture"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
module tests:
    def test_missing_fixture(missing: int) -> None:
        pass
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected missing inline fixture to fail collection.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stderr.contains("missing fixture `missing`"),
            "expected collection-time missing fixture diagnostic.\nstderr:\n{}",
            stderr,
        );
        assert!(
            !stdout.contains("could not compile") && !stderr.contains("could not compile"),
            "missing fixtures should not fall through to generated Rust compile errors.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_conftest_does_not_apply_to_inline_src_tests() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "loaf.toml",
            r#"[project]
name = "inline_conftest_boundary"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def shared() -> int:
    return 42
"#,
        )?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
module tests:
    from std.testing import assert_eq

    def test_src_inline(shared: int) -> None:
        assert_eq(shared, 42)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected tests/conftest fixture not to apply to src inline tests.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stderr.contains("missing fixture `shared`"));
        Ok(())
    }

    #[test]
    fn e2e_failure_skip_and_assert_reporting_share_one_project() {
        let dir = write_test_project(
            "test_failure_skip_and_assert_reporting.incn",
            r#"
from std.testing import assert_eq, skip

def test_message() -> None:
    assert False, "custom boom"

def test_eq_message() -> None:
    assert 1 == 2, "math broke"

def test_wrong() -> None:
    assert_eq(1 + 1, 99)

@skip("not implemented yet")
def test_todo() -> None:
    pass
"#,
        );

        // The three failing forms share one project and the normal runner
        // reports every failure before returning its aggregate non-zero exit.
        // Keep one complete failure report rather than rebuilding that project
        // separately for each asserted diagnostic.
        let failures = run_incan_test_with_args(&dir, &["--verbose"]);
        let failures_stdout = String::from_utf8_lossy(&failures.stdout);
        let failures_stderr = String::from_utf8_lossy(&failures.stderr);
        let failures_combined = format!("{failures_stdout}\n{failures_stderr}");
        assert!(
            !failures.status.success(),
            "expected assertion failures to make the complete report fail.\n{}",
            failures_combined,
        );
        for expected in [
            "AssertionError: custom boom",
            "AssertionError: math broke",
            "left != right",
            "test_wrong",
        ] {
            assert!(
                failures_combined.contains(expected),
                "expected complete failure report to contain `{expected}`.\n{}",
                failures_combined,
            );
        }
        assert!(
            failures_combined.contains("FAILED") || failures_combined.contains("failed"),
            "expected generic failed status in output.\n{}",
            failures_combined,
        );

        let skip = run_incan_test_with_args(&dir, &["-k", "test_todo"]);
        let skip_stdout = String::from_utf8_lossy(&skip.stdout);

        assert!(
            skip.status.success(),
            "expected skipped test to succeed overall.\nstdout:\n{}",
            skip_stdout,
        );
        assert!(
            skip_stdout.contains("SKIPPED") || skip_stdout.contains("skipped"),
            "expected SKIPPED in output.\nstdout:\n{}",
            skip_stdout,
        );
    }
}

/// Test specific parser behavior
mod parser_tests {
    use incan_frontend::ast::*;
    use incan_frontend::{lexer, parser};

    fn parse_str(source: &str) -> Result<Program, ()> {
        let tokens = lexer::lex(source).map_err(|_| ())?;
        parser::parse(&tokens).map_err(|_| ())
    }

    #[test]
    fn test_model_with_decorator() {
        let source = r#"
@derive(Debug, Eq)
model User:
  name: str
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Model(m) => {
                assert_eq!(m.decorators.len(), 1);
                assert_eq!(m.decorators[0].node.name, "derive");
            }
            _ => panic!("Expected model"),
        }
    }

    #[test]
    fn test_class_with_traits() {
        let source = r#"
class Service with Loggable, Serializable:
  name: str
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.traits.len(), 2);
                assert_eq!(c.traits[0].node.name, "Loggable");
                assert_eq!(c.traits[1].node.name, "Serializable");
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_trait_supertraits_compile_source() {
        let source = r#"
trait Collection[T]:
  def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
  def sorted(self) -> Self: ...

model BoxedValue[T] with OrderedCollection:
  value: T

  def first(self) -> T:
    return self.value

  def sorted(self) -> Self:
    return self

def take_first(values: Collection[int]) -> int:
  return values.first()

def take_sorted(values: OrderedCollection[int]) -> OrderedCollection[int]:
  return values.sorted()
"#;

        let result = super::compile_source(source);
        assert!(
            result.is_ok(),
            "expected trait hierarchy program to typecheck, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_trait_constructor_rejected_in_full_pipeline() {
        let source = r#"
trait Runnable:
  def run(self) -> None: ...

def main() -> None:
  let _r = Runnable()
"#;

        let result = super::compile_source(source);
        let Err(errs) = result else {
            panic!("expected trait construction to fail");
        };
        assert!(
            errs.iter()
                .any(|message| message.contains("Cannot construct trait 'Runnable'")),
            "unexpected errors: {:?}",
            errs
        );
    }

    #[test]
    fn test_method_with_mut_self() {
        let source = r#"
class Counter:
  value: int = 0
  
  def inc(mut self) -> Unit:
    pass
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Class(c) => {
                assert_eq!(c.methods[0].node.receiver, Some(Receiver::Mutable));
            }
            _ => panic!("Expected class"),
        }
    }

    #[test]
    fn test_generic_instance_method_full_pipeline() {
        let source = r#"
class Box:
  def get[T with Clone](self, value: T) -> T:
    return value

model Shelf[U]:
  item: U

  def swap[T with Clone](self, value: T) -> T:
    return value

trait Echo:
  def echo[T with Clone](self, value: T) -> T:
    return value

class EchoBox with Echo:
  marker: int

type Wrapper[U] = newtype U:
  def echo[T with Clone](self, value: T) -> T:
    return value

def main() -> None:
  let b = Box()
  let _x = b.get(1)
  let shelf = Shelf(item=1)
  let _y = shelf.swap("ok")
  let echo = EchoBox(marker=1)
  let _z = echo.echo(True)
  let wrapper = Wrapper(1)
  let _w = wrapper.echo(1.5)
"#;
        let result = super::compile_source(source);
        assert!(
            result.is_ok(),
            "expected generic instance methods across owner kinds to typecheck and lower, got {:?}",
            result.err()
        );
    }

    #[test]
    fn test_issue388_generic_type_owned_factories_run() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
@derive(Clone)
class FactoryBox[T with Clone]:
  pub value: T

  @classmethod
  def make(cls, value: T) -> Self:
    return cls(value=value)

  @staticmethod
  def make_static(value: T) -> Self:
    return FactoryBox(value=value)

def main() -> None:
  from_classmethod = FactoryBox[int].make(1)
  from_staticmethod = FactoryBox[int].make_static(2)
  println(str(from_classmethod.value))
  println(str(from_staticmethod.value))
"#;
        let output = super::incan_command()
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected generic type-owned factories to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["1", "2"],
            "unexpected generic type-owned factory output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_match_with_case() {
        let source = r#"
def foo(x: Option[int]) -> int:
  match x:
    case Some(n):
      return n
    case None:
      return 0
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.body.len(), 1);
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_list_comprehension() {
        let source = r#"
def squares(nums: List[int]) -> List[int]:
  return [x * x for x in nums if x > 0]
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        assert_eq!(program.declarations.len(), 1);
    }

    #[test]
    fn test_generic_type() {
        let source = r#"
def foo() -> Result[int, str]:
  return Ok(42)
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => match &f.return_type.node {
                Type::Generic(name, args) => {
                    assert_eq!(name, "Result");
                    assert_eq!(args.len(), 2);
                }
                _ => panic!("Expected generic type"),
            },
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_yield_expression() {
        let source = r#"
def fixture() -> str:
  value = "test"
  yield value
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Function(f) => {
                assert_eq!(f.body.len(), 2);
                // Second statement should be the yield
                match &f.body[1].node {
                    Statement::Expr(expr) => {
                        match &expr.node {
                            Expr::Yield(Some(_)) => {} // Success
                            _ => panic!("Expected yield expression with value"),
                        }
                    }
                    _ => panic!("Expected expression statement"),
                }
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_fixture_decorator() {
        let source = r#"
from std.testing import fixture

@fixture(scope="module")
def database() -> Database:
  db = connect()
  yield db
"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        // declarations[0] is the import, declarations[1] is the function
        match &program.declarations[1].node {
            Declaration::Function(f) => {
                assert_eq!(f.decorators.len(), 1);
                assert_eq!(f.decorators[0].node.name, "fixture");
                assert!(!f.decorators[0].node.args.is_empty());
            }
            _ => panic!("Expected function"),
        }
    }

    #[test]
    fn test_rust_crate_import() {
        let source = r#"import rust::serde_json as json"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Import(i) => {
                match &i.kind {
                    ImportKind::RustCrate {
                        crate_name,
                        path,
                        version,
                        features,
                    } => {
                        assert_eq!(crate_name, "serde_json");
                        assert!(path.is_empty());
                        assert!(version.is_none());
                        assert!(features.is_empty());
                    }
                    _ => panic!("Expected RustCrate import kind"),
                }
                assert_eq!(i.alias.as_deref(), Some("json"));
            }
            _ => panic!("Expected import"),
        }
    }

    #[test]
    fn test_rust_from_import() {
        let source = r#"from rust::time import Instant, Duration"#;
        let Ok(program) = parse_str(source) else {
            panic!("parse failed");
        };
        match &program.declarations[0].node {
            Declaration::Import(i) => match &i.kind {
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    version,
                    features,
                    items,
                } => {
                    assert_eq!(crate_name, "time");
                    assert!(path.is_empty());
                    assert!(version.is_none());
                    assert!(features.is_empty());
                    assert_eq!(items.len(), 2);
                    assert_eq!(items[0].name, "Instant");
                    assert_eq!(items[1].name, "Duration");
                }
                _ => panic!("Expected RustFrom import kind"),
            },
            _ => panic!("Expected import"),
        }
    }
}

/// Exercise compiled SDK codecs through both a user re-export and an isolated root module import.
#[test]
fn std_toml_manifest_and_lock_roundtrip_through_compiled_sdk() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected_stdout, uses_facade) in [
        (
            fs::read_to_string(incan_test_support::fixture("valid/std_toml_typed_lookup.incn"))?
                .replace("from std.toml import", "from codec import"),
            "TOML typed lookup, strict kinds, paths, and locations passed",
            true,
        ),
        (
            fs::read_to_string(incan_test_support::fixture("valid/std_toml_surface.incn"))?
                .replace("from std.toml import", "from codec import"),
            "TOML manifest, lock, datetime, and located errors passed",
            true,
        ),
        (
            fs::read_to_string(incan_test_support::fixture("valid/std_toml_module_import.incn"))?.to_owned(),
            "TOML root module-only roundtrip passed",
            false,
        ),
    ] {
        let temporary = tempfile::tempdir()?;
        let source_dir = temporary.path().join("src");
        fs::create_dir_all(&source_dir)?;
        fs::write(
            temporary.path().join("loaf.toml"),
            "[project]\nname = \"std_toml_native_surface\"\nversion = \"0.1.0\"\n",
        )?;
        if uses_facade {
            fs::write(
                source_dir.join("codec.incn"),
                "pub from std.toml import TomlValue, TomlError, TomlKind, TomlErrorKind, TomlDatetime, parse, deserialize, serialize, serialize_pretty, locate\n",
            )?;
        }
        let main = source_dir.join("main.incn");
        fs::write(&main, source)?;

        let mut bake = incan_command();
        bake.current_dir(temporary.path())
            .args(["oven", "bake", "--project"])
            .arg(temporary.path());
        support::configure_explicit_oven_bake_command(&mut bake)?;
        let baked = bake.output()?;
        assert!(
            baked.status.success(),
            "TOML native bake failed:\n{}\n{}",
            String::from_utf8_lossy(&baked.stdout),
            String::from_utf8_lossy(&baked.stderr)
        );
        let output = incan_command()
            .current_dir(temporary.path())
            .args(["run", "--locked"])
            .arg(main)
            .output()?;
        assert!(
            output.status.success(),
            "TOML native assertions failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout)).trim(),
            expected_stdout
        );
        let generated = temporary.path().join("target/incan/std_toml_native_surface");
        assert!(
            !generated.join("src/__incan_std/toml.rs").exists(),
            "the consumer must select compiled std.toml rather than materialize its implementation"
        );
        let provider = compiled_sdk_provider_artifact_root(&generated, "incan_stdlib_data")?;
        assert!(
            provider.join("src/toml.rs").is_file(),
            "compiled stdlib-data must own the TOML implementation"
        );
    }
    Ok(())
}

/// Native Rust checking must enforce imported bounds after Incan accepts scalar and collection instantiations.
#[test]
fn imported_rust_generic_bounds_remain_native_obligations() -> Result<(), Box<dyn std::error::Error>> {
    let temporary = tempfile::tempdir()?;
    let source_dir = temporary.path().join("src");
    fs::create_dir_all(&source_dir)?;
    fs::write(
        temporary.path().join("loaf.toml"),
        "[project]\nname = \"foreign_bounds\"\nversion = \"0.1.0\"\n[rust-dependencies]\nserde = \"1.0\"\n",
    )?;
    fs::write(
        source_dir.join("bounds.incn"),
        fs::read_to_string(incan_test_support::fixture("valid/rust_generic_bounds.incn"))?,
    )?;
    let main = source_dir.join("main.incn");
    fs::write(
        &main,
        "from bounds import identity, Reader\ndef main() -> None:\n    assert identity(\"demo\") == \"demo\"\n    assert Reader().identity([\"linux\"]) == [\"linux\"]\n",
    )?;
    let mut bake = incan_command();
    bake.current_dir(temporary.path())
        .args(["oven", "bake", "--project"])
        .arg(temporary.path());
    support::configure_explicit_oven_bake_command(&mut bake)?;
    let baked = bake.output()?;
    assert!(
        baked.status.success(),
        "native bound bake failed:\n{}\n{}",
        String::from_utf8_lossy(&baked.stdout),
        String::from_utf8_lossy(&baked.stderr)
    );
    let ran = incan_command()
        .current_dir(temporary.path())
        .args(["run", "--locked"])
        .arg(&main)
        .output()?;
    assert!(
        ran.status.success(),
        "native bound assertions failed:\n{}\n{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );

    fs::write(
        &main,
        "from bounds import identity\nmodel Plain:\n    value: str\ndef main() -> None:\n    identity(Plain(value=\"demo\"))\n",
    )?;
    let mut bake = incan_command();
    bake.current_dir(temporary.path())
        .args(["oven", "bake", "--project"])
        .arg(temporary.path());
    support::configure_explicit_oven_bake_command(&mut bake)?;
    let rejected = bake.output()?;
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(
        !rejected.status.success(),
        "a model without Deserialize must not satisfy the foreign bound"
    );
    assert!(
        diagnostic.contains("E0277") && diagnostic.contains("Deserialize") && diagnostic.contains("Plain"),
        "expected native trait-obligation failure, got:\n{diagnostic}"
    );
    assert!(
        !diagnostic.contains("violates generic bound"),
        "foreign obligations must reach native checking: {diagnostic}"
    );
    Ok(())
}
