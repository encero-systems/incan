#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

use support::repo_root;

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
    // The installed stdlib Loaf already supplies this source-only program; no project inspection bake is needed.
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
