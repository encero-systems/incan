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

static RUNTIME_ERROR_PROJECT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a minimal throwaway Incan project for end-to-end runtime error assertions.
///
/// The generated project name includes both the current process id and a local counter so parallel nextest workers do
/// not trample each other's `target/incan/<name>` outputs.
fn write_runtime_error_project(source: &str) -> Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let unique = RUNTIME_ERROR_PROJECT_COUNTER.fetch_add(1, Ordering::Relaxed);
    let project_name = format!("runtime_error_contract_{}_{}", std::process::id(), unique);
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

    let check_output = Command::new(incan_debug_binary())
        .arg("--check")
        .arg(&main_path)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        check_output.status.success(),
        "expected --check to succeed so the failure is runtime.\nstderr:\n{}",
        String::from_utf8_lossy(&check_output.stderr)
    );

    let run_output = Command::new(incan_debug_binary())
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

    let output = Command::new(incan_debug_binary())
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
fn std_logging_logger_surface_filters_and_preserves_bound_context() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import ColorPolicy, Level, LogStyle, basic_config, get_logger

def main() -> None:
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
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected std.logging source surface run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
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

    Ok(())
}

#[test]
fn std_logging_source_json_renderer_preserves_record_shape() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, LogFormat, basic_config, get_logger

def main() -> None:
  basic_config(level=Level.DEBUG, format=LogFormat.JSON, target="stdout")
  log = get_logger()
  log.debug("json works", fields={"request_id": "abc", "component": "loader"})
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected source-defined std.logging JSON run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(records.len(), 1, "expected one JSON log record, got:\n{stdout}");
    let record = &records[0];
    assert_eq!(record["SeverityText"], serde_json::json!("DEBUG"));
    assert_eq!(record["SeverityNumber"], serde_json::json!(5));
    assert_eq!(record["InstrumentationScope"]["Name"], serde_json::json!("root"));
    assert_eq!(record["Body"]["Type"], serde_json::json!("string"));
    assert_eq!(record["Body"]["StringValue"], serde_json::json!("json works"));
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

    Ok(())
}

#[test]
fn std_logging_default_target_writes_stderr() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, basic_config, get_logger

def main() -> None:
  basic_config(level=Level.INFO)
  get_logger("app").info("stderr event")
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected std.logging stderr target run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("stderr event") && stderr.contains("stderr event"),
        "expected default logging target to route the event to stderr.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    Ok(())
}

#[test]
fn std_logging_default_logger_infers_source_module() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "std_logging_module_source"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"from std.logging import Level, LogStyle, basic_config
from worker import run_worker

def main() -> None:
  basic_config(level=Level.INFO, style=LogStyle.VERBOSE, target="stdout")
  run_worker()
"#,
    )?;
    fs::write(
        src_dir.join("worker.incn"),
        r#"from std.logging import get_logger

pub def run_worker() -> None:
  log = get_logger()
  log.info("worker ready")
"#,
    )?;

    let output = Command::new(incan_debug_binary())
        .arg("run")
        .arg("src/main.incn")
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected module-aware std.logging run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("worker ready") && stdout.contains("logger=worker") && !stdout.contains("logger=root"),
        "expected get_logger() in worker.incn to infer logger=worker, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn std_logging_ambient_log_infers_source_module() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "std_logging_ambient_log"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("main.incn"),
        r#"from std.logging import Level, LogStyle, basic_config
from worker import run_worker

def main() -> None:
  basic_config(level=Level.INFO, style=LogStyle.VERBOSE, target="stdout")
  run_worker()
"#,
    )?;
    fs::write(
        src_dir.join("worker.incn"),
        r#"pub def run_worker() -> None:
  log.info("worker ambient log ready")
"#,
    )?;

    let output = Command::new(incan_debug_binary())
        .arg("run")
        .arg("src/main.incn")
        .current_dir(tmp.path())
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected ambient std.logging log run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("worker ambient log ready")
            && stdout.contains("logger=worker")
            && !stdout.contains("logger=root")
            && !stdout.contains("logger=std.logging"),
        "expected ambient log in worker.incn to infer logger=worker, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn std_logging_ambient_log_is_shadowable() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, LogFormat, basic_config

model LocalLog:
  def info(self, message: str) -> None:
    println(f"local:{message}")

def main() -> None:
  basic_config(level=Level.INFO, format=LogFormat.JSON, target="stdout")
  log = LocalLog()
  log.info("shadowed")
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected local log binding to shadow ambient std.logging.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("local:shadowed") && !stdout.contains("InstrumentationScope"),
        "expected local log binding to remain ordinary source, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn std_logging_ambient_log_snippet_falls_back_to_root() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, LogFormat, basic_config

def main() -> None:
  basic_config(level=Level.INFO, format=LogFormat.JSON, target="stdout")
  log.info("snippet ambient")
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected metadata-free ambient log to fall back to root.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(r#""InstrumentationScope":{"Name":"root""#) && stdout.contains("snippet ambient"),
        "expected ambient log in -c snippet to emit with root logger, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn std_logging_rejects_invalid_logger_names() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        (
            "empty logger name",
            r#"from std.logging import get_logger

def main() -> None:
  get_logger("").info("should not emit")
"#,
            "std.logging logger names must not be empty",
        ),
        (
            "empty logger segment",
            r#"from std.logging import get_logger

def main() -> None:
  get_logger("app..db").info("should not emit")
"#,
            "std.logging logger names must not contain empty segments",
        ),
        (
            "invalid child suffix",
            r#"from std.logging import get_logger

def main() -> None:
  get_logger("app").child(".db").info("should not emit")
"#,
            "std.logging logger names must not contain empty segments",
        ),
    ];

    for (case, source, expected) in cases {
        let output = Command::new(incan_debug_binary())
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            !output.status.success(),
            "expected {case} to fail.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let combined = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains(expected),
            "expected {case} validation message `{expected}`, got:\n{combined}"
        );
    }

    Ok(())
}

#[test]
fn std_logging_rejects_invalid_output_target() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, basic_config, get_logger

def main() -> None:
  basic_config(level=Level.INFO, target="bogus")
  get_logger("app").info("should not emit")
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        !output.status.success(),
        "expected invalid std.logging target to fail.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("std.logging target must be 'stdout' or 'stderr'"),
        "expected target validation message, got:\n{combined}"
    );

    Ok(())
}

#[test]
fn std_logging_json_preserves_structured_field_values() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, LogFormat, basic_config
from std.telemetry.core import TelemetryValue

def main() -> None:
  basic_config(level=Level.INFO, format=LogFormat.JSON, target="stdout")
  log.info("structured", fields={
    "rows": 42,
    "ok": true,
    "ratio": 1.5,
    "missing": None,
    "items": TelemetryValue.array([TelemetryValue.int(1), TelemetryValue.bool(false)]),
    "nested": TelemetryValue.map({"child": TelemetryValue.string("yes")}),
  })
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected structured std.logging fields to compile and emit JSON.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(records.len(), 1, "expected one JSON log record, got:\n{stdout}");
    let attributes = &records[0]["Attributes"];
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
        records[0].get("rows").is_none() && records[0].get("nested").is_none(),
        "expected structured fields to stay under Attributes, got:\n{}",
        records[0]
    );

    Ok(())
}

#[test]
fn std_traits_convert_usage_runs() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!("codegen_snapshots/std_traits_convert_usage.incn");

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected std.traits.convert classmethod usage to compile and run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "42\n3\n");

    Ok(())
}

#[test]
fn std_logging_human_styles_render_distinct_shapes() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.logging import Level, LogStyle, basic_config, get_logger

def main() -> None:
  basic_config(level=Level.INFO, style=LogStyle.MINIMAL, target="stdout")
  get_logger("app").info("minimal event")
  basic_config(level=Level.INFO, style=LogStyle.SHORT, target="stdout")
  get_logger("app").info("short event")
  basic_config(level=Level.INFO, style=LogStyle.COMPLETE, target="stdout")
  get_logger("app").info("complete event")
  basic_config(level=Level.INFO, style=LogStyle.VERBOSE, target="stdout")
  get_logger("app").info("verbose event")
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected std.logging human style run to succeed.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
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
        complete_line.contains("T") && complete_line.contains("Z [INFO] complete event"),
        "expected complete style to use full datetime timestamp, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[INFO] verbose event\n  logger=app"),
        "expected verbose style to add logger metadata on a second line, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn telemetry_value_class_constructors_are_callable() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.telemetry.core import TelemetryValue

def main() -> None:
  text = TelemetryValue.string("alpha")
  payload = TelemetryValue.map({
    "items": TelemetryValue.array([TelemetryValue.int(42), TelemetryValue.bool(true)]),
    "empty": TelemetryValue.none(),
    "encoded": TelemetryValue.bytes("ff"),
    "ratio": TelemetryValue.float(1.5),
  })
  println(text.display_text())
  println(payload.display_text())
"#;

    let output = Command::new(incan_debug_binary())
        .args(["run", "-c", source])
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;

    assert!(
        output.status.success(),
        "expected telemetry value constructors to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("alpha")
            && stdout.contains(r#""Type":"map""#)
            && stdout.contains(r#""items":{"Type":"array""#)
            && stdout.contains(r#""IntValue":42"#)
            && stdout.contains(r#""BoolValue":true"#)
            && stdout.contains(r#""BytesValue":"ff""#)
            && stdout.contains(r#""FloatValue":1.5"#),
        "expected class constructors to preserve structured telemetry values, got:\n{stdout}"
    );

    Ok(())
}

#[test]
fn validated_newtype_runtime_success_coerces_approved_sites() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(incan_debug_binary())
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

    Ok(())
}

#[test]
fn validated_newtype_runtime_fail_fast_reports_validation_error() -> Result<(), Box<dyn std::error::Error>> {
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
    )
}

#[test]
fn validated_newtype_runtime_aggregates_model_field_errors() -> Result<(), Box<dyn std::error::Error>> {
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
    )
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

    let output = Command::new(incan_debug_binary())
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
        return path.into();
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let p = std::path::PathBuf::from(&target_dir).join("debug/incan");
        if p.exists() {
            return p;
        }
    }
    std::path::PathBuf::from("target/debug/incan")
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
    let status = Command::new(incan_debug_binary()).arg("fmt").arg(&path).status()?;
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

    let output = Command::new(incan_debug_binary()).arg("fmt").arg(&path).output()?;
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

    let status = Command::new(incan_debug_binary()).arg("fmt").arg(&path).status()?;
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

    let output = Command::new(incan_debug_binary()).arg("--check").arg(&path).output()?;
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

    let status = Command::new(incan_debug_binary()).arg("fmt").arg(&path).status()?;
    assert!(status.success(), "incan fmt failed");

    let formatted = fs::read_to_string(&path)?;
    assert!(
        formatted.contains(r#"f"a\n{1}""#),
        "expected formatted output to preserve escaped newline text, got:\n{}",
        formatted
    );

    let output = Command::new(incan_debug_binary()).arg("--check").arg(&path).output()?;
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

    let status = Command::new(incan_debug_binary()).arg("fmt").arg(&path).status()?;
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

    let status = Command::new(incan_debug_binary()).arg("fmt").arg(&path).status()?;
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

    let status = Command::new(incan_debug_binary()).arg("fmt").arg(&path).status()?;
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

    let output = Command::new(incan_debug_binary()).arg("--check").arg(&path).output()?;
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
    let output = Command::new(incan_debug_binary()).arg("--help").output()?;
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
    let output = Command::new(incan_debug_binary()).arg("--version").output()?;
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

    let new_output = Command::new(incan_debug_binary())
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
    assert!(project_dir.join("src/main.incn").exists());
    assert!(project_dir.join("tests/test_main.incn").exists());

    let empty_list_output = Command::new(incan_debug_binary())
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

    let default_overview_output = Command::new(incan_debug_binary())
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

    let default_show_output = Command::new(incan_debug_binary())
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

    let dry_run = Command::new(incan_debug_binary())
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

    let version_output = Command::new(incan_debug_binary())
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

    let set_output = Command::new(incan_debug_binary())
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

    let keep_prerelease_output = Command::new(incan_debug_binary())
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

    let missing_request_output = Command::new(incan_debug_binary())
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

    let conflicting_request_output = Command::new(incan_debug_binary())
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

    let list_output = Command::new(incan_debug_binary())
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

    let list_json_output = Command::new(incan_debug_binary())
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

    let show_output = Command::new(incan_debug_binary())
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

    let show_overview_output = Command::new(incan_debug_binary())
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

    let show_overview_json_output = Command::new(incan_debug_binary())
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

    let show_json_output = Command::new(incan_debug_binary())
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

    let dry_run_env = Command::new(incan_debug_binary())
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

    let run_env = Command::new(incan_debug_binary())
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

    let bare_run = Command::new(incan_debug_binary())
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

    let env_run = Command::new(incan_debug_binary())
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

    let bare_show = Command::new(incan_debug_binary())
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

    let env_show = Command::new(incan_debug_binary())
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
    let Ok(output) = Command::new(incan_debug_binary())
        .arg("--definitely-not-a-flag")
        .output()
    else {
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
    let Ok(output) = Command::new(incan_debug_binary()).args(["run", "-c", source]).output() else {
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
    let output = Command::new(incan_debug_binary())
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
    let output = Command::new(incan_debug_binary())
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
fn runtime_error_missing_dict_key_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  let values = {\"a\": 1}\n  println(values[\"b\"])\n",
        "KeyError",
        &["not found in dict"],
    )
}

#[test]
fn runtime_error_list_index_out_of_range_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  let values = [1, 2, 3]\n  println(values[99])\n",
        "IndexError",
        &["out of range for list"],
    )
}

#[test]
fn runtime_error_list_index_method_not_found_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  let values = [1, 2, 3]\n  println(values.index(99))\n",
        "ValueError",
        &["value not found in list"],
    )
}

#[test]
fn runtime_error_int_conversion_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  println(int(\"abc\"))\n",
        "ValueError",
        &["cannot convert 'abc' to int"],
    )
}

#[test]
fn runtime_error_float_conversion_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  println(float(\"abc\"))\n",
        "ValueError",
        &["cannot convert 'abc' to float"],
    )
}

#[test]
fn runtime_error_list_remove_out_of_range_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  mut values = [1, 2, 3]\n  values.remove(99)\n",
        "IndexError",
        &["out of range for list"],
    )
}

#[test]
fn runtime_error_list_swap_out_of_range_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    assert_runtime_error_cli(
        "def main() -> None:\n  mut values = [1, 2, 3]\n  values.swap(0, 99)\n",
        "IndexError",
        &["out of range for list"],
    )
}

#[test]
fn runtime_error_route_marker_runtime_misuse_is_explicit() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let web_macros_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates")
        .join("incan_web_macros");
    let manifest = format!(
        "[project]\nname = \"route_runtime_misuse\"\nversion = \"0.3.0-dev.1\"\n\n[rust-dependencies]\nincan_web_macros = {{ path = \"{}\" }}\n",
        web_macros_path.display()
    );
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(tmp.path().join("incan.toml"), manifest)?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        "from std.web import route\n\ndef main() -> None:\n  route(\"/users\", methods=[\"GET\"])\n",
    )?;

    let check_output = Command::new(incan_debug_binary())
        .arg("--check")
        .arg(&main_path)
        .env("CARGO_NET_OFFLINE", "true")
        .output()?;
    assert!(
        check_output.status.success(),
        "expected --check to succeed so the failure is runtime.\nstderr:\n{}",
        String::from_utf8_lossy(&check_output.stderr)
    );

    let run_output = Command::new(incan_debug_binary())
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
        combined.contains("decorator marker 'incan_web_macros::route' cannot be called at runtime"),
        "expected explicit decorator misuse runtime diagnostic, got:\n{combined}"
    );
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

    let Ok(output) = Command::new(incan_debug_binary())
        .args(["test", dir.to_string_lossy().as_ref()])
        .output()
    else {
        panic!("failed to run incan test");
    };
    assert!(
        output.status.success(),
        "expected empty collection to succeed by default: status={:?} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let Ok(output) = Command::new(incan_debug_binary())
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
    let Ok(output) = Command::new(incan_debug_binary())
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
    let Ok(output) = Command::new(incan_debug_binary())
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
    let Ok(output) = Command::new(incan_debug_binary())
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
    let output = Command::new(incan_debug_binary())
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
    let output = Command::new(incan_debug_binary())
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
    let output = Command::new(incan_debug_binary())
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
    let output = Command::new(incan_debug_binary())
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
    let Ok(output) = Command::new(incan_debug_binary())
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
    let Ok(output) = Command::new(incan_debug_binary())
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
    fn test_floor_div_tokens() {
        let Ok(tokens) = lex("a //= b\nc // d") else {
            panic!("lex failed");
        };
        let has_floor_div_eq = tokens.iter().any(|t| t.kind.is_operator(OperatorId::SlashSlashEq));
        let has_floor_div = tokens.iter().any(|t| t.kind.is_operator(OperatorId::SlashSlash));
        assert!(has_floor_div_eq, "expected to see //= token");
        assert!(has_floor_div, "expected to see // token");
    }

    #[test]
    fn test_rust_style_imports() {
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
    }

    #[test]
    fn test_try_operator() {
        let Ok(tokens) = lex("result?") else {
            panic!("lex failed");
        };
        assert!(matches!(&tokens[0].kind, TokenKind::Ident(s) if s == "result"));
        assert!(tokens[1].kind.is_punctuation(PunctuationId::Question));
    }

    #[test]
    fn test_fat_arrow() {
        let Ok(tokens) = lex("x => y") else {
            panic!("lex failed");
        };
        assert!(tokens[1].kind.is_punctuation(PunctuationId::FatArrow));
    }

    #[test]
    fn test_case_keyword() {
        let Ok(tokens) = lex("case Some(x):") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Case));
    }

    #[test]
    fn test_pass_keyword() {
        let Ok(tokens) = lex("pass") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Pass));
    }

    #[test]
    fn test_mut_self() {
        let Ok(tokens) = lex("mut self") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Mut));
        assert!(tokens[1].kind.is_keyword(KeywordId::SelfKw));
    }

    #[test]
    fn test_fstring() {
        let Ok(tokens) = lex(r#"f"Hello {name}""#) else {
            panic!("lex failed");
        };
        assert!(matches!(&tokens[0].kind, TokenKind::FString(_)));
    }

    #[test]
    fn test_yield_keyword() {
        let Ok(tokens) = lex("yield value") else {
            panic!("lex failed");
        };
        assert!(tokens[0].kind.is_keyword(KeywordId::Yield));
        assert!(matches!(&tokens[1].kind, TokenKind::Ident(s) if s == "value"));
    }

    #[test]
    fn test_rust_keyword() {
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
    use super::{incan_debug_binary, strip_ansi_escapes};
    use incan::backend::IrCodegen;
    use incan::frontend::{lexer, parser, typechecker};
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn run_incan_source(source: &str) -> std::process::Output {
        Command::new(incan_debug_binary())
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap_or_else(|e| panic!("failed to run incan source: {e}"))
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
    fn test_std_fs_compile_and_run_path_file_and_tree_operations() -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("incan_std_fs_integration_{}", std::process::id()));
        let root = base.join("root");
        let copied = base.join("copy");
        let moved = base.join("moved");
        let source = format!(
            r#"
from std.fs import IoError, OpenOptions, Path
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
        let output = Command::new(incan_debug_binary())
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
                "true"
            ],
            "unexpected std.fs output:\n{stdout}"
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

        let output = Command::new(incan_debug_binary())
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
    fn test_std_fs_glob_string_api_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
    fn test_match_rust_result_non_clone_payload_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
    fn test_result_method_closure_callbacks_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
    fn test_collection_literal_spreads_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
    fn test_issue502_independent_union_narrowing_branches_compile_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
    fn test_field_backed_by_value_method_args_do_not_require_user_clone_issue241()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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
        let output = Command::new(incan_debug_binary())
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

        let output = Command::new(incan_debug_binary())
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
    fn test_build_web_route_uses_proc_macro_passthrough() {
        let project_dir = make_temp_dir("incan_web_proc_macro_test");
        let source_path = project_dir.join("main.incn");
        let out_dir = project_dir.join("out");
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

        let Ok(output) = Command::new(incan_debug_binary())
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
            "incan build web route failed: status={:?} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );

        let generated_main = out_dir.join("src/main.rs");
        let Ok(main_rs) = std::fs::read_to_string(&generated_main) else {
            panic!("failed to read generated Rust source");
        };
        assert!(
            main_rs.contains("#[incan_web_macros::route("),
            "expected generated web route to use proc macro passthrough:\n{}",
            main_rs
        );
        assert!(
            !main_rs.contains("__incan_router!"),
            "legacy __incan_router! macro should not be emitted:\n{}",
            main_rs
        );
        assert!(
            !main_rs.contains("set_router"),
            "legacy set_router() call should not be emitted:\n{}",
            main_rs
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

        let output = Command::new(incan_debug_binary())
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

        let Ok(output) = Command::new(incan_debug_binary())
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
        let build_output = Command::new(incan_debug_binary())
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

        let run_output = Command::new(incan_debug_binary())
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

        let output = Command::new(incan_debug_binary())
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

        let output = Command::new(incan_debug_binary())
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
        let Ok(output) = Command::new(incan_debug_binary())
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
        let Ok(output) = Command::new(incan_debug_binary())
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
        let Ok(output) = Command::new(incan_debug_binary())
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
        let Ok(output) = Command::new(incan_debug_binary())
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
    fn test_run_std_uuid_surface() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
    fn test_run_u128_modulo_floor_div() -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new(incan_debug_binary())
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
        let Ok(output) = Command::new(incan_debug_binary())
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

        let output = Command::new(incan_debug_binary())
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

        let output = Command::new(incan_debug_binary())
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
        let path = Path::new("benchmarks/sorting/quicksort/quicksort.incn");
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
        let Ok(output) = Command::new(incan_debug_binary())
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
        let Ok(output) = Command::new(incan_debug_binary())
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
            rust_code.contains("collect::<HashMap<_, _>>()"),
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
        let Ok(output) = Command::new(incan_debug_binary())
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
    fn test_std_async_race_helper_first_completion_runs() {
        let output = run_incan_source(
            r#"
from std.async.race import arm, race
from std.async.time import sleep

def label(value: int) -> str:
    return f"win:{value}"

async def fast() -> int:
    return 1

async def slow() -> int:
    await sleep(0.01)
    return 2

async def main() -> None:
    println(await race(arm(slow(), label), arm(fast(), label)))
"#,
        );
        assert!(
            output.status.success(),
            "std.async.race first-completion run failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout.lines().last().map(str::trim),
            Some("win:1"),
            "unexpected stdout:\n{stdout}"
        );
    }

    #[test]
    fn test_std_async_race_helper_ready_tie_uses_source_order() {
        let output = run_incan_source(
            r#"
from std.async.race import arm, race

def label(value: int) -> str:
    return f"win:{value}"

async def first() -> int:
    return 1

async def second() -> int:
    return 2

async def main() -> None:
    println(await race(arm(first(), label), arm(second(), label)))
"#,
        );
        assert!(
            output.status.success(),
            "std.async.race ready-tie run failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout.lines().last().map(str::trim),
            Some("win:1"),
            "unexpected stdout:\n{stdout}"
        );
    }

    #[test]
    fn test_race_for_expression_first_completion_runs_through_shared_runtime() {
        let output = run_incan_source(
            r#"
import std.async
from std.async.time import sleep

async def fast() -> int:
    return 1

async def slow() -> int:
    await sleep(0.01)
    return 2

async def main() -> None:
    prefix = "win"
    result = race for value:
        await slow() => f"{prefix}:{value}"
        await fast() => f"{prefix}:{value}"
    println(result)
"#,
        );
        assert!(
            output.status.success(),
            "race for first-completion run failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout.lines().last().map(str::trim),
            Some("win:1"),
            "unexpected stdout:\n{stdout}"
        );
    }

    #[test]
    fn test_race_for_expression_ready_tie_uses_stdlib_source_order() {
        let output = run_incan_source(
            r#"
import std.async

async def first() -> int:
    return 1

async def second() -> int:
    return 2

async def main() -> None:
    result = race for value:
        await first() => value
        await second() => value
    println(result)
"#,
        );
        assert!(
            output.status.success(),
            "race for ready-tie run failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = strip_ansi_escapes(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout.lines().last().map(str::trim),
            Some("1"),
            "unexpected stdout:\n{stdout}"
        );
    }

    #[test]
    fn test_std_math_module_constants_and_functions_run() {
        let Ok(output) = Command::new(incan_debug_binary())
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

        let output = Command::new(incan_debug_binary())
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
    fn test_rust_associated_call_in_elif_branch_uses_path_syntax() {
        let Ok(output) = Command::new(incan_debug_binary())
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
}

/// End-to-end integration tests for `incan test`.
///
/// These tests exercise the full pipeline: write an Incan test file → run `incan test` via the CLI → verify
/// stdout/stderr/exit code. They catch integration bugs like broken per-file `cargo test` harness wiring or parametrize
/// expansion that unit tests cannot detect.
mod test_runner_e2e {
    use super::incan_debug_binary;
    use std::path::Path;
    use std::process::Command;
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
        Command::new(incan_debug_binary())
            .args(["test", path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    /// Run `incan test` on a directory and return the combined output.
    fn run_incan_test(dir: &Path) -> std::process::Output {
        run_incan_test_path(dir)
    }

    /// Run `incan test` with extra flags.
    fn run_incan_test_with_args(dir: &Path, extra: &[&str]) -> std::process::Output {
        let mut cmd = Command::new(incan_debug_binary());
        cmd.arg("test");
        for arg in extra {
            cmd.arg(arg);
        }
        cmd.arg(dir.to_string_lossy().as_ref());
        cmd.env("CARGO_NET_OFFLINE", "true");
        cmd.output()
            .unwrap_or_else(|e| panic!("failed to run `incan test`: {}", e))
    }

    /// Run `incan test` with `cwd` and a relative path argument.
    fn run_incan_test_relative(cwd: &Path, relative_path: &str) -> std::process::Output {
        Command::new(incan_debug_binary())
            .arg("test")
            .arg(relative_path)
            .env("CARGO_NET_OFFLINE", "true")
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("failed to run `incan test {relative_path}`: {}", e))
    }

    /// Run `incan build <entry> <out_dir>` for an inline-test production source.
    fn run_incan_build(entry: &Path, out_dir: &Path) -> std::process::Output {
        let output = Command::new(incan_debug_binary())
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

    #[test]
    fn e2e_passing_test_succeeds() {
        let dir = write_test_project(
            "test_math.incn",
            r#"
from std.testing import assert_eq

def test_addition() -> None:
    assert_eq(1 + 1, 2)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected passing test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("PASSED") || stdout.contains("passed"),
            "expected PASSED in output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_two_tests_in_one_file_share_single_cargo_batch() {
        let dir = write_test_project(
            "test_pair.incn",
            r#"
from std.testing import assert_eq

def test_one() -> None:
    assert_eq(1, 1)

def test_two() -> None:
    assert_eq(2, 2)
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
            stdout.contains("test_pair.incn::test_one") && stdout.contains("test_pair.incn::test_two"),
            "expected each test name in reporter output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.match_indices("PASSED").count() >= 2,
            "expected two passing results (per-test PASSED lines).\nstdout:\n{}",
            stdout,
        );
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
    fn e2e_explicit_test_decorator_discovers_non_prefixed_function() {
        let dir = write_test_project(
            "test_decorator.incn",
            r#"
from std.testing import assert_eq, test

@test
def verifies_total() -> None:
    assert_eq(40 + 2, 42)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected @test-decorated function to run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_decorator.incn::verifies_total"),
            "expected decorated test id in output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_list_and_keyword_filter_use_stable_test_ids() {
        let dir = write_test_project(
            "test_list_filter.incn",
            r#"
from std.testing import assert_eq

def test_alpha() -> None:
    assert_eq(1, 1)

def test_beta() -> None:
    assert_eq(2, 2)
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--list", "-k", "test_beta"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected --list -k run to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.lines().any(|line| line == "test_list_filter.incn::test_beta"),
            "expected exact listed beta id rooted at the explicit test directory.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains(dir.to_string_lossy().as_ref()),
            "expected --list output to avoid machine-local absolute paths.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains("test_list_filter.incn::test_alpha"),
            "expected keyword filter to hide alpha.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_json_format_emits_result_records() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_json_report.incn",
            r#"
from std.testing import assert_eq

def test_json_one() -> None:
    assert_eq(1, 1)
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--format", "json", "--shuffle", "--seed", "7"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected JSON-format run to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        let mut saw_result = false;
        let mut saw_summary = false;
        for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line)?;
            if value.get("test_id").is_some() {
                saw_result = true;
                assert_eq!(
                    value.get("schema_version").and_then(|v| v.as_str()),
                    Some("incan.test.v1")
                );
                assert_eq!(
                    value.get("test_id").and_then(|v| v.as_str()),
                    Some("test_json_report.incn::test_json_one")
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
            stdout
        );
        assert!(saw_summary, "expected a JSON summary record.\nstdout:\n{}", stdout);
        Ok(())
    }

    #[test]
    fn e2e_junit_report_writes_testcase_xml() {
        let dir = write_test_project(
            "test_junit_report.incn",
            r#"
from std.testing import assert_eq

def test_junit_one() -> None:
    assert_eq(1, 1)
"#,
        );
        let report = dir.join("reports").join("junit.xml");
        let report_arg = report.to_string_lossy().to_string();
        let output = run_incan_test_with_args(&dir, &["--junit", report_arg.as_str()]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected JUnit report run to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        let Ok(xml) = std::fs::read_to_string(&report) else {
            panic!("failed to read {}", report.display());
        };
        assert!(
            xml.contains("<testsuite") && xml.contains("test_junit_one"),
            "expected JUnit XML with test case, got:\n{}",
            xml,
        );
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
    fn e2e_conftest_fixture_is_visible_to_nested_tests() {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "conftest_fixture"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests").join("unit");
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            dir.join("tests").join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def answer() -> int:
    return 42
"#,
        ) {
            panic!("failed to write conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_answer.incn"),
            r#"
from std.testing import assert_eq

def test_answer(answer: int) -> None:
    assert_eq(answer, 42)
"#,
        ) {
            panic!("failed to write nested test: {}", err);
        }

        let output = run_incan_test_relative(&dir, "tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected conftest fixture injection to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_answer.incn::test_answer"),
            "expected nested stable id in output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_nested_test_root_uses_same_conftest_boundary_for_collection_and_execution() {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "nested_conftest_boundary"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        let unit_dir = tests_dir.join("unit");
        if let Err(err) = std::fs::create_dir_all(&unit_dir) {
            panic!("failed to create nested tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def answer() -> int:
    return 1
"#,
        ) {
            panic!("failed to write parent conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            unit_dir.join("conftest.incn"),
            r#"
from std.testing import fixture

@fixture
def answer() -> int:
    return 2
"#,
        ) {
            panic!("failed to write nested conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            unit_dir.join("test_value.incn"),
            r#"
from std.testing import assert_eq

def test_answer(answer: int) -> None:
    assert_eq(answer, 2)
"#,
        ) {
            panic!("failed to write nested conftest test: {}", err);
        }

        let output = run_incan_test(&unit_dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected nested root run to use only root-bounded conftest sources.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
    }

    #[test]
    fn e2e_nested_conftest_fixture_overrides_parent_fixture() {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "nested_conftest_precedence"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        let unit_dir = tests_dir.join("unit");
        if let Err(err) = std::fs::create_dir_all(&unit_dir) {
            panic!("failed to create nested tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("conftest.incn"),
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
            unit_dir.join("conftest.incn"),
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
            unit_dir.join("test_precedence.incn"),
            r#"
from std.testing import assert_eq

def test_uses_nearest_fixture(shared: str) -> None:
    assert_eq(shared, "child")
"#,
        ) {
            panic!("failed to write nested conftest test: {}", err);
        }

        let output = run_incan_test(&dir);
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
    fn e2e_builtin_tmp_path_fixture_is_injected() {
        let dir = write_test_project(
            "test_tmp_path.incn",
            r#"
from std.testing import assert_eq
from rust::std::path import PathBuf

def test_tmp_path_fixture(tmp_path: PathBuf) -> None:
    assert_eq(tmp_path.exists(), true)
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
    }

    #[test]
    fn e2e_std_testing_assert_helper_is_normalized_before_codegen() {
        let dir = write_test_project(
            "test_assert_helper.incn",
            r#"
import std.testing as testing

def test_assert_helper() -> None:
    testing.assert(True)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected one-argument std.testing.assert call to run without generated Rust string rewriting.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );
        assert!(stdout.contains("test_assert_helper"));
    }

    #[test]
    fn e2e_marker_expr_and_strict_markers_use_conftest_registry() {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "strict_markers"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("conftest.incn"),
            r#"
const TEST_MARKERS: List[str] = ["smoke"]
const TEST_MARKS: List[str] = ["smoke"]
"#,
        ) {
            panic!("failed to write conftest: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_markers.incn"),
            r#"
from std.testing import assert_eq

def test_inherited_smoke() -> None:
    assert_eq(1, 1)

def test_other() -> None:
    assert_eq(1, 1)
"#,
        ) {
            panic!("failed to write marker test: {}", err);
        }

        let listed = run_incan_test_with_args(&tests_dir, &["--list", "-m", "smoke", "--strict-markers"]);
        let stdout = String::from_utf8_lossy(&listed.stdout);
        let stderr = String::from_utf8_lossy(&listed.stderr);
        assert!(
            listed.status.success(),
            "expected strict registered marker list to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_markers.incn::test_inherited_smoke"));

        let strict_error = run_incan_test_with_args(&tests_dir, &["--list", "-m", "missing", "--strict-markers"]);
        let strict_stdout = String::from_utf8_lossy(&strict_error.stdout);
        let strict_stderr = String::from_utf8_lossy(&strict_error.stderr);
        assert!(
            !strict_error.status.success(),
            "expected unknown strict marker to fail.\nstdout:\n{}\nstderr:\n{}",
            strict_stdout,
            strict_stderr,
        );
        assert!(strict_stderr.contains("unknown marker `missing`"));
    }

    #[test]
    fn e2e_marker_expr_boolean_grammar_filters_tests() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_marker_expr.incn",
            r#"
from std.testing import assert_eq, mark, slow

const TEST_MARKERS: List[str] = ["api", "db"]

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
"#,
        );

        let output = run_incan_test_with_args(
            &dir,
            &["--list", "-m", "api and not slow", "--strict-markers", "--slow"],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected boolean marker expression to collect.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_marker_expr.incn::test_api"));
        assert!(!stdout.contains("test_marker_expr.incn::test_api_slow"));
        assert!(!stdout.contains("test_marker_expr.incn::test_db"));

        let invalid = run_incan_test_with_args(&dir, &["--list", "-m", "api and ("]);
        let invalid_stderr = String::from_utf8_lossy(&invalid.stderr);
        assert!(
            !invalid.status.success(),
            "expected invalid marker expression to fail.\nstderr:\n{}",
            invalid_stderr,
        );
        assert!(invalid_stderr.contains("expected marker name or parenthesized expression"));
        Ok(())
    }

    #[test]
    fn e2e_slow_marker_is_excluded_by_default_and_included_with_flag() {
        let dir = write_test_project(
            "test_slow_filter.incn",
            r#"
from std.testing import assert_eq, slow

def test_fast() -> None:
    assert_eq(1, 1)

@slow
def test_slow_case() -> None:
    assert_eq(1, 1)
"#,
        );

        let default_list = run_incan_test_with_args(&dir, &["--list"]);
        let default_stdout = String::from_utf8_lossy(&default_list.stdout);
        assert!(
            default_list.status.success(),
            "expected default list to succeed.\nstdout:\n{}",
            default_stdout,
        );
        assert!(default_stdout.contains("test_slow_filter.incn::test_fast"));
        assert!(!default_stdout.contains("test_slow_filter.incn::test_slow_case"));

        let slow_list = run_incan_test_with_args(&dir, &["--list", "--slow"]);
        let slow_stdout = String::from_utf8_lossy(&slow_list.stdout);
        assert!(
            slow_list.status.success(),
            "expected --slow list to succeed.\nstdout:\n{}",
            slow_stdout,
        );
        assert!(slow_stdout.contains("test_slow_filter.incn::test_fast"));
        assert!(slow_stdout.contains("test_slow_filter.incn::test_slow_case"));
    }

    #[test]
    fn e2e_parametrize_case_ids_and_marks_affect_collection() {
        let dir = write_test_project(
            "test_case_ids.incn",
            r#"
from std.testing import assert_eq, param_case, parametrize, xfail

@parametrize("x, expected", [
    param_case((1, 3), marks=[xfail("known")], id="one-three"),
    (2, 4),
], ids=["ignored", "two-four"])
def test_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)
"#,
        );

        let listed = run_incan_test_with_args(&dir, &["--list"]);
        let stdout = String::from_utf8_lossy(&listed.stdout);
        let stderr = String::from_utf8_lossy(&listed.stderr);
        assert!(
            listed.status.success(),
            "expected parametrized list to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_case_ids.incn::test_double[one-three]"));
        assert!(stdout.contains("test_case_ids.incn::test_double[two-four]"));

        let run = run_incan_test(&dir);
        let run_stdout = String::from_utf8_lossy(&run.stdout);
        let run_stderr = String::from_utf8_lossy(&run.stderr);
        assert!(
            run.status.success(),
            "expected xfailed case and passing case to make the run succeed.\nstdout:\n{}\nstderr:\n{}",
            run_stdout,
            run_stderr,
        );
        assert!(run_stdout.contains("xfailed") || run_stdout.contains("XFAIL"));
    }

    #[test]
    fn e2e_stacked_parametrize_lists_cartesian_product_ids() {
        let dir = write_test_project(
            "test_parametrize_product.incn",
            r#"
from std.testing import assert_eq, parametrize

@parametrize("x", [1, 2], ids=["one", "two"])
@parametrize("y", [10, 20], ids=["ten", "twenty"])
def test_pair(x: int, y: int) -> None:
    assert_eq(x < y, true)
"#,
        );

        let listed = run_incan_test_with_args(&dir, &["--list"]);
        let stdout = String::from_utf8_lossy(&listed.stdout);
        let stderr = String::from_utf8_lossy(&listed.stderr);
        assert!(
            listed.status.success(),
            "expected stacked parametrized list to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_parametrize_product.incn::test_pair[one-ten]"));
        assert!(stdout.contains("test_parametrize_product.incn::test_pair[one-twenty]"));
        assert!(stdout.contains("test_parametrize_product.incn::test_pair[two-ten]"));
        assert!(stdout.contains("test_parametrize_product.incn::test_pair[two-twenty]"));
    }

    #[test]
    fn e2e_parametrize_arity_mismatch_is_collection_error() {
        let dir = write_test_project(
            "test_parametrize_arity.incn",
            r#"
from std.testing import parametrize

@parametrize("x, y", [1])
def test_bad_case(x: int, y: int) -> None:
    pass
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected arity mismatch to fail during collection.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stderr.contains("parametrize case `1`"));
        assert!(stderr.contains("expected 2 value(s)"));
    }

    #[test]
    fn e2e_timeout_marks_slow_test_failed() {
        let dir = write_test_project(
            "test_timeout.incn",
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_slow() -> None:
    sleep(Duration.from_millis(100))
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--timeout", "1ms"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected timeout run to fail.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("timed out after"));
    }

    #[test]
    fn e2e_conditional_markers_evaluate_collection_probes() {
        let platform = std::env::consts::OS;
        let dir = write_test_project(
            "test_conditional_markers.incn",
            &format!(
                r#"
from std.testing import assert_eq, feature, platform, skipif, xfailif

@skipif(platform() == "{platform}", reason="host platform")
def test_skip_on_platform_probe() -> None:
    assert_eq(1, 0)

@xfailif(feature("known_bug"), reason="feature-gated known issue")
def test_feature_xfail() -> None:
    assert_eq(1, 0)
"#
            ),
        );

        let without_feature = run_incan_test_with_args(&dir, &["-k", "test_feature_xfail"]);
        let without_stdout = String::from_utf8_lossy(&without_feature.stdout);
        let without_stderr = String::from_utf8_lossy(&without_feature.stderr);
        assert!(
            !without_feature.status.success(),
            "expected feature-gated xfail to run as an ordinary failing test without --feature.\nstdout:\n{}\nstderr:\n{}",
            without_stdout,
            without_stderr,
        );

        let output = run_incan_test_with_args(&dir, &["--feature", "known_bug"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected skipif/xfailif probes to make the run successful.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("SKIPPED") || stdout.contains("skipped"));
        assert!(stdout.contains("XFAIL") || stdout.contains("xfailed"));
    }

    #[test]
    fn e2e_conditional_marker_rejects_runtime_expression() {
        let dir = write_test_project(
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

        let output = run_incan_test(&dir);
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
    sleep(Duration.from_millis(1200))
"#,
        );
        let second = dir.join("test_sleep_b.incn");
        std::fs::write(
            &second,
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_sleep_b() -> None:
    sleep(Duration.from_millis(1200))
"#,
        )?;

        let sequential_start = std::time::Instant::now();
        let sequential = run_incan_test_with_args(&dir, &["--jobs", "1"]);
        let sequential_elapsed = sequential_start.elapsed();
        let sequential_stdout = String::from_utf8_lossy(&sequential.stdout);
        let sequential_stderr = String::from_utf8_lossy(&sequential.stderr);
        assert!(
            sequential.status.success(),
            "expected sequential warm-up run to pass.\nstdout:\n{}\nstderr:\n{}",
            sequential_stdout,
            sequential_stderr,
        );

        let parallel_start = std::time::Instant::now();
        let parallel = run_incan_test_with_args(&dir, &["--jobs", "2"]);
        let parallel_elapsed = parallel_start.elapsed();
        let parallel_stdout = String::from_utf8_lossy(&parallel.stdout);
        let parallel_stderr = String::from_utf8_lossy(&parallel.stderr);
        assert!(
            parallel.status.success(),
            "expected parallel run to pass.\nstdout:\n{}\nstderr:\n{}",
            parallel_stdout,
            parallel_stderr,
        );
        assert!(
            parallel_elapsed + std::time::Duration::from_millis(500) < sequential_elapsed,
            "expected --jobs 2 to run independent file batches concurrently; sequential={:?}, parallel={:?}\nparallel stdout:\n{}",
            sequential_elapsed,
            parallel_elapsed,
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
    sleep(Duration.from_millis(3000))
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
    sleep(Duration.from_millis(1000))
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
    sleep(Duration.from_millis(1000))
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
            elapsed >= std::time::Duration::from_millis(1800),
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
    sleep(Duration.from_millis(1000))
"#,
        );
        std::fs::write(
            dir.join("test_regular.incn"),
            r#"
from rust::std::thread import sleep
from rust::std::time import Duration

def test_regular() -> None:
    sleep(Duration.from_millis(1000))
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
            elapsed >= std::time::Duration::from_millis(1800),
            "expected @serial worker to run alone; elapsed={:?}\nstdout:\n{}",
            elapsed,
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_nocapture_prints_passing_test_output() {
        let dir = write_test_project(
            "test_capture.incn",
            r#"
def test_prints() -> None:
    print("VISIBLE_CAPTURE")
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--nocapture"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected nocapture run to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("VISIBLE_CAPTURE"));
    }

    #[test]
    fn e2e_sequential_single_file_runs_do_not_cross_wire_relative_paths() {
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
    }

    #[test]
    fn e2e_sequential_single_file_runs_do_not_cross_wire_absolute_paths() {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "session_isolation_absolute"
version = "0.1.0"
"#,
        );
        let tests_dir = dir.join("tests");
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
    fn e2e_test_runner_preserves_project_fixture_cwd_for_file_and_batch_runs() {
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
    }

    #[test]
    fn e2e_test_runner_preserves_fixture_cwd_without_manifest_for_file_and_batch_runs() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let mut dir = std::env::temp_dir();
        let Ok(duration) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            panic!("system time before UNIX epoch");
        };
        dir.push(format!("incan_e2e_test_nomani_{}", duration.as_nanos()));
        if let Err(err) = std::fs::create_dir_all(&dir) {
            panic!("failed to create temp dir: {}", err);
        }
        let tests_dir = dir.join("tests");
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

        let single = run_incan_test_relative(&dir, "tests/test_cwd.incn");
        let single_stdout = String::from_utf8_lossy(&single.stdout);
        let single_stderr = String::from_utf8_lossy(&single.stderr);
        assert!(
            single.status.success(),
            "expected manifest-less single-file fixture-path run to succeed.\nstdout:\n{}\nstderr:\n{}",
            single_stdout,
            single_stderr,
        );

        let batch = run_incan_test_relative(&dir, "tests");
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
    fn e2e_imported_pub_static_scalar_read_in_tests_succeeds() {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "pub_static_scalar_read"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        let tests_dir = dir.join("tests");

        if let Err(err) = std::fs::create_dir_all(&src_dir) {
            panic!("failed to create src dir: {}", err);
        }
        if let Err(err) = std::fs::create_dir_all(&tests_dir) {
            panic!("failed to create tests dir: {}", err);
        }
        if let Err(err) = std::fs::write(src_dir.join("widgets.incn"), "pub static MARKER: int = 41\n") {
            panic!("failed to write widgets source: {}", err);
        }
        if let Err(err) = std::fs::write(
            tests_dir.join("test_widgets_static.incn"),
            r#"
from std.testing import assert_eq
from widgets import MARKER

def test_imported_pub_static_scalar_read() -> None:
    assert_eq(MARKER, 41)
"#,
        ) {
            panic!("failed to write widget static test: {}", err);
        }

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected imported pub static scalar read test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
    }

    #[test]
    fn e2e_empty_list_arguments_in_tests_preserve_string_element_type() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "empty_list_test"
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
pub def count_names(names: List[str]) -> int:
    return len(names)
"#,
        )?;
        std::fs::write(
            tests_dir.join("test_empty_names.incn"),
            r#"
from std.testing import assert_eq
from helpers import count_names

def test_empty_names() -> None:
    assert_eq(count_names([]), 0)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected empty list string arg test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
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

        Ok(())
    }

    #[test]
    fn e2e_assert_statement_with_module_import_succeeds() {
        let dir = write_test_project(
            "test_assert_stmt.incn",
            r#"
import std.testing

def test_assert_statement_sugar() -> None:
    assert 1 + 1 == 2
    assert 3 != 4
    assert not False
    assert True
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected assert-statement test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("PASSED") || stdout.contains("passed"),
            "expected PASSED in output.\nstdout:\n{}",
            stdout,
        );
    }

    #[test]
    fn e2e_inline_module_tests_are_discovered_and_run() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "inline_module_tests_run"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
def add(a: int, b: int) -> int:
    return a + b

def main() -> None:
    pass

module tests:
    from std.testing import assert_eq

    def test_addition() -> None:
        assert_eq(add(2, 3), 5)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected inline module test run to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("1 passed"),
            "expected inline test to run.\nstdout:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_module_tests_can_access_private_enclosing_names() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "inline_private_access"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
def secret() -> str:
    return "private"

def main() -> None:
    pass

module tests:
    from std.testing import assert_eq

    def test_secret() -> None:
        assert_eq(secret(), "private")
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected inline module test to access enclosing private helper.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_module_std_testing_assert_helper_is_normalized_before_codegen()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "inline_assert_helper"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
def main() -> None:
    pass

module tests:
    import std.testing as testing

    def test_assert_helper() -> None:
        testing.assert(True)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected inline one-argument std.testing.assert call to be normalized before codegen.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr
        );
        assert!(stdout.contains("test_assert_helper"));
        Ok(())
    }

    #[test]
    fn e2e_inline_module_test_imports_do_not_affect_build() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "inline_imports_do_not_affect_build"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"
def main() -> None:
    println("production")

module tests:
    from std.testing import assert_eq

    def test_production() -> None:
        assert_eq(1 + 1, 2)
"#,
        )?;

        let out_dir = dir.join("out");
        let output = run_incan_build(&entry, &out_dir);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected production build to ignore inline test imports.\nstderr:\n{}",
            stderr,
        );
        let main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            !main_rs.contains("__incan_std::testing"),
            "inline test import should not leak into generated production code:\n{}",
            main_rs,
        );
        assert!(
            !main_rs.contains("test_production"),
            "inline test function should not leak into generated production code:\n{}",
            main_rs,
        );
        Ok(())
    }

    #[test]
    fn e2e_inline_module_test_decorator_list_and_keyword_filter() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "inline_decorator_list_filter"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("math.incn"),
            r#"
def add(a: int, b: int) -> int:
    return a + b

module tests:
    from std.testing import assert_eq, test

    @test
    def checks_sum() -> None:
        assert_eq(add(20, 22), 42)

    def test_by_name() -> None:
        assert_eq(add(1, 1), 2)
"#,
        )?;

        let output = run_incan_test_with_args(&dir, &["--list", "-k", "checks_sum"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected inline --list -k run to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.lines().any(|line| line == "src/math.incn::checks_sum"),
            "expected decorated inline test id in --list output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            !stdout.contains("src/math.incn::test_by_name"),
            "expected keyword filter to hide the name-discovered inline test.\nstdout:\n{}",
            stdout,
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
    fn e2e_inline_module_fixtures_builtins_and_autouse() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "inline_fixture_builtins"
version = "0.1.0"
"#,
        );
        let src_dir = dir.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("main.incn"),
            r#"
module tests:
    from rust::incan_stdlib::testing import TestEnv
    from rust::std::path import PathBuf
    from std.testing import assert_eq, assert_is_some, fixture

    @fixture(autouse=true)
    def seed() -> int:
        return 40

    @fixture
    def answer(seed: int) -> int:
        return seed + 2

    def test_fixture_and_tmp_path(answer: int, tmp_path: PathBuf) -> None:
        assert_eq(answer, 42)
        assert_eq(tmp_path.exists(), true)

    def test_tmp_workdir(tmp_workdir: PathBuf) -> None:
        assert_eq(tmp_workdir.exists(), true)

    def test_env_fixture(mut env: TestEnv) -> None:
        env.set("INCAN_INLINE_ENV_FIXTURE", "set")
        assert_eq(assert_is_some(env.get("INCAN_INLINE_ENV_FIXTURE")), "set")
        env.unset("INCAN_INLINE_ENV_FIXTURE")
        assert_eq(env.get("INCAN_INLINE_ENV_FIXTURE"), None)
"#,
        )?;

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected inline fixtures and built-ins to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_fixture_and_tmp_path"),
            "expected inline fixture test name in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_tmp_workdir"),
            "expected inline tmp_workdir test name in output.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_module_scoped_fixture_is_reused_within_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_module_scope_fixture.incn",
            r#"
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture(scope="module")
def once() -> int:
    calls += 1
    return calls

def test_first(once: int) -> None:
    assert_eq(once, 1)

def test_second(once: int) -> None:
    assert_eq(once, 1)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected module-scoped fixture value to be reused across tests in the same file.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(stdout.contains("test_first") && stdout.contains("test_second"));
        Ok(())
    }

    #[test]
    fn e2e_yield_fixture_teardown_runs_after_failure() -> Result<(), Box<dyn std::error::Error>> {
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

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected the intentionally failing test to fail the run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_2_observes_teardown PASSED"),
            "expected teardown to run before the following test observed shared state.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_yield_fixture_teardown_failure_fails_run() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_yield_fixture_teardown_failure.incn",
            r#"
from std.testing import assert_eq, fixture

@fixture
def resource() -> int:
    yield 42
    assert_eq(1, 2)

def test_body_passes(resource: int) -> None:
    assert_eq(resource, 42)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected teardown failure to fail the run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_body_passes FAILED") || stderr.contains("test_body_passes"),
            "expected passing body with failing teardown to be reported as failed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_yield_fixture_teardown_failures_are_aggregated() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_yield_fixture_teardown_aggregate.incn",
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
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");
        assert!(
            !output.status.success(),
            "expected aggregate teardown failures to fail the run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            combined.contains("fixture teardown failed")
                && combined.contains("child teardown failed")
                && combined.contains("parent teardown failed"),
            "expected both teardown failures in aggregate output.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_yield_fixture_teardown_captures_setup_locals() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_yield_fixture_capture.incn",
            r#"
from std.testing import assert_eq, fixture

static observed: int = 0

@fixture
def resource() -> int:
    value: int = 41
    yield value + 1
    observed += value

def test_body(resource: int) -> None:
    assert_eq(resource, 42)

def test_after_teardown() -> None:
    assert_eq(observed, 41)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected yield teardown to capture setup locals.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_module_yield_fixture_teardown_runs_at_module_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_module_yield_fixture.incn",
            r#"
from std.testing import assert_eq, fixture

static calls: int = 0

@fixture(scope="module")
def shared() -> int:
    yield 10
    assert_eq(calls, 2)

def test_first(shared: int) -> None:
    calls += 1
    assert_eq(shared, 10)

def test_second(shared: int) -> None:
    calls += 1
    assert_eq(shared, 10)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected module yield teardown after all tests in the file.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_session_fixture_reused_across_files_with_single_worker() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "incan.toml",
            r#"[project]
name = "session_fixture_reuse"
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

        let output = run_incan_test_with_args(&dir, &["--jobs", "1"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected session fixture to be reused across files in one worker batch.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_yield_fixture_teardown_runs_in_reverse_dependency_order() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_yield_teardown_order.incn",
            r#"
from std.testing import assert_eq, fixture

static order: int = 0

@fixture
def outer() -> int:
    yield 1
    assert_eq(order, 1)
    order += 1

@fixture
def inner(outer: int) -> int:
    yield outer + 1
    assert_eq(order, 0)
    order += 1

def test_body(inner: int) -> None:
    assert_eq(inner, 2)

def test_after() -> None:
    assert_eq(order, 2)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected dependent fixtures to tear down in reverse dependency order.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_async_yield_fixture_setup_and_teardown_are_awaited() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_async_yield_fixture.incn",
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture

static order: int = 0

@fixture
def seed() -> int:
    order += 1
    return 40

@fixture
async def resource(seed: int) -> int:
    await sleep_ms(1)
    order += 1
    yield seed + 2
    await sleep_ms(1)
    order += 10

def test_1_uses_async_fixture(resource: int) -> None:
    assert_eq(resource, 42)
    assert_eq(order, 2)

def test_2_observes_async_teardown() -> None:
    assert_eq(order, 12)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected async fixture setup and teardown to be awaited.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_async_yield_fixture_teardown_runs_after_failure() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_async_yield_fixture_failure.incn",
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
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "expected the intentionally failing async-fixture test to fail the run.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_2_observes_async_teardown PASSED"),
            "expected async teardown to run before the following test observed shared state.\nstdout:\n{}",
            stdout,
        );
        Ok(())
    }

    #[test]
    fn e2e_async_yield_fixture_teardown_runs_in_reverse_dependency_order() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_async_yield_teardown_order.incn",
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture

static order: str = ""

@fixture
async def parent() -> int:
    order += "setup-parent;"
    await sleep_ms(1)
    yield 1
    await sleep_ms(1)
    order += "teardown-parent;"

@fixture
async def child(parent: int) -> int:
    order += "setup-child;"
    await sleep_ms(1)
    yield parent + 1
    await sleep_ms(1)
    order += "teardown-child;"

def test_1_uses_child(child: int) -> None:
    assert_eq(child, 2)
    assert_eq(order, "setup-parent;setup-child;")

def test_2_observes_reverse_teardown() -> None:
    assert_eq(order, "setup-parent;setup-child;teardown-child;teardown-parent;")
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected async yield teardowns to run in reverse dependency order.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        Ok(())
    }

    #[test]
    fn e2e_async_fixture_composes_with_parametrize_before_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let dir = write_test_project(
            "test_async_param_fixture.incn",
            r#"
from std.async import sleep_ms
from std.testing import assert_eq, fixture, parametrize

static setups: int = 0

@fixture
async def base() -> int:
    setups += 1
    await sleep_ms(1)
    yield 10

@parametrize("value", [1, 2])
async def test_param_async_fixture(value: int, base: int) -> None:
    await sleep_ms(1)
    assert_eq(base, 10)
    assert_eq(value > 0, true)

def test_after_param_cases() -> None:
    assert_eq(setups, 2)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "expected parametrized async tests to resolve async fixtures per expanded case.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );
        assert!(
            stdout.contains("test_param_async_fixture[1]") && stdout.contains("test_param_async_fixture[2]"),
            "expected both parametrized async cases in reporter output.\nstdout:\n{}",
            stdout,
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
    fn e2e_assert_failure_message_is_reported() {
        let dir = write_test_project(
            "test_assert_message.incn",
            r#"
def test_message() -> None:
    assert False, "custom boom"
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");

        assert!(
            !output.status.success(),
            "expected assertion failure test to fail.\n{}",
            combined,
        );
        assert!(
            combined.contains("AssertionError: custom boom"),
            "expected custom assertion message in output.\n{}",
            combined,
        );
    }

    #[test]
    fn e2e_assert_eq_failure_reports_kind_and_message() {
        let dir = write_test_project(
            "test_assert_eq_message.incn",
            r#"
def test_eq_message() -> None:
    assert 1 == 2, "math broke"
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{stdout}\n{stderr}");

        assert!(
            !output.status.success(),
            "expected assertion failure test to fail.\n{}",
            combined,
        );
        assert!(
            combined.contains("AssertionError: math broke"),
            "expected custom equality assertion message in output.\n{}",
            combined,
        );
        assert!(
            combined.contains("left != right"),
            "expected equality failure kind in output.\n{}",
            combined,
        );
    }

    // ---- Failing test ----

    #[test]
    fn e2e_failing_test_reports_failure() {
        let dir = write_test_project(
            "test_bad.incn",
            r#"
from std.testing import assert_eq

def test_wrong() -> None:
    assert_eq(1 + 1, 99)
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            !output.status.success(),
            "expected failing test to exit non-zero.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("FAILED") || stdout.contains("failed"),
            "expected FAILED in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Skip marker ----

    #[test]
    fn e2e_skip_marker_skips_test() {
        let dir = write_test_project(
            "test_skip.incn",
            r#"
from std.testing import skip

@skip("not implemented yet")
def test_todo() -> None:
    pass
"#,
        );

        let output = run_incan_test(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);

        assert!(
            output.status.success(),
            "expected skipped test to succeed overall.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("SKIPPED") || stdout.contains("skipped"),
            "expected SKIPPED in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Parametrize expansion ----

    #[test]
    fn e2e_parametrize_expands_and_runs_all_cases() {
        let dir = write_test_project(
            "test_param.incn",
            r#"
from std.testing import parametrize, assert_eq

@parametrize("a, b, expected", [(1, 2, 3), (10, 20, 30), (0, 0, 0)])
def test_add(a: int, b: int, expected: int) -> None:
    assert_eq(a + b, expected)
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--verbose"]);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "expected parametrized test to succeed.\nstdout:\n{}\nstderr:\n{}",
            stdout,
            stderr,
        );

        // All three parametrized variants should appear in the output.
        assert!(
            stdout.contains("test_add[1-2-3]"),
            "expected test_add[1-2-3] in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_add[10-20-30]"),
            "expected test_add[10-20-30] in output.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("test_add[0-0-0]"),
            "expected test_add[0-0-0] in output.\nstdout:\n{}",
            stdout,
        );

        // Should report 3 passed
        assert!(
            stdout.contains("3 passed"),
            "expected '3 passed' in output.\nstdout:\n{}",
            stdout,
        );
    }

    // ---- Parametrize with a failing case ----

    #[test]
    fn e2e_parametrize_reports_failing_case() {
        let dir = write_test_project(
            "test_param_fail.incn",
            r#"
from std.testing import parametrize, assert_eq

@parametrize("x, expected", [(2, 4), (3, 7)])
def test_double(x: int, expected: int) -> None:
    assert_eq(x * 2, expected)
"#,
        );

        let output = run_incan_test_with_args(&dir, &["--verbose"]);
        let stdout = String::from_utf8_lossy(&output.stdout);

        // 2*2==4 passes, 3*2==6!=7 fails
        assert!(
            !output.status.success(),
            "expected one failing case to make the run fail.\nstdout:\n{}",
            stdout,
        );
        assert!(
            stdout.contains("1 passed") && stdout.contains("1 failed"),
            "expected '1 passed' and '1 failed'.\nstdout:\n{}",
            stdout,
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
        let output = std::process::Command::new(super::incan_debug_binary())
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
    use incan::library_manifest::{FunctionExport, LibraryManifest, ModelExport, ParamExport, TypeRef};
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    fn incan_bin_path() -> std::path::PathBuf {
        super::incan_debug_binary()
    }

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
        Ok(Command::new(incan_bin_path()).arg("--check").arg(main_path).output()?)
    }

    fn run_build(main_path: &Path, out_dir: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_bin_path())
            .args([
                "build",
                main_path.to_string_lossy().as_ref(),
                out_dir.to_string_lossy().as_ref(),
            ])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn run_lock(entry_path: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_bin_path())
            .args(["lock", entry_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    fn run_test(target: &Path) -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(Command::new(incan_bin_path())
            .args(["test", target.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
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
        Ok(Command::new(incan_bin_path())
            .args(["build", "--lib"])
            .current_dir(project_root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?)
    }

    #[test]
    fn explicit_serialize_trait_adoption_runs_with_default_to_json() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"serialize_trait_default\"\n",
            "from std.serde.json import Serialize\n\nmodel Payload with Serialize:\n  value: int\n\ndef main() -> None:\n  println(Payload(value=1).to_json())\n",
        )?;

        let output = Command::new(incan_bin_path())
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected explicit Serialize adoption to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("{\"value\":1}"),
            "expected JSON output from default Serialize trait implementation, got:\n{}",
            stdout
        );
        Ok(())
    }

    #[test]
    fn generated_runtime_helpers_run_for_pop_min_max_and_to_json() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"generated_runtime_helpers\"\nversion = \"0.3.0-dev.1\"\n",
            "from std.serde.json import Serialize\n\nmodel Payload with Serialize:\n  value: int\n\ndef main() -> None:\n  mut xs = [3, 1, 4]\n  println(xs.pop())\n  println(min(xs))\n  println(max(xs))\n  println(Payload(value=2).to_json())\n",
        )?;

        let output = Command::new(incan_bin_path())
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected generated runtime helper path project to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines.first().copied(),
            Some("4"),
            "expected xs.pop() output first, got:\n{stdout}"
        );
        assert_eq!(
            lines.get(1).copied(),
            Some("1"),
            "expected min(xs) after pop, got:\n{stdout}"
        );
        assert_eq!(
            lines.get(2).copied(),
            Some("3"),
            "expected max(xs) after pop, got:\n{stdout}"
        );
        assert_eq!(
            lines.get(3).copied(),
            Some("{\"value\":2}"),
            "expected Payload.to_json() output, got:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn std_json_deserialize_from_json_runs_through_incan_surface() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"std_json_deserialize_from_json\"\nversion = \"0.3.0-dev.1\"\n",
            r#"from std.serde import json

@derive(json)
model Payload:
  value: int
  label: str

def main() -> None:
  match Payload.from_json('{"value":7,"label":"dogfood"}'):
    case Ok(payload):
      println(payload.to_json())
    case Err(err):
      println(err)
"#,
        )?;

        let output = Command::new(incan_bin_path())
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected std JSON Deserialize.from_json to run successfully through Incan source.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.lines().next(),
            Some("{\"value\":7,\"label\":\"dogfood\"}"),
            "expected round-tripped JSON payload, got:\n{stdout}"
        );
        Ok(())
    }

    #[test]
    fn generated_runtime_helpers_support_frozen_float_list_min_max() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"generated_runtime_helpers_frozen_float\"\nversion = \"0.3.0-dev.1\"\n",
            "const NUMBERS: FrozenList[float] = [3.0, 1.5, 4.25]\n\ndef main() -> None:\n  println(min(NUMBERS))\n  println(max(NUMBERS))\n",
        )?;

        let output = Command::new(incan_bin_path())
            .arg("run")
            .arg(&main_path)
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;

        assert!(
            output.status.success(),
            "expected frozen-list min/max helper path project to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(
            lines.first().copied(),
            Some("1.5"),
            "expected min(NUMBERS) output first, got:\n{stdout}"
        );
        assert_eq!(
            lines.get(1).copied(),
            Some("4.25"),
            "expected max(NUMBERS) output second, got:\n{stdout}"
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

    fn write_nested_wasm_vocab_companion_crate(
        project_root: &Path,
        relative_path: &str,
        package_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let crate_root = project_root.join(relative_path);
        std::fs::create_dir_all(crate_root.join("src"))?;
        std::fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\ncrate-type = [\"rlib\", \"cdylib\"]\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[workspace]\n",
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("crates")
                    .join("incan_vocab")
                    .display()
            ),
        )?;
        std::fs::write(
            crate_root.join("src/lib.rs"),
            r#"use incan_vocab::{
    DesugarError, DesugarOutput, HelperBinding, IncanExpr, IncanStatement, KeywordActivation,
    KeywordPlacement, KeywordRegistration, KeywordSpec, KeywordSurfaceKind, LibraryManifest,
    VocabBodyItem, VocabClause, VocabClauseBody, VocabDeclaration, VocabDesugarer,
    VocabFieldSpec, VocabRegistration, VocabSyntaxNode,
};

#[derive(Default)]
pub struct NestedOutputDesugarer;

pub fn library_vocab() -> VocabRegistration {
    VocabRegistration::new()
        .with_keyword_registration(KeywordRegistration {
            activation: KeywordActivation::OnImport {
                namespace: "nested.dsl".to_string(),
            },
            keywords: vec![
                KeywordSpec::new("compose", KeywordSurfaceKind::BlockDeclaration),
                context_keyword("action", &["compose"]),
                context_keyword("layout", &["compose"]),
                context_keyword("page", &["compose"]),
                context_keyword("projection", &["compose"]),
                context_keyword("region", &["layout", "page"]),
                context_keyword("heading", &["region"]),
                context_keyword("text", &["region"]),
                context_keyword("interaction", &["page"]),
                context_keyword("require", &["interaction"]),
            ],
            valid_decorators: Vec::new(),
        })
        .with_library_manifest(LibraryManifest {
            helper_bindings: vec![
                helper_binding("action"),
                helper_binding("layout"),
                helper_binding("page_with_interactions"),
                helper_binding("projection"),
                helper_binding("region"),
                helper_binding("heading"),
                helper_binding("text"),
                helper_binding("interaction"),
                helper_binding("required_input"),
                helper_binding("surface_with_governance"),
            ],
            ..LibraryManifest::default()
        })
        .with_desugarer(NestedOutputDesugarer)
}

impl VocabDesugarer for NestedOutputDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        match node {
            VocabSyntaxNode::Declaration(declaration) if declaration.keyword == "compose" => Ok(
                DesugarOutput::Statements(vec![complex_artifact_let_statement(declaration)?]),
            ),
            VocabSyntaxNode::Declaration(_) => Err(DesugarError::new(
                "nested output desugarer expected a compose declaration",
            )),
            _ => Err(DesugarError::new(
                "nested output desugarer expected a declaration node",
            )),
        }
    }
}

fn helper_binding(name: &str) -> HelperBinding {
    HelperBinding {
        key: name.to_string(),
        exported_name: name.to_string(),
    }
}

fn context_keyword(name: &str, parents: &[&str]) -> KeywordSpec {
    KeywordSpec::new(name, KeywordSurfaceKind::BlockContextKeyword)
        .with_placement(KeywordPlacement::in_block(parents.iter().copied()))
}

fn complex_artifact_let_statement(declaration: &VocabDeclaration) -> Result<IncanStatement, DesugarError> {
    Ok(IncanStatement::Let {
        name: "nested_artifact".to_string(),
        mutable: false,
        value: complex_artifact_call(declaration)?,
    })
}

fn complex_artifact_call(declaration: &VocabDeclaration) -> Result<IncanExpr, DesugarError> {
    let name = declaration
        .head
        .name
        .clone()
        .ok_or_else(|| DesugarError::new("compose declarations require a name"))?;
    let mut title = name.clone();
    let mut base = "/".to_string();
    let mut actions = Vec::new();
    let mut layouts = Vec::new();
    let mut pages = Vec::new();
    let mut projections = Vec::new();

    for item in &declaration.body {
        match item {
            VocabBodyItem::Statement(statement) => apply_surface_statement(&mut title, &mut base, statement)?,
            VocabBodyItem::Clause(clause) => match clause.keyword.as_str() {
                "action" => actions.push(desugar_action(clause)?),
                "layout" => layouts.push(desugar_layout(clause)?),
                "page" => pages.push(desugar_page(clause)?),
                "projection" => projections.push(desugar_projection(clause)?),
                other => return Err(DesugarError::new(format!("unsupported compose clause `{other}`"))),
            },
            VocabBodyItem::Declaration(declaration) => {
                return Err(DesugarError::new(format!(
                    "unsupported nested declaration `{}`",
                    declaration.keyword
                )));
            }
            _ => return Err(DesugarError::new("unsupported compose body item")),
        }
    }

    Ok(call(
        "surface_with_governance",
        vec![
            string(&name),
            string(&title),
            string(&base),
            list(actions),
            list(layouts),
            list(pages),
            list(projections),
        ],
    ))
}

fn apply_surface_statement(title: &mut String, base: &mut String, statement: &IncanStatement) -> Result<(), DesugarError> {
    match statement {
        IncanStatement::Let { name, value, .. } | IncanStatement::Assign { target: name, value } => match name.as_str() {
            "title" => *title = string_value(value, "compose title")?,
            "base" => *base = string_value(value, "compose base")?,
            other => return Err(DesugarError::new(format!("unsupported compose assignment `{other}`"))),
        },
        IncanStatement::Pass => {}
        _ => return Err(DesugarError::new("compose body only supports assignments and nested clauses")),
    }
    Ok(())
}

fn desugar_action(clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let name = required_head_name(clause, "action")?;
    let mut capability = name.clone();
    let mut required_evidence = String::new();
    match &clause.body {
        VocabClauseBody::FieldSet(fields) => {
            for field in fields {
                apply_action_assignment(
                    &mut capability,
                    &mut required_evidence,
                    &field.name,
                    field_value(field, "action assignment")?,
                )?;
            }
        }
        _ => {
            for item in clause_items(clause)? {
                match item {
                    VocabBodyItem::Statement(statement) => match statement {
                        IncanStatement::Let { name, value, .. } | IncanStatement::Assign { target: name, value } => {
                            apply_action_assignment(&mut capability, &mut required_evidence, name, value)?;
                        }
                        IncanStatement::Pass => {}
                        _ => return Err(DesugarError::new("action body only supports assignments")),
                    },
                    VocabBodyItem::Clause(other) => return Err(DesugarError::new(format!("unsupported `{}` block inside action", other.keyword))),
                    VocabBodyItem::Declaration(_) => return Err(DesugarError::new("action body only supports assignments")),
                    _ => return Err(DesugarError::new("action body only supports assignments")),
                }
            }
        }
    }
    Ok(call("action", vec![string(&name), string(&capability), string(&required_evidence)]))
}

fn apply_action_assignment(
    capability: &mut String,
    required_evidence: &mut String,
    name: &str,
    value: &IncanExpr,
) -> Result<(), DesugarError> {
    match name {
        "capability" => *capability = string_value(value, "action capability")?,
        "requires" | "required_evidence" => *required_evidence = string_value(value, "action required evidence")?,
        other => return Err(DesugarError::new(format!("unsupported action assignment `{other}`"))),
    }
    Ok(())
}

fn desugar_layout(clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let name = required_head_name(clause, "layout")?;
    let mut regions = Vec::new();
    for item in clause_items(clause)? {
        match item {
            VocabBodyItem::Clause(region_clause) if region_clause.keyword == "region" => {
                regions.push(string(&required_head_name(region_clause, "layout region")?));
            }
            VocabBodyItem::Statement(IncanStatement::Pass) => {}
            VocabBodyItem::Clause(other) => return Err(DesugarError::new(format!("unsupported `{}` block inside layout", other.keyword))),
            _ => return Err(DesugarError::new("layout body only supports region blocks")),
        }
    }
    Ok(call("layout", vec![string(&name), list(regions)]))
}

fn desugar_page(clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let name = required_head_name(clause, "page")?;
    let mut route = "/".to_string();
    let mut title = name.clone();
    let mut layout_name = "SimplePage".to_string();
    let mut regions = Vec::new();
    let mut interactions = Vec::new();
    for item in clause_items(clause)? {
        match item {
            VocabBodyItem::Statement(statement) => match statement {
                IncanStatement::Let { name, value, .. } | IncanStatement::Assign { target: name, value } => match name.as_str() {
                    "route" => route = string_value(value, "page route")?,
                    "title" => title = string_value(value, "page title")?,
                    "layout" => layout_name = string_or_name_value(value, "page layout")?,
                    other => return Err(DesugarError::new(format!("unsupported page assignment `{other}`"))),
                },
                IncanStatement::Pass => {}
                _ => return Err(DesugarError::new("page body only supports assignments, regions, and interactions")),
            },
            VocabBodyItem::Clause(region_clause) if region_clause.keyword == "region" => {
                regions.push(desugar_region(region_clause)?);
            }
            VocabBodyItem::Clause(interaction_clause) if interaction_clause.keyword == "interaction" => {
                interactions.push(desugar_interaction(interaction_clause)?);
            }
            VocabBodyItem::Clause(other) => return Err(DesugarError::new(format!("unsupported `{}` block inside page", other.keyword))),
            VocabBodyItem::Declaration(_) => return Err(DesugarError::new("page body does not support nested declarations")),
            _ => return Err(DesugarError::new("page body only supports assignments, regions, and interactions")),
        }
    }
    Ok(call(
        "page_with_interactions",
        vec![
            string(&name),
            string(&route),
            string(&title),
            string(&layout_name),
            list(regions),
            list(interactions),
        ],
    ))
}

fn desugar_region(clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let name = required_head_name(clause, "region")?;
    let mut nodes = Vec::new();
    for item in clause_items(clause)? {
        match item {
            VocabBodyItem::Clause(node_clause) if node_clause.keyword == "heading" => {
                nodes.push(call("heading", vec![string(&required_head_string(node_clause, "heading")?)]));
            }
            VocabBodyItem::Clause(node_clause) if node_clause.keyword == "text" => {
                nodes.push(call("text", vec![string(&required_head_string(node_clause, "text")?)]));
            }
            VocabBodyItem::Statement(IncanStatement::Pass) => {}
            VocabBodyItem::Clause(other) => return Err(DesugarError::new(format!("unsupported `{}` block inside region", other.keyword))),
            _ => return Err(DesugarError::new("region body only supports heading and text blocks")),
        }
    }
    Ok(call("region", vec![string(&name), list(nodes)]))
}

fn desugar_interaction(clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let name = required_head_name(clause, "interaction")?;
    let mut action = name.clone();
    let mut constraints = Vec::new();
    match &clause.body {
        VocabClauseBody::FieldSet(fields) => {
            for field in fields {
                if field.name == "action" {
                    action = string_or_name_value(field_value(field, "interaction action")?, "interaction action")?;
                } else {
                    return Err(DesugarError::new(format!("unsupported interaction assignment `{}`", field.name)));
                }
            }
        }
        _ => {
            for item in clause_items(clause)? {
                match item {
                    VocabBodyItem::Statement(statement) => match statement {
                        IncanStatement::Let { name, value, .. } | IncanStatement::Assign { target: name, value } => match name.as_str() {
                            "action" => action = string_or_name_value(value, "interaction action")?,
                            other => return Err(DesugarError::new(format!("unsupported interaction assignment `{other}`"))),
                        },
                        IncanStatement::Pass => {}
                        _ => return Err(DesugarError::new("interaction body only supports assignments and require blocks")),
                    },
                    VocabBodyItem::Clause(require_clause) if require_clause.keyword == "require" => {
                        constraints.push(desugar_required_input(&name, require_clause)?);
                    }
                    VocabBodyItem::Clause(other) => return Err(DesugarError::new(format!("unsupported `{}` block inside interaction", other.keyword))),
                    VocabBodyItem::Declaration(_) => return Err(DesugarError::new("interaction body does not support nested declarations")),
                    _ => return Err(DesugarError::new("interaction body only supports assignments and require blocks")),
                }
            }
        }
    }
    Ok(call("interaction", vec![string(&name), string(&action), list(constraints)]))
}

fn desugar_required_input(interaction_name: &str, clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let mut field = required_input_field(clause)?;
    let mut label = field.clone();
    let mut min_length = "1".to_string();
    let mut evidence_key = String::new();
    match &clause.body {
        VocabClauseBody::FieldSet(fields) => {
            for field_spec in fields {
                apply_required_input_assignment(
                    &mut field,
                    &mut label,
                    &mut min_length,
                    &mut evidence_key,
                    &field_spec.name,
                    field_value(field_spec, "require input assignment")?,
                )?;
            }
        }
        _ => {
            for item in clause_items(clause)? {
                match item {
                    VocabBodyItem::Statement(statement) => match statement {
                        IncanStatement::Let { name, value, .. } | IncanStatement::Assign { target: name, value } => {
                            apply_required_input_assignment(
                                &mut field,
                                &mut label,
                                &mut min_length,
                                &mut evidence_key,
                                name,
                                value,
                            )?;
                        }
                        IncanStatement::Pass => {}
                        _ => return Err(DesugarError::new("require input body only supports assignments")),
                    },
                    _ => return Err(DesugarError::new("require input body only supports assignments")),
                }
            }
        }
    }
    Ok(call(
        "required_input",
        vec![
            string(interaction_name),
            string(&field),
            string(&label),
            string(&min_length),
            string(&evidence_key),
        ],
    ))
}

fn apply_required_input_assignment(
    field: &mut String,
    label: &mut String,
    min_length: &mut String,
    evidence_key: &mut String,
    name: &str,
    value: &IncanExpr,
) -> Result<(), DesugarError> {
    match name {
        "field" => *field = string_or_name_value(value, "required input field")?,
        "label" => *label = string_value(value, "required input label")?,
        "min_length" => *min_length = int_or_string_value(value, "required input min_length")?,
        "evidence" | "evidence_key" => *evidence_key = string_value(value, "required input evidence")?,
        other => return Err(DesugarError::new(format!("unsupported require input assignment `{other}`"))),
    }
    Ok(())
}

fn desugar_projection(clause: &VocabClause) -> Result<IncanExpr, DesugarError> {
    let name = required_head_name(clause, "projection")?;
    let mut target = "static-web".to_string();
    match &clause.body {
        VocabClauseBody::FieldSet(fields) => {
            for field in fields {
                if field.name == "target" {
                    target = string_value(field_value(field, "projection target")?, "projection target")?;
                } else {
                    return Err(DesugarError::new(format!("unsupported projection assignment `{}`", field.name)));
                }
            }
        }
        _ => {
            for item in clause_items(clause)? {
                match item {
                    VocabBodyItem::Statement(statement) => match statement {
                        IncanStatement::Let { name, value, .. } | IncanStatement::Assign { target: name, value } if name == "target" => {
                            target = string_value(value, "projection target")?;
                        }
                        IncanStatement::Pass => {}
                        _ => return Err(DesugarError::new("projection body only supports target assignment")),
                    },
                    _ => return Err(DesugarError::new("projection body only supports target assignment")),
                }
            }
        }
    }
    Ok(call("projection", vec![string(&name), string(&target)]))
}

fn clause_items(clause: &VocabClause) -> Result<&[VocabBodyItem], DesugarError> {
    match &clause.body {
        VocabClauseBody::Empty => Ok(&[]),
        VocabClauseBody::Items(items) => Ok(items.as_slice()),
        _ => Err(DesugarError::new(format!("unsupported `{}` body shape", clause.keyword))),
    }
}

fn required_head_name(clause: &VocabClause, label: &str) -> Result<String, DesugarError> {
    let Some(value) = clause.head.first() else {
        return Err(DesugarError::new(format!("{label} requires a name")));
    };
    string_or_name_value(value, label)
}

fn required_head_string(clause: &VocabClause, label: &str) -> Result<String, DesugarError> {
    let Some(value) = clause.head.first() else {
        return Err(DesugarError::new(format!("{label} requires text")));
    };
    string_value(value, label)
}

fn required_input_field(clause: &VocabClause) -> Result<String, DesugarError> {
    if !clause.compound_tokens.is_empty() && clause.compound_tokens[0] == "input" {
        if let Some(value) = clause.head.first() {
            return string_or_name_value(value, "require input field");
        }
        return Ok(String::new());
    }
    if !clause.head.is_empty() {
        let first = string_or_name_value(&clause.head[0], "require input marker")?;
        if first == "input" {
            if clause.head.len() >= 2 {
                return string_or_name_value(&clause.head[1], "require input field");
            }
            return Ok(String::new());
        }
    }
    Err(DesugarError::new("required-input constraints must use `require input`"))
}

fn string_value(expr: &IncanExpr, label: &str) -> Result<String, DesugarError> {
    match expr {
        IncanExpr::Str(value) => Ok(value.clone()),
        _ => Err(DesugarError::new(format!("{label} must be a string literal"))),
    }
}

fn string_or_name_value(expr: &IncanExpr, label: &str) -> Result<String, DesugarError> {
    match expr {
        IncanExpr::Str(value) | IncanExpr::Name(value) => Ok(value.clone()),
        _ => Err(DesugarError::new(format!("{label} must be a name or string literal"))),
    }
}

fn int_or_string_value(expr: &IncanExpr, label: &str) -> Result<String, DesugarError> {
    match expr {
        IncanExpr::Int(value) => Ok(value.to_string()),
        IncanExpr::Str(value) => Ok(value.clone()),
        _ => Err(DesugarError::new(format!("{label} must be an integer or string literal"))),
    }
}

fn field_value<'a>(field: &'a VocabFieldSpec, label: &str) -> Result<&'a IncanExpr, DesugarError> {
    field
        .default_value
        .as_ref()
        .ok_or_else(|| DesugarError::new(format!("{label} `{}` requires a value", field.name)))
}

fn call(helper: &str, args: Vec<IncanExpr>) -> IncanExpr {
    IncanExpr::Call {
        callee: Box::new(IncanExpr::Helper(helper.to_string())),
        args,
    }
}

fn list(items: Vec<IncanExpr>) -> IncanExpr {
    IncanExpr::List(items)
}

fn string(value: &str) -> IncanExpr {
    IncanExpr::Str(value.to_string())
}

incan_vocab::export_wasm_desugarer!(NestedOutputDesugarer);
"#,
        )?;
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
        let desugarer_path = artifact_root.join("desugarers").join("inql_desugarer.wasm");
        std::fs::write(&desugarer_path, desugarer_bytes)?;

        let mut manifest = LibraryManifest::new(manifest_name, "0.1.0");
        manifest.exports.functions.push(FunctionExport {
            name: "filter".to_string(),
            type_params: Vec::new(),
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: incan::library_manifest::ParamKindExport::Normal,
                has_default: false,
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
                relative_path: "desugarers/inql_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(desugarer_bytes)),
            }),
        });
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
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

    fn write_pub_library_with_assert_keyword(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
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
            provider_manifest: incan_vocab::LibraryManifest::default(),
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
    fn build_lib_artifacts_and_consumer_alias_linkage() -> Result<(), Box<dyn std::error::Error>> {
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
            producer_root.join("src/lib.incn"),
            "pub from widgets import Widget, make_widget\n",
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
            "from pub::widgets import Widget as PublicWidget, make_widget\n\ndef main() -> None:\n  w: PublicWidget = make_widget(\"ok\")\n  print(w.name)\n",
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&consumer_main, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected consumer build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );

        let generated_toml = std::fs::read_to_string(out_dir.join("Cargo.toml"))?;
        assert!(
            generated_toml.contains("[dependencies.widgets]"),
            "expected library alias dependency entry, got:\n{generated_toml}"
        );
        assert!(
            generated_toml.contains("package = \"widgets_core\""),
            "expected package alias mapping in Cargo.toml, got:\n{generated_toml}"
        );
        assert!(
            generated_toml.contains("path = "),
            "expected path dependency in Cargo.toml, got:\n{generated_toml}"
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated_main_rs.contains("use widgets::Widget as PublicWidget;"),
            "expected pub:: item alias import emission, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("use widgets::make_widget;"),
            "expected pub:: item import emission, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("pub use widgets::Widget as PublicWidget;"),
            "private pub:: item alias import should not become a public Rust reexport, got:\n{generated_main_rs}"
        );
        assert!(
            !generated_main_rs.contains("pub use widgets::make_widget;"),
            "private pub:: item import should not become a public Rust reexport, got:\n{generated_main_rs}"
        );

        Ok(())
    }

    #[test]
    fn build_accepts_pub_from_reexport_in_src_submodule_facade() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("session_facade_project");
        std::fs::create_dir_all(project_root.join("src/session"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"session_facade\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/session/types.incn"),
            "pub class Session:\n  pub id: int\n",
        )?;
        std::fs::write(
            project_root.join("src/session/mod.incn"),
            "pub from crate.session.types import Session\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "from session import Session\n\ndef main() -> None:\n  s = Session(id=1)\n  print(s.id)\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected `build` to accept src submodule facade re-export.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_imported_enum_loop_ownership() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("imported_enum_loop_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"imported_enum_loop\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/rels.incn"),
            "@derive(Clone)\npub enum ConformanceRel:\n  Read\n  Filter\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "from rels import ConformanceRel\n\ndef relation_kind_name_from_conformance(rel: ConformanceRel) -> str:\n  match rel:\n    ConformanceRel.Read =>\n      return \"ReadRel\"\n    _ =>\n      return \"Other\"\n\ndef scenario_matches(required: list[ConformanceRel]) -> bool:\n  for expected in required:\n    if expected == ConformanceRel.Read:\n      if relation_kind_name_from_conformance(expected) == \"ReadRel\":\n        return true\n  return false\n\ndef main() -> None:\n  println(scenario_matches([ConformanceRel.Read]))\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected imported enum loop project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_len_comparison_on_recursive_list_field() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("len_comparison_recursive_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"len_comparison_recursive\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "@derive(Clone)\npub enum ExprKind:\n  Column\n  Add\n\n@derive(Clone)\npub model Expr:\n  pub kind: ExprKind\n  pub column_name: str\n  pub arguments: list[Expr]\n\npub def lower(expr: Expr) -> int:\n  if expr.kind == ExprKind.Column:\n    return 0\n  if len(expr.arguments) < 2:\n    return -1\n  return 1\n\ndef main() -> None:\n  println(lower(Expr(kind=ExprKind.Add, column_name=\"root\", arguments=[])))\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected recursive list-field len comparison project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_loop_helper_shared_string_list() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("loop_helper_shared_string_list_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"loop_helper_shared_string_list\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "def match_index(xs: list[str], y: int) -> int:\n  mut idx = 0\n  while idx < len(xs):\n    if len(xs[idx]) == y:\n      return idx\n    idx = idx + 1\n  return -1\n\n\
def helper_loop(xs: list[str], ys: list[int]) -> list[int]:\n  mut out: list[int] = []\n  for y in ys:\n    out.append(match_index(xs, y))\n  return out\n\n\
def main() -> None:\n  helper_loop([\"a\", \"bb\", \"ccc\"], [1, 2])\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected loop helper shared string-list project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_dict_comp_reusing_noncopy_key() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("dict_comp_reuses_noncopy_key_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"dict_comp_reuses_noncopy_key\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "def lengths(names: list[str]) -> dict[str, int]:\n  return {name: len(name) for name in names}\n\n\
def main() -> None:\n  values = lengths([\"alice\", \"bob\"])\n  println(values[\"alice\"])\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected dict comprehension with reused non-Copy key to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_for_tuple_unpack_enumerate() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("for_tuple_unpack_enumerate_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"for_tuple_unpack_enumerate\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "model Binding:\n  name: str\n  output_index: int\n  expr_index: int\n\n\
def field_ref(index: int) -> int:\n  return index\n\n\
pub def bind(xs: list[str]) -> list[Binding]:\n  mut out: list[Binding] = []\n  for idx, name in enumerate(xs):\n    out.append(Binding(name=name, output_index=idx, expr_index=field_ref(idx)))\n  return out\n\n\
def main() -> None:\n  bind([\"a\", \"bb\"])\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected for-loop tuple unpacking with enumerate to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_list_comp_tuple_unpack_enumerate() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("list_comp_tuple_unpack_enumerate_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"list_comp_tuple_unpack_enumerate\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "model Binding:\n  name: str\n  output_index: int\n  expr_index: int\n\n\
def field_ref(index: int) -> int:\n  return index\n\n\
pub def bind(xs: list[str]) -> list[Binding]:\n  return [Binding(name=name, output_index=idx, expr_index=field_ref(idx)) for idx, name in enumerate(xs)]\n\n\
def main() -> None:\n  bind([\"a\", \"bb\"])\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected list-comprehension tuple unpacking with enumerate to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_list_str_append_literal() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("list_str_append_literal_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"list_str_append_literal\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "pub def columns(input_columns: list[str]) -> list[str]:\n  mut columns: list[str] = []\n  columns.append(input_columns[0])\n  columns.append(\"count\")\n  return columns\n\n\
def main() -> None:\n  columns([\"orders_total\"])\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected list[str] literal append to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_imported_sum_helper_shadowing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("imported_sum_shadow_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"imported_sum_shadow\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/functions.incn"),
            "pub model ColumnRef:\n  pub name: str\n\npub model AggregateMeasure:\n  pub column_name: str\n\npub def col(name: str) -> ColumnRef:\n  return ColumnRef(name=name)\n\npub def sum(expr: ColumnRef) -> AggregateMeasure:\n  return AggregateMeasure(column_name=expr.name)\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "from functions import col, sum\n\ndef selected_column_name() -> str:\n  amount = col(\"amount\")\n  result = sum(amount)\n  return result.column_name\n\ndef main() -> None:\n  println(selected_column_name())\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected imported sum helper to shadow builtin sum and build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_cross_module_ordinary_union_forwarding() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("cross_module_union_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"cross_module_union\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            project_root.join("src/producers.incn"),
            "pub def parse_value(flag: bool) -> int | str:\n  if flag:\n    return 1\n  return \"fallback\"\n",
        )?;
        std::fs::write(
            project_root.join("src/consumers.incn"),
            "pub def describe(value: int | str) -> str:\n  if isinstance(value, int):\n    return \"number\"\n  else:\n    return value.upper()\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "from producers import parse_value\nfrom consumers import describe\n\n\
def main() -> None:\n  println(describe(parse_value(False)))\n  println(describe(\"literal\"))\n",
        )?;

        let build_output = run_build(&main_path, &project_root.join("out"))?;
        assert!(
            build_output.status.success(),
            "expected cross-module ordinary union project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_succeeds_for_qualified_enum_constructor_match() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path().join("enum_constructor_match_project");
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"enum_constructor_match\"\nversion = \"0.1.0\"\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(
            &main_path,
            "pub enum ConformanceRel:\n  Read\n  Filter\n  Project\n\npub def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:\n  match rel:\n    ConformanceRel.Read =>\n      return \"ReadRel\"\n    ConformanceRel.Filter =>\n      return \"FilterRel\"\n    ConformanceRel.Project =>\n      return \"ProjectRel\"\n    _ =>\n      return \"UnknownRel\"\n\ndef main() -> None:\n  println(relation_kind_name_from_conformance(ConformanceRel.Filter))\n",
        )?;

        let out_dir = project_root.join("out");
        let project_build = run_build(&main_path, &out_dir)?;
        assert!(
            project_build.status.success(),
            "expected qualified enum constructor match project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&project_build.stdout),
            String::from_utf8_lossy(&project_build.stderr)
        );

        Ok(())
    }

    #[test]
    fn build_and_run_rfc088_iterator_adapter_pipeline() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"rfc088_iterator_pipeline\"\nversion = \"0.1.0\"\n",
            "def is_even(n: int) -> bool:\n  return n % 2 == 0\n\n\
def double(n: int) -> int:\n  return n * 2\n\n\
def main() -> None:\n  xs = [1, 2, 3, 4, 5]\n  ys = xs.iter().filter(is_even).map(double).take(2).collect()\n  batches = xs.iter().batch(2).collect()\n  println(len(ys))\n  println(ys[0])\n  println(len(batches))\n",
        )?;

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected RFC 088 iterator pipeline to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let run_output = Command::new(incan_bin_path())
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected RFC 088 iterator pipeline to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(stdout.lines().collect::<Vec<_>>(), vec!["2", "4", "3"]);

        Ok(())
    }

    #[test]
    fn build_and_run_list_comprehension_stays_eager_after_rfc088() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"rfc088_comprehension_regression\"\nversion = \"0.1.0\"\n",
            "def main() -> None:\n  xs = [1, 2, 3]\n  ys = [n * 2 for n in xs if n > 1]\n  println(len(ys))\n  println(ys[0])\n  println(len(xs))\n",
        )?;

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected eager list comprehension regression to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let run_output = Command::new(incan_bin_path())
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected eager list comprehension regression to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(stdout.lines().collect::<Vec<_>>(), vec!["2", "4", "3"]);

        Ok(())
    }

    #[test]
    fn build_and_run_rfc049_if_let_while_let() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"rfc049_if_let_while_let\"\nversion = \"0.1.0\"\n",
            "def maybe_double(opt: Option[int]) -> int:\n  if let Some(value) = opt:\n    return value * 2\n  return 0\n\n\
def next_value(values: list[Option[int]], idx: int) -> Option[int]:\n  if idx < len(values):\n    return values[idx]\n  return None\n\n\
def sum_values(values: list[Option[int]]) -> int:\n  mut idx = 0\n  mut total = 0\n  while let Some(value) = next_value(values, idx):\n    total = total + value\n    idx = idx + 1\n  return total\n\n\
def main() -> None:\n  println(maybe_double(Some(21)))\n  println(maybe_double(None))\n  println(sum_values([Some(1), Some(2), None, Some(99)]))\n",
        )?;

        let out_dir = tmp.path().join("out");
        let build_output = run_build(&main_path, &out_dir)?;
        assert!(
            build_output.status.success(),
            "expected RFC 049 sample project to build successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&build_output.stdout),
            String::from_utf8_lossy(&build_output.stderr)
        );

        let run_output = Command::new(incan_bin_path())
            .args(["run", main_path.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected RFC 049 sample project to run successfully.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );

        let stdout = String::from_utf8_lossy(&run_output.stdout);
        assert_eq!(stdout.lines().collect::<Vec<_>>(), vec!["42", "0", "3"]);

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
    fn build_lib_preserves_generic_instance_methods_for_consumers() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("generic_methods_lib");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"generic_methods_core\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/boxmod.incn"),
            "pub class Box:\n  def get[T with Clone](self, value: T) -> T:\n    return value\n",
        )?;
        std::fs::write(producer_root.join("src/lib.incn"), "pub from boxmod import Box\n")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("generic_methods_consumer");
        std::fs::create_dir_all(consumer_root.join("src"))?;
        std::fs::write(
            consumer_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nboxlib = { path = \"../generic_methods_lib\" }\n",
        )?;
        let consumer_main = consumer_root.join("src/main.incn");
        std::fs::write(
            &consumer_main,
            "from pub::boxlib import Box\n\ndef main() -> None:\n  box: Box = Box()\n  value: int = box.get(1)\n  print(value)\n",
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&consumer_main, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected consumer build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );
        Ok(())
    }

    #[test]
    fn check_pub_boundary_preserves_method_result_types_for_question_mark() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_pub_boundary_type_fidelity_library(tmp.path())?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\npubdemo = { path = \"pub_boundary_library\" }\n",
            r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected `lazy.collect()?` across pub boundary to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn check_pub_boundary_preserves_derived_method_chain_result_types() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_pub_boundary_type_fidelity_library(tmp.path())?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\npubdemo = { path = \"pub_boundary_library\" }\n",
            r#"from pub::pubdemo import LazyFrame, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  lazy = LazyFrame[Row](_type_witness=[])
  df = lazy.clone().collect()?
  print(df.to_substrait_plan())
  return Ok(None)
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected `lazy.clone().collect()?` across pub boundary to typecheck.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    #[test]
    fn check_pub_boundary_preserves_trait_supertype_acceptance() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_pub_boundary_type_fidelity_library(tmp.path())?;

        let main_path = write_project_files(
            tmp.path(),
            "[project]\nname = \"consumer\"\n\n[dependencies]\npubdemo = { path = \"pub_boundary_library\" }\n",
            r#"from pub::pubdemo import DataFrame, SessionError, display

model Row:
  value: int

def main() -> Result[None, SessionError]:
  df = DataFrame[Row](_type_witness=[])
  display(df)
  return Ok(None)
"#,
        )?;

        let output = run_check(&main_path)?;
        assert!(
            output.status.success(),
            "expected `DataFrame[T]` to satisfy `DataSet[T]` across pub boundary.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
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
    fn consumer_run_accepts_nested_real_wasm_desugar_output() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let producer_root = tmp.path().join("nested_vocab_project");
        std::fs::create_dir_all(producer_root.join("src"))?;
        std::fs::write(
            producer_root.join("incan.toml"),
            "[project]\nname = \"nested_core\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        std::fs::write(
            producer_root.join("src/helpers.incn"),
            r#"pub def surface_with_governance(
  name: str,
  title: str,
  base: str,
  actions: list[str],
  layouts: list[str],
  pages: list[str],
  projections: list[str],
) -> str:
  return name

pub def action(name: str, capability: str, required_evidence: str) -> str:
  return name

pub def layout(name: str, regions: list[str]) -> str:
  return name

pub def page_with_interactions(
  name: str,
  route: str,
  title: str,
  layout_name: str,
  regions: list[str],
  interactions: list[str],
) -> str:
  return name

pub def region(name: str, nodes: list[str]) -> str:
  return name

pub def heading(text: str) -> str:
  return text

pub def text(text: str) -> str:
  return text

pub def interaction(name: str, action_name: str, constraints: list[str]) -> str:
  return name

pub def required_input(
  interaction_name: str,
  field: str,
  label: str,
  min_length: str,
  evidence_key: str,
) -> str:
  return field

pub def projection(name: str, target: str) -> str:
  return name
"#,
        )?;
        std::fs::write(
            producer_root.join("src/lib.incn"),
            "pub from helpers import action, heading, interaction, layout, page_with_interactions, projection, region, required_input, surface_with_governance, text\n",
        )?;
        write_nested_wasm_vocab_companion_crate(&producer_root, "vocab_companion", "nested_vocab_companion")?;

        let producer_build = run_build_lib(&producer_root)?;
        assert!(
            producer_build.status.success(),
            "expected `build --lib` with real wasm vocab companion to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&producer_build.stdout),
            String::from_utf8_lossy(&producer_build.stderr)
        );

        let consumer_root = tmp.path().join("nested_consumer");
        let consumer_main = write_project_files(
            &consumer_root,
            "[project]\nname = \"consumer\"\n\n[dependencies]\nnested = { path = \"../nested_vocab_project\" }\n",
            r#"import pub::nested

def main() -> None:
  compose FullNestedCase:
    title = "Full Nested Case"
    base = "/"

    action EscalateCase:
      capability = "case.escalate"
      requires = "escalation.explanation"

    layout SimplePage:
      region body:
        pass

    page Review:
      route = "/cases/123"
      title = "Case Review"
      layout = "SimplePage"

      region body:
        heading "Case Review":
          pass
        text "High risk case requires escalation review.":
          pass

      interaction Escalate:
        action = "EscalateCase"

        require input:
          field = "explanation"
          label = "Explanation"
          min_length = 20
          evidence = "escalation.explanation"

    projection web:
      target = "static-web"
	"#,
        )?;

        let out_dir = consumer_root.join("out");
        let consumer_build = run_build(&consumer_main, &out_dir)?;
        assert!(
            consumer_build.status.success(),
            "expected consumer build to accept nested real wasm desugar output.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&consumer_build.stdout),
            String::from_utf8_lossy(&consumer_build.stderr)
        );

        let generated_main_rs = std::fs::read_to_string(out_dir.join("src/main.rs"))?;
        assert!(
            generated_main_rs.contains("__incan_vocab_helper_nested_surface_with_governance"),
            "expected hidden helper alias for nested surface output, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("__incan_vocab_helper_nested_required_input"),
            "expected hidden helper alias for nested required-input output, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("let _nested_artifact ="),
            "expected wasm desugar output to splice a let binding, got:\n{generated_main_rs}"
        );

        let run_output = Command::new(incan_bin_path())
            .args(["run", consumer_main.to_string_lossy().as_ref()])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            run_output.status.success(),
            "expected consumer run to accept nested real wasm desugar output.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run_output.stdout),
            String::from_utf8_lossy(&run_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn consumer_build_injects_helper_import_for_vocab_desugarer_calls() -> Result<(), Box<dyn std::error::Error>> {
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
            generated_main_rs.contains("__incan_vocab_helper_filterkit_filter"),
            "expected hidden helper alias in generated Rust, got:\n{generated_main_rs}"
        );
        assert!(
            generated_main_rs.contains("filterkit::filter"),
            "expected generated Rust to import the provider helper from the dependency crate, got:\n{generated_main_rs}"
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
    fn equivalent_helper_backed_keywords_emit_identical_rust() -> Result<(), Box<dyn std::error::Error>> {
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

        let where_out = tmp.path().join("where_out");
        let where_build = run_build(&where_main, &where_out)?;
        assert!(
            where_build.status.success(),
            "expected helper-backed `where` build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&where_build.stdout),
            String::from_utf8_lossy(&where_build.stderr)
        );

        let screen_out = tmp.path().join("screen_out");
        let screen_build = run_build(&screen_main, &screen_out)?;
        assert!(
            screen_build.status.success(),
            "expected helper-backed `screen` build to succeed.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&screen_build.stdout),
            String::from_utf8_lossy(&screen_build.stderr)
        );

        let where_rust = std::fs::read_to_string(where_out.join("src/main.rs"))?;
        let screen_rust = std::fs::read_to_string(screen_out.join("src/main.rs"))?;
        assert_eq!(
            where_rust, screen_rust,
            "expected equivalent helper-backed keywords to emit identical Rust"
        );
        Ok(())
    }

    #[test]
    fn provider_requirements_flow_through_build_test_and_lock() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;

        write_pub_library_with_provider_requirements(
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
            "def test_provider_parity() -> None:\n  pass\n",
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
    fn test_runner_activates_pub_vocab_keywords_from_dependency_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;

        write_pub_library_with_assert_keyword(project_root, "widgets", "widgets_core")?;

        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\n",
        )?;
        std::fs::write(project_root.join("src/main.incn"), "def main() -> None:\n  pass\n")?;
        std::fs::write(
            project_root.join("tests/test_pub_vocab.incn"),
            "import pub::widgets\n\ndef test_pub_vocab() -> None:\n  assert true\n",
        )?;

        let test_output = run_test(&project_root.join("tests"))?;
        assert!(
            test_output.status.success(),
            "expected `incan test` to honor serialized pub vocab keywords.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&test_output.stdout),
            String::from_utf8_lossy(&test_output.stderr)
        );
        Ok(())
    }

    #[test]
    fn lock_parses_tests_using_pub_vocab_keywords() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::create_dir_all(project_root.join("tests"))?;

        write_pub_library_with_assert_keyword(project_root, "widgets", "widgets_core")?;

        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\n",
        )?;
        let main_path = project_root.join("src/main.incn");
        std::fs::write(&main_path, "def main() -> None:\n  pass\n")?;
        std::fs::write(
            project_root.join("tests/test_pub_vocab.incn"),
            "import pub::widgets\n\ndef test_pub_vocab() -> None:\n  assert true\n",
        )?;

        let lock_output = run_lock(&main_path)?;
        assert!(
            lock_output.status.success(),
            "expected `incan lock` to parse test files with pub vocab keywords.\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&lock_output.stdout),
            String::from_utf8_lossy(&lock_output.stderr)
        );
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

    #[test]
    fn test_std_tempfile_compile_and_run_named_file_and_directory() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"
from std.fs import IoError, Path
from std.tempfile import NamedTemporaryFile, SpooledTemporaryFile, TemporaryDirectory

def run() -> Result[None, IoError]:
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
    return Ok(None)

def main() -> None:
    match run():
        Ok(_) => pass
        Err(err) => println(err.message())
"#;
        let output = Command::new(incan_debug_binary())
            .args(["run", "-c", source])
            .env("CARGO_NET_OFFLINE", "true")
            .output()?;
        assert!(
            output.status.success(),
            "incan run std.tempfile smoke failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let lines = stdout.lines().collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![
                "hello", "world", "false", "6", "true", "true", "6", "true", "true", "true",
            ],
            "unexpected std.tempfile output:\n{stdout}"
        );
        Ok(())
    }
}
