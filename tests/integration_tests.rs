//! Integration tests for the Incan compiler frontend

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use incan::frontend::module::{ExportedTypeLikeDoc, ExportedTypeLikeKind, exported_type_like_docs};
use incan::frontend::{lexer, parser, typechecker};

/// Shared with `src/frontend/module.rs` tests (`exported_type_like_docs`) for GitHub #247.
const BLOCK_DOCSTRING_PUBLIC_TYPE_LIKE: &str = include_str!("fixtures/block_docstring_public_type_like.incn");

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

fn strip_ansi_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
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

static TEST_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a throwaway project name that does not collide under parallel nextest workers.
///
/// Several CLI tests rely on the default `target/incan/<name>` output location. The generated project name includes
/// both the current process id and a local counter so those tests do not trample each other's generated Cargo projects.
fn unique_test_project_name(prefix: &str) -> String {
    let unique = TEST_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{}_{}", std::process::id(), unique)
}

/// Create a minimal throwaway Incan project for end-to-end runtime error assertions.
fn write_runtime_error_project(source: &str) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = unique_test_project_name("runtime_error_contract");
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(&main_path, source)?;
    Ok((tmp, main_path))
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

    Ok(())
}

#[test]
fn bare_incan_run_uses_project_main_script() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
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
            project_root.join("incan.toml"),
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
        tmp.path().join("incan.toml"),
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

    let run = incan_command().arg("run").arg(&source_path).output()?;
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
        tmp.path().join("incan.toml"),
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

/// Locate the `incan` binary for subprocess tests.
///
/// Uses `CARGO_BIN_EXE_incan` when present (integration tests under `cargo test`) so we always run the artifact from
/// the current build, including when `CARGO_TARGET_DIR` is not the default `target/`.
fn incan_debug_binary() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_incan") {
        let path = std::path::PathBuf::from(path);
        if path.exists() {
            return path;
        }
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let p = std::path::PathBuf::from(&target_dir).join("debug/incan");
        if p.exists() {
            return p;
        }
    }
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/incan")
}

fn shared_generated_cargo_target_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("incan_generated_shared_target")
}

fn incan_command() -> Command {
    let mut command = Command::new(incan_debug_binary());
    command
        .env("INCAN_GENERATED_CARGO_TARGET_DIR", shared_generated_cargo_target_dir())
        .env("CARGO_NET_OFFLINE", "true")
        .env(
            "INCAN_INTERNAL_SDK_PROVIDER_STORE",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_test_sdk_provider_store"),
        );
    command
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
        dir.join("incan.toml"),
        r#"[project]
name = "cycle_explicit_call_site_generics"
version = "0.1.0"
"#,
    )?;
    std::fs::write(
        src_dir.join("dataset.incn"),
        r#"from session import collect_with_active_session

pub model DataSet[T]:
  value: T

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
/// `format_files` delegates to [`incan::format::format_source`]; this still covers subprocess + I/O if those paths
/// diverge from in-process formatting.
#[test]
fn test_cli_fmt_preserves_block_decl_docstrings_and_export_doc_surface() -> Result<(), Box<dyn std::error::Error>> {
    let dir = make_temp_test_dir();
    let path = dir.join("block_docstrings_cli.incn");
    fs::write(&path, BLOCK_DOCSTRING_PUBLIC_TYPE_LIKE)?;
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
    let fixtures_dir = Path::new("tests/fixtures/valid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(fixtures_dir) else {
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
    let fixtures_dir = Path::new("tests/fixtures/invalid");
    if !fixtures_dir.exists() {
        return; // Skip if fixtures not present
    }

    let mut matched = 0usize;
    let Ok(entries) = fs::read_dir(fixtures_dir) else {
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

    let manifest_path = project_dir.join("incan.toml");
    let initial_manifest = fs::read_to_string(&manifest_path)?;
    assert!(initial_manifest.contains(r#"name = "greeter""#));
    assert!(initial_manifest.contains(r#"description = "A generated greeting app""#));
    assert!(initial_manifest.contains(r#"authors = ["Danny <danny@example.com>"]"#));
    assert!(initial_manifest.contains(r#"license = "MIT""#));
    assert!(initial_manifest.contains(r#"requires-incan = ">=0.5.0-0,<0.6.0""#));
    assert!(project_dir.join("src/main.incn").exists());
    assert!(project_dir.join("tests/test_main.incn").exists());

    let empty_list_output = incan_command()
        .args(["env", "list"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        empty_list_output.status.success(),
        "env list on fresh project failed: {}",
        String::from_utf8_lossy(&empty_list_output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&empty_list_output.stdout).trim(),
        "default",
        "fresh projects should expose the ambient default env"
    );

    let default_overview_output = incan_command()
        .args(["env", "show"])
        .current_dir(&project_dir)
        .output()?;
    assert!(
        default_overview_output.status.success(),
        "env show overview on fresh project failed: {}",
        String::from_utf8_lossy(&default_overview_output.stderr)
    );
    let default_overview_stdout = String::from_utf8_lossy(&default_overview_output.stdout);
    assert!(default_overview_stdout.contains("Name"));
    assert!(default_overview_stdout.contains("default"));

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
        "dry-run must not modify incan.toml"
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
        project_root.join("incan.toml"),
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
        project_root.join("incan.toml"),
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
        project_root.join("child/incan.toml"),
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
fn runtime_error_canonicalization_cases() -> Result<(), Box<dyn std::error::Error>> {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "def main() -> None:\n  let values = {\"a\": 1}\n  println(values[\"b\"])\n",
            "KeyError",
            &["not found in dict"],
        ),
        (
            "def main() -> None:\n  let values = [1, 2, 3]\n  println(values[99])\n",
            "IndexError",
            &["out of range for list"],
        ),
        (
            "def main() -> None:\n  let values = [1, 2, 3]\n  println(values.index(99))\n",
            "ValueError",
            &["value not found in list"],
        ),
        (
            "def main() -> None:\n  println(int(\"abc\"))\n",
            "ValueError",
            &["cannot convert 'abc' to int"],
        ),
        (
            "def main() -> None:\n  println(float(\"abc\"))\n",
            "ValueError",
            &["cannot convert 'abc' to float"],
        ),
        (
            "def main() -> None:\n  mut values = [1, 2, 3]\n  values.remove(99)\n",
            "IndexError",
            &["out of range for list"],
        ),
        (
            "def main() -> None:\n  mut values = [1, 2, 3]\n  values.swap(0, 99)\n",
            "IndexError",
            &["out of range for list"],
        ),
    ];
    for (source, expected_type, expected_substrings) in cases {
        assert_runtime_error_cli(source, expected_type, expected_substrings)?;
    }
    Ok(())
}

#[test]
fn assert_false_can_satisfy_typed_failure_path() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        r#"
def fail_int(message: str) -> int:
  assert false, message

def main() -> None:
  _ = fail_int("boom")
"#,
        r#"
def fail_as[T](message: str) -> T:
  assert false, message

def main() -> None:
  _ = fail_as[int]("boom")
"#,
    ];
    for source in cases {
        assert_runtime_error_cli(source, "AssertionError", &["boom"])?;
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
        dir.join("incan.toml"),
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
    use incan::frontend::lexer::{TokenKind, lex};
    use incan_core::lang::keywords::KeywordId;
    use incan_core::lang::operators::OperatorId;
    use incan_core::lang::punctuation::PunctuationId;

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
    use incan::frontend::{lexer, parser, typechecker};

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
    use super::{compiled_sdk_provider_artifact_root, incan_command, strip_ansi_escapes};
    use incan::backend::IrCodegen;
    use incan::frontend::{lexer, parser, typechecker};
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
        let path = Path::new("examples/hello.incn");
        if !path.exists() {
            return; // Skip if example not present
        }

        let Ok(source) = fs::read_to_string(path) else {
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
    fn test_method_alias_codegen_rewrites_to_target_method() {
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
        let Ok(tokens) = lexer::lex(source) else {
            panic!("lex failed");
        };
        let Ok(ast) = parser::parse(&tokens) else {
            panic!("parse failed");
        };
        let Ok(rust_code) = IrCodegen::new().try_generate(&ast) else {
            panic!("codegen failed");
        };
        assert!(
            rust_code.contains(".avg("),
            "expected method alias call to lower to target method, got:\n{rust_code}"
        );
        assert!(
            !rust_code.contains(".mean("),
            "method alias must not emit an independent wrapper call, got:\n{rust_code}"
        );
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
            root.join("incan.toml"),
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
        // this deadline comfortably above observed cold MSRV compilation time while retaining a finite failure bound.
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
from std.io import BytesIO, Endian, IoError

def run() -> Result[None, IoError]:
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
                "2",
                "2",
                "4",
                "4",
                "4",
                "0",
                "unexpected_eof",
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
            .args(["run", "tests/fixtures/valid/std_encoding_hex_surface.incn"])
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
"#,
        )?;
        let main_path = root.join("factory_call.incn");
        fs::write(
            &main_path,
            r#"
from pkg.factory import BytesIO

def main() -> None:
    println(BytesIO())
    println(BytesIO(3))
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
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "7\n3");
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
    fn test_match_rust_result_non_clone_payload_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from rust::std::fs import read_dir
from rust::std::path import Path as RustPath

def main() -> None:
    mut seen = False
    match read_dir(RustPath.new(".")):
        Ok(entries) =>
            for entry_result in entries:
                match entry_result:
                    Ok(entry) =>
                        seen = seen or entry.path().to_string_lossy().into_owned() != ""
                    Err(err) => println(err.to_string())
        Err(err) => println(err.to_string())
    println(seen)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "rust Result non-Clone match regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(lines, vec!["true"], "unexpected output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_result_inspect_rust_result_non_clone_payload_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
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

def main() -> None:
    result = read_dir(RustPath.new(".")).inspect(observe_entries)
    match result:
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
            "Result.inspect Rust Result non-Clone regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["true"],
            "unexpected Result.inspect non-Clone Rust Result output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_user_authored_result_tap_borrows_callback_payload() -> Result<(), Box<dyn std::error::Error>> {
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
    result = tap(read_dir(RustPath.new(".")), observe_entries)
    match result:
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
            "user-authored Result tap borrowed callback regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec!["true"],
            "unexpected user-authored Result tap output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_std_result_helpers_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.result import map as result_map, map_err as result_map_err
from std.result import and_then as result_and_then, or_else as result_or_else

def double(value: int) -> int:
    return value * 2

def prefix(error: str) -> str:
    return f"error: {error}"

def keep_even(value: int) -> Result[int, str]:
    if value % 2 == 0:
        return Ok(value)
    return Err("odd")

def recover(_error: str) -> Result[int, str]:
    return Ok(7)

def main() -> None:
    ok_value: Result[int, str] = Ok(2)
    err_value: Result[int, str] = Err("bad")
    even_value: Result[int, str] = Ok(4)
    missing_value: Result[int, str] = Err("missing")
    match result_map(ok_value, double):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_map_err(err_value, prefix):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_and_then(even_value, keep_even):
        Ok(value) => println(value)
        Err(error) => println(error)
    match result_or_else(missing_value, recover):
        Ok(value) => println(value)
        Err(error) => println(error)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "std.result helper run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
            vec!["4", "error: bad", "4", "7"],
            "unexpected std.result helper output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_result_methods_dogfood_std_result_helpers_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def double(value: int) -> int:
    return value * 2

def prefix(error: str) -> str:
    return f"error: {error}"

def keep_even(value: int) -> Result[int, str]:
    if value % 2 == 0:
        return Ok(value)
    return Err("odd")

def recover(_error: str) -> Result[int, str]:
    return Ok(7)

def main() -> None:
    ok_value: Result[int, str] = Ok(2)
    err_value: Result[int, str] = Err("bad")
    missing_value: Result[int, str] = Err("missing")
    match ok_value.map(double).and_then(keep_even):
        Ok(value) => println(value)
        Err(error) => println(error)
    match err_value.map_err(prefix):
        Ok(value) => println(value)
        Err(error) => println(error)
    match missing_value.or_else(recover).map(double):
        Ok(value) => println(value)
        Err(error) => println(error)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Result method std.result helper run-path regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
            vec!["4", "error: bad", "14"],
            "unexpected Result method std.result helper output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_result_map_err_accepts_callable_object_trait_adoption() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
from std.traits.callable import Callable1

model Prefixer with Callable1[str, str]:
    prefix: str

    def __call__(self, error: str) -> str:
        return f"{self.prefix}: {error}"

def main() -> None:
    value: Result[int, str] = Err("bad")
    match value.map_err(Prefixer(prefix="error")):
        Ok(value) => println(value)
        Err(error) => println(error)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Result.map_err callable-object regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
            vec!["error: bad"],
            "unexpected callable-object output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_result_method_closure_callbacks_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    prefix = "uuid"
    value: Result[int, str] = Err("bad")
    mapped = value.map_err((err) => f"{prefix}: {err}")
    match mapped:
        Ok(number) => println(number)
        Err(error) => println(error)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Result method closure callback regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
            vec!["uuid: bad"],
            "unexpected Result method closure callback output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_question_mark_list_comprehension_propagates_result_issue633() -> Result<(), Box<dyn std::error::Error>> {
        let output = run_incan_source(
            r#"
def parse_value(value: int) -> Result[int, str]:
    if value == 2:
        return Err("bad value")
    return Ok(value)


def parse_all(values: list[int]) -> Result[list[int], str]:
    return Ok([parse_value(value)? for value in values])


def main() -> None:
    match parse_all([1, 2, 3]):
        Ok(values) => println(values[0])
        Err(err) => println(err)
"#,
        );
        assert!(
            output.status.success(),
            "question-mark list comprehension regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
        assert_eq!(lines, vec!["bad value"], "unexpected issue633 output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_question_mark_dict_comprehension_propagates_result_issue633() -> Result<(), Box<dyn std::error::Error>> {
        let output = run_incan_source(
            r#"
def parse_key(value: int) -> Result[str, str]:
    if value == 2:
        return Err("bad key")
    return Ok(str(value))


def parse_map(values: list[int]) -> Result[dict[str, int], str]:
    return Ok({parse_key(value)?: value for value in values})


def main() -> None:
    match parse_map([1, 2, 3]):
        Ok(values) => println(values["1"])
        Err(err) => println(err)
"#,
        );
        assert!(
            output.status.success(),
            "question-mark dict comprehension regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
        assert_eq!(lines, vec!["bad key"], "unexpected issue633 dict output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_result_map_err_accepts_capturing_inline_closure() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    prefix = "error"
    value: Result[int, str] = Err("bad")
    match value.map_err((error) => f"{prefix}: {error}"):
        Ok(value) => println(value)
        Err(error) => println(error)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "Result.map_err inline closure regression failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
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
        assert_eq!(lines, vec!["error: bad"], "unexpected inline closure output:\n{stdout}");
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
            project_root.join("incan.toml"),
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
    fn test_filtered_comprehensions_run_with_borrowed_iterables() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
@derive(Clone)
model StoredNode:
    store_id_raw: int
    node: str

def main() -> None:
    nodes: list[StoredNode] = [
        StoredNode(store_id_raw=1, node="a"),
        StoredNode(store_id_raw=2, node="b"),
    ]
    filtered = [stored.node for stored in nodes if stored.store_id_raw == 1]
    scores = [1, 2, 3, 4]
    squared_evens = {x: x * x for x in scores if x % 2 == 0}
    println(filtered[0])
    println(squared_evens[2])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c filtered comprehension regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["a", "4"],
            "unexpected filtered comprehension output:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_generator_expression_runs_lazily_with_source_ordered_clauses() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def main() -> None:
    xs = [1, 2, 3]
    ys = [2, 3, 4]
    values = (x * y for x in xs if x > 1 for y in ys if y > x).collect()
    println(values[0])
    println(values[1])
    println(values[2])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c generator expression regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["6", "8", "12"], "unexpected generator output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generator_helper_chain_builds_and_runs() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def triple(x: int) -> int:
    return x * 3

def big(x: int) -> bool:
    return x > 6

def main() -> None:
    xs = [1, 2, 3, 4, 5]
    values = (x for x in xs).map(triple).filter(big).take(2).collect()
    println(values[0])
    println(values[1])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c generator helper regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["9", "12"], "unexpected generator helper output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generator_function_yield_builds_and_runs() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def numbers() -> Generator[int]:
    yield 1
    yield 2

def main() -> None:
    values = numbers().collect()
    println(values[0])
    println(values[1])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c generator function regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["1", "2"], "unexpected generator function output:\n{stdout}");
        Ok(())
    }

    #[test]
    fn test_generator_function_body_starts_on_first_consumption() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def numbers() -> Generator[int]:
    println("started")
    yield 1

def main() -> None:
    values = numbers()
    println("after construction")
    items = values.collect()
    println(items[0])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c generator laziness regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["after construction", "started", "1"],
            "generator body should not run until first consumption:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn test_generic_generator_function_yield_builds_and_runs() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def singleton[T](value: T) -> Generator[T]:
    yield value

def main() -> None:
    values = singleton[int](3).collect()
    println(values[0])
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c generic generator function regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(lines, vec!["3"], "unexpected generic generator output:\n{stdout}");
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
    fn test_result_ok_string_literals_run_without_manual_str_wrapping() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args([
                "run",
                "-c",
                r#"
def returns_result() -> Result[str, str]:
    return Ok("from_return")

def main() -> None:
    direct: Result[str, str] = Ok("from_call")
    match direct:
        case Ok(msg):
            println(msg)
        case Err(err):
            println(err)

    match returns_result():
        case Ok(msg):
            println(msg)
        case Err(err):
            println(err)
"#,
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run -c Result[str, E] string regression failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        let lines: Vec<&str> = stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        assert_eq!(
            lines,
            vec!["from_call", "from_return"],
            "unexpected Result[str, E] output:\n{stdout}"
        );
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
        assert!(
            normalized.contains("register_sources().await?;"),
            "expected awaited-then-try ordering in generated Rust, got:\n{}",
            main_rs
        );
        assert!(
            !normalized.contains("register_sources()?.await;"),
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
            project_dir.join("incan.toml"),
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
        assert!(
            normalized_main.contains("crate::r#extern::root_value"),
            "expected generated use path to escape top-level keyword module, got:\n{main_rs}"
        );
        assert!(
            normalized_main.contains("crate::api::r#extern::nested_value"),
            "expected generated use path to escape nested keyword module, got:\n{main_rs}"
        );
        assert!(
            normalized_api_mod.contains("#[path=\"extern.rs\"]pubmodr#extern;"),
            "expected nested keyword module path attr in api/mod.rs, got:\n{api_mod_rs}"
        );

        let run_output = incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
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
            .args(["run", "tests/fixtures/repro_model_traits.incn"])
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
            .args(["run", "tests/fixtures/field_info_reflection.incn"])
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

        // Inherited class fields should appear in __fields__()
        assert!(
            stdout.contains("child_field:base_id|type:int"),
            "expected inherited base field in __fields__; got:\n{}",
            stdout
        );
        assert!(
            stdout.contains("child_field:name|type:str"),
            "expected child field in __fields__; got:\n{}",
            stdout
        );
    }

    /// RFC 023: Runtime parity check for source-defined stdlib surfaces migrated off helper stubs.
    #[test]
    fn test_run_rfc023_stdlib_behavior_parity() {
        let Ok(output) = incan_command()
            .args(["run", "tests/fixtures/rfc023_stdlib_behavior_parity.incn"])
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
            .args(["run", "tests/fixtures/rfc030_std_collections_behavior.incn"])
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
            .args(["run", "tests/codegen_snapshots/rfc088_iterator_adapters.incn"])
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
    fn test_run_rfc088_iterator_sum_float_and_newtype_matrix() {
        let Ok(output) = incan_command()
            .args(["run", "tests/fixtures/rfc088_iterator_sum_runtime.incn"])
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
            .args(["run", "tests/fixtures/rfc064_std_encoding_behavior.incn"])
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
    fn test_run_std_uuid_surface() -> Result<(), Box<dyn std::error::Error>> {
        // Anchor `std.uuid`'s generated-project runtime dependency in the root test build so offline CI shards do not
        // depend on cache order.
        let mut rng = rand::thread_rng();
        let _ = rand::Rng::gen_range(&mut rng, 0..1);

        let output = incan_command()
            .args(["run", "tests/fixtures/valid/std_uuid_surface.incn"])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "incan run std_uuid_surface failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "std.uuid ok");
        Ok(())
    }

    #[test]
    fn test_run_std_ordinal_map_surface() -> Result<(), Box<dyn std::error::Error>> {
        let output = incan_command()
            .args(["run", "tests/fixtures/valid/std_ordinal_map_surface.incn"])
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

        let generated_main = fs::read_to_string("target/incan/std_ordinal_map_surface/src/main.rs")
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
            !std::path::Path::new("target/incan/std_ordinal_map_surface/src/__incan_std/collections.rs").exists(),
            "a compiled std.collections module must not be materialized in the consumer"
        );
        let artifact_root = compiled_sdk_provider_artifact_root(
            Path::new("target/incan/std_ordinal_map_surface"),
            "incan_stdlib_data",
        )?;
        let generated_collections = fs::read_to_string(artifact_root.join("src/collections.rs"))
            .map_err(|error| format!("failed to read compiled std.collections artifact: {error}"))?;
        assert!(
            generated_collections.contains("incan_stdlib::__incan_ordinal_map_string_fast_impls!();"),
            "the compiled std.collections artifact should splice in the stdlib-owned OrdinalMap string support:\n{generated_collections}"
        );
        Ok(())
    }

    fn assert_std_regex_surface_from_cold_sdk_provider(
        fresh_cargo_home: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = tmp.path().join("std_regex_surface.incn");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/valid/std_regex_surface.incn"),
            &source,
        )?;
        let generated_project = tmp.path().join("target/incan/std_regex_surface");
        let provider_store = tmp.path().join("sdk-provider-store");
        let generated_cargo_target = tmp.path().join("generated-cargo-target");
        let cargo_home = tmp.path().join("cargo-home");

        let mut command = incan_command();
        command
            .current_dir(tmp.path())
            .env("INCAN_SOURCE_ROOT", env!("CARGO_MANIFEST_DIR"))
            .env(
                "INCAN_STDLIB",
                Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
            )
            .env_remove("INCAN_STDLIB_DIR")
            .env_remove("INCAN_SDK_INVENTORY")
            .env_remove("INCAN_HOME")
            .env("INCAN_INTERNAL_SDK_PROVIDER_STORE", &provider_store)
            .env("INCAN_GENERATED_CARGO_TARGET_DIR", &generated_cargo_target)
            .arg("run")
            .arg(&source);
        if fresh_cargo_home {
            // The explicit acceptance lane starts without a Cargo registry or crate cache, so it permits Cargo to
            // populate the isolated home while resolving the generated std.regex consumer's direct dependency.
            command.env("CARGO_HOME", &cargo_home).env_remove("CARGO_NET_OFFLINE");
        }
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
        let provider_store = fs::canonicalize(&provider_store)?;
        assert!(
            artifact_root.starts_with(&provider_store),
            "std.regex should be compiled from the isolated cold SDK provider store: {}",
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
                .matches("incan_stdlib::strings::str_concat(&out,&str_slice_byte_range(&text,last_end,start),)")
                .count(),
            3,
            "every replacement loop should pass the owned Rust byte-range result directly to str_concat:\n{generated_core}"
        );
        assert_eq!(
            compact_generated_core
                .matches("incan_stdlib::strings::str_concat(&out,&str_slice_from_byte_offset(&text,last_end),)")
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
    fn test_run_std_regex_rfc059_surface_from_cold_sdk_provider() -> Result<(), Box<dyn std::error::Error>> {
        assert_std_regex_surface_from_cold_sdk_provider(false)
    }

    #[test]
    #[ignore = "requires an online crates.io registry to populate a genuinely fresh Cargo home"]
    fn test_run_std_regex_rfc059_surface_from_fresh_cargo_home() -> Result<(), Box<dyn std::error::Error>> {
        assert_std_regex_surface_from_cold_sdk_provider(true)
    }

    #[test]
    fn explicit_stale_sdk_inventory_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source = tmp.path().join("std_regex_surface.incn");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/valid/std_regex_surface.incn"),
            &source,
        )?;
        let stale_inventory = tmp.path().join("stale-sdk-inventory.json");
        fs::write(
            &stale_inventory,
            r#"{
  "schema_version": 2,
  "sdk_id": "incan",
  "sdk_version": "0.5.0",
  "compiler_requirement": ">=0.5.0-dev.6,<0.6.0",
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
            stderr.contains("generated with provider codegen revision 4") && stderr.contains("requires revision 5"),
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
            project.join("incan.toml"),
            "[project]\nname = \"provider_backed_library\"\nversion = \"0.1.0\"\n\n[project.scripts]\nlibrary = \"src/lib.incn\"\n",
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
            .args(["run", "tests/fixtures/valid/u128_modulo_floor_div.incn"])
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
            .args(["run", "tests/fixtures/rfc030_field_overlay_reflection.incn"])
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
        let path = Path::new("workspaces/benchmarks/sorting/quicksort/quicksort.incn");
        if !path.exists() {
            return;
        }

        let Ok(source) = fs::read_to_string(path) else {
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

        // Note: This test uses standalone rustc compilation, which can't access incan_stdlib/incan_derive.
        // Skip the compilation check if generated Rust references external Incan crates.
        if rust_code.contains("incan_stdlib::") || rust_code.contains("incan_derive::") {
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
            rust_code.contains("incan_stdlib::num::try_resize::<_, i8>(wide)"),
            "expected try_resize to call stdlib checked resize helper, got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("incan_stdlib::num::saturating_resize::<_, i8>(wide)"),
            "expected saturating_resize to call stdlib saturating helper, got:\n{rust_code}"
        );
        assert!(
            rust_code.contains("let _price: incan_stdlib::num::Decimal128")
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
        let runtime_source = std::fs::read_to_string("crates/incan_stdlib/stdlib/datetime/runtime.incn")?;
        let mut civil_sources = Vec::new();
        civil_sources.push(std::fs::read_to_string(
            "crates/incan_stdlib/stdlib/datetime/civil.incn",
        )?);
        for entry in std::fs::read_dir("crates/incan_stdlib/stdlib/datetime/civil")? {
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
            .args(["run", "tests/fixtures/valid/std_datetime_surface.incn"])
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
            .args(["run", "tests/fixtures/valid/std_compression_surface.incn"])
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
/// stdout/stderr/exit code. They catch integration bugs like broken per-file `cargo test` harness wiring or parametrize
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
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir())
            .output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    fn shared_test_runner_target_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("incan_e2e_shared_target")
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
        cmd.env("CARGO_NET_OFFLINE", "true");
        cmd.env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir());
        cmd.output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    /// Run `incan test` with `cwd` and a relative path argument.
    fn run_incan_test_relative(cwd: &Path, relative_path: &str) -> std::process::Output {
        incan_command()
            .arg("test")
            .arg(relative_path)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir())
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

        let listed = run_incan_test_with_args(&dir, &["--list", "-k", "test_beta"]);
        let listed_stdout = String::from_utf8_lossy(&listed.stdout);
        let listed_stderr = String::from_utf8_lossy(&listed.stderr);
        assert!(
            listed.status.success(),
            "expected --list -k run to succeed.\nstdout:\n{}\nstderr:\n{}",
            listed_stdout,
            listed_stderr,
        );
        assert!(
            listed_stdout
                .lines()
                .any(|line| line == "test_runner_surface.incn::test_beta"),
            "expected exact listed beta id rooted at the explicit test directory.\nstdout:\n{}",
            listed_stdout,
        );
        assert!(
            !listed_stdout.contains(dir.to_string_lossy().as_ref()),
            "expected --list output to avoid machine-local absolute paths.\nstdout:\n{}",
            listed_stdout,
        );
        assert!(
            !listed_stdout.contains("test_runner_surface.incn::test_alpha"),
            "expected keyword filter to hide alpha.\nstdout:\n{}",
            listed_stdout,
        );

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
    fn e2e_generated_harness_preheat_is_fingerprinted() {
        let dir = write_test_project(
            "test_preheat.incn",
            r#"
from std.testing import assert_eq

def test_preheat() -> None:
    assert_eq(1, 1)
"#,
        );

        let first = run_incan_test_with_args(&dir, &["-v"]);
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first preheat run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );
        assert!(
            first_stdout.contains("preheat phase: ran"),
            "expected first run to preheat stale harness.\nstdout:\n{}",
            first_stdout,
        );
        assert!(
            first_stdout.contains("planned 1 generated harness unit(s)"),
            "expected verbose run to report generated harness planning.\nstdout:\n{}",
            first_stdout,
        );
        assert!(
            first_stdout.contains("cargo test phase: completed"),
            "expected verbose run to report cargo test phase timing.\nstdout:\n{}",
            first_stdout,
        );

        let second = run_incan_test_with_args(&dir, &["-v"]);
        let second_stdout = String::from_utf8_lossy(&second.stdout);
        let second_stderr = String::from_utf8_lossy(&second.stderr);
        assert!(
            second.status.success(),
            "expected second preheat run to succeed.\nstdout:\n{}\nstderr:\n{}",
            second_stdout,
            second_stderr,
        );
        assert!(
            second_stdout.contains("preheat phase: up-to-date"),
            "expected second run to reuse preheated harness.\nstdout:\n{}",
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
            "incan.toml",
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
            "incan.toml",
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

        let strict_smoke = run_incan_test_with_args(&dir, &["--list", "-m", "smoke", "--strict-markers"]);
        let strict_smoke_stdout = String::from_utf8_lossy(&strict_smoke.stdout);
        let strict_smoke_stderr = String::from_utf8_lossy(&strict_smoke.stderr);
        assert!(
            strict_smoke.status.success(),
            "expected strict registered marker list to succeed.\nstdout:\n{}\nstderr:\n{}",
            strict_smoke_stdout,
            strict_smoke_stderr,
        );
        assert!(strict_smoke_stdout.contains("test_runner_collection_surface.incn::test_inherited_smoke"));

        let strict_error = run_incan_test_with_args(&dir, &["--list", "-m", "missing", "--strict-markers"]);
        let strict_stderr = String::from_utf8_lossy(&strict_error.stderr);
        assert!(
            !strict_error.status.success(),
            "expected unknown strict marker to fail.\nstderr:\n{}",
            strict_stderr,
        );
        assert!(strict_stderr.contains("unknown marker `missing`"));

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

        let default_list = run_incan_test_with_args(&dir, &["--list"]);
        let default_stdout = String::from_utf8_lossy(&default_list.stdout);
        assert!(
            default_list.status.success(),
            "expected default list to succeed.\nstdout:\n{}",
            default_stdout,
        );
        assert!(default_stdout.contains("test_runner_collection_surface.incn::test_fast"));
        assert!(!default_stdout.contains("test_runner_collection_surface.incn::test_slow_case"));

        let slow_list = run_incan_test_with_args(&dir, &["--list", "--slow"]);
        let slow_stdout = String::from_utf8_lossy(&slow_list.stdout);
        assert!(
            slow_list.status.success(),
            "expected --slow list to succeed.\nstdout:\n{}",
            slow_stdout,
        );
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_fast"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_slow_case"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_marked_double[one-three]"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_marked_double[two-four]"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_pair[one-ten]"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_pair[one-twenty]"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_pair[two-ten]"));
        assert!(slow_stdout.contains("test_runner_collection_surface.incn::test_pair[two-twenty]"));

        let marked_run = run_incan_test_with_args(&dir, &["-k", "test_marked_double"]);
        let marked_stdout = String::from_utf8_lossy(&marked_run.stdout);
        let marked_stderr = String::from_utf8_lossy(&marked_run.stderr);
        assert!(
            marked_run.status.success(),
            "expected xfailed case and passing case to make the run succeed.\nstdout:\n{}\nstderr:\n{}",
            marked_stdout,
            marked_stderr,
        );
        assert!(marked_stdout.contains("xfailed") || marked_stdout.contains("XFAIL"));

        let add_run = run_incan_test_with_args(&dir, &["--verbose", "-k", "test_add"]);
        let add_stdout = String::from_utf8_lossy(&add_run.stdout);
        let add_stderr = String::from_utf8_lossy(&add_run.stderr);
        assert!(
            add_run.status.success(),
            "expected parametrized test to succeed.\nstdout:\n{}\nstderr:\n{}",
            add_stdout,
            add_stderr,
        );
        assert!(add_stdout.contains("test_add[1-2-3]"));
        assert!(add_stdout.contains("test_add[10-20-30]"));
        assert!(add_stdout.contains("test_add[0-0-0]"));
        assert!(add_stdout.contains("3 passed"));

        let failing_param = run_incan_test_with_args(&dir, &["--verbose", "-k", "test_double_failure"]);
        let failing_param_stdout = String::from_utf8_lossy(&failing_param.stdout);
        assert!(
            !failing_param.status.success(),
            "expected one failing case to make the run fail.\nstdout:\n{}",
            failing_param_stdout,
        );
        assert!(failing_param_stdout.contains("1 passed") && failing_param_stdout.contains("1 failed"));

        let skip_run = run_incan_test_with_args(&dir, &["-k", "test_skip_on_platform_probe"]);
        let skip_stdout = String::from_utf8_lossy(&skip_run.stdout);
        let skip_stderr = String::from_utf8_lossy(&skip_run.stderr);
        assert!(
            skip_run.status.success(),
            "expected skipif probe to make the run successful.\nstdout:\n{}\nstderr:\n{}",
            skip_stdout,
            skip_stderr,
        );
        assert!(skip_stdout.contains("SKIPPED") || skip_stdout.contains("skipped"));

        let without_feature = run_incan_test_with_args(&dir, &["-k", "test_feature_xfail"]);
        let without_stdout = String::from_utf8_lossy(&without_feature.stdout);
        let without_stderr = String::from_utf8_lossy(&without_feature.stderr);
        assert!(
            !without_feature.status.success(),
            "expected feature-gated xfail to run as an ordinary failing test without --feature.\nstdout:\n{}\nstderr:\n{}",
            without_stdout,
            without_stderr,
        );

        let with_feature = run_incan_test_with_args(&dir, &["--feature", "known_bug", "-k", "test_feature_xfail"]);
        let with_feature_stdout = String::from_utf8_lossy(&with_feature.stdout);
        let with_feature_stderr = String::from_utf8_lossy(&with_feature.stderr);
        assert!(
            with_feature.status.success(),
            "expected xfailif probe to make the run successful.\nstdout:\n{}\nstderr:\n{}",
            with_feature_stdout,
            with_feature_stderr,
        );
        assert!(with_feature_stdout.contains("XFAIL") || with_feature_stdout.contains("xfailed"));

        let timeout = run_incan_test_with_args(&dir, &["-k", "test_timeout_marker"]);
        let timeout_stdout = String::from_utf8_lossy(&timeout.stdout);
        let timeout_stderr = String::from_utf8_lossy(&timeout.stderr);
        assert!(
            !timeout.status.success(),
            "expected timeout marker to fail the test.\nstdout:\n{}\nstderr:\n{}",
            timeout_stdout,
            timeout_stderr,
        );
        assert!(timeout_stdout.contains("timed out after"));

        let arity_dir = write_test_project(
            "test_parametrize_arity.incn",
            r#"
from std.testing import parametrize

@parametrize("x, y", [1])
def test_bad_case(x: int, y: int) -> None:
    pass
"#,
        );
        let arity_output = run_incan_test(&arity_dir);
        let arity_stdout = String::from_utf8_lossy(&arity_output.stdout);
        let arity_stderr = String::from_utf8_lossy(&arity_output.stderr);
        assert!(
            !arity_output.status.success(),
            "expected arity mismatch to fail during collection.\nstdout:\n{}\nstderr:\n{}",
            arity_stdout,
            arity_stderr,
        );
        assert!(arity_stderr.contains("parametrize case `1`"));
        assert!(arity_stderr.contains("expected 2 value(s)"));

        let invalid_marker = run_incan_test_with_args(&dir, &["--list", "-m", "api and ("]);
        let invalid_marker_stderr = String::from_utf8_lossy(&invalid_marker.stderr);
        assert!(
            !invalid_marker.status.success(),
            "expected invalid marker expression to fail.\nstderr:\n{}",
            invalid_marker_stderr,
        );
        assert!(invalid_marker_stderr.contains("expected marker name or parenthesized expression"));

        let bad_conditional_dir = write_test_project(
            "test_bad_conditional_marker.incn",
            r#"
from std.testing import skipif

def helper() -> bool:
    return true

@skipif(helper(), reason="dynamic")
def test_dynamic_condition() -> None:
    pass
"#,
        );

        let output = run_incan_test(&bad_conditional_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected unsupported conditional marker expression to fail collection.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stderr.contains("platform()") && stderr.contains("feature"),
            "expected collection-time expression diagnostic.\nstderr:\n{}",
            stderr,
        );
    }

    #[test]
    fn e2e_jobs_run_independent_files_concurrently() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_sleep_a.incn",
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_sleep_a() -> None:
    sleep(Duration.from_millis(600))
"#,
        );
        let second = dir.join("test_sleep_b.incn");
        std::fs::write(
            &second,
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_sleep_b() -> None:
    sleep(Duration.from_millis(600))
"#,
        )?;

        let parallel = run_incan_test_with_args(&dir, &["--jobs", "2"]);
        let parallel_stdout = String::from_utf8_lossy(&parallel.stdout);
        let parallel_stderr = String::from_utf8_lossy(&parallel.stderr);
        assert!(
            parallel.status.success(),
            "expected parallel run to pass.\nstdout:\n{}\nstderr:\n{}",
            parallel_stdout,
            parallel_stderr,
        );
        let running_a = parallel_stdout
            .find("test_sleep_a.incn (1 item(s))")
            .ok_or("expected parallel output to announce test_sleep_a.incn")?;
        let running_b = parallel_stdout
            .find("test_sleep_b.incn (1 item(s))")
            .ok_or("expected parallel output to announce test_sleep_b.incn")?;
        let passed_a = parallel_stdout
            .find("test_sleep_a.incn::test_sleep_a PASSED")
            .ok_or("expected parallel output to report test_sleep_a passing")?;
        let passed_b = parallel_stdout
            .find("test_sleep_b.incn::test_sleep_b PASSED")
            .ok_or("expected parallel output to report test_sleep_b passing")?;
        let first_pass = passed_a.min(passed_b);
        assert!(
            running_a < first_pass && running_b < first_pass,
            "expected --jobs 2 to launch both independent file batches before either completed\nparallel stdout:\n{}",
            parallel_stdout,
        );
        Ok(())
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
        let warmup = run_incan_test_with_args(&dir, &["--jobs", "1", "-k", "test_b_slow"]);
        let warmup_stdout = String::from_utf8_lossy(&warmup.stdout);
        let warmup_stderr = String::from_utf8_lossy(&warmup.stderr);
        assert!(
            warmup.status.success(),
            "expected slow test warm-up to pass.\nstdout:\n{}\nstderr:\n{}",
            warmup_stdout,
            warmup_stderr,
        );

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
    fn e2e_resource_marker_prevents_overlapping_workers() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_resource_a.incn",
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration
from std.testing import resource

@resource("db")
def test_resource_a() -> None:
    sleep(Duration.from_millis(700))
"#,
        );
        std::fs::write(
            dir.join("test_resource_b.incn"),
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration
from std.testing import resource

@resource("db")
def test_resource_b() -> None:
    sleep(Duration.from_millis(700))
"#,
        )?;

        let warmup = run_incan_test_with_args(&dir, &["--jobs", "1"]);
        let warmup_stdout = String::from_utf8_lossy(&warmup.stdout);
        let warmup_stderr = String::from_utf8_lossy(&warmup.stderr);
        assert!(
            warmup.status.success(),
            "expected resource warm-up to pass.\nstdout:\n{}\nstderr:\n{}",
            warmup_stdout,
            warmup_stderr,
        );

        let start = std::time::Instant::now();
        let output = run_incan_test_with_args(&dir, &["--jobs", "2"]);
        let elapsed = start.elapsed();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected resource-constrained run to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(1200),
            "expected shared @resource workers not to overlap; elapsed={:?}\nstdout:\n{}",
            elapsed,
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_serial_marker_runs_alone() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_serial.incn",
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration
from std.testing import serial

@serial
def test_serial() -> None:
    sleep(Duration.from_millis(700))
"#,
        );
        std::fs::write(
            dir.join("test_regular.incn"),
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_regular() -> None:
    sleep(Duration.from_millis(700))
"#,
        )?;

        let warmup = run_incan_test_with_args(&dir, &["--jobs", "1"]);
        let warmup_stdout = String::from_utf8_lossy(&warmup.stdout);
        let warmup_stderr = String::from_utf8_lossy(&warmup.stderr);
        assert!(
            warmup.status.success(),
            "expected serial warm-up to pass.\nstdout:\n{}\nstderr:\n{}",
            warmup_stdout,
            warmup_stderr,
        );

        let start = std::time::Instant::now();
        let output = run_incan_test_with_args(&dir, &["--jobs", "2"]);
        let elapsed = start.elapsed();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected serial-constrained run to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            elapsed >= std::time::Duration::from_millis(1200),
            "expected @serial worker to run alone; elapsed={:?}\nstdout:\n{}",
            elapsed,
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_sequential_single_file_runs_do_not_cross_wire_paths() {
        let dir = write_test_project(
            "incan.toml",
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

        let abs_dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "session_isolation_absolute"
version = "0.1.0"
"#,
        );
        let tests_dir = abs_dir.join("tests");
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        let alpha_path = tests_dir.join("test_alpha_abs.incn");
        let beta_path = tests_dir.join("test_beta_abs.incn");
        if let Err(err) = std::fs::write(
            &alpha_path,
            r#"
from std.testing import assert_eq

def test_alpha_abs_one() -> None:
    assert_eq(10, 10)
"#,
        ) {
            panic!("failed to write test_alpha_abs.incn: {}", err);
        }
        if let Err(err) = std::fs::write(
            &beta_path,
            r#"
from std.testing import assert_eq

def test_beta_abs_only() -> None:
    assert_eq(20, 20)
"#,
        ) {
            panic!("failed to write test_beta_abs.incn: {}", err);
        }

        let first = run_incan_test_path(&alpha_path);
        let first_stdout = String::from_utf8_lossy(&first.stdout);
        let first_stderr = String::from_utf8_lossy(&first.stderr);
        assert!(
            first.status.success(),
            "expected first absolute-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            first_stdout,
            first_stderr,
        );

        let second = run_incan_test_path(&beta_path);
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
            "incan.toml",
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
            "incan.toml",
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
            "incan.toml",
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
    from rust::incan_stdlib::testing import TestEnv
    from rust::std::path import PathBuf
    import std.testing as testing
    from std.testing import assert_eq, assert_is_some, fixture, test

    @fixture(autouse=true)
    def seed() -> int:
        return 40

    @fixture
    def answer(seed: int) -> int:
        return seed + 2

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

        let listed = run_incan_test_with_args(&dir, &["--list", "-k", "decorated_inline_case"]);
        let listed_stdout = String::from_utf8_lossy(&listed.stdout);
        let listed_stderr = String::from_utf8_lossy(&listed.stderr);
        assert!(
            listed.status.success(),
            "expected inline --list -k run to succeed.\nstdout:\n{}\nstderr:\n{}",
            listed_stdout,
            listed_stderr,
        );
        assert!(
            listed_stdout
                .lines()
                .any(|line| line == "src/main.incn::decorated_inline_case"),
            "expected decorated inline test id in --list output.\nstdout:\n{}",
            listed_stdout,
        );
        assert!(
            !listed_stdout.contains("src/main.incn::test_inline_addition"),
            "expected keyword filter to hide the name-discovered inline test.\nstdout:\n{}",
            listed_stdout,
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
            "incan.toml",
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
            "incan.toml",
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
            "incan.toml",
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
            "incan.toml",
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
        assert!(
            generated.contains("Registry :: add_static")
                || generated.contains("Registry::add_static")
                || generated.contains("Registry :: add_static ::"),
            "class static method decorator should lower as associated function syntax:\n{}",
            generated,
        );
        assert!(
            generated.contains(".with_mut(|__incan_static_value|")
                && (generated.contains("let __incan_static_arg_0 = \"instance\".to_string();")
                    || generated.contains("let __incan_static_arg_0 = \"instance\".into();"))
                && generated.contains("__incan_static_value.add(__incan_static_arg_0)"),
            "static registry receiver should lower through static storage access:\n{}",
            generated,
        );
        Ok(())
    }

    #[test]
    fn build_lib_imported_static_decorator_receiver_materializes_string_arg_issue671()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
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
            "incan.toml",
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

        let alpha = run_incan_test_path(&src_dir.join("alpha.incn"));
        assert!(
            alpha.status.success(),
            "expected direct alpha inline test run to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&alpha.stdout),
            String::from_utf8_lossy(&alpha.stderr),
        );
        let beta = run_incan_test_path(&src_dir.join("beta.incn"));
        assert!(
            beta.status.success(),
            "expected direct beta inline test run to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&beta.stdout),
            String::from_utf8_lossy(&beta.stderr),
        );
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
            "incan.toml",
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

        let listed = run_incan_test_with_args(&dir, &["--list", "-m", "smoke", "--strict-markers"]);
        let listed_stdout = String::from_utf8_lossy(&listed.stdout);
        let listed_stderr = String::from_utf8_lossy(&listed.stderr);
        assert!(
            listed.status.success(),
            "expected inline strict marker list to succeed.\nstdout:\n{}\nstderr:\n{}",
            listed_stdout,
            listed_stderr,
        );
        assert!(listed_stdout.contains("src/math.incn::test_double[one-three]"));
        assert!(listed_stdout.contains("src/math.incn::test_double[two-four]"));
        assert!(listed_stdout.contains("src/math.incn::test_timeout_marker"));

        let run = run_incan_test_with_args(&dir, &["-k", "test_double"]);
        let run_stdout = String::from_utf8_lossy(&run.stdout);
        let run_stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            run.status.success(),
            "expected inline parametrized xfail/pass cases to succeed.\nstdout:\n{}\nstderr:\n{}",
            run_stdout,
            run_stderr,
        );
        assert!(run_stdout.contains("XFAIL") || run_stdout.contains("xfailed"));

        let timeout = run_incan_test_with_args(&dir, &["-k", "test_timeout_marker"]);
        let timeout_stdout = String::from_utf8_lossy(&timeout.stdout);
        let timeout_stderr = String::from_utf8_lossy(&timeout.stderr);
        assert!(
            !timeout.status.success(),
            "expected inline timeout marker to fail the test.\nstdout:\n{}\nstderr:\n{}",
            timeout_stdout,
            timeout_stderr,
        );
        assert!(timeout_stdout.contains("timed out after"));
        Ok(())
    }

    #[test]
    fn e2e_fixture_lifetime_success_scenarios_share_one_project() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
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
            "incan.toml",
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
            "incan.toml",
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

        let message = run_incan_test_with_args(&dir, &["-k", "test_message"]);
        let message_stdout = String::from_utf8_lossy(&message.stdout);
        let message_stderr = String::from_utf8_lossy(&message.stderr);
        let message_combined = format!("{message_stdout}\n{message_stderr}");

        assert!(
            !message.status.success(),
            "expected assertion failure test to fail.\n{}",
            message_combined,
        );
        assert!(
            message_combined.contains("AssertionError: custom boom"),
            "expected custom assertion message in output.\n{}",
            message_combined,
        );

        let eq = run_incan_test_with_args(&dir, &["-k", "test_eq_message"]);
        let eq_stdout = String::from_utf8_lossy(&eq.stdout);
        let eq_stderr = String::from_utf8_lossy(&eq.stderr);
        let eq_combined = format!("{eq_stdout}\n{eq_stderr}");

        assert!(
            !eq.status.success(),
            "expected assertion failure test to fail.\n{}",
            eq_combined,
        );
        assert!(
            eq_combined.contains("AssertionError: math broke"),
            "expected custom equality assertion message in output.\n{}",
            eq_combined,
        );
        assert!(
            eq_combined.contains("left != right"),
            "expected equality failure kind in output.\n{}",
            eq_combined,
        );

        let wrong = run_incan_test_with_args(&dir, &["-k", "test_wrong"]);
        let wrong_stdout = String::from_utf8_lossy(&wrong.stdout);

        assert!(
            !wrong.status.success(),
            "expected failing test to exit non-zero.\nstdout:\n{}",
            wrong_stdout,
        );
        assert!(
            wrong_stdout.contains("FAILED") || wrong_stdout.contains("failed"),
            "expected FAILED in output.\nstdout:\n{}",
            wrong_stdout,
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
    use incan::frontend::ast::*;
    use incan::frontend::{lexer, parser};

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
  value: T

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

mod rfc031_pub_import_integration_tests {
    use super::*;
    use incan::library_manifest::{
        FieldVisibilityExport, FunctionExport, LibraryManifest, ModelExport, ParamExport, TypeRef,
    };
    use incan::manifest::{INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV};
    use sha2::{Digest, Sha256};
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn write_project_files(
        root: &Path,
        manifest_content: &str,
        main_source: &str,
    ) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("incan.toml"), manifest_content)?;
        let main_path = root.join("src").join("main.incn");
        std::fs::write(&main_path, main_source)?;
        Ok(main_path)
    }

    fn run_check(main_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command().arg("--check").arg(main_path).output()?)
    }

    fn run_build(main_path: &Path, out_dir: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    #[test]
    fn consumer_build_infers_rust_generic_return_from_unwrap_context_issue852() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"generic_json_return_repro\"\n\n[rust-dependencies.serde_json]\nversion = \"1.0\"\n",
            r#"from rust::serde_json import Value
from rust::serde_json import from_str as json_parse


def parse_value() -> Value:
    return json_parse("{}").unwrap()


def accept_value(value: Value) -> None:
    pass


def main() -> None:
    value: Value = parse_value()
    accept_value(json_parse("{}").unwrap())
"#,
        )?;

        let check_output = run_check(&main_path)?;
        assert!(
            check_output.status.success(),
            "expected generic Rust JSON parser result to infer through unwrap context.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected generated Rust to compile for the inferred generic JSON result.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let tests_dir = tmp.path().join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        std::fs::write(
            tests_dir.join("test_generic_json_return.incn"),
            r#"from rust::serde_json import Value
from rust::serde_json import from_str as json_parse


def accept_value(value: Value) -> None:
    pass


def test_generic_json_result_infers_from_parameter_context() -> None:
    accept_value(json_parse("{}").unwrap())
    assert true
"#,
        )?;
        let test_output = run_test(&tests_dir)?;
        assert!(
            test_output.status.success(),
            "expected the package test batch to compile and run the inferred generic JSON result.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    fn run_run(main_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn run_lock(entry_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["lock", entry_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn run_test(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["test", target.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir())
            .output()?)
    }

    fn run_fmt(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["fmt", target.to_string_lossy().as_ref()])
            .output()?)
    }

    fn run_fmt_check(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["fmt", "--check", target.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn shared_test_runner_target_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("incan_e2e_shared_target")
    }

    fn test_runner_batch_manifest_path(file_path: &Path) -> PathBuf {
        let canonical = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string_lossy().as_bytes());
        let digest = hex::encode(hasher.finalize());
        let suffix = format!("batch_{}", &digest[..16]);
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target/incan_tests")
            .join(suffix)
            .join("Cargo.toml")
    }

    fn run_build_lib(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn run_build_lib_artifact_only(project_root: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(super::incan_command()
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY", "1")
            .output()?)
    }

    /// Verifies absolute crate imports across direct checking/building, library re-export, and test-batch compilation.
    #[test]
    fn boundary_parity_preserves_absolute_crate_public_types_issue882() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("absolute_crate_public_types");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"absolute_crate_public_types\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/types.incn"),
            r#"pub enum Access:
    Allowed
    Denied


pub model Decision:
    pub admitted: bool
    pub reason: str
"#,
        )?;
        let consumer_path = project_root.join("src/consumer.incn");
        std::fs::write(
            &consumer_path,
            r#"from crate.types import Access, Decision


pub def allowed() -> Access:
    return Access.Allowed


pub def explain(decision: Decision) -> str:
    if decision.admitted:
        return decision.reason
    return "denied"


def main() -> None:
    decision = Decision(admitted=true, reason="allowed")
    assert allowed() == Access.Allowed
    assert explain(decision) == "allowed"
"#,
        )?;
        std::fs::write(
            project_root.join("src/lib.incn"),
            r#"pub from crate.consumer import allowed, explain
pub from crate.types import Access, Decision
"#,
        )?;
        std::fs::write(
            project_root.join("tests/test_public_types.incn"),
            r#"from crate.consumer import allowed, explain
from crate.types import Access, Decision


def test_absolute_crate_public_types() -> None:
    decision = Decision(admitted=true, reason="allowed")
    assert allowed() == Access.Allowed
    assert explain(decision) == "allowed"
"#,
        )?;

        let check_output = run_check(&consumer_path)?;
        assert!(
            check_output.status.success(),
            "expected direct check to preserve absolute crate import metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let direct_build = run_build(&consumer_path, &project_root.join("out"))?;
        assert!(
            direct_build.status.success(),
            "expected direct build to preserve absolute crate import metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&direct_build.stdout),
            String::from_utf8_lossy(&direct_build.stderr)
        );

        let library_build = run_build_lib(&project_root)?;
        assert!(
            library_build.status.success(),
            concat!(
                "expected library build and re-export to preserve absolute crate import metadata.\n",
                "stdout:\n{}\nstderr:\n{}",
            ),
            String::from_utf8_lossy(&library_build.stdout),
            String::from_utf8_lossy(&library_build.stderr)
        );

        let test_output = run_test(&project_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected test-batch parity for absolute crate public types.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn compiled_provider_annotations_survive_generated_test_batch_issue902() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("count_provider");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("incan.toml"),
            "[project]\nname = \"counts\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            r#"pub model Count:
  pub value: int
"#,
        )?;

        let provider_build = run_build_lib(&provider_root)?;
        assert!(
            provider_build.status.success(),
            "expected #902 provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_build.stdout),
            String::from_utf8_lossy(&provider_build.stderr),
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"count_consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\ncounts = { path = \"../count_provider\" }\n",
        )?;
        std::fs::write(
            consumer_root.join("src/bridge.incn"),
            r#"from pub::counts import Count

pub def make_count(value: int) -> Count:
  return Count(value=value)
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/count_annotation_test.incn"),
            r#"from pub::counts import Count
from crate.bridge import make_count

def identity(value: Count) -> Count:
  return value

def test_compiled_provider_annotation() -> None:
  count: Count = identity(make_count(7))
  assert count.value == 7
"#,
        )?;

        let batch = run_test(&consumer_root.join("tests"))?;
        let stdout = String::from_utf8_lossy(&batch.stdout);
        let stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected #902 compiled-provider test batch to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("count_annotation_test.incn::test_compiled_provider_annotation"),
            "expected the compiled-provider annotation regression to run.\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );

        Ok(())
    }

    #[test]
    fn test_batch_keeps_transitive_pub_imports_out_of_later_test_scope_issue898()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("probe_lib_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"probe_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            r#"pub model Record:
  pub value: int

pub def marker() -> int:
  return 7
"#,
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected #898 provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr),
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"pub_import_batch\"\nversion = \"0.1.0\"\n\n[dependencies]\nprobe_lib = { path = \"../probe_lib_provider\" }\n",
        )?;
        std::fs::write(
            consumer_root.join("src/bridge.incn"),
            r#"from pub::probe_lib import Record, marker

pub def bridged() -> int:
  _ = Record(value=marker())
  return marker()

pub def bridged_record() -> Record:
  return Record(value=marker())
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/aaa_indirect_test.incn"),
            r#"from crate.bridge import bridged, bridged_record

def test_indirect_import() -> None:
  assert bridged() == 7
  assert bridged_record().value == 7
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/zzz_direct_test.incn"),
            r#"from pub::probe_lib import Record, marker
from crate.bridge import bridged, bridged_record

def test_direct_import() -> None:
  record: Record = bridged_record()
  assert record.value == 7
  assert marker() == 7
  assert bridged() == 7
"#,
        )?;

        let batch = run_test(&consumer_root.join("tests"))?;
        let stdout = String::from_utf8_lossy(&batch.stdout);
        let stderr = String::from_utf8_lossy(&batch.stderr);
        assert!(
            batch.status.success(),
            "expected #898 indirect/direct public-import test batch to pass.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_indirect_import") && stdout.contains("test_direct_import"),
            "expected both #898 regression tests to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            !stderr.contains("already in scope"),
            "transitive dependency imports must not leak into a later test module.\nstderr:\n{}",
            stderr,
        );

        Ok(())
    }

    #[test]
    fn boundary_parity_preserves_dependency_owned_union_helpers_through_facade()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("boundarykit_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"boundarykit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/exprs.incn"),
            r#"@derive(Clone)
pub model ColumnRefExpr:
  name: str

@derive(Clone)
pub model SortExpr:
  direction: str

pub type ColumnExpr = Union[ColumnRefExpr, SortExpr]

@derive(Clone)
pub class Frame:
  def order_by(self, columns: list[ColumnExpr]) -> Self:
    return self

pub def frame() -> Frame:
  return Frame()

pub def col(name: str) -> ColumnRefExpr:
  return ColumnRefExpr(name=name)

pub def desc(expr: ColumnExpr) -> ColumnExpr:
  return SortExpr(direction="desc")
"#,
        )?;
        std::fs::write(
            producer_root.join("src/facade.incn"),
            "pub from exprs import ColumnExpr, ColumnRefExpr, SortExpr, Frame, frame, col, desc\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from facade import ColumnExpr, ColumnRefExpr, SortExpr, Frame, frame, col, desc\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected boundarykit provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let main_path = write_project_files(
            &consumer_root,
            "[project]\nname = \"consumer\"\n\n[dependencies]\nboundarykit = { path = \"../boundarykit_provider\" }\n",
            r#"from pub::boundarykit import Frame, frame
from pub::boundarykit import col as __incan_vocab_helper_boundarykit_col
from pub::boundarykit import desc as __incan_vocab_helper_boundarykit_desc

def main() -> None:
  ordered: Frame = frame().order_by([
    __incan_vocab_helper_boundarykit_desc(__incan_vocab_helper_boundarykit_col("amount"))
  ])
  ordered.order_by([])
"#,
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            consumer_build.status.success(),
            "expected boundary parity consumer build to preserve dependency-owned union identity.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main,
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        assert!(
            !generated_main.contains("pub enum __IncanUnion"),
            "consumer must not re-own provider anonymous unions.\ngenerated main.rs:\n{generated_main}"
        );
        assert!(
            generated_main.contains("boundarykit::__IncanUnion"),
            "expected public helper calls to use provider-qualified union wrappers.\ngenerated main.rs:\n{generated_main}"
        );

        Ok(())
    }

    #[test]
    fn boundary_parity_preserves_decorated_alias_partial_identity_through_facade()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("callkit_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"callkit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/registry.incn"),
            r#"pub model FunctionSpec:
  namespace: str
  name: str
  deterministic: bool

pub static registered_names: list[str] = []
pub static registered_specs: list[FunctionSpec] = []
pub static package_markers: list[str] = []

pub deterministic_spec = partial FunctionSpec(namespace="core", deterministic=true)

pub def register[F](spec: FunctionSpec) -> ((F) -> F):
  registered_specs.append(spec)
  return (func) => capture[F](func)

def capture[F](func: F) -> F:
  registered_names.append(func.__name__)
  return func

@register(deterministic_spec(name="scale"))
pub def scale(value: int) -> int:
  return value * 2

pub scale_alias = alias scale

pub def registered_count() -> int:
  return len(registered_names)

pub def registered_name(index: int) -> str:
  return registered_names[index]

pub def registered_spec_name(index: int) -> str:
  return registered_specs[index].name
"#,
        )?;
        std::fs::write(
            producer_root.join("src/facade.incn"),
            r#"pub from registry import FunctionSpec, deterministic_spec, package_markers, registered_count, registered_name, registered_spec_name, scale, scale_alias
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from facade import FunctionSpec, deterministic_spec, package_markers, registered_count, registered_name, registered_spec_name, scale, scale_alias\n",
        )?;
        std::fs::create_dir_all(producer_root.join("tests"))?;
        std::fs::write(
            producer_root.join("tests/test_direct_identity.incn"),
            r#"from registry import registered_count, registered_name, registered_spec_name, scale, scale_alias


def test_direct_source_import_identity() -> None:
  assert scale(3) == 6
  assert scale_alias(4) == 8
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
"#,
        )?;
        std::fs::write(
            producer_root.join("tests/test_facade_identity.incn"),
            r#"from facade import registered_count, registered_name, registered_spec_name, scale, scale_alias


def test_facade_source_import_identity() -> None:
  assert scale(5) == 10
  assert scale_alias(6) == 12
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
"#,
        )?;
        std::fs::write(
            producer_root.join("tests/test_mixed_identity.incn"),
            r#"from registry import registered_count, registered_name, registered_spec_name, scale as direct_scale, scale_alias as direct_scale_alias
from facade import scale as facade_scale, scale_alias as facade_scale_alias


def test_direct_and_facade_identity_share_one_static() -> None:
  assert direct_scale(3) == 6
  assert direct_scale_alias(4) == 8
  assert facade_scale(5) == 10
  assert facade_scale_alias(6) == 12
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
"#,
        )?;

        let producer_tests = run_test(&producer_root.join("tests"))?;
        assert!(
            producer_tests.status.success(),
            "expected decorated alias partial identity source and facade test batch to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_tests.stdout),
            String::from_utf8_lossy(&producer_tests.stderr)
        );
        let mixed_identity_test = run_test(&producer_root.join("tests/test_mixed_identity.incn"))?;
        assert!(
            mixed_identity_test.status.success(),
            "expected combined direct/facade identity fixture to succeed by itself.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&mixed_identity_test.stdout),
            String::from_utf8_lossy(&mixed_identity_test.stderr)
        );

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected callkit provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let main_path = write_project_files(
            &consumer_root,
            "[project]\nname = \"consumer\"\n\n[dependencies]\ncallkit = { path = \"../callkit_provider\" }\n",
            r#"from pub::callkit import package_markers, registered_count, registered_name, registered_spec_name, scale, scale_alias

def main() -> None:
  assert scale(3) == 6
  assert scale_alias(4) == 8
  assert registered_count() == 1
  assert registered_name(0) == "scale"
  assert registered_spec_name(0) == "scale"
  package_markers.append(f"consumer")
  assert len(package_markers) == 1
"#,
        )?;

        let consumer_check = run_check(&main_path)?;
        assert!(
            consumer_check.status.success(),
            "expected decorated alias partial identity consumer check to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );
        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected decorated alias partial identity consumer build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        let consumer_run = run_run(&main_path)?;
        assert!(
            consumer_run.status.success(),
            "expected decorated alias partial identity consumer run to execute shared package statics.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        Ok(())
    }

    #[test]
    fn boundary_parity_preserves_enum_method_defaults_through_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("enumkit_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"enumkit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/status.incn"),
            r#"pub enum Status(str):
  Ready = "ready"
  Paused = "paused"

  def label(self, prefix: str = "state") -> str:
    match self:
      Status.Ready => return f"{prefix}:ready"
      Status.Paused => return f"{prefix}:paused"
"#,
        )?;
        std::fs::write(producer_root.join("src/facade.incn"), "pub from status import Status\n")?;
        std::fs::write(producer_root.join("src/lib.incn"), "pub from facade import Status\n")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected enumkit provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let main_path = write_project_files(
            &consumer_root,
            "[project]\nname = \"consumer\"\n\n[dependencies]\nenumkit = { path = \"../enumkit_provider\" }\n",
            r#"from pub::enumkit import Status


def main() -> None:
  assert Status.Ready.label() == "state:ready"
  assert Status.Paused.label(prefix="custom") == "custom:paused"
"#,
        )?;
        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&main_path, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected enum method defaults to survive provider/facade/consumer boundary.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        Ok(())
    }

    #[test]
    fn boundary_parity_activates_dependency_vocab_across_check_fmt_and_test() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        write_pub_library_with_provider_requirements_and_assert_keyword(
            tmp.path(),
            "widgets",
            "widgets_core",
            Vec::new(),
            Vec::new(),
        )?;

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../deps/widgets\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import pub::widgets


def main() -> None:
    assert true
"#,
        )?;
        let test_path = consumer_root.join("tests/test_vocab.incn");
        std::fs::write(
            &test_path,
            r#"import pub::widgets


def test_external_vocab_assert_keyword() -> None:
    assert true
"#,
        )?;

        let check_output = run_check(&main_path)?;
        assert!(
            check_output.status.success(),
            "expected dependency vocab to activate for --check.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );
        let fmt_check_output = run_fmt_check(&consumer_root.join("src"))?;
        assert!(
            fmt_check_output.status.success(),
            "expected dependency vocab to activate for fmt --check.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_check_output.stdout),
            String::from_utf8_lossy(&fmt_check_output.stderr)
        );
        let test_output = run_test(&consumer_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected dependency vocab to activate for incan test.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn fmt_dependency_collection_does_not_prepare_persistent_library_artifact() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        write_vocab_companion_crate_with_assert_keyword(&producer_root, "vocab_companion", "widgets_vocab_companion")?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            r#"pub model Marker:
    value: int
"#,
        )?;

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_provider\" }\n",
        )?;
        std::fs::write(
            consumer_root.join("src/main.incn"),
            r#"import pub::widgets


def main() -> None:
    assert true
"#,
        )?;

        let output = run_fmt_check(&consumer_root.join("src"))?;
        assert!(
            output.status.success(),
            "expected fmt --check to parse with source-backed dependency context without failing.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !producer_root.join("target/lib/widgets_core.incnlib").exists(),
            "fmt dependency collection should not create a persistent provider .incnlib manifest"
        );
        let combined_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !combined_output.contains("Preparing missing pub::widgets dependency artifact"),
            "fmt dependency collection should not invoke persistent artifact preparation, got: {combined_output}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_artifact_only_writes_manifest_without_nested_cargo_build() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("widgets_provider");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/lib.incn"),
            r#"pub model Marker:
    value: int

pub def marker(value: int) -> Marker:
    return Marker(value=value)
"#,
        )?;

        let output = run_build_lib_artifact_only(&project_root)?;
        assert!(
            output.status.success(),
            "expected artifact-only build --lib to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            project_root.join("target/lib/widgets_core.incnlib").exists(),
            "artifact-only build should still write the provider library manifest"
        );
        assert!(
            !project_root.join("target/lib/target").exists(),
            "artifact-only build should not run generated Cargo build or create a nested target directory"
        );
        Ok(())
    }

    #[test]
    fn relocated_artifact_only_provider_keeps_transitive_feature_and_rust_dependency_graph()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let original = tmp.path().join("original");
        let serializer_root = original.join("serializer");
        std::fs::create_dir_all(serializer_root.join("src"))?;
        std::fs::write(
            serializer_root.join("incan.toml"),
            r#"[project]
name = "serializer_core"
version = "0.5.0"

[project.features]
default = ["json"]
json = []
"#,
        )?;
        std::fs::write(
            serializer_root.join("src/lib.incn"),
            "pub def serialized_value() -> int:\n    return 7\n",
        )?;
        let serializer_build = run_build_lib_artifact_only(&serializer_root)?;
        assert!(
            serializer_build.status.success(),
            "expected leaf provider artifact build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&serializer_build.stdout),
            String::from_utf8_lossy(&serializer_build.stderr)
        );

        let reporting_root = original.join("reporting");
        std::fs::create_dir_all(reporting_root.join("src"))?;
        std::fs::write(
            reporting_root.join("incan.toml"),
            r#"[project]
name = "reporting_core"
version = "0.5.0"

[project.features]
default = ["json"]
json = ["dep:serializer", "serializer/json"]

[dependencies]
serializer = { path = "../serializer", optional = true, default-features = false }
"#,
        )?;
        std::fs::write(
            reporting_root.join("src/lib.incn"),
            "pub def report_value() -> int:\n    return 11\n",
        )?;
        let reporting_build = run_build_lib_artifact_only(&reporting_root)?;
        assert!(
            reporting_build.status.success(),
            "expected parent provider artifact build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&reporting_build.stdout),
            String::from_utf8_lossy(&reporting_build.stderr)
        );
        let reporting_manifest =
            LibraryManifest::read_from_path(&reporting_root.join("target/lib/reporting_core.incnlib"))?;
        let dependency = reporting_manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .first()
            .ok_or("reporting artifact omitted its active provider dependency")?;
        assert_eq!(dependency.dependency_key, "serializer");
        assert_eq!(dependency.requested_features, BTreeSet::from(["json".to_string()]));
        assert!(dependency.relative_artifact_path.starts_with("../"));

        let consumer_root = original.join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"artifact_consumer\"\n\n[dependencies]\nreporting = { path = \"../reporting\" }\n",
        )?;
        std::fs::write(consumer_root.join("src/main.incn"), "def main() -> None:\n    pass\n")?;

        let relocated = tmp.path().join("relocated");
        std::fs::rename(&original, &relocated)?;
        std::fs::remove_file(relocated.join("serializer/incan.toml"))?;
        std::fs::remove_file(relocated.join("reporting/incan.toml"))?;
        std::fs::remove_dir_all(relocated.join("serializer/src"))?;
        std::fs::remove_dir_all(relocated.join("reporting/src"))?;

        let relocated_consumer = relocated.join("consumer");
        let output = run_build(
            &relocated_consumer.join("src/main.incn"),
            &relocated_consumer.join("out"),
        )?;
        assert!(
            output.status.success(),
            "expected relocated source-free transitive provider graph to compile.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn build_keeps_return_context_string_literal_union_arg_as_union_value() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("return_context_union_arg");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"return_context_union_arg\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/projection_builders.incn"),
            r#"pub model ColumnRefExpr:
    column_name: str

pub model StringLiteralExpr:
    value: str

pub model FloatLiteralExpr:
    value: float

pub model EqExpr:
    arguments: list[ColumnExpr]

pub type ColumnExpr = Union[ColumnRefExpr, StringLiteralExpr, FloatLiteralExpr, EqExpr]

pub def col(name: str) -> ColumnExpr:
    return ColumnRefExpr(column_name=name)

pub def str_expr(value: str) -> ColumnExpr:
    return StringLiteralExpr(value=value)

pub def float_expr(value: float) -> ColumnExpr:
    return FloatLiteralExpr(value=value)

pub def lit(value: Union[int, float, str, bool]) -> ColumnExpr:
    match value:
        float(number) => return float_expr(number)
        str(text) => return str_expr(text)
        bool(flag) => return str_expr("bool")
        int(number) => return str_expr("int")

pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return EqExpr(arguments=[left, right])
"#,
        )?;
        std::fs::write(
            project_root.join("src/functions.incn"),
            "from projection_builders import col as col_builder, eq as eq_builder, lit as lit_builder\n\npub col = alias col_builder\npub lit = alias lit_builder\npub eq = alias eq_builder\n",
        )?;
        std::fs::write(
            project_root.join("src/dataset.incn"),
            r#"from projection_builders import ColumnExpr

pub class LazyFrame[T with Clone]:
    pub rows: list[T]

    def filter(self, predicate: ColumnExpr) -> Self:
        return self
"#,
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from dataset import LazyFrame
from functions import col, eq, lit

model OrderLine:
    status: str
    discount: float

def repro(lines: LazyFrame[OrderLine]) -> LazyFrame[OrderLine]:
    return lines.filter(eq(col("status"), lit("open"))).filter(eq(col("discount"), lit(0.9)))

def main() -> None:
    lines: LazyFrame[OrderLine] = LazyFrame[OrderLine](rows=[])
    _ = repro(lines)
    println("done")
"#,
        )?;

        let out_dir = project_root.join("out");
        let output = run_build(&main_path, &out_dir)?;
        assert!(
            output.status.success(),
            "expected union literal regression build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let normalized: String = generated_main.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            normalized.contains("lit(crate::__IncanUnion43fbd19e99c1db05::V0(\"open\".to_string()))"),
            "expected string literal to be wrapped directly as the union string arm, got:\n{generated_main}"
        );
        assert!(
            !normalized.contains("V0(\"open\".to_string()).to_string()"),
            "union wrapper must not receive a post-wrapper string coercion, got:\n{generated_main}"
        );
        Ok(())
    }

    #[test]
    fn std_json_and_generated_runtime_surfaces_share_one_generated_run() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"std_json_runtime_surface_batch\"\nversion = \"0.3.0-dev.1\"\n",
            r#"from std.serde import json
from std.serde.json import Deserialize, Serialize
from std.json import JsonValue

model SerializePayload with Serialize:
  value: int

model HelperPayload with Serialize:
  value: int

@derive(json)
model JsonPayload:
  value: int
  label: str

@derive(Deserialize)
model DirectPayload:
  value: int

@derive(json)
model Envelope:
  status: int
  data: JsonValue

@derive(json)
model Probe:
  name: Option[JsonValue]
  first: Option[JsonValue]
  missing: Option[JsonValue]

const NUMBERS: FrozenList[float] = [3.0, 1.5, 4.25]

def run_explicit_serialize_trait() -> None:
  println(SerializePayload(value=1).to_json())

def run_generated_runtime_helpers() -> None:
  mut xs = [3, 1, 4]
  println(xs.pop())
  println(min(xs))
  println(max(xs))
  println(HelperPayload(value=2).to_json())

def run_std_json_deserialize() -> None:
  match JsonPayload.from_json('{"value":7,"label":"dogfood"}'):
    case Ok(payload):
      println(payload.to_json())
    case Err(err):
      println(err)

def run_direct_deserialize_derive() -> None:
  match DirectPayload.from_json('{"value":7}'):
    case Ok(payload):
      println(f"{payload.value}")
    case Err(err):
      println(err)

def run_json_value_model_field_roundtrip() -> None:
  match Envelope.from_json('{"status":200,"data":{"name":"Ada","items":[1,2]}}'):
    case Ok(envelope):
      match envelope.data["items"]:
        case Some(items):
          let probe = Probe(name=envelope.data["name"], first=items[0], missing=items[9])
          println(probe.to_json())
        case None:
          println("missing items")
    case Err(err):
      println(err)

def run_std_json_value_broad_surface() -> None:
  match JsonValue.parse('{"items":[1,2],"name":"Ada","n":null}'):
    case Ok(data):
      assert data.kind().as_str() == "object"
      assert JsonValue.str("Ada").as_str() == Some("Ada")
      match data.get("n"):
        case Some(value):
          assert value.is_null()
        case None:
          assert false
      match data.get("missing"):
        case Some(_):
          assert false
        case None:
          pass
      match data["items"]:
        case Some(items):
          match items[0]:
            case Some(first):
              match first.expect_int():
                case Ok(n):
                  assert n == 1
                case Err(_):
                  assert false
            case None:
              assert false
          match items[-1]:
            case Some(_):
              assert false
            case None:
              pass
        case None:
          assert false
      mut target = JsonValue.object({"a": JsonValue.int(1)})
      match target.merge(JsonValue.object({"a": JsonValue.int(2), "b": JsonValue.str("bee")})):
        case Ok(_):
          assert target.contains_key("b")
          match target.require("a"):
            case Ok(value):
              assert value.as_int() == Some(2)
            case Err(_):
              assert false
        case Err(_):
          assert false
    case Err(err):
      println(err.message())
      assert false

def run_frozen_float_helpers() -> None:
  println(min(NUMBERS))
  println(max(NUMBERS))

def main() -> None:
  run_explicit_serialize_trait()
  run_generated_runtime_helpers()
  run_std_json_deserialize()
  run_direct_deserialize_derive()
  run_json_value_model_field_roundtrip()
  run_std_json_value_broad_surface()
  run_frozen_float_helpers()
"#,
        )?;

        let output = super::incan_command()
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected std/json and generated runtime surface batch to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            vec![
                "{\"value\":1}",
                "4",
                "1",
                "3",
                "{\"value\":2}",
                "{\"value\":7,\"label\":\"dogfood\"}",
                "7",
                "{\"name\":\"Ada\",\"first\":1,\"missing\":null}",
                "1.5",
                "4.25",
            ],
            "expected std/json and generated runtime surface transcript, got:\n{stdout}"
        );
        Ok(())
    }

    fn write_pub_boundary_type_fidelity_library(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let producer_root = root.join("pub_boundary_library");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"pub_boundary_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/dataset.incn"),
            r#"pub model SessionError:
  kind: str

pub trait DataSet[T]:
  def to_substrait_plan(self) -> int: ...

pub trait BoundedDataSet[T] with DataSet[T]:
  pass

@derive(Clone)
pub class DataFrame[T] with BoundedDataSet:
  pub _type_witness: list[T]

  def to_substrait_plan(self) -> int:
    return 1

@derive(Clone)
pub class LazyFrame[T] with BoundedDataSet:
  pub _type_witness: list[T]

  def to_substrait_plan(self) -> int:
    return 1

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Ok(DataFrame[T](_type_witness=[]))
"#,
        )?;
        std::fs::write(
            producer_root.join("src/functions.incn"),
            r#"from dataset import DataSet

pub def display[T](data: DataSet[T]) -> None:
  print(data.to_substrait_plan())
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from dataset import SessionError, DataSet, BoundedDataSet, DataFrame, LazyFrame\npub from functions import display\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected pub-boundary library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        Ok(())
    }

    fn write_minimal_library_crate(artifact_root: &Path, package_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(artifact_root.join("src"))?;
        std::fs::write(
            artifact_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
            ),
        )?;
        std::fs::write(artifact_root.join("src/lib.rs"), "pub fn linked() {}\n")?;
        Ok(())
    }

    fn write_library_crate_with_source(
        artifact_root: &Path,
        package_name: &str,
        lib_source: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(artifact_root.join("src"))?;
        std::fs::write(
            artifact_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n"
            ),
        )?;
        std::fs::write(artifact_root.join("src/lib.rs"), lib_source)?;
        Ok(())
    }

    fn write_vocab_companion_crate(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("crates")
                    .join("incan_vocab")
                    .display()
            ),
        )?;
        std::fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"await\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: vec![\"route\".to_string()],\n        },\n    )\n}\n",
        )?;
        Ok(())
    }

    fn write_vocab_companion_crate_with_assert_keyword(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("crates")
                    .join("incan_vocab")
                    .display()
            ),
        )?;
        std::fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"assert\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: vec![\"route\".to_string()],\n        },\n    )\n}\n",
        )?;
        Ok(())
    }

    fn write_vocab_companion_crate_with_source(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
        lib_source: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("crates")
                    .join("incan_vocab")
                    .display()
            ),
        )?;
        std::fs::write(crate_root.join("src/lib.rs"), lib_source)?;
        Ok(())
    }

    fn wat_bytes_string(bytes: &[u8]) -> String {
        let mut escaped = String::new();
        for byte in bytes {
            escaped.push('\\');
            escaped.push_str(&format!("{byte:02x}"));
        }
        escaped
    }

    fn wat_data_string(text: &str) -> String {
        wat_bytes_string(text.as_bytes())
    }

    fn wat_i32_cell(value: i32) -> String {
        wat_bytes_string(&value.to_le_bytes())
    }

    fn compile_desugarer_wasm(
        status_code: i32,
        output_payload: &str,
        error_payload: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output_ptr_cell = 0usize;
        let output_len_cell = 4usize;
        let error_ptr_cell = 8usize;
        let error_len_cell = 12usize;
        let input_ptr_cell = 16usize;
        let input_capacity_cell = 20usize;
        let input_len_cell = 24usize;
        let output_offset = 128usize;
        let output_len = output_payload.len();
        let error_offset = output_offset + output_len + 32;
        let input_offset = error_offset + error_payload.len() + 32;
        let input_capacity = 4096usize;
        let wat_source = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    (i32.const {status_code})
  )
)"#,
            output_ptr_cell = output_ptr_cell,
            output_len_cell = output_len_cell,
            error_ptr_cell = error_ptr_cell,
            error_len_cell = error_len_cell,
            input_ptr_cell = input_ptr_cell,
            input_capacity_cell = input_capacity_cell,
            input_len_cell = input_len_cell,
            output_ptr_data = wat_i32_cell(output_offset as i32),
            output_len_data = wat_i32_cell(output_payload.len() as i32),
            error_ptr_data = wat_i32_cell(error_offset as i32),
            error_len_data = wat_i32_cell(error_payload.len() as i32),
            input_ptr_data = wat_i32_cell(input_offset as i32),
            input_capacity_data = wat_i32_cell(input_capacity as i32),
            input_len_data = wat_i32_cell(0),
            output_data = wat_data_string(output_payload),
            error_data = wat_data_string(error_payload),
        );
        Ok(wat::parse_str(wat_source)?)
    }

    fn compile_desugarer_wasm_requiring_request(
        output_payload: &str,
        error_payload: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output_ptr_cell = 0usize;
        let output_len_cell = 4usize;
        let error_ptr_cell = 8usize;
        let error_len_cell = 12usize;
        let input_ptr_cell = 16usize;
        let input_capacity_cell = 20usize;
        let input_len_cell = 24usize;
        let output_offset = 128usize;
        let output_len = output_payload.len();
        let error_offset = output_offset + output_len + 32;
        let input_offset = error_offset + error_payload.len() + 32;
        let input_capacity = 4096usize;
        let wat_source = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (func (export "__incan_init_desugarer"))
  (func (export "desugar_block") (result i32)
    global.get $input_len_cell
    i32.load
    i32.eqz
    if (result i32)
      (i32.const 1)
    else
      global.get $input_ptr_cell
      i32.load
      i32.load8_u
      i32.const 123
      i32.eq
      if (result i32)
        (i32.const 0)
      else
        (i32.const 1)
      end
    end
  )
)"#,
            output_ptr_cell = output_ptr_cell,
            output_len_cell = output_len_cell,
            error_ptr_cell = error_ptr_cell,
            error_len_cell = error_len_cell,
            input_ptr_cell = input_ptr_cell,
            input_capacity_cell = input_capacity_cell,
            input_len_cell = input_len_cell,
            output_ptr_data = wat_i32_cell(output_offset as i32),
            output_len_data = wat_i32_cell(output_payload.len() as i32),
            error_ptr_data = wat_i32_cell(error_offset as i32),
            error_len_data = wat_i32_cell(error_payload.len() as i32),
            input_ptr_data = wat_i32_cell(input_offset as i32),
            input_capacity_data = wat_i32_cell(input_capacity as i32),
            input_len_data = wat_i32_cell(0),
            output_data = wat_data_string(output_payload),
            error_data = wat_data_string(error_payload),
        );
        Ok(wat::parse_str(wat_source)?)
    }

    fn compile_desugarer_wasm_requiring_request_substring(
        output_payload: &str,
        error_payload: &str,
        needle: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let output_ptr_cell = 0usize;
        let output_len_cell = 4usize;
        let error_ptr_cell = 8usize;
        let error_len_cell = 12usize;
        let input_ptr_cell = 16usize;
        let input_capacity_cell = 20usize;
        let input_len_cell = 24usize;
        let output_offset = 128usize;
        let output_len = output_payload.len();
        let error_offset = output_offset + output_len + 32;
        let input_offset = error_offset + error_payload.len() + 32;
        let input_capacity = 16_384usize;
        let needle_offset = input_offset + input_capacity + 32;
        let needle_len = needle.len();
        let wat_source = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $input_ptr_cell (export "__incan_input_ptr") i32 (i32.const {input_ptr_cell}))
  (global (export "__incan_input_capacity") i32 (i32.const {input_capacity_cell}))
  (global $input_len_cell (export "__incan_input_len") i32 (i32.const {input_len_cell}))
  (global (export "__incan_output_ptr") i32 (i32.const {output_ptr_cell}))
  (global (export "__incan_output_len") i32 (i32.const {output_len_cell}))
  (global (export "__incan_error_ptr") i32 (i32.const {error_ptr_cell}))
  (global (export "__incan_error_len") i32 (i32.const {error_len_cell}))
  (data (i32.const {output_ptr_cell}) "{output_ptr_data}")
  (data (i32.const {output_len_cell}) "{output_len_data}")
  (data (i32.const {error_ptr_cell}) "{error_ptr_data}")
  (data (i32.const {error_len_cell}) "{error_len_data}")
  (data (i32.const {input_ptr_cell}) "{input_ptr_data}")
  (data (i32.const {input_capacity_cell}) "{input_capacity_data}")
  (data (i32.const {input_len_cell}) "{input_len_data}")
  (data (i32.const {output_offset}) "{output_data}")
  (data (i32.const {error_offset}) "{error_data}")
  (data (i32.const {needle_offset}) "{needle_data}")
  (func (export "__incan_init_desugarer"))
  (func $matches_at (param $pos i32) (result i32)
    (local $j i32)
    (block $fail
      (loop $scan
        local.get $j
        i32.const {needle_len}
        i32.ge_u
        if
          i32.const 1
          return
        end
        local.get $pos
        local.get $j
        i32.add
        i32.load8_u
        i32.const {needle_offset}
        local.get $j
        i32.add
        i32.load8_u
        i32.ne
        br_if $fail
        local.get $j
        i32.const 1
        i32.add
        local.set $j
        br $scan
      )
    )
    i32.const 0
  )
  (func (export "desugar_block") (result i32)
    (local $input_ptr i32)
    (local $input_len i32)
    (local $end i32)
    (local $i i32)
    global.get $input_ptr_cell
    i32.load
    local.set $input_ptr
    global.get $input_len_cell
    i32.load
    local.set $input_len
    local.get $input_len
    i32.const {needle_len}
    i32.lt_u
    if
      i32.const 1
      return
    end
    local.get $input_ptr
    local.get $input_len
    i32.add
    i32.const {needle_len}
    i32.sub
    i32.const 1
    i32.add
    local.set $end
    local.get $input_ptr
    local.set $i
    (block $not_found
      (loop $search
        local.get $i
        local.get $end
        i32.ge_u
        br_if $not_found
        local.get $i
        call $matches_at
        if
          i32.const 0
          return
        end
        local.get $i
        i32.const 1
        i32.add
        local.set $i
        br $search
      )
    )
    i32.const 1
  )
)"#,
            output_ptr_cell = output_ptr_cell,
            output_len_cell = output_len_cell,
            error_ptr_cell = error_ptr_cell,
            error_len_cell = error_len_cell,
            input_ptr_cell = input_ptr_cell,
            input_capacity_cell = input_capacity_cell,
            input_len_cell = input_len_cell,
            output_ptr_data = wat_i32_cell(output_offset as i32),
            output_len_data = wat_i32_cell(output_payload.len() as i32),
            error_ptr_data = wat_i32_cell(error_offset as i32),
            error_len_data = wat_i32_cell(error_payload.len() as i32),
            input_ptr_data = wat_i32_cell(input_offset as i32),
            input_capacity_data = wat_i32_cell(input_capacity as i32),
            input_len_data = wat_i32_cell(0),
            output_data = wat_data_string(output_payload),
            error_data = wat_data_string(error_payload),
            needle_offset = needle_offset,
            needle_len = needle_len,
            needle_data = wat_data_string(needle),
        );
        Ok(wat::parse_str(wat_source)?)
    }

    fn write_pub_library_with_vocab_desugarer(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        desugarer_bytes: &[u8],
        keyword: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, manifest_name)?;
        let desugarer_path = artifact_root.join("desugarers").join("routes_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: format!("{dependency_key}.dsl"),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: keyword.to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/routes_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn write_pub_library_with_querykit_surface_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "querykit.query".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "query".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: vec![
                incan_vocab::DslSurface::on_import("querykit.query")
                    .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                            .in_declaration_body("query")
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration),
                    ),
            ],
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    fn write_pub_library_with_querykit_select_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let metadata = incan_vocab::VocabRegistration::new()
            .with_surface(
                incan_vocab::DslSurface::on_import("querykit.query").with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clause(
                            incan_vocab::ClauseSurface::expr_list("SELECT")
                                .with_expression_item_modifiers([
                                    incan_vocab::ExpressionItemModifierSurface::expr("for"),
                                    incan_vocab::ExpressionItemModifierSurface::expr("with"),
                                ])
                                .required(),
                        ),
                ),
            )
            .metadata();
        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    fn write_pub_library_with_querykit_expression_clause_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let metadata = incan_vocab::VocabRegistration::new()
            .with_surface(
                incan_vocab::DslSurface::on_import("querykit.query").with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("GROUP BY").optional(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required(),
                            incan_vocab::ClauseSurface::expr_list("ORDER BY").optional(),
                            incan_vocab::ClauseSurface::nested_items("WINDOW BY").optional(),
                        ]),
                ),
            )
            .metadata();
        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    /// Write a package whose rich quality-clause ownership disagrees with legacy low-level keyword surface kinds.
    ///
    /// This preserves the compatibility shape used by existing vocab packages: parser activation still receives raw
    /// keyword registrations, while the richer `DslSurface` is the authority on which nested blocks are clauses.
    fn write_pub_library_with_mixed_quality_clause_desugarer(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join("querykit").join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_minimal_library_crate(&artifact_root, "querykit_core")?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let metadata = incan_vocab::VocabRegistration::new()
            .with_keyword_registration(incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "querykit.query".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec::block("quality"),
                    incan_vocab::KeywordSpec::block("FROM").in_block("quality"),
                    incan_vocab::KeywordSpec::block("REQUIRE").in_block("quality"),
                    incan_vocab::KeywordSpec::block("GROUP")
                        .with_compound_tokens(["BY"])
                        .in_block("quality"),
                    incan_vocab::KeywordSpec::block("EXPECT").in_block("quality"),
                ],
                valid_decorators: Vec::new(),
            })
            .with_surface(
                incan_vocab::DslSurface::on_import("querykit.query")
                    .with_declaration(
                        incan_vocab::DeclarationSurface::named("quality")
                            .with_mixed_body()
                            .desugars_to_expression()
                            .with_clauses([
                                incan_vocab::ClauseSurface::expr("FROM").optional(),
                                incan_vocab::ClauseSurface::expr_list("GROUP BY")
                                    .repeating()
                                    .after("FROM"),
                                incan_vocab::ClauseSurface::expr_list("EXPECT")
                                    .repeating()
                                    .after("FROM"),
                                incan_vocab::ClauseSurface::expr_list("REQUIRE")
                                    .repeating()
                                    .after("FROM"),
                            ]),
                    )
                    .with_scoped_surface(
                        incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("quality.group.field")
                            .in_clause_body("quality", "GROUP")
                            .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                    ),
            )
            .metadata();
        let mut manifest = LibraryManifest::new("querykit_core", "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join("querykit_core.incnlib"))?;
        Ok(())
    }

    fn write_pub_library_with_vocab_desugarer_and_filter_helper(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        desugarer_bytes: &[u8],
        keyword: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        write_pub_library_with_vocab_desugarer_and_filter_helper_keywords(
            root,
            dependency_key,
            manifest_name,
            desugarer_bytes,
            &[keyword],
        )
    }

    fn write_pub_library_with_vocab_desugarer_and_filter_helper_keywords(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        desugarer_bytes: &[u8],
        keywords: &[&str],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_library_crate_with_source(
            &artifact_root,
            manifest_name,
            "pub fn filter(value: i64) -> i64 {\n    value\n}\n",
        )?;
        let desugarer_path = artifact_root.join("desugarers").join("incql_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.exports.functions.push(FunctionExport {
            name: "filter".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: incan::library_manifest::ParamKindExport::Normal,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named {
                name: "int".to_string(),
            },
            is_async: false,
        });
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: format!("{dependency_key}.dsl"),
                },
                keywords: keywords
                    .iter()
                    .map(|keyword| incan_vocab::KeywordSpec {
                        name: (*keyword).to_string(),
                        surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                        compound_tokens: Vec::new(),
                        placement: incan_vocab::KeywordPlacement::TopLevel,
                    })
                    .collect(),
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                helper_bindings: vec![incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "filter".to_string(),
                }],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/incql_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn write_pub_library_with_vocab_desugarer_and_string_helper(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dependency_key = "helperkit";
        let manifest_name = "helperkit_core";
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");

        // ---- Context: helperkit Rust artifact and desugarer asset ----
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        write_library_crate_with_source(
            &artifact_root,
            manifest_name,
            "pub fn lit(value: i64) -> i64 {\n    value\n}\n\npub fn aggregate_as(_value: i64, label: String) -> String {\n    label\n}\n",
        )?;
        let desugarer_path = artifact_root.join("desugarers").join("helperkit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        // ---- Context: public helper manifest surface ----
        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.exports.functions.push(FunctionExport {
            name: "lit".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: incan::library_manifest::ParamKindExport::Normal,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named {
                name: "int".to_string(),
            },
            is_async: false,
        });
        manifest.exports.functions.push(FunctionExport {
            name: "aggregate_as".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: vec![
                ParamExport {
                    name: "value".to_string(),
                    ty: TypeRef::Named {
                        name: "int".to_string(),
                    },
                    kind: incan::library_manifest::ParamKindExport::Normal,
                    has_default: false,
                    default: None,
                },
                ParamExport {
                    name: "label".to_string(),
                    ty: TypeRef::Named {
                        name: "str".to_string(),
                    },
                    kind: incan::library_manifest::ParamKindExport::Normal,
                    has_default: false,
                    default: None,
                },
            ],
            return_type: TypeRef::Named {
                name: "str".to_string(),
            },
            is_async: false,
        });

        // ---- Context: vocab activation and helper bindings ----
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "helperkit.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "where".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                helper_bindings: vec![
                    incan_vocab::HelperBinding {
                        key: "lit".to_string(),
                        exported_name: "lit".to_string(),
                    },
                    incan_vocab::HelperBinding {
                        key: "aggregate_as".to_string(),
                        exported_name: "aggregate_as".to_string(),
                    },
                ],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/helperkit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn write_source_pub_library_with_vocab_desugarer_and_query_helpers(
        root: &Path,
        desugarer_bytes: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let producer_root = root.join("deps").join("querykit");

        // ---- Context: source-backed helper library ----
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/helpers.incn"),
            r#"pub model IntLiteralExpr:
  value: int

pub model StringLiteralExpr:
  value: str

pub type LiteralValue = Union[int, str]
pub type ColumnExpr = Union[IntLiteralExpr, StringLiteralExpr]

pub model AggregateMeasure:
  expr: ColumnExpr
  label: str

pub const DEFAULT_LABEL: str = "orders"
pub const COUNT_SENTINEL: str = "__querykit_count_no_argument__"

pub def lit(value: LiteralValue) -> ColumnExpr:
  match value:
    int(number) => return IntLiteralExpr(value=number)
    str(text) => return StringLiteralExpr(value=text)

pub def col(name: str) -> ColumnExpr:
  return StringLiteralExpr(value=name)

pub def count(expr: ColumnExpr = col(COUNT_SENTINEL)) -> ColumnExpr:
  return expr

pub def aggregate_as(expr: ColumnExpr, output_name: str) -> AggregateMeasure:
  return AggregateMeasure(expr=expr, label=output_name)

pub def aggregate_default(expr: ColumnExpr, output_name: str = DEFAULT_LABEL) -> AggregateMeasure:
  return AggregateMeasure(expr=expr, label=output_name)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from helpers import IntLiteralExpr, StringLiteralExpr, LiteralValue, ColumnExpr, AggregateMeasure, DEFAULT_LABEL, lit, count, aggregate_as, aggregate_default\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected querykit producer build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        // ---- Context: vocab activation attached to the built library manifest ----
        let artifact_root = producer_root.join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("desugarers"))?;
        let desugarer_path = artifact_root.join("desugarers").join("querykit_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;
        let manifest_path = artifact_root.join("querykit.incnlib");
        let mut manifest = LibraryManifest::read_from_path(&manifest_path)?;
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "querykit.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "where".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                helper_bindings: vec![
                    incan_vocab::HelperBinding {
                        key: "lit".to_string(),
                        exported_name: "lit".to_string(),
                    },
                    incan_vocab::HelperBinding {
                        key: "count".to_string(),
                        exported_name: "count".to_string(),
                    },
                    incan_vocab::HelperBinding {
                        key: "aggregate_as".to_string(),
                        exported_name: "aggregate_as".to_string(),
                    },
                ],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: Some(incan::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/querykit_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&manifest_path)?;
        Ok(())
    }

    fn write_pub_library_with_provider_requirements(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        required_dependencies: Vec<incan_vocab::CargoDependency>,
        required_stdlib_features: Vec<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("src"))?;
        write_minimal_library_crate(&artifact_root, manifest_name)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: format!("{dependency_key}_vocab_companion"),
            package_name: format!("{dependency_key}_vocab_companion"),
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies,
                required_stdlib_features: required_stdlib_features
                    .into_iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn write_pub_library_with_provider_requirements_and_assert_keyword(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        required_dependencies: Vec<incan_vocab::CargoDependency>,
        required_stdlib_features: Vec<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("src"))?;
        write_minimal_library_crate(&artifact_root, manifest_name)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.vocab = Some(incan::library_manifest::VocabExports {
            crate_path: format!("{dependency_key}_vocab_companion"),
            package_name: format!("{dependency_key}_vocab_companion"),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: format!("{dependency_key}.dsl"),
                },
                keywords: vec![incan_vocab::KeywordSpec::new(
                    "assert",
                    incan_vocab::KeywordSurfaceKind::ControlFlow,
                )],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies,
                required_stdlib_features: required_stdlib_features
                    .into_iter()
                    .map(std::string::ToString::to_string)
                    .collect(),
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    fn mylib_manifest_with_widget() -> LibraryManifest {
        let mut manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
        });
        manifest
    }

    #[test]
    fn check_reports_unknown_pub_library() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n",
            "from pub::missinglib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for unknown library, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Unknown `pub::` library `missinglib`"),
            "expected unknown-library diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_missing_pub_export() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        write_minimal_library_crate(
            dep_manifest_path.parent().ok_or("missing dependency artifact root")?,
            "mylib",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import MissingSymbol\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for missing export, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("is not exported by `pub::mylib`"),
            "expected missing-export diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_pub_manifest_load_failure() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        std::fs::write(&dep_manifest_path, "{ not-json }\n")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for manifest load failure, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Failed to load manifest for `pub::mylib`"),
            "expected manifest-load diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_passes_for_pub_imported_manifest_type() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        write_minimal_library_crate(
            dep_manifest_path.parent().ok_or("missing dependency artifact root")?,
            "mylib",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n\ndef build(x: Widget) -> Widget:\n  return x\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass for valid pub import, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn check_reports_missing_pub_library_artifacts() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_manifest_path = tmp
            .path()
            .join("deps")
            .join("mylib")
            .join("target")
            .join("lib")
            .join("mylib.incnlib");
        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dependency manifest parent")?)?;
        mylib_manifest_with_widget().write_to_path(&dep_manifest_path)?;
        // Intentionally do not write Cargo.toml / src/lib.rs to exercise artifact-contract diagnostics.

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nmylib = { path = \"deps/mylib\" }\n",
            "from pub::mylib import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for missing crate artifacts, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Missing generated crate artifacts for `pub::mylib`"),
            "expected missing-artifact diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn check_reports_pub_library_artifact_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dep_artifact_root = tmp.path().join("deps").join("widgets-lib").join("target").join("lib");
        std::fs::create_dir_all(&dep_artifact_root)?;
        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.exports.models.push(ModelExport {
            name: "Widget".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
        });
        manifest.write_to_path(&dep_artifact_root.join("widgets_core.incnlib"))?;
        write_minimal_library_crate(&dep_artifact_root, "different_package_name")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"app\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets-lib\" }\n",
            "from pub::widgets import Widget\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail for artifact mismatch, stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("Generated crate metadata mismatch for `pub::widgets`"),
            "expected artifact mismatch diagnostic, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn build_lib_artifacts_and_consumer_alias_typecheck() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_core_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/widgets.incn"),
            "pub model Widget:\n  name: str\n\npub def make_widget(name: str) -> Widget:\n  return Widget(name=name)\n",
        )?;
        std::fs::write(
            producer_root.join("src/boxmod.incn"),
            "pub class Box:\n  def get[T with Clone](self, value: T) -> T:\n    return value\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from boxmod import Box\npub from widgets import Widget, make_widget\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let producer_artifact_root = producer_root.join("target").join("lib");
        assert!(producer_artifact_root.join("Cargo.toml").is_file());
        assert!(producer_artifact_root.join("src/lib.rs").is_file());
        assert!(producer_artifact_root.join("widgets_core.incnlib").is_file());

        let consumer_root = tmp.path().join("consumer_app");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_core_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::widgets import Box, Widget as PublicWidget, make_widget\n\ndef main() -> None:\n  w: PublicWidget = make_widget(\"ok\")\n  box: Box = Box()\n  value: int = box.get(1)\n  print(w.name)\n  print(value)\n",
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer check to accept pub:: alias and generic carrier imports.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );

        Ok(())
    }

    /// Regression for #892: split aliases keep their provider identity in real library consumers and test batches.
    #[test]
    fn build_lib_consumer_preserves_split_pub_import_alias_identity_issue892() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let provider_root = tmp.path().join("proposal_provider");
        std::fs::create_dir_all(provider_root.join("src"))?;
        std::fs::write(
            provider_root.join("incan.toml"),
            "[project]\nname = \"proposal_provider\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            provider_root.join("src/proposals.incn"),
            r#"pub model ConsoleProposal:
  pub title: str

pub def make_console_proposal(title: str) -> ConsoleProposal:
  return ConsoleProposal(title=title)
"#,
        )?;
        std::fs::write(
            provider_root.join("src/lib.incn"),
            "pub from proposals import ConsoleProposal, make_console_proposal\n",
        )?;

        let provider_build = run_build_lib(&provider_root)?;
        assert!(
            provider_build.status.success(),
            "expected #892 provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&provider_build.stdout),
            String::from_utf8_lossy(&provider_build.stderr)
        );
        assert!(
            provider_root.join("target/lib/proposal_provider.incnlib").is_file(),
            "expected the provider's compiled .incnlib manifest"
        );

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::create_dir_all(consumer_root.join("tests"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nproposals = { path = \"../proposal_provider\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from pub::proposals import ConsoleProposal as ProviderConsoleProposal
from pub::proposals import make_console_proposal

def proposal() -> ProviderConsoleProposal:
  return make_console_proposal("ready")

def main() -> None:
  println(proposal().title)
"#,
        )?;

        let check_output = run_check(&main_path)?;
        assert!(
            check_output.status.success(),
            "expected split pub import aliases to typecheck against the compiled library.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let out_dir = consumer_root.join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected generated Rust for split pub import aliases to compile.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated_main.contains("proposals::ConsoleProposal"),
            "expected generated Rust to retain the provider-qualified model path.\ngenerated main.rs:\n{generated_main}"
        );

        std::fs::write(
            consumer_root.join("tests/test_split_alias.incn"),
            r#"from pub::proposals import ConsoleProposal as SplitConsoleProposal
from pub::proposals import make_console_proposal

def make_split() -> SplitConsoleProposal:
  return make_console_proposal("split")

def test_split_pub_import_alias() -> None:
  proposal: SplitConsoleProposal = make_split()
  assert proposal.title == "split"
"#,
        )?;
        std::fs::write(
            consumer_root.join("tests/test_same_statement_alias.incn"),
            r#"from pub::proposals import ConsoleProposal as SameStatementConsoleProposal, make_console_proposal

def make_same_statement() -> SameStatementConsoleProposal:
  return make_console_proposal("same")

def test_same_statement_pub_import_alias() -> None:
  proposal: SameStatementConsoleProposal = make_same_statement()
  assert proposal.title == "same"
"#,
        )?;

        let test_output = run_test(&consumer_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected split and same-statement aliases to coexist in one compiled test batch.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        let test_stdout = String::from_utf8_lossy(&test_output.stdout);
        assert!(
            test_stdout.contains("test_split_alias.incn::test_split_pub_import_alias")
                && test_stdout.contains("test_same_statement_alias.incn::test_same_statement_pub_import_alias"),
            "expected both #892 regression tests in the batch output.\nstdout:\n{test_stdout}"
        );

        Ok(())
    }

    #[test]
    fn build_lib_consumer_preserves_private_class_field_visibility_issue883() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("sealed_class_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"sealed_class_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/vaults.incn"),
            r#"const DEFAULT_PRIVATE_TEXT: str = "authority"

def default_label() -> str:
  return "sealed"

pub class VaultBase:
  private_text: str = DEFAULT_PRIVATE_TEXT
  computed_secret: int = 1 + 2
  pub base_count: int

pub class Vault extends VaultBase:
  secret: str
  pub label: str = default_label()

  def reveal(self) -> str:
    return self.secret

pub def make_vault(secret: str) -> Vault:
  return Vault(base_count=7, secret=secret)
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from crate.vaults import Vault as PublicVault, VaultBase, make_vault\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected private-field provider library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let provider_manifest =
            LibraryManifest::read_from_path(&producer_root.join("target/lib/sealed_class_lib.incnlib"))?;
        let vault = provider_manifest
            .contract_metadata
            .api
            .as_ref()
            .and_then(|api| api.modules.iter().find(|module| module.module_path == ["vaults"]))
            .and_then(|module| {
                module.declarations.iter().find_map(|declaration| match declaration {
                    incan::frontend::api_metadata::ApiDeclaration::Class(class) if class.name == "Vault" => Some(class),
                    _ => None,
                })
            })
            .ok_or("expected facade-backed Vault in checked API metadata")?;
        assert_eq!(
            vault.fields.iter().map(|field| field.name.as_str()).collect::<Vec<_>>(),
            vec!["private_text", "computed_secret", "base_count", "secret", "label"],
            "manifest constructor fields must retain the provider's parent-first ABI order"
        );
        let private_text = vault
            .fields
            .iter()
            .find(|field| field.name == "private_text")
            .ok_or("expected inherited private_text field in provider manifest")?;
        let secret = vault
            .fields
            .iter()
            .find(|field| field.name == "secret")
            .ok_or("expected secret field in provider manifest")?;
        let computed_secret = vault
            .fields
            .iter()
            .find(|field| field.name == "computed_secret")
            .ok_or("expected computed_secret field in provider manifest")?;
        let label = vault
            .fields
            .iter()
            .find(|field| field.name == "label")
            .ok_or("expected label field in provider manifest")?;
        assert_eq!(secret.visibility, FieldVisibilityExport::Private);
        assert_eq!(label.visibility, FieldVisibilityExport::Public);
        assert_eq!(private_text.visibility, FieldVisibilityExport::Private);
        assert_eq!(computed_secret.visibility, FieldVisibilityExport::Private);
        assert!(computed_secret.has_default);
        assert_eq!(computed_secret.default, None);
        assert!(private_text.has_default);
        assert_eq!(
            private_text.default,
            Some(incan::library_manifest::ParamDefaultExport::ConstRef(vec![
                "vaults".to_string(),
                "DEFAULT_PRIVATE_TEXT".to_string(),
            ]))
        );
        assert!(label.has_default);
        assert!(matches!(
            label.default,
            Some(incan::library_manifest::ParamDefaultExport::Call {
                ref path,
                ref signature,
                ..
            }) if path == &["vaults".to_string(), "default_label".to_string()] && signature.is_some()
        ));

        let decoy_root = tmp.path().join("decoy_class_lib");
        std::fs::create_dir_all(decoy_root.join("src"))?;
        std::fs::write(
            decoy_root.join("incan.toml"),
            "[project]\nname = \"decoy_class_lib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            decoy_root.join("src/lib.incn"),
            r#"pub class PublicVault:
  private_text: int = 11
  computed_secret: str = "wrong" + "provider"
  pub base_count: str
  secret: int
  pub label: int = 99
"#,
        )?;
        let decoy_build = run_build_lib(&decoy_root)?;
        assert!(
            decoy_build.status.success(),
            "expected duplicate-short-name decoy library build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&decoy_build.stdout),
            String::from_utf8_lossy(&decoy_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"sealed_class_consumer\"\n\n[dependencies]\nsealed_class_lib = { path = \"../sealed_class_lib\" }\ndecoy_class_lib = { path = \"../decoy_class_lib\" }\n",
            r#"from pub::sealed_class_lib import PublicVault

def main() -> None:
  value: PublicVault = PublicVault(base_count=9, secret="authority")
  overridden: PublicVault = PublicVault(computed_secret=4, base_count=10, secret="override")
  println(value.label)
  println(value.base_count)
  println(overridden.base_count)
"#,
        )?;

        let public_check = run_check(&consumer_main)?;
        assert!(
            public_check.status.success(),
            "expected public field access and existing named class construction to remain valid.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&public_check.stdout),
            String::from_utf8_lossy(&public_check.stderr)
        );
        let public_build = run_build(&consumer_main, &consumer_root.join("out"))?;
        assert!(
            public_build.status.success(),
            "expected public access and named construction to generate valid Rust.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&public_build.stdout),
            String::from_utf8_lossy(&public_build.stderr)
        );

        std::fs::write(
            &consumer_main,
            r#"from pub::sealed_class_lib import PublicVault as ConsumerVault

def main() -> None:
  value: ConsumerVault = ConsumerVault(base_count=9, secret="authority")
  println(value.label)
"#,
        )?;
        let alias_build = run_build(&consumer_main, &consumer_root.join("out"))?;
        assert!(
            alias_build.status.success(),
            "expected a consumer alias of a facade-renamed class to retain the exact provider bridge.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&alias_build.stdout),
            String::from_utf8_lossy(&alias_build.stderr)
        );

        std::fs::write(
            &consumer_main,
            r#"from pub::sealed_class_lib import PublicVault as ConsumerVault

def main() -> None:
  value: ConsumerVault = ConsumerVault(base_count=7, secret="authority")
  println(value.secret)
"#,
        )?;
        let private_check = run_check(&consumer_main)?;
        assert!(
            !private_check.status.success(),
            "expected compiled-library private field access to fail Incan typechecking"
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&private_check.stderr));
        assert!(
            stderr.contains("Field 'secret' on 'ConsumerVault' is private"),
            "expected source-level private field diagnostic, got:\n{stderr}"
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_pub_import_regression_batch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("pub_import_regression_batch_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"pub_import_regression_batch\"\nversion = \"0.1.0\"\n",
        )?;

        let files = [
            (
                "src/session/types.incn",
                r#"pub class Session:
  pub id: int
"#,
            ),
            ("src/session/mod.incn", "pub from crate.session.types import Session\n"),
            (
                "src/session_facade_case.incn",
                r#"from session import Session

pub def run_session_facade() -> None:
  s = Session(id=1)
  print(s.id)
"#,
            ),
            (
                "src/imported_enum_loop_rels.incn",
                r#"@derive(Clone)
pub enum ConformanceRel:
  Read
  Filter
"#,
            ),
            (
                "src/imported_enum_loop_case.incn",
                r#"from imported_enum_loop_rels import ConformanceRel

def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    ConformanceRel.Read =>
      return "ReadRel"
    _ =>
      return "Other"

def scenario_matches(required: list[ConformanceRel]) -> bool:
  for expected in required:
    if expected == ConformanceRel.Read:
      if relation_kind_name_from_conformance(expected) == "ReadRel":
        return true
  return false

pub def run_imported_enum_loop() -> None:
  println(scenario_matches([ConformanceRel.Read]))
"#,
            ),
            (
                "src/len_comparison_recursive_case.incn",
                r#"@derive(Clone)
pub enum ExprKind:
  Column
  Add

@derive(Clone)
pub model Expr:
  pub kind: ExprKind
  pub column_name: str
  pub arguments: list[Expr]

pub def lower(expr: Expr) -> int:
  if expr.kind == ExprKind.Column:
    return 0
  if len(expr.arguments) < 2:
    return -1
  return 1

pub def run_len_comparison_recursive() -> None:
  println(lower(Expr(kind=ExprKind.Add, column_name="root", arguments=[])))
"#,
            ),
            (
                "src/loop_helper_shared_string_list_case.incn",
                r#"def match_index(xs: list[str], y: int) -> int:
  mut idx = 0
  while idx < len(xs):
    if len(xs[idx]) == y:
      return idx
    idx = idx + 1
  return -1

def helper_loop(xs: list[str], ys: list[int]) -> list[int]:
  mut out: list[int] = []
  for y in ys:
    out.append(match_index(xs, y))
  return out

pub def run_loop_helper_shared_string_list() -> None:
  helper_loop(["a", "bb", "ccc"], [1, 2])
"#,
            ),
            (
                "src/dict_comp_reuses_noncopy_key_case.incn",
                r#"def lengths(names: list[str]) -> dict[str, int]:
  return {name: len(name) for name in names}

pub def run_dict_comp_reuses_noncopy_key() -> None:
  values = lengths(["alice", "bob"])
  println(values["alice"])
"#,
            ),
            (
                "src/tuple_unpack_enumerate_cases.incn",
                r#"model Binding:
  name: str
  output_index: int
  expr_index: int

def field_ref(index: int) -> int:
  return index

def bind_loop(xs: list[str]) -> list[Binding]:
  mut out: list[Binding] = []
  for idx, name in enumerate(xs):
    out.append(Binding(name=name, output_index=idx, expr_index=field_ref(idx)))
  return out

def bind_comp(xs: list[str]) -> list[Binding]:
  return [Binding(name=name, output_index=idx, expr_index=field_ref(idx)) for idx, name in enumerate(xs)]

pub def run_tuple_unpack_enumerate_cases() -> None:
  bind_loop(["a", "bb"])
  bind_comp(["a", "bb"])
"#,
            ),
            (
                "src/list_str_append_literal_case.incn",
                r#"pub def columns(input_columns: list[str]) -> list[str]:
  mut columns: list[str] = []
  columns.append(input_columns[0])
  columns.append("count")
  return columns

pub def run_list_str_append_literal() -> None:
  columns(["orders_total"])
"#,
            ),
            (
                "src/imported_sum_functions.incn",
                r#"pub model ColumnRef:
  pub name: str

pub model AggregateMeasure:
  pub column_name: str

pub def col(name: str) -> ColumnRef:
  return ColumnRef(name=name)

pub def sum(expr: ColumnRef) -> AggregateMeasure:
  return AggregateMeasure(column_name=expr.name)
"#,
            ),
            (
                "src/imported_sum_shadow_case.incn",
                r#"from imported_sum_functions import col, sum

def selected_column_name() -> str:
  amount = col("amount")
  result = sum(amount)
  return result.column_name

pub def run_imported_sum_shadow() -> None:
  println(selected_column_name())
"#,
            ),
            (
                "src/cross_module_union_producers.incn",
                r#"pub def parse_value(flag: bool) -> int | str:
  if flag:
    return 1
  return "fallback"
"#,
            ),
            (
                "src/cross_module_union_consumers.incn",
                r#"pub def describe(value: int | str) -> str:
  if isinstance(value, int):
    return "number"
  else:
    return value.upper()
"#,
            ),
            (
                "src/cross_module_union_case.incn",
                r#"from cross_module_union_producers import parse_value
from cross_module_union_consumers import describe

pub def run_cross_module_union() -> None:
  println(describe(parse_value(False)))
  println(describe("literal"))
"#,
            ),
            (
                "src/qualified_enum_constructor_match_case.incn",
                r#"pub enum QualifiedConformanceRel:
  Read
  Filter
  Project

pub def relation_kind_name_from_conformance(rel: QualifiedConformanceRel) -> str:
  match rel:
    QualifiedConformanceRel.Read =>
      return "ReadRel"
    QualifiedConformanceRel.Filter =>
      return "FilterRel"
    QualifiedConformanceRel.Project =>
      return "ProjectRel"
    _ =>
      return "UnknownRel"

pub def run_qualified_enum_constructor_match() -> None:
  println(relation_kind_name_from_conformance(QualifiedConformanceRel.Filter))
"#,
            ),
            (
                "src/main.incn",
                r#"from cross_module_union_case import run_cross_module_union
from dict_comp_reuses_noncopy_key_case import run_dict_comp_reuses_noncopy_key
from imported_enum_loop_case import run_imported_enum_loop
from imported_sum_shadow_case import run_imported_sum_shadow
from len_comparison_recursive_case import run_len_comparison_recursive
from list_str_append_literal_case import run_list_str_append_literal
from loop_helper_shared_string_list_case import run_loop_helper_shared_string_list
from qualified_enum_constructor_match_case import run_qualified_enum_constructor_match
from session_facade_case import run_session_facade
from tuple_unpack_enumerate_cases import run_tuple_unpack_enumerate_cases

def main() -> None:
  run_session_facade()
  run_imported_enum_loop()
  run_len_comparison_recursive()
  run_loop_helper_shared_string_list()
  run_dict_comp_reuses_noncopy_key()
  run_tuple_unpack_enumerate_cases()
  run_list_str_append_literal()
  run_imported_sum_shadow()
  run_cross_module_union()
  run_qualified_enum_constructor_match()
"#,
            ),
        ];

        for (relative, source) in files {
            let path = project_root.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, source)?;
        }

        let main_path = project_root.join("src/main.incn");
        let build_output = run_build(&main_path, &project_root.join("out"))?;
        assert!(
            build_output.status.success(),
            "expected pub import regression batch project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_and_run_iterator_comprehension_and_if_let_scenarios() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"iterator_comprehension_if_let_batch\"\nversion = \"0.1.0\"\n",
            "def is_even(n: int) -> bool:\n  return n % 2 == 0\n\n\
def double(n: int) -> int:\n  return n * 2\n\n\
def maybe_double(opt: Option[int]) -> int:\n  if let Some(value) = opt:\n    return value * 2\n  return 0\n\n\
def next_value(values: list[Option[int]], idx: int) -> Option[int]:\n  if idx < len(values):\n    return values[idx]\n  return None\n\n\
def sum_values(values: list[Option[int]]) -> int:\n  mut idx = 0\n  mut total = 0\n  while let Some(value) = next_value(values, idx):\n    total = total + value\n    idx = idx + 1\n  return total\n\n\
def main() -> None:\n  xs = [1, 2, 3, 4, 5]\n  ys = xs.iter().filter(is_even).map(double).take(2).collect()\n  batches = xs.iter().batch(2).collect()\n  println(len(ys))\n  println(ys[0])\n  println(len(batches))\n  comp_source = [1, 2, 3]\n  comp = [n * 2 for n in comp_source if n > 1]\n  println(len(comp))\n  println(comp[0])\n  println(len(comp_source))\n  println(maybe_double(Some(21)))\n  println(maybe_double(None))\n  println(sum_values([Some(1), Some(2), None, Some(99)]))\n",
        )?;

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected iterator/comprehension/if-let batch to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let run_output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected iterator/comprehension/if-let batch to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(
            stdout.lines().collect::<Vec<_>>(),
            vec!["2", "4", "3", "2", "4", "3", "42", "0", "3"]
        );

        Ok(())
    }

    #[test]
    fn build_lib_with_vocab_companion_embeds_vocab_payload() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate(&producer_root, "vocab_companion", "widgets_vocab_companion")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let manifest_path = producer_root.join("target").join("lib").join("widgets_core.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let vocab = manifest.vocab.as_ref().ok_or("expected vocab payload in .incnlib")?;
        assert_eq!(vocab.crate_path, "vocab_companion");
        assert_eq!(vocab.package_name, "widgets_vocab_companion");
        assert_eq!(vocab.keyword_registrations.len(), 1);
        assert_eq!(
            manifest.soft_keywords.activations,
            vec![incan::library_manifest::SoftKeywordActivation {
                namespace: "widgets.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
        Ok(())
    }

    #[test]
    fn build_lib_preserves_ordinal_map_metadata_for_consumer_check() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("ordinal_keys_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"ordinal_keys_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/status.incn"),
            r#"import std.collections as collections
from std.collections import OrdinalKey as Key, OrdinalMap, OrdinalMapError

pub enum Status(str):
    Open = "open"
    Paid = "paid"
    Cancelled = "cancelled"


@derive(Clone, Eq)
pub trait StableKey with Key:
    def stable_marker(self) -> int: ...


@derive(Clone, Eq)
pub model SmallKey with StableKey:
    value: int

    @staticmethod
    def ordinal_encoding() -> str:
        return "ordinal-keys-core:small-key-v1"

    @staticmethod
    def from_ordinal_bytes(data: bytes) -> Result[Self, OrdinalMapError]:
        if len(data) != 1:
            return Err(OrdinalMapError.invalid_key_record("SmallKey requires one byte"))
        return Ok(SmallKey(value=int(data[0])))

    def ordinal_bytes(self) -> bytes:
        value: u8 = self.value.wrapping_resize()
        return [value]

    def ordinal_hash(self) -> int:
        return 10_000 + self.value

    def stable_marker(self) -> int:
        return self.value


pub def echo_key[T with Key](value: T) -> T:
    return value


pub def status_map_bytes() -> bytes:
    statuses: list[Status] = [Status.Open, Status.Paid, Status.Cancelled]
    match OrdinalMap.from_keys(statuses):
        Ok(columns) => return columns.to_bytes()
        Err(_) => return b""


pub def small_key_map_bytes() -> bytes:
    alpha = SmallKey(value=1)
    beta = SmallKey(value=2)
    gamma = SmallKey(value=3)
    match OrdinalMap.from_pairs([(alpha, 10), (beta, 20), (gamma, 30)]):
        Ok(columns) => return columns.to_bytes()
        Err(_) => return b""
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from status import SmallKey, StableKey as PublicStableKey, Status, echo_key, small_key_map_bytes, status_map_bytes\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        assert!(
            producer_root
                .join("target")
                .join("lib")
                .join("ordinal_keys_core.incnlib")
                .is_file()
        );
        let manifest = LibraryManifest::read_from_path(
            &producer_root
                .join("target")
                .join("lib")
                .join("ordinal_keys_core.incnlib"),
        )?;
        let stable_key = manifest
            .exports
            .traits
            .iter()
            .find(|trait_export| trait_export.name == "PublicStableKey")
            .ok_or("expected aliased StableKey export")?;
        assert_eq!(stable_key.source_name.as_deref(), Some("StableKey"));
        assert_eq!(stable_key.supertraits[0].name, "Key");
        assert_eq!(stable_key.supertraits[0].source_name.as_deref(), Some("OrdinalKey"));
        let status = manifest
            .exports
            .enums
            .iter()
            .find(|enum_export| enum_export.name == "Status")
            .ok_or("expected Status value enum export")?;
        assert_eq!(
            status.ordinal_type_identity.as_deref(),
            Some("ordinal_keys_core.Status")
        );

        let consumer_root = tmp.path().join("ordinal_keys_consumer");
        let consumer_name = unique_test_project_name("ordinal_keys_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            format!(
                "[project]\nname = \"{consumer_name}\"\n\n[dependencies]\nordinal_keys = {{ path = \"../ordinal_keys_lib\" }}\n"
            ),
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from std.collections import OrdinalMap, OrdinalMapError\nfrom pub::ordinal_keys import SmallKey, Status, echo_key, small_key_map_bytes, status_map_bytes\n\ndef run() -> Result[None, OrdinalMapError]:\n  probe = echo_key(\"probe\")\n  if len(probe) == 0:\n    print(probe)\n  status_map: OrdinalMap[Status] = OrdinalMap.from_bytes(status_map_bytes())?\n  small_key_map: OrdinalMap[SmallKey] = OrdinalMap.from_bytes(small_key_map_bytes())?\n  print(status_map.require(Status.Paid)?)\n  print(small_key_map.require(SmallKey(value=2))?)\n  return Ok(None)\n\ndef main() -> None:\n  match run():\n    Ok(_) => pass\n    Err(err) => print(err.message())\n",
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer check to accept imported OrdinalMap metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn build_lib_preserves_std_environ_typed_reads_for_consumers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("environ_types_provider");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"environ_types_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/types.incn"),
            r#"from std.traits.convert import TryFrom

pub model EnvToken with TryFrom[str]:
  value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if len(value) == 0:
      return Err("token must not be empty")
    return Ok(EnvToken(value=value))

pub model MultiToken with TryFrom[str], TryFrom[int]:
  value: str

  @classmethod
  def try_from(cls, value: str) for TryFrom[str] -> Result[Self, str]:
    if len(value) == 0:
      return Err("token must not be empty")
    return Ok(MultiToken(value=value))

  @classmethod
  def try_from(cls, value: int) for TryFrom[int] -> Result[Self, str]:
    return Ok(MultiToken(value=f"{value}"))

pub trait EnvReadable[T] with TryFrom[T]:
  def source_name(self) -> str: ...

pub model TraitToken with EnvReadable[str]:
  value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(TraitToken(value=value))

  def source_name(self) -> str:
    return "environment"

pub class EnvClass with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(EnvClass(value=value))

pub enum EnvMode with TryFrom[str]:
  Dev
  Prod

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if value == "prod":
      return Ok(EnvMode.Prod)
    return Ok(EnvMode.Dev)

pub type ExplicitLabel = newtype str with TryFrom[str]:
  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(ExplicitLabel(value))

pub type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

pub type WrappedPort = newtype Port
pub type Positive = newtype int[gt=0]
pub type PositiveBox = newtype Positive
pub type Ratio = newtype float[ge=0, le=1]
pub type Boxed[T] = newtype T
pub type ClonedBox[T with Clone] = newtype T

@no_implicit_coercion
pub type StrictPort = newtype int
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from types import Boxed as PublicBoxed, ClonedBox, EnvClass, EnvMode, EnvReadable, EnvToken, ExplicitLabel, MultiToken, Port, Port as PublicPort, Positive as PublicPositive, PositiveBox as PublicPositiveBox, Ratio, StrictPort, TraitToken, WrappedPort\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected std.environ type provider to build.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let provider_manifest = LibraryManifest::read_from_path(
            &producer_root
                .join("target")
                .join("lib")
                .join("environ_types_core.incnlib"),
        )?;
        let positive = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicPositive")
            .ok_or("missing aliased constrained newtype export")?;
        assert_eq!(positive.constraints.len(), 1);
        assert_eq!(positive.constraints[0].value, 0);
        let positive_box = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicPositiveBox")
            .ok_or("missing aliased composed newtype export")?;
        assert_eq!(
            positive_box.underlying,
            TypeRef::Named {
                name: "PublicPositive".to_string()
            }
        );
        let boxed = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "PublicBoxed")
            .ok_or("missing aliased generic newtype export")?;
        assert_eq!(boxed.type_params.len(), 1);
        let strict = provider_manifest
            .exports
            .newtypes
            .iter()
            .find(|newtype| newtype.name == "StrictPort")
            .ok_or("missing no-implicit-coercion newtype export")?;
        assert!(!strict.implicit_coercion_enabled);

        let consumer_root = tmp.path().join("environ_types_consumer");
        let consumer_name = unique_test_project_name("environ_types_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            format!(
                "[project]\nname = \"{consumer_name}\"\n\n[dependencies]\nenviron_types = {{ path = \"../environ_types_provider\" }}\n"
            ),
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            r#"from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom
from pub::environ_types import ClonedBox, EnvClass, EnvMode, EnvToken, ExplicitLabel, MultiToken, Port, PublicBoxed, PublicPort, PublicPositive, PublicPositiveBox, Ratio, StrictPort, TraitToken, WrappedPort

def require[T with TryFrom[str]](key: str) -> Result[T, EnvironError]:
  match get_as[T](key)?:
    Some(value) => return Ok(value)
    None => return Err(EnvironError.missing(key))

def mode_name(mode: EnvMode) -> str:
  match mode:
    EnvMode.Dev => return "Dev"
    EnvMode.Prod => return "Prod"

def read_values() -> Result[None, EnvironError]:
  token = require[EnvToken]("INCAN_PACKAGE_ENV_TOKEN")?
  trait_token = require[TraitToken]("INCAN_PACKAGE_ENV_TRAIT_TOKEN")?
  env_class = require[EnvClass]("INCAN_PACKAGE_ENV_CLASS")?
  env_mode = require[EnvMode]("INCAN_PACKAGE_ENV_MODE")?
  explicit_label = require[ExplicitLabel]("INCAN_PACKAGE_ENV_EXPLICIT_LABEL")?
  port = require[Port]("INCAN_PACKAGE_ENV_PORT")?
  public_port = require[PublicPort]("INCAN_PACKAGE_ENV_PUBLIC_PORT")?
  wrapped = require[WrappedPort]("INCAN_PACKAGE_ENV_WRAPPED")?
  multi = require[MultiToken]("INCAN_PACKAGE_ENV_MULTI")?
  positive = require[PublicPositive]("INCAN_PACKAGE_ENV_POSITIVE")?
  positive_box = require[PublicPositiveBox]("INCAN_PACKAGE_ENV_POSITIVE_BOX")?
  boxed = require[PublicBoxed[int]]("INCAN_PACKAGE_ENV_BOXED")?
  cloned = require[ClonedBox[str]]("INCAN_PACKAGE_ENV_CLONED")?
  ratio = require[Ratio]("INCAN_PACKAGE_ENV_RATIO")?
  strict = require[StrictPort]("INCAN_PACKAGE_ENV_STRICT")?
  unwrapped = wrapped.0
  positive_inner = positive_box.0
  defaulted = get_as[Port]("INCAN_PACKAGE_ENV_MISSING", default=8080)?
  println(f"{token.value}:{trait_token.value}:{env_class.value}:{mode_name(env_mode)}:{explicit_label.0}:{multi.value}:{port.0}:{public_port.0}:{unwrapped.0}:{positive.0}:{positive_inner.0}:{boxed.0}:{cloned.0}:{ratio.0}:{strict.0}:{defaulted.0}")
  return Ok(None)

def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(f"unexpected:{error.kind_name()}")
  match get_as[Port]("INCAN_PACKAGE_ENV_BAD_PORT"):
    Ok(_) => println("invalid:unexpected")
    Err(error) => println(f"invalid:{error.kind_name()}")
  match get_as[PublicPositive]("INCAN_PACKAGE_ENV_NON_POSITIVE"):
    Ok(_) => println("constrained:unexpected")
    Err(error) => println(f"constrained:{error.kind_name()}")
  match get_as[Ratio]("INCAN_PACKAGE_ENV_RATIO_HIGH"):
    Ok(_) => println("ratio-high:unexpected")
    Err(error) => println(f"ratio-high:{error.kind_name()}")
"#,
        )?;

        let consumer_check = run_check(&consumer_main)?;
        assert!(
            consumer_check.status.success(),
            "expected consumer typecheck to accept package typed environment reads.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_check.stdout),
            String::from_utf8_lossy(&consumer_check.stderr)
        );

        let consumer_run = super::incan_command()
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_PACKAGE_ENV_TOKEN", "secret-token")
            .env("INCAN_PACKAGE_ENV_TRAIT_TOKEN", "trait-token")
            .env("INCAN_PACKAGE_ENV_CLASS", "class-token")
            .env("INCAN_PACKAGE_ENV_MODE", "prod")
            .env("INCAN_PACKAGE_ENV_EXPLICIT_LABEL", "explicit")
            .env("INCAN_PACKAGE_ENV_PORT", "5432")
            .env("INCAN_PACKAGE_ENV_PUBLIC_PORT", "5433")
            .env("INCAN_PACKAGE_ENV_WRAPPED", "6543")
            .env("INCAN_PACKAGE_ENV_MULTI", "multi-token")
            .env("INCAN_PACKAGE_ENV_POSITIVE", "12")
            .env("INCAN_PACKAGE_ENV_POSITIVE_BOX", "13")
            .env("INCAN_PACKAGE_ENV_BOXED", "88")
            .env("INCAN_PACKAGE_ENV_CLONED", "cloned")
            .env("INCAN_PACKAGE_ENV_RATIO", "0.25")
            .env("INCAN_PACKAGE_ENV_STRICT", "9090")
            .env("INCAN_PACKAGE_ENV_BAD_PORT", "70000")
            .env("INCAN_PACKAGE_ENV_NON_POSITIVE", "0")
            .env("INCAN_PACKAGE_ENV_RATIO_HIGH", "1.5")
            .env_remove("INCAN_PACKAGE_ENV_MISSING")
            .output()?;
        assert!(
            consumer_run.status.success(),
            "expected package typed environment reads to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_run.stdout),
            String::from_utf8_lossy(&consumer_run.stderr)
        );
        assert_eq!(
            String::from_utf8(consumer_run.stdout)?,
            concat!(
                "secret-token:trait-token:class-token:Prod:explicit:multi-token:5432:5433:6543:12:13:88:cloned:0.25:9090:8080\n",
                "invalid:invalid_value\n",
                "constrained:invalid_value\n",
                "ratio-high:invalid_value\n",
            )
        );

        let strict_default = consumer_root.join("src/strict_default.incn");
        std::fs::write(
            &strict_default,
            r#"from std.environ import get_as
from pub::environ_types import StrictPort

def main() -> None:
  get_as[StrictPort]("INCAN_PACKAGE_ENV_STRICT_DEFAULT", 8080)
"#,
        )?;
        let strict_check = run_check(&strict_default)?;
        assert!(
            !strict_check.status.success(),
            "expected package no-implicit-coercion policy to reject underlying default"
        );
        assert!(
            String::from_utf8_lossy(&strict_check.stderr)
                .contains("Implicit coercion into newtype 'StrictPort' is disabled"),
            "expected package coercion diagnostic, got:\n{}",
            String::from_utf8_lossy(&strict_check.stderr)
        );
        Ok(())
    }

    #[test]
    fn std_environ_typed_reads_survive_facades_and_test_batches() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_facade_test_batch");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/env_types.incn"),
            r#"pub type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))
"#,
        )?;
        std::fs::write(
            project_root.join("src/environ_facade.incn"),
            "pub from env_types import Port\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from environ_facade import Port
from std.environ import get_as

def main() -> None:
  match get_as[Port]("INCAN_ENVIRON_FACADE_MISSING", default=8080):
    Ok(port) => println(port.0)
    Err(error) => println(error.kind_name())
"#,
        )?;
        std::fs::write(
            project_root.join("tests/test_environ.incn"),
            r#"from environ_facade import Port
from std.environ import get_as
from std.testing import assert_eq, fail

def test_defaulted_port_through_facade() -> None:
  match get_as[Port]("INCAN_ENVIRON_TEST_BATCH_MISSING", 9090):
    Ok(port) => assert_eq(port.0, 9090)
    Err(error) => fail(f"expected defaulted Port, got {error.kind_name()}")
  match get_as[Port]("INCAN_ENVIRON_TEST_BATCH_INVALID"):
    Ok(_) => fail("expected invalid facade Port to fail validation")
    Err(error) => assert_eq(error.kind_name(), "invalid_value")
"#,
        )?;

        let run_output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env_remove("INCAN_ENVIRON_FACADE_MISSING")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected facade typed environment read to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        assert_eq!(String::from_utf8(run_output.stdout)?, "8080\n");

        let test_output = super::incan_command()
            .args(["test", project_root.join("tests").to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_TEST_SHARED_TARGET_DIR", shared_test_runner_target_dir())
            .env("INCAN_ENVIRON_TEST_BATCH_INVALID", "70000")
            .env_remove("INCAN_ENVIRON_TEST_BATCH_MISSING")
            .output()?;
        assert!(
            test_output.status.success(),
            "expected test batch typed environment read to pass.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn std_environ_module_qualified_overloads_run() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_module_qualified");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import std.environ as environ
from std.environ import EnvironError


def read_values() -> Result[None, EnvironError]:
  present = environ.get_as[int]("INCAN_ENVIRON_QUALIFIED_PRESENT")?.unwrap_or(0)
  fallback = environ.get_as[int]("INCAN_ENVIRON_QUALIFIED_MISSING", default=8080)?
  println(f"{present}:{fallback}")
  return Ok(None)


def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.kind_name())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_ENVIRON_QUALIFIED_PRESENT", "42")
            .env_remove("INCAN_ENVIRON_QUALIFIED_MISSING")
            .output()?;
        assert!(
            output.status.success(),
            "expected module-qualified std.environ overloads to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "42:8080\n");
        Ok(())
    }

    #[test]
    fn std_environ_overloads_run_through_source_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let project_name = unique_test_project_name("environ_function_facade");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            format!("[project]\nname = \"{project_name}\"\nversion = \"0.1.0\"\n"),
        )?;
        std::fs::write(
            project_root.join("src/environ_facade.incn"),
            "pub from std.environ import EnvironError, get_as\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"from environ_facade import EnvironError, get_as


def read_values() -> Result[None, EnvironError]:
  present = get_as[int]("INCAN_ENVIRON_FACADE_PRESENT")?.unwrap_or(0)
  fallback = get_as[int]("INCAN_ENVIRON_FACADE_MISSING", 9090)?
  println(f"{present}:{fallback}")
  return Ok(None)


def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.kind_name())
"#,
        )?;

        let output = super::incan_command()
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env("INCAN_ENVIRON_FACADE_PRESENT", "73")
            .env_remove("INCAN_ENVIRON_FACADE_MISSING")
            .output()?;
        assert!(
            output.status.success(),
            "expected std.environ overloads re-exported by a source facade to run.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout)?, "73:9090\n");
        Ok(())
    }

    #[test]
    fn check_pub_boundary_preserves_consumer_type_fidelity_cases() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_pub_boundary_type_fidelity_library(tmp.path())?;

        let cases = [
            (
                "question_mark_result",
                "`lazy.collect()?` across pub boundary",
                r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
            ),
            (
                "derived_method_chain",
                "`lazy.clone().collect()?` across pub boundary",
                r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.clone().collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
            ),
            (
                "trait_supertype",
                "`DataFrame[T]` satisfying `DataSet[T]` across pub boundary",
                r#"from pub::pubdemo import DataFrame, SessionError, display

model Row:
  value: int

def main() -> Result[None, SessionError]:
  df = DataFrame[Row](_type_witness=[])
  display(df)
  return Ok(None)
"#,
            ),
        ];

        for (name, description, source) in cases {
            let case_root = tmp.path().join(name);
            let main_path = write_project_files(
                &case_root,
                "[project]\nname = \"consumer\"\n\n[dependencies]\npubdemo = { path = \"../pub_boundary_library\" }\n",
                source,
            )?;

            let output = run_check(&main_path)?;
            assert!(
                output.status.success(),
                "expected {description} to typecheck.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    #[test]
    fn build_lib_fails_early_for_invalid_helper_binding_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("invalid_helper_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate_with_source(
            &producer_root,
            "vocab_companion",
            "widgets_vocab_companion",
            "use incan_vocab::{HelperBinding, LibraryManifest, VocabRegistration};\n\npub fn library_vocab() -> VocabRegistration {\n    VocabRegistration::new().with_library_manifest(LibraryManifest {\n        helper_bindings: vec![HelperBinding {\n            key: \"filter\".to_string(),\n            exported_name: \"filter\".to_string(),\n        }],\n        ..LibraryManifest::default()\n    })\n}\n",
        )?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            !producer_build.status.success(),
            "expected `build --lib` to fail for invalid helper binding.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&producer_build.stderr));
        assert!(
            stderr.contains("unknown exported symbol `filter`"),
            "expected helper-binding validation failure, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn consumer_check_uses_serialized_vocab_metadata_for_keyword_activation() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("widgets_assert_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"widgets_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def make_widget(name: str) -> str:\n  return name\n",
        )?;
        write_vocab_companion_crate_with_assert_keyword(&producer_root, "vocab_companion", "widgets_vocab_companion")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with assert vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("consumer_with_vocab_keyword");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"../widgets_assert_vocab_project\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "import pub::widgets\n\ndef main() -> None:\n  assert true\n",
        )?;

        let check_output = run_check(&consumer_main)?;
        assert!(
            check_output.status.success(),
            "expected consumer check to parse/typecheck assert keyword from serialized vocab metadata.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_external_vocab_block_via_wasm() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed after wasm desugaring.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_passes_request_payload_into_external_vocab_desugarer() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request(&output_payload, "missing request payload")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed when request payload is visible to the wasm desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_accepts_expression_desugar_output_in_statement_position() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(1));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed when wasm desugarer returns expression output.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_reports_external_vocab_desugarer_failure() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let wasm = compile_desugarer_wasm(1, "", "boom from wasm desugarer")?;
        write_pub_library_with_vocab_desugarer(tmp.path(), "routes", "routes_core", &wasm, "route")?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nroutes = { path = \"deps/routes\" }\n",
            "import pub::routes\n\ndef main() -> None:\n  route \"/health\":\n    pass\n",
        )?;

        let output = run_check(&main_path)?;
        assert!(
            !output.status.success(),
            "expected check to fail when wasm desugarer reports failure.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stderr = strip_ansi_escapes(&String::from_utf8_lossy(&output.stderr));
        assert!(
            stderr.contains("vocab desugar pass failed"),
            "expected desugar-pass error prefix, got:\n{stderr}"
        );
        assert!(
            stderr.contains("boom from wasm desugarer"),
            "expected wasm runtime error message, got:\n{stderr}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_uses_provider_paths_for_vocab_desugarer_calls() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Helper("filter".to_string())),
            args: vec![incan_vocab::IncanExpr::Int(1)],
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer_and_filter_helper(
            tmp.path(),
            "filterkit",
            "filterkit_core",
            &wasm,
            "where",
        )?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nfilterkit = { path = \"deps/filterkit\" }\n",
            "import pub::filterkit\n\ndef main() -> None:\n  where true:\n    pass\n",
        )?;

        let check_output = run_check(&main_path)?;
        assert!(
            check_output.status.success(),
            "expected check to succeed when desugared output uses a provider helper binding.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected build to succeed when desugared output uses a provider helper binding.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            !generated_main_rs.contains("__incan_vocab_helper_filterkit_filter"),
            "expected generated Rust to avoid hidden helper aliases, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("filterkit::filter"),
            "expected generated Rust to call the provider helper through its dependency crate path, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_plans_vocab_helper_calls_like_ordinary_calls_issue729() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Helper("aggregate_as".to_string())),
            args: vec![
                incan_vocab::IncanExpr::Call {
                    callee: Box::new(incan_vocab::IncanExpr::Helper("lit".to_string())),
                    args: vec![incan_vocab::IncanExpr::Int(5)],
                },
                incan_vocab::IncanExpr::Str("total".to_string()),
            ],
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer_and_string_helper(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nhelperkit = { path = \"deps/helperkit\" }\n",
            r#"import pub::helperkit

def main() -> None:
  where true:
    pass
"#,
        )?;

        let out_dir = tmp.path().join("out");
        let output = run_build(&main_path, &out_dir)?;
        assert!(
            output.status.success(),
            "expected helper-backed desugared calls to use normal call planning.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        let normalized: String = generated_main_rs.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(
            normalized.contains("helperkit::aggregate_as(helperkit::lit(5),\"total\".to_string()")
                || normalized.contains(
                    "__incan_vocab_helper_helperkit_aggregate_as(__incan_vocab_helper_helperkit_lit(5),\"total\".to_string()"
                ),
            "expected nested helper calls to keep independent call planning, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_plans_source_backed_vocab_helper_calls_with_defaults_and_unions_issue729()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Tuple(vec![
            incan_vocab::IncanExpr::Call {
                callee: Box::new(incan_vocab::IncanExpr::Helper("aggregate_as".to_string())),
                args: vec![
                    incan_vocab::IncanExpr::Call {
                        callee: Box::new(incan_vocab::IncanExpr::Helper("lit".to_string())),
                        args: vec![incan_vocab::IncanExpr::Int(5)],
                    },
                    incan_vocab::IncanExpr::Str("adjusted".to_string()),
                ],
            },
            incan_vocab::IncanExpr::Call {
                callee: Box::new(incan_vocab::IncanExpr::Helper("aggregate_as".to_string())),
                args: vec![
                    incan_vocab::IncanExpr::Call {
                        callee: Box::new(incan_vocab::IncanExpr::Helper("count".to_string())),
                        args: Vec::new(),
                    },
                    incan_vocab::IncanExpr::Str("order_count".to_string()),
                ],
            },
        ]));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_source_pub_library_with_vocab_desugarer_and_query_helpers(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  where true:
    pass
"#,
        )?;

        let out_dir = tmp.path().join("out");
        let output = run_build(&main_path, &out_dir)?;
        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs")).unwrap_or_default();
        assert!(
            output.status.success(),
            "expected source-backed helper calls to keep defaults, union wrapping, and string planning.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main_rs,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        assert!(
            generated_main_rs.contains("querykit::count(")
                || generated_main_rs.contains("__incan_vocab_helper_querykit_count("),
            "expected omitted count() argument to be filled from the helper's default expression, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("__incan_vocab_helper_querykit_count()"),
            "helper default planning must not emit a zero-argument Rust count call, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("querykit::helpers::COUNT_SENTINEL"),
            "dependency-owned const defaults must keep the defining provider module path, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("pub enum __IncanUnion"),
            "public dependency helper unions must stay owned by the dependency crate, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains(".to_string()"),
            "expected helper string arguments to use normal owned-string conversion, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_build_plans_source_backed_pub_helper_calls_with_defaults_and_unions_issue729()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let wasm = compile_desugarer_wasm(0, "[]", "")?;
        write_source_pub_library_with_vocab_desugarer_and_query_helpers(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"from pub::querykit import aggregate_as, aggregate_default, count, lit

def main() -> None:
  aggregate_as(lit(5), "adjusted")
  aggregate_as(count(), "order_count")
  aggregate_default(lit(7))
"#,
        )?;

        let out_dir = tmp.path().join("out");
        let output = run_build(&main_path, &out_dir)?;
        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs")).unwrap_or_default();
        assert!(
            output.status.success(),
            "expected ordinary pub helper calls to share exported default, union, and string planning.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main_rs,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            generated_main_rs.contains("querykit::count(")
                || generated_main_rs.contains("__incan_vocab_helper_querykit_count("),
            "expected omitted count() argument to be filled from the helper's default expression, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("querykit::count()")
                && !generated_main_rs.contains("__incan_vocab_helper_querykit_count()"),
            "ordinary pub helper default planning must not emit a zero-argument Rust count call, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("querykit::helpers::COUNT_SENTINEL"),
            "ordinary public dependency const defaults must keep the defining provider module path, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("pub enum __IncanUnion"),
            "ordinary public dependency calls must not re-own dependency anonymous unions, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains(".to_string()"),
            "expected ordinary pub helper string arguments to use normal owned-string conversion, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("querykit::helpers::DEFAULT_LABEL"),
            "expected public const defaults to emit through their provider module path, got:\n{generated_main_rs}"
        );
        Ok(())
    }

    #[test]
    fn consumer_check_passes_scoped_query_surface_artifacts_to_desugarer() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "query_generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing scoped query surface artifact",
            r#""descriptor_key":"query.field""#,
        )?;
        write_pub_library_with_querykit_surface_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  query:
    .amount > 100
    .customer_id
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to succeed when querykit-style leading-dot artifacts reach the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let negative_main_path = write_project_files(
            tmp.path().join("negative_consumer").as_path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"../deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  query:
    amount > 100
"#,
        )?;
        let negative_output = run_check(&negative_main_path)?;
        assert!(
            !negative_output.status.success(),
            "expected check to fail when no scoped query artifact reaches the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&negative_output.stdout),
            String::from_utf8_lossy(&negative_output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&negative_output.stderr).contains("missing scoped query surface artifact"),
            "expected desugarer failure to prove the request substring assertion was active.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&negative_output.stdout),
            String::from_utf8_lossy(&negative_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_passes_expr_list_item_metadata_to_desugarer_issue724() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::statements(vec![incan_vocab::IncanStatement::Let {
            name: "query_generated".to_string(),
            mutable: false,
            value: incan_vocab::IncanExpr::Int(1),
        }]);
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing expression-list modifier payload",
            r#""keyword":"with""#,
        )?;
        write_pub_library_with_querykit_select_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  query:
    SELECT:
      sum(amount) as total for customer with context
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass expression-list item metadata to the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_colon_vocab_expression_in_assignment_issue727() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing expression-desugaring declaration payload",
            r#""keyword":"query""#,
        )?;
        write_pub_library_with_querykit_select_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  value: int = query:
    SELECT:
      amount as total
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to desugar expression-position vocab block in assignment.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_colon_vocab_expression_in_return_issue727() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing expression-desugaring declaration payload",
            r#""keyword":"query""#,
        )?;
        write_pub_library_with_querykit_select_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def build_value() -> int:
  return query:
    SELECT:
      amount as total
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to desugar expression-position vocab block in return.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_colon_vocab_expression_preserves_inline_clauses_issue727()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing inline FROM clause payload",
            r#""keyword":"FROM""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  selected: int = query:
    FROM orders
    SELECT:
      amount as total
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to pass inline colon-expression clauses to the desugarer.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_braced_vocab_expression_with_compound_clauses_issue727()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing compound clause payload",
            r#""compound_tokens":["BY"]"#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
  value: int = query { FROM orders GROUP BY amount as grouped SELECT total as total }
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected check to desugar braced expression-position vocab block.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugared_public_field_callee_call_typechecks_as_method_issue727()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Field {
                object: Box::new(incan_vocab::IncanExpr::Name("orders".to_string())),
                field: "select".to_string(),
            }),
            args: Vec::new(),
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing FROM clause payload",
            r#""keyword":"FROM""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

class LazyFrame:
  def select(self) -> Self:
    return self

def main() -> None:
  orders = LazyFrame()
  selected: LazyFrame = query { FROM orders SELECT amount as amount }
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected public field-callee desugar output to typecheck as a method call.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugared_generic_method_call_uses_expected_return_type_issue735()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Field {
                object: Box::new(incan_vocab::IncanExpr::Name("orders".to_string())),
                field: "select".to_string(),
            }),
            args: vec![incan_vocab::IncanExpr::List(vec![incan_vocab::IncanExpr::Call {
                callee: Box::new(incan_vocab::IncanExpr::Name("with_column_assignment".to_string())),
                args: vec![
                    incan_vocab::IncanExpr::Str("customer".to_string()),
                    incan_vocab::IncanExpr::Call {
                        callee: Box::new(incan_vocab::IncanExpr::Name("current_field".to_string())),
                        args: vec![
                            incan_vocab::IncanExpr::Name("orders".to_string()),
                            incan_vocab::IncanExpr::Str("customer_id".to_string()),
                        ],
                    },
                ],
            }])],
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing SELECT clause payload",
            r#""keyword":"SELECT""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

@derive(Clone)
model Order:
  customer_id: str

@derive(Clone)
model Selected:
  customer: str

@derive(Clone)
model ColumnExpr:
  source: str

@derive(Clone)
model ColumnAssignment[T with Clone]:
  name: str

def current_field[T with Clone](_frame: LazyFrame[T], source: str) -> ColumnExpr:
  return ColumnExpr(source=source)

def with_column_assignment[T with Clone](name: str, _expr: ColumnExpr) -> ColumnAssignment[T]:
  return ColumnAssignment[T](name=name)

@derive(Clone)
class LazyFrame[T with Clone]:
  _type_witness: list[T]

  def select[U with Clone](self, columns: list[ColumnAssignment[U]]) -> LazyFrame[U]:
    return LazyFrame[U](_type_witness=[])

def direct_method_call(orders: LazyFrame[Order]) -> LazyFrame[Selected]:
  return orders.select([with_column_assignment("customer", current_field(orders, "customer_id"))])

def query_block_call(orders: LazyFrame[Order]) -> LazyFrame[Selected]:
  return query { FROM orders SELECT customer_id as customer }
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected desugared generic method call to use the same contextual return type as direct source.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_test_activates_dependency_vocab_surfaces_issue730_issue756() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "missing SELECT clause payload",
            r#""keyword":"SELECT""#,
        )?;
        write_pub_library_with_querykit_expression_clause_desugarer(tmp.path(), &wasm)?;

        write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            "def main() -> None:\n  return\n",
        )?;
        let tests_dir = tmp.path().join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        let test_path = tests_dir.join("test_query_vocab.incn");
        std::fs::write(
            &test_path,
            r#"import pub::querykit

def test_dependency_vocab_query_block() -> None:
    selected: int = query {
        FROM orders
        GROUP BY
            amount as grouped,
            region as region_group
        SELECT
            amount as total
        ORDER BY amount
        WINDOW BY
            rank = amount
    }
    assert selected == 7
"#,
        )?;

        let fmt_output = run_fmt(&test_path)?;
        assert!(
            fmt_output.status.success(),
            "expected incan fmt to parse dependency-activated vocab in a nested package test file.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );

        let formatted_source = std::fs::read_to_string(&test_path)?;
        for clause in [
            "selected: int = query {",
            "        GROUP BY\n            amount as grouped,\n            region as region_group",
            "        ORDER BY amount",
            "        WINDOW BY\n            rank = amount",
        ] {
            assert!(
                formatted_source.contains(clause),
                "expected incan fmt to preserve dependency-vocab expression block shape `{clause}`.\nformatted source:\n{}",
                formatted_source
            );
        }
        for rejected_clause in ["query:", "GROUP BY:", "ORDER BY:", "WINDOW BY:"] {
            assert!(
                !formatted_source.contains(rejected_clause),
                "expected incan fmt not to rewrite expression vocab block through colon clause `{rejected_clause}`.\nformatted source:\n{}",
                formatted_source
            );
        }

        let fmt_output = run_fmt_check(&test_path)?;
        assert!(
            fmt_output.status.success(),
            "expected formatted dependency-activated vocab file to pass incan fmt --check.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );

        let check_output = run_check(&test_path)?;
        assert!(
            check_output.status.success(),
            "expected ordinary check to parse dependency-activated vocab in a test file.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let test_output = run_test(&test_path)?;
        assert!(
            test_output.status.success(),
            "expected incan test to parse and run dependency-activated vocab in a test file.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_build_and_test_desugars_quality_expression_vocab_clauses_issue813()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm_requiring_request_substring(
            &output_payload,
            "quality expression vocab body did not expose EXPECT as a clause",
            r#""keyword":"EXPECT""#,
        )?;
        write_pub_library_with_mixed_quality_clause_desugarer(tmp.path(), &wasm)?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
            r#"import pub::querykit

def main() -> None:
    checks: int = quality {
        FROM orders
        REQUIRE row_count() >= 1 as non_empty_orders
        GROUP BY .customer_id
        EXPECT count() >= 1 as customer_groups_present
    }
    assert checks == 7
"#,
        )?;
        let tests_dir = tmp.path().join("tests");
        std::fs::create_dir_all(&tests_dir)?;
        let test_path = tests_dir.join("test_quality.incn");
        std::fs::write(
            &test_path,
            r#"import pub::querykit

def test_quality_expression_vocab_result() -> None:
    checks: int = quality {
        FROM orders
        REQUIRE row_count() >= 1 as non_empty_orders
        GROUP BY .customer_id
        EXPECT count() >= 1 as customer_groups_present
    }
    assert checks == 7
"#,
        )?;

        let check_output = run_check(&main_path)?;
        assert!(
            check_output.status.success(),
            "expected quality expression vocab declaration to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&check_output.stdout),
            String::from_utf8_lossy(&check_output.stderr)
        );

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        let generated_main = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            build_output.status.success(),
            "expected generated Rust build for the quality expression vocab declaration to succeed.\ngenerated main.rs:\n{}\nstdout:\n{}\nstderr:\n{}",
            generated_main,
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );
        assert!(
            generated_main.contains("checks"),
            "expected generated Rust to retain the desugared typed assignment.\ngenerated main.rs:\n{generated_main}"
        );

        let test_output = run_test(&test_path)?;
        assert!(
            test_output.status.success(),
            "expected incan test to execute a typed result from the quality expression vocab declaration.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_check_desugars_each_quality_clause_issue813() -> Result<(), Box<dyn std::error::Error>> {
        for (clause, request_fragment) in [
            ("FROM", r#""keyword":"FROM""#),
            ("REQUIRE", r#""keyword":"REQUIRE""#),
            ("GROUP BY", r#""keyword":"GROUP","compound_tokens":["BY"]"#),
            ("EXPECT", r#""keyword":"EXPECT""#),
        ] {
            let tmp = tempfile::tempdir()?;
            let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Int(7));
            let output_payload = serde_json::to_string(&response)?;
            let wasm = compile_desugarer_wasm_requiring_request_substring(
                &output_payload,
                &format!("quality expression vocab body did not expose {clause} as a clause"),
                request_fragment,
            )?;
            write_pub_library_with_mixed_quality_clause_desugarer(tmp.path(), &wasm)?;

            let main_path = write_project_files(
                tmp.path(),
                "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"deps/querykit\" }\n",
                r#"import pub::querykit

def main() -> None:
    checks: int = quality {
        FROM orders
        REQUIRE row_count() >= 1 as non_empty_orders
        GROUP BY .customer_id
        EXPECT count() >= 1 as customer_groups_present
    }
    assert checks == 7
"#,
            )?;

            let check_output = run_check(&main_path)?;
            assert!(
                check_output.status.success(),
                "expected quality expression vocab declaration to expose {clause} to the desugarer.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&check_output.stdout),
                String::from_utf8_lossy(&check_output.stderr)
            );
        }
        Ok(())
    }

    #[test]
    fn fmt_activates_clean_source_dependency_vocab_before_parsing_issue756() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("deps").join("querykit");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"querykit\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub def ready() -> int:\n  return 1\n",
        )?;
        write_vocab_companion_crate_with_source(
            &producer_root,
            "vocab_companion",
            "querykit_vocab_companion",
            r#"use incan_vocab::{ClauseSurface, DeclarationSurface, DslSurface, VocabRegistration};

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new().with_surface(
        DslSurface::on_import("querykit").with_declaration(
            DeclarationSurface::named("query")
                .with_clause_body()
                .desugars_to_expression()
                .with_clauses([
                    ClauseSurface::expr("FROM").required(),
                    ClauseSurface::expr_list("SELECT").required(),
                ]),
        ),
    )
}
"#,
        )?;

        let consumer_root = tmp.path().join("consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\nversion = \"0.1.0\"\n\n[dependencies]\nquerykit = { path = \"../deps/querykit\" }\n",
        )?;
        let main_path = consumer_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            r#"import pub::querykit

def main() -> None:
    value = query {
        FROM orders
        SELECT
            amount as total
    }
"#,
        )?;

        let artifact_root = producer_root.join("target").join("lib");
        assert!(
            !artifact_root.exists(),
            "regression must start from a clean source dependency without prebuilt library artifacts"
        );

        let fmt_output = super::incan_command()
            .args(["fmt", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .env(INTERNAL_MANIFEST_OVERRIDE_ENV, consumer_root.join("incan.toml"))
            .env(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV, &consumer_root)
            .output()?;
        assert!(
            fmt_output.status.success(),
            "expected fmt to prepare source dependency vocab before parsing clean query block, even when the parent command carries an internal manifest override.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );
        assert!(
            !artifact_root.join("querykit.incnlib").exists(),
            "fmt should activate parser vocab from source without preparing a persistent dependency artifact"
        );
        let combined_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&fmt_output.stdout),
            String::from_utf8_lossy(&fmt_output.stderr)
        );
        assert!(
            !combined_output.contains("Preparing missing pub::querykit dependency artifact"),
            "fmt source vocab activation should not invoke persistent artifact preparation, got: {combined_output}"
        );

        Ok(())
    }

    #[test]
    fn equivalent_helper_backed_keywords_typecheck() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let response = incan_vocab::DesugarResponse::expression(incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Helper("filter".to_string())),
            args: vec![incan_vocab::IncanExpr::Int(1)],
        });
        let output_payload = serde_json::to_string(&response)?;
        let wasm = compile_desugarer_wasm(0, &output_payload, "")?;
        write_pub_library_with_vocab_desugarer_and_filter_helper_keywords(
            tmp.path(),
            "querykit",
            "querykit_core",
            &wasm,
            &["where", "screen"],
        )?;

        let where_main = write_project_files(
            tmp.path().join("where_consumer").as_path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"../deps/querykit\" }\n",
            "import pub::querykit\n\ndef main() -> None:\n  where true:\n    pass\n",
        )?;
        let screen_main = write_project_files(
            tmp.path().join("screen_consumer").as_path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nquerykit = { path = \"../deps/querykit\" }\n",
            "import pub::querykit\n\ndef main() -> None:\n  screen true:\n    pass\n",
        )?;

        let where_check = run_check(&where_main)?;
        assert!(
            where_check.status.success(),
            "expected helper-backed `where` check to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&where_check.stdout),
            String::from_utf8_lossy(&where_check.stderr)
        );

        let screen_check = run_check(&screen_main)?;
        assert!(
            screen_check.status.success(),
            "expected helper-backed `screen` check to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&screen_check.stdout),
            String::from_utf8_lossy(&screen_check.stderr)
        );

        let where_out_dir = tmp.path().join("where_out");
        let where_build = run_build(&where_main, &where_out_dir)?;
        assert!(
            where_build.status.success(),
            "expected helper-backed `where` build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&where_build.stdout),
            String::from_utf8_lossy(&where_build.stderr)
        );
        let screen_out_dir = tmp.path().join("screen_out");
        let screen_build = run_build(&screen_main, &screen_out_dir)?;
        assert!(
            screen_build.status.success(),
            "expected helper-backed `screen` build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&screen_build.stdout),
            String::from_utf8_lossy(&screen_build.stderr)
        );
        let where_generated = std::fs::read_to_string(where_out_dir.join("src/main.rs"))?;
        let screen_generated = std::fs::read_to_string(screen_out_dir.join("src/main.rs"))?;
        assert_eq!(
            where_generated, screen_generated,
            "equivalent helper-backed keywords should emit identical Rust"
        );
        Ok(())
    }

    #[test]
    fn provider_requirements_and_pub_vocab_flow_through_build_test_and_lock() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;

        write_pub_library_with_provider_requirements_and_assert_keyword(
            project_root,
            "widgets",
            "widgets_core",
            vec![incan_vocab::CargoDependency {
                crate_name: "axum".to_string(),
                source: incan_vocab::CargoDependencySource::Version("0.8".to_string()),
            }],
            vec!["web"],
        )?;

        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(&main_path, "def main() -> None:\n  pass\n")?;
        std::fs::write(
            project_root.join("tests/test_provider.incn"),
            "import pub::widgets\n\ndef test_provider_parity() -> None:\n  assert true\n",
        )?;

        let build_out_dir = project_root.join("out");
        let build_output = run_build(&main_path, &build_out_dir)?;
        assert!(
            build_output.status.success(),
            "expected build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let lock_output = run_lock(&main_path)?;
        assert!(
            lock_output.status.success(),
            "expected lock to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&lock_output.stdout),
            String::from_utf8_lossy(&lock_output.stderr)
        );

        let test_output = run_test(&project_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected test run to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );

        let build_toml = std::fs::read_to_string(build_out_dir.join("Cargo.toml"))?;
        let lock_toml = std::fs::read_to_string(project_root.join("target/incan_lock/Cargo.toml"))?;
        let test_manifest_path = test_runner_batch_manifest_path(&project_root.join("tests/test_provider.incn"));
        let test_toml = std::fs::read_to_string(&test_manifest_path).map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!(
                    "failed reading test runner Cargo.toml at {}: {err}",
                    test_manifest_path.display()
                ),
            )
        })?;

        for cargo_toml in [&build_toml, &lock_toml, &test_toml] {
            assert!(
                cargo_toml.contains(r#"axum = "0.8""#),
                "expected provider dependency in generated Cargo.toml, got:\n{cargo_toml}"
            );
            assert!(
                cargo_toml.contains("incan_stdlib"),
                "expected stdlib dependency in generated Cargo.toml, got:\n{cargo_toml}"
            );
            assert!(
                cargo_toml.contains("\"web\""),
                "expected provider stdlib feature in generated Cargo.toml, got:\n{cargo_toml}"
            );
        }

        Ok(())
    }

    #[test]
    fn conflicting_provider_requirements_fail_build_test_and_lock() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;

        write_pub_library_with_provider_requirements(
            project_root,
            "widgets",
            "widgets_core",
            vec![incan_vocab::CargoDependency {
                crate_name: "serde_json".to_string(),
                source: incan_vocab::CargoDependencySource::Version("1.0".to_string()),
            }],
            vec![],
        )?;
        write_pub_library_with_provider_requirements(
            project_root,
            "analytics",
            "analytics_core",
            vec![incan_vocab::CargoDependency {
                crate_name: "serde_json".to_string(),
                source: incan_vocab::CargoDependencySource::Version("2.0".to_string()),
            }],
            vec![],
        )?;

        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\nanalytics = { path = \"deps/analytics\" }\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(&main_path, "def main() -> None:\n  pass\n")?;
        std::fs::write(
            project_root.join("tests/test_conflict.incn"),
            "def test_conflict_path() -> None:\n  pass\n",
        )?;

        let build_output = run_build(&main_path, &project_root.join("out"))?;
        assert!(
            !build_output.status.success(),
            "expected build to fail for conflicting provider deps.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );
        let build_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&build_output.stderr));
        assert!(
            build_stderr.contains("failed to merge provider requirements"),
            "expected provider conflict diagnostic in build stderr, got:\n{build_stderr}"
        );
        assert!(
            build_stderr.contains("serde_json"),
            "expected conflicting crate name in build stderr, got:\n{build_stderr}"
        );

        let lock_output = run_lock(&main_path)?;
        assert!(
            !lock_output.status.success(),
            "expected lock to fail for conflicting provider deps.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&lock_output.stdout),
            String::from_utf8_lossy(&lock_output.stderr)
        );
        let lock_stderr = strip_ansi_escapes(&String::from_utf8_lossy(&lock_output.stderr));
        assert!(
            lock_stderr.contains("failed to merge provider requirements"),
            "expected provider conflict diagnostic in lock stderr, got:\n{lock_stderr}"
        );
        assert!(
            lock_stderr.contains("serde_json"),
            "expected conflicting crate name in lock stderr, got:\n{lock_stderr}"
        );

        let test_output = run_test(&project_root.join("tests"))?;
        assert!(
            !test_output.status.success(),
            "expected test to fail for conflicting provider deps.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        let test_stdout = strip_ansi_escapes(&String::from_utf8_lossy(&test_output.stdout));
        assert!(
            test_stdout.contains("failed to merge provider requirements"),
            "expected provider conflict diagnostic in test output, got:\n{test_stdout}"
        );
        assert!(
            test_stdout.contains("serde_json"),
            "expected conflicting crate name in test output, got:\n{test_stdout}"
        );

        Ok(())
    }
}
