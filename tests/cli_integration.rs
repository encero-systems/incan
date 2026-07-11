use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn incan_binary() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_incan") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let path = PathBuf::from(target_dir).join("debug").join("incan");
        if path.exists() {
            return path;
        }
    }

    manifest_dir.join("target").join("debug").join("incan")
}

fn run_incan(current_dir: &Path, args: &[&str]) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_env(current_dir, args, &[])
}

fn run_incan_with_env(
    current_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output, Box<dyn std::error::Error>> {
    run_incan_with_env_and_removed(current_dir, args, envs, &[])
}

fn run_incan_with_env_and_removed(
    current_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
    removed_envs: &[&str],
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut command = Command::new(incan_binary());
    for key in removed_envs {
        command.env_remove(key);
    }
    Ok(command
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env(
            "INCAN_STDLIB",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env(
            "INCAN_STDLIB_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env(
            "INCAN_GENERATED_CARGO_TARGET_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_generated_shared_target"),
        )
        .envs(envs.iter().copied())
        .output()?)
}

#[cfg(unix)]
fn run_incan_with_os_env(
    current_dir: &Path,
    args: &[&str],
    key: &str,
    value: std::ffi::OsString,
) -> Result<Output, Box<dyn std::error::Error>> {
    Ok(Command::new(incan_binary())
        .args(args)
        .current_dir(current_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("INCAN_NO_BANNER", "1")
        .env(
            "INCAN_STDLIB",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env(
            "INCAN_STDLIB_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib"),
        )
        .env(
            "INCAN_GENERATED_CARGO_TARGET_DIR",
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/incan_generated_shared_target"),
        )
        .env(key, value)
        .output()?)
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

fn write_minimal_project(root: &Path, name: &str, extra_manifest: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        root.join("incan.toml"),
        format!(
            r#"[project]
name = "{name}"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
{extra_manifest}"#
        ),
    )?;

    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;
    Ok(main_path)
}

fn parse_json_stdout(output: &Output) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn parse_jsonl_stdout(output: &Output) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let stdout = String::from_utf8(output.stdout.clone())?;
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

#[test]
fn run_std_environ_string_accessors_issue557() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_string_accessors", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import get, get_optional, get_or

def main() -> None:
  match get("INCAN_ENVIRON_PRESENT"):
    Ok(value) => println(value)
    Err(err) => println(err.kind_name())
  match get("INCAN_ENVIRON_MISSING_TEST"):
    Ok(value) => println(value)
    Err(err) => println(f"{err.kind_name()}:{err.key}")
  match get_optional("INCAN_ENVIRON_MISSING_TEST"):
    Some(value) => println(value)
    None => println("optional-missing")
  match get_optional("INCAN_ENVIRON_PRESENT"):
    Some(value) => println(f"optional:{value}")
    None => println("optional:unexpected")
  println(get_or("INCAN_ENVIRON_MISSING_TEST", "fallback"))
  println(get_or("INCAN_ENVIRON_PRESENT", "unexpected"))
  match get_optional(""):
    Some(value) => println(value)
    None => println("optional-invalid")
  println(get_or("", "invalid-fallback"))
  match get(""):
    Ok(value) => println(value)
    Err(err) => println(err.kind_name())
  match get("A=B"):
    Ok(value) => println(value)
    Err(err) => println(f"{err.kind_name()}:{err.detail}")
  match get("A\0B"):
    Ok(value) => println(value)
    Err(err) => println(f"nul:{err.kind_name()}")
"#,
    )?;

    let check_output = run_incan(tmp.path(), &["check", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&check_output, "incan check for std.environ string accessors");

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[("INCAN_ENVIRON_PRESENT", "present-value")],
        &["INCAN_ENVIRON_MISSING_TEST"],
    )?;
    assert_success(&run_output, "incan run for std.environ string accessors");

    assert_eq!(
        String::from_utf8(run_output.stdout)?,
        concat!(
            "present-value\n",
            "missing:INCAN_ENVIRON_MISSING_TEST\n",
            "optional-missing\n",
            "optional:present-value\n",
            "fallback\n",
            "present-value\n",
            "optional-invalid\n",
            "invalid-fallback\n",
            "invalid_key\n",
            "invalid_key:environment variable key must not be empty or contain `=` or NUL\n",
            "nul:invalid_key\n",
        ),
    );

    Ok(())
}

#[test]
fn run_std_environ_try_from_typed_accessors_issue557() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_typed_accessors", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom


model EnvPort with TryFrom[str]:
  value: int

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    port = int(value)
    if port < 1 or port > 65535:
      return Err("port out of range")
    return Ok(EnvPort(value=port))


class EnvLabel with TryFrom[str]:
  pub value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if len(value) == 0:
      return Err("label must not be empty")
    return Ok(EnvLabel(value=value))


enum EnvMode with TryFrom[str]:
  Dev
  Prod

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    if value == "dev":
      return Ok(EnvMode.Dev)
    elif value == "prod":
      return Ok(EnvMode.Prod)
    return Err("unknown mode")


def print_port(label: str, result: Result[Option[EnvPort], EnvironError]) -> None:
  match result:
    Ok(value) =>
      match value:
        Some(port) => println(f"{label}:{port.value}")
        None => println(f"{label}:missing")
    Err(err) => println(f"{label}:{err.kind_name()}:{err.key}")


def main() -> None:
  print_port("present", get_as[EnvPort]("INCAN_ENVIRON_PORT"))
  print_port("missing", get_as[EnvPort]("INCAN_ENVIRON_MISSING_PORT"))
  print_port("invalid", get_as[EnvPort]("INCAN_ENVIRON_PORT_BAD"))
  print_port("empty", get_as[EnvPort](""))
  match get_as[EnvLabel]("INCAN_ENVIRON_LABEL"):
    Ok(Some(label)) => println(f"label:{label.value}")
    _ => println("label:unexpected")
  match get_as[EnvMode]("INCAN_ENVIRON_MODE"):
    Ok(Some(EnvMode.Prod)) => println("mode:prod")
    _ => println("mode:unexpected")
"#,
    )?;

    let check_output = run_incan(tmp.path(), &["check", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&check_output, "incan check for std.environ typed accessors");

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_PORT", "5432"),
            ("INCAN_ENVIRON_PORT_BAD", "70000"),
            ("INCAN_ENVIRON_LABEL", "worker"),
            ("INCAN_ENVIRON_MODE", "prod"),
        ],
        &["INCAN_ENVIRON_MISSING_PORT"],
    )?;
    assert_success(&run_output, "incan run for std.environ typed accessors");

    assert_eq!(
        String::from_utf8(run_output.stdout)?,
        concat!(
            "present:5432\n",
            "missing:missing\n",
            "invalid:invalid_value:INCAN_ENVIRON_PORT_BAD\n",
            "empty:invalid_key:\n",
            "label:worker\n",
            "mode:prod\n",
        ),
    );

    Ok(())
}

#[test]
fn run_std_environ_primitive_typed_accessors_rfc089() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_primitive_typed_accessors", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import EnvironError, get_as
from std.traits.convert import TryFrom

def require[T with TryFrom[str]](key: str) -> Result[T, EnvironError]:
  match get_as[T](key)?:
    Some(value) => return Ok(value)
    None => return Err(EnvironError.missing(key))

def read_values() -> Result[None, EnvironError]:
  integer = get_as[int]("INCAN_ENVIRON_INTEGER")?.unwrap_or(0)
  floating = get_as[float]("INCAN_ENVIRON_FLOAT")?.unwrap_or(0.0)
  flag = get_as[bool]("INCAN_ENVIRON_BOOL")?.unwrap_or(false)
  text = get_as[str]("INCAN_ENVIRON_TEXT")?.unwrap_or("missing")
  println(f"{integer}:{floating}:{flag}:{text}")

  match get_as[i8]("INCAN_ENVIRON_I8")?:
    Some(narrow) => println(narrow)
    None => println("missing-i8")

  match get_as[f32]("INCAN_ENVIRON_F32")?:
    Some(narrow_float) => println(f"f32:{narrow_float}")
    None => println("missing-f32")

  i16_value = require[i16]("INCAN_ENVIRON_I16")?
  i32_value = require[i32]("INCAN_ENVIRON_I32")?
  i64_value = require[i64]("INCAN_ENVIRON_I64")?
  i128_value = require[i128]("INCAN_ENVIRON_I128")?
  isize_value = require[isize]("INCAN_ENVIRON_ISIZE")?
  u8_value = require[u8]("INCAN_ENVIRON_U8")?
  u16_value = require[u16]("INCAN_ENVIRON_U16")?
  u32_value = require[u32]("INCAN_ENVIRON_U32")?
  u64_value = require[u64]("INCAN_ENVIRON_U64")?
  u128_value = require[u128]("INCAN_ENVIRON_U128")?
  usize_value = require[usize]("INCAN_ENVIRON_USIZE")?
  f64_value = require[f64]("INCAN_ENVIRON_F64")?
  println(f"widths:{i16_value}:{i32_value}:{i64_value}:{i128_value}:{isize_value}:{u8_value}:{u16_value}:{u32_value}:{u64_value}:{u128_value}:{usize_value}:{f64_value}")

  match get_as[bool]("INCAN_ENVIRON_FALSE")?:
    Some(false) => println("bool:false")
    _ => println("bool:unexpected")

  match get_as[i8]("INCAN_ENVIRON_I8_OVERFLOW"):
    Ok(_) => println("i8-overflow:unexpected")
    Err(error) => println(f"i8-overflow:{error.kind_name()}")

  match get_as[u8]("INCAN_ENVIRON_U8_NEGATIVE"):
    Ok(_) => println("u8-negative:unexpected")
    Err(error) => println(f"u8-negative:{error.kind_name()}")

  match get_as[f64]("INCAN_ENVIRON_BAD_F64"):
    Ok(_) => println("f64-invalid:unexpected")
    Err(error) => println(error.message())

  match get_as[bool]("INCAN_ENVIRON_BAD_BOOL"):
    Ok(_) => println("bool-invalid:unexpected")
    Err(error) => println(f"bool-invalid:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_BAD_INTEGER"):
    Ok(_) => println("unexpected-valid")
    Err(error) => println(f"{error.kind_name()}:{error.key}")

  match get_as[int]("INCAN_ENVIRON_REDACTED"):
    Ok(_) => println("redaction:unexpected")
    Err(error) => println(error.message())

  match get_as[int]("INCAN_ENVIRON_MISSING_INTEGER"):
    Ok(None) => println("missing")
    Ok(Some(_)) => println("unexpected-present")
    Err(_) => println("unexpected-error")
  return Ok(None)

def main() -> None:
  match read_values():
    Ok(_) => pass
    Err(error) => println(error.message())
"#,
    )?;

    let check_output = run_incan(tmp.path(), &["check", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&check_output, "incan check for std.environ primitive typed accessors");

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_INTEGER", "42"),
            ("INCAN_ENVIRON_FLOAT", "3.5"),
            ("INCAN_ENVIRON_BOOL", "true"),
            ("INCAN_ENVIRON_TEXT", "hello"),
            ("INCAN_ENVIRON_I8", "-8"),
            ("INCAN_ENVIRON_F32", "1.25"),
            ("INCAN_ENVIRON_I16", "-16"),
            ("INCAN_ENVIRON_I32", "-32"),
            ("INCAN_ENVIRON_I64", "-64"),
            ("INCAN_ENVIRON_I128", "-128"),
            ("INCAN_ENVIRON_ISIZE", "-7"),
            ("INCAN_ENVIRON_U8", "8"),
            ("INCAN_ENVIRON_U16", "16"),
            ("INCAN_ENVIRON_U32", "32"),
            ("INCAN_ENVIRON_U64", "64"),
            ("INCAN_ENVIRON_U128", "128"),
            ("INCAN_ENVIRON_USIZE", "7"),
            ("INCAN_ENVIRON_F64", "6.25"),
            ("INCAN_ENVIRON_FALSE", "false"),
            ("INCAN_ENVIRON_I8_OVERFLOW", "128"),
            ("INCAN_ENVIRON_U8_NEGATIVE", "-1"),
            ("INCAN_ENVIRON_BAD_F64", "not-a-float"),
            ("INCAN_ENVIRON_BAD_BOOL", "yes"),
            ("INCAN_ENVIRON_BAD_INTEGER", "not-an-int"),
            ("INCAN_ENVIRON_REDACTED", "secret-value-must-not-appear"),
        ],
        &["INCAN_ENVIRON_MISSING_INTEGER"],
    )?;
    assert_success(&run_output, "incan run for std.environ primitive typed accessors");

    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
        concat!(
            "42:3.5:true:hello\n",
            "-8\n",
            "f32:1.25\n",
            "widths:-16:-32:-64:-128:-7:8:16:32:64:128:7:6.25\n",
            "bool:false\n",
            "i8-overflow:invalid_value\n",
            "u8-negative:invalid_value\n",
            "environment variable `INCAN_ENVIRON_BAD_F64` could not be parsed or validated as `float`\n",
            "bool-invalid:invalid_value\n",
            "invalid_value:INCAN_ENVIRON_BAD_INTEGER\n",
            "environment variable `INCAN_ENVIRON_REDACTED` could not be parsed or validated as `int`\n",
            "missing\n",
        ),
    );
    assert!(!String::from_utf8_lossy(&run_output.stdout).contains("secret-value-must-not-appear"));
    assert!(!String::from_utf8_lossy(&run_output.stderr).contains("secret-value-must-not-appear"));

    Ok(())
}

#[test]
fn run_std_environ_validated_newtype_accessors_rfc089() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_validated_newtypes", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import EnvironError, get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

type Label = newtype str
type Positive = newtype int[gt=0]
type Ratio = newtype float[ge=0, le=1]
type Boxed[T] = newtype T
type BinaryBlob = newtype bytes

trait TryFrom[T]:
  @classmethod
  def try_from(cls, value: T) -> Result[Self, str]: ...

model LocalToken with TryFrom[str]:
  value: str

  @classmethod
  def try_from(cls, value: str) -> Result[Self, str]:
    return Ok(LocalToken(value=value))

def print_port(label: str, result: Result[Option[Port], EnvironError]) -> None:
  match result:
    Ok(Some(port)) => println(f"{label}:{port.0}")
    Ok(None) => println(f"{label}:missing")
    Err(error) => println(f"{label}:{error.kind_name()}:{error.key}")

def main() -> None:
  local = LocalToken(value="local")
  println(local.value)
  print_port("valid", get_as[Port]("INCAN_ENVIRON_VALID_PORT"))
  print_port("invalid", get_as[Port]("INCAN_ENVIRON_INVALID_PORT"))
  print_port("malformed", get_as[Port]("INCAN_ENVIRON_MALFORMED_PORT"))

  match get_as[Label]("INCAN_ENVIRON_LABEL"):
    Ok(Some(label)) => println(f"label:{label.0}")
    Ok(None) => println("label:missing")
    Err(error) => println(f"label:{error.kind_name()}")

  match get_as[Positive]("INCAN_ENVIRON_POSITIVE"):
    Ok(Some(value)) => println(f"positive:{value.0}")
    Ok(None) => println("positive:missing")
    Err(error) => println(f"positive:{error.kind_name()}")

  match get_as[Positive]("INCAN_ENVIRON_NON_POSITIVE"):
    Ok(_) => println("non-positive:unexpected-valid")
    Err(error) => println(f"non-positive:{error.kind_name()}")

  match get_as[Boxed[int]]("INCAN_ENVIRON_BOXED"):
    Ok(Some(value)) => println(f"boxed:{value.0}")
    Ok(None) => println("boxed:missing")
    Err(error) => println(f"boxed:{error.kind_name()}")

  match get_as[Ratio]("INCAN_ENVIRON_RATIO"):
    Ok(Some(value)) => println(f"ratio:{value.0}")
    _ => println("ratio:unexpected")

  match get_as[Ratio]("INCAN_ENVIRON_RATIO_HIGH"):
    Ok(_) => println("ratio-high:unexpected")
    Err(error) => println(f"ratio-high:{error.kind_name()}")
"#,
    )?;

    let check_output = run_incan(tmp.path(), &["check", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&check_output, "incan check for std.environ validated newtypes");

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_VALID_PORT", "5432"),
            ("INCAN_ENVIRON_INVALID_PORT", "70000"),
            ("INCAN_ENVIRON_MALFORMED_PORT", "not-a-port"),
            ("INCAN_ENVIRON_LABEL", "service"),
            ("INCAN_ENVIRON_POSITIVE", "9"),
            ("INCAN_ENVIRON_NON_POSITIVE", "0"),
            ("INCAN_ENVIRON_BOXED", "77"),
            ("INCAN_ENVIRON_RATIO", "0.5"),
            ("INCAN_ENVIRON_RATIO_HIGH", "1.5"),
        ],
        &[],
    )?;
    assert_success(&run_output, "incan run for std.environ validated newtypes");

    assert_eq!(
        String::from_utf8(run_output.stdout)?,
        concat!(
            "local\n",
            "valid:5432\n",
            "invalid:invalid_value:INCAN_ENVIRON_INVALID_PORT\n",
            "malformed:invalid_value:INCAN_ENVIRON_MALFORMED_PORT\n",
            "label:service\n",
            "positive:9\n",
            "non-positive:invalid_value\n",
            "boxed:77\n",
            "ratio:0.5\n",
            "ratio-high:invalid_value\n",
        ),
    );

    Ok(())
}

#[test]
fn run_std_environ_defaulted_typed_accessors_rfc089() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_defaulted_typed_accessors", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

def main() -> None:
  match get_as[int]("INCAN_ENVIRON_DEFAULT_MISSING", 8080):
    Ok(value) => println(f"positional:{value}")
    Err(error) => println(f"positional:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_DEFAULT_PRESENT", default=8080):
    Ok(value) => println(f"keyword:{value}")
    Err(error) => println(f"keyword:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_DEFAULT_INVALID", 8080):
    Ok(value) => println(f"invalid:unexpected:{value}")
    Err(error) => println(f"invalid:{error.kind_name()}")

  match get_as[Port]("INCAN_ENVIRON_DEFAULT_PORT", 5432):
    Ok(port) => println(f"newtype:{port.0}")
    Err(error) => println(f"newtype:{error.kind_name()}")
"#,
    )?;

    let check_output = run_incan(tmp.path(), &["check", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&check_output, "incan check for defaulted std.environ typed accessors");

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_DEFAULT_PRESENT", "9090"),
            ("INCAN_ENVIRON_DEFAULT_INVALID", "not-an-int"),
        ],
        &["INCAN_ENVIRON_DEFAULT_MISSING", "INCAN_ENVIRON_DEFAULT_PORT"],
    )?;
    assert_success(&run_output, "incan run for defaulted std.environ typed accessors");

    assert_eq!(
        String::from_utf8(run_output.stdout)?,
        "positional:8080\nkeyword:9090\ninvalid:invalid_value\nnewtype:5432\n",
    );

    Ok(())
}

#[test]
fn run_std_environ_invalid_newtype_default_uses_checked_construction() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_invalid_newtype_default", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import get_as

type Port = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(Port(value))

def main() -> None:
  get_as[Port]("INCAN_ENVIRON_INVALID_DEFAULT_MISSING", default=70000)
"#,
    )?;

    let output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[],
        &["INCAN_ENVIRON_INVALID_DEFAULT_MISSING"],
    )?;
    assert_failure(&output, "invalid newtype default checked construction");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("validated newtype construction failed") || stderr.contains("port out of range"),
        "expected ordinary checked-construction failure, got:\n{stderr}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn run_std_environ_reports_non_unicode_through_public_api() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::ffi::OsStringExt;

    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_non_unicode", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import get

def main() -> None:
  match get("INCAN_ENVIRON_NON_UNICODE"):
    Ok(_) => println("unexpected")
    Err(error) => println(f"{error.kind_name()}:{error.key}")
"#,
    )?;

    let output = run_incan_with_os_env(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        "INCAN_ENVIRON_NON_UNICODE",
        std::ffi::OsString::from_vec(vec![0xff, b's', b'e', b'c', b'r', b'e', b't']),
    )?;
    assert_success(&output, "non-Unicode std.environ public API probe");
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "not_unicode:INCAN_ENVIRON_NON_UNICODE\n"
    );
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("secret"),
        "non-Unicode environment values must not appear in diagnostics:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn assert_codegraph_v04_record_contract(records: &[serde_json::Value]) {
    assert!(!records.is_empty(), "codegraph export should include a header record");
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["schema_version"], serde_json::json!(1));
    assert_eq!(records[0]["languages"], serde_json::json!(["incan"]));
    assert!(
        records[0]["degraded"].is_boolean(),
        "codegraph header should carry degraded state: {}",
        records[0]
    );

    for record in records.iter().skip(1) {
        assert_eq!(
            record["language"],
            serde_json::json!("incan"),
            "v0.4 codegraph fact records should be explicitly Incan-language facts: {record}"
        );
        assert!(
            record["provenance"].is_string(),
            "codegraph fact records should carry provenance: {record}"
        );
        assert!(
            record["degraded"].is_boolean(),
            "codegraph fact records should carry degraded state: {record}"
        );

        if let Some(span) = record.get("span").filter(|span| span.is_object()) {
            assert_source_span_shape(span, record);
        }
        if let Some(span) = record.get("primary_span").filter(|span| span.is_object()) {
            assert_source_span_shape(span, record);
        }
    }

    assert!(
        records
            .iter()
            .skip(1)
            .all(|record| record["language"] != serde_json::json!("rust")),
        "v0.4 should not emit Rust codegraph facts before first-class Rust support lands"
    );
}

fn assert_source_span_shape(span: &serde_json::Value, record: &serde_json::Value) {
    assert!(
        span["file"].is_string()
            && span["start"].is_number()
            && span["end"].is_number()
            && span["start_line"].is_number()
            && span["start_column"].is_number()
            && span["end_line"].is_number()
            && span["end_column"].is_number(),
        "source-backed codegraph records should carry stable file and span identity: {record}"
    );
}

#[test]
fn semantic_inspection_surfaces_share_project_identity() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
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
    value: int

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
    assert_eq!(check_json["schema_version"], serde_json::json!(1));
    assert_eq!(check_json["ok"], serde_json::json!(true));
    assert_eq!(check_json["diagnostics"], serde_json::json!([]));

    let build = run_incan(tmp.path(), &["build", main_arg, "--offline", "--report", "json"])?;
    assert_success(&build, "incan build --report json semantic inspection fixture");
    let build_json = parse_json_stdout(&build)?;
    assert_eq!(build_json["schema_version"], check_json["schema_version"]);
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
    assert_codegraph_v04_record_contract(&records);
    assert_eq!(records[0]["schema_version"], build_json["schema_version"]);
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
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("make_widget")
            && record["provenance"] == serde_json::json!("checked")
    }));

    Ok(())
}

fn assert_source_files_include(
    report: &serde_json::Value,
    suffixes: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    let files = report["source_files"]
        .as_array()
        .ok_or_else(|| format!("report source_files should be an array: {report}"))?;
    for suffix in suffixes {
        if !files
            .iter()
            .any(|file| file["path"].as_str().is_some_and(|path| path.ends_with(suffix)))
        {
            return Err(format!("expected report to include source file ending with {suffix}: {report}").into());
        }
    }
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
    assert_eq!(json["schema_version"], serde_json::json!(1));
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

#[cfg(feature = "rust_inspect")]
#[test]
fn rust_std_result_interop_supports_try_operator_issue801() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "result_interop_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    let source_path = src_dir.join("main.incn");
    fs::write(
        &source_path,
        r#"from rust::std::fs import metadata
from rust::std::io import Error as IoError
from rust::std::path import Path as RustPath

pub def file_len(path: str) -> Result[int, IoError]:
    meta = metadata(RustPath.new(path))?
    return Ok(int(meta.len()))

def main() -> None:
    result = file_len("incan.toml")
    print("checked")
"#,
    )?;
    let source_arg = source_path.to_str().ok_or("source path was not valid UTF-8")?;

    let check = run_incan(tmp.path(), &["check", source_arg, "--format", "json"])?;
    assert_success(&check, "incan check should type std::fs::metadata as Result");
    let check_json = parse_json_stdout(&check)?;
    assert_eq!(check_json["ok"], serde_json::json!(true));

    let build = run_incan(tmp.path(), &["build", source_arg, "--offline"])?;
    assert_success(
        &build,
        "incan build should emit Rust for std::fs::metadata try operator",
    );

    Ok(())
}

#[test]
fn contextual_f32_float_literals_emit_inferable_rust_issue802() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "f32_literal_probe"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
"#,
    )?;
    let source_path = src_dir.join("main.incn");
    fs::write(
        &source_path,
        r#"def accepts_f32(value: f32) -> None:
    print("ok")

def main() -> None:
    zero: f32 = 0.0
    accepts_f32(1.5)
"#,
    )?;
    let source_arg = source_path.to_str().ok_or("source path was not valid UTF-8")?;

    let build = run_incan(tmp.path(), &["build", source_arg, "--offline"])?;
    assert_success(
        &build,
        "incan build should let Rust infer contextual f32 float literals",
    );
    let generated = fs::read_to_string(tmp.path().join("target/incan/f32_literal_probe/src/main.rs"))?;
    assert!(
        !generated.contains("0f64") && !generated.contains("1.5f64"),
        "contextual float literals should not be hard-suffixed as f64:\n{generated}"
    );

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
        tmp.path().join("incan.toml"),
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
    assert_eq!(known_json["schema_version"], serde_json::json!(1));
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
    assert_eq!(report["schema_version"], serde_json::json!(1));
    assert_eq!(report["status"], serde_json::json!("success"));
    assert_eq!(report["mode"], serde_json::json!("executable"));
    assert_eq!(report["profile"], serde_json::json!("release"));
    assert!(
        report["generated"]["project_path"]
            .as_str()
            .is_some_and(|path| path.contains("target/incan"))
    );
    assert!(
        report["generated"]["manifest_path"]
            .as_str()
            .is_some_and(|path| path.ends_with("Cargo.toml"))
    );
    assert!(report["source_files"].as_array().is_some_and(|files| {
        files.iter().any(|file| {
            file["path"].as_str().is_some_and(|path| path.ends_with("main.incn"))
                && file["module_path"]
                    .as_array()
                    .is_some_and(|segments| segments.as_slice() == [serde_json::json!("main")])
        })
    }));
    assert_eq!(report["cargo"]["offline"], serde_json::json!(true));
    assert!(report["artifacts"].as_array().is_some_and(|artifacts| {
        artifacts.iter().any(|artifact| {
            artifact["kind"] == serde_json::json!("binary") && artifact["exists"] == serde_json::json!(true)
        })
    }));
    assert!(report["timings_ms"]["total"].as_u64().is_some());
    assert!(report["notes"].as_array().is_some_and(|notes| {
        notes
            .iter()
            .any(|note| note.as_str().is_some_and(|text| text.contains("not a stable Rust ABI")))
    }));

    Ok(())
}

#[test]
fn build_report_output_file_describes_library_build() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
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
    let generated_target_dir = tmp.path().join("target").join("generated-cargo-target");
    let output = run_incan(
        tmp.path(),
        &[
            "build",
            "--lib",
            "--offline",
            "--generated-cargo-target-dir",
            generated_target_dir
                .to_str()
                .ok_or("generated target path was not valid UTF-8")?,
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
    assert_eq!(
        report["generated"]["cargo_target_dir"],
        serde_json::json!(generated_target_dir.to_string_lossy().to_string())
    );
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
            artifact["kind"] == serde_json::json!("generated_cargo_manifest")
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
    assert!(report["timings_ms"]["cargo_build"].as_u64().is_some());
    assert!(report["timings_ms"]["total"].as_u64().is_some());

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
        project.path().join("incan.toml"),
        r#"[project]
name = "inspect_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub model Widget:
    """Widget docs survive into generated Rust."""
    value: int

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

#[test]
fn inspect_codegraph_exports_multifile_imports_and_public_symbols() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "graph_demo"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"pub model Widget:
    value: int

pub def make_widget(value: int) -> Widget:
    return Widget(value=value)
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"from helpers import make_widget

def local_value() -> int:
    return 3

pub def entrypoint() -> int:
    return make_widget(local_value()).value
"#,
    )?;

    let first = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&first, "incan inspect codegraph");
    let second = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&second, "second incan inspect codegraph");
    assert_eq!(first.stdout, second.stdout, "codegraph JSONL should be deterministic");

    let records = parse_jsonl_stdout(&first)?;
    assert_codegraph_v04_record_contract(&records);
    assert_eq!(records[0]["record"], serde_json::json!("header"));
    assert_eq!(records[0]["package"]["name"], serde_json::json!("graph_demo"));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("import")
            && record["path"] == serde_json::json!("helpers")
            && record["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item.as_str().is_some_and(|value| value == "make_widget"))
            })
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["kind"] == serde_json::json!("function")
            && record["name"] == serde_json::json!("entrypoint")
            && record["visibility"] == serde_json::json!("public")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("export")
            && record["name"] == serde_json::json!("entrypoint")
            && record["kind"] == serde_json::json!("declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("module_contains_declaration")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("make_widget")
            && record["argument_count"] == serde_json::json!(1)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["kind"] == serde_json::json!("function")
            && record["callee"] == serde_json::json!("local_value")
            && record["argument_count"] == serde_json::json!(0)
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("make_widget")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("make_widget")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("identifier")
            && record["name"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("reference")
            && record["kind"] == serde_json::json!("field")
            && record["name"] == serde_json::json!("value")
            && record["target_id"] == serde_json::Value::Null
            && record["provenance"] == serde_json::json!("syntax")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("containment")
            && record["kind"] == serde_json::json!("declaration_contains_call")
    }));

    let directory = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            src_dir.to_str().ok_or("src directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_success(&directory, "directory incan inspect codegraph");
    let directory_records = parse_jsonl_stdout(&directory)?;
    assert!(directory_records.iter().any(|record| {
        record["record"] == serde_json::json!("call")
            && record["callee"] == serde_json::json!("local_value")
            && record["target_id"].as_str().is_some_and(|target_id| {
                directory_records.iter().any(|candidate| {
                    candidate["record"] == serde_json::json!("declaration")
                        && candidate["id"] == serde_json::json!(target_id)
                        && candidate["name"] == serde_json::json!("local_value")
                })
            })
            && record["provenance"] == serde_json::json!("checked")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_tolerant_directory_keeps_parseable_facts_and_diagnostics() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("ok.incn"),
        r#"pub def ok() -> int:
    return 1
"#,
    )?;
    let nested = tmp.path().join("nested");
    fs::create_dir_all(&nested)?;
    fs::write(
        nested.join("extra.incn"),
        r#"pub def extra() -> int:
    return 2
"#,
    )?;
    fs::write(tmp.path().join("broken.incn"), "def broken(:\n")?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(&strict, "strict incan inspect codegraph should reject broken source");

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(&tolerant, "tolerant incan inspect codegraph");
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_v04_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("ok")
            && record["provenance"] == serde_json::json!("syntax")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("module")
            && record["module_path"] == serde_json::json!(["nested", "extra"])
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-P0001")
            && record["phase"] == serde_json::json!("parse")
    }));

    Ok(())
}

#[test]
fn inspect_codegraph_strict_directory_rejects_semantic_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("bad.incn"),
        r#"pub def bad() -> int:
    return missing()
"#,
    )?;

    let strict = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
        ],
    )?;
    assert_failure(
        &strict,
        "strict incan inspect codegraph should reject directory typecheck diagnostics",
    );
    let strict_stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(
        strict_stderr.contains("Unknown symbol 'missing'"),
        "expected strict directory codegraph to report typecheck diagnostic, got:\n{strict_stderr}"
    );

    let tolerant = run_incan(
        tmp.path(),
        &[
            "inspect",
            "codegraph",
            tmp.path().to_str().ok_or("directory path was not valid UTF-8")?,
            "--format",
            "jsonl",
            "--allow-errors",
        ],
    )?;
    assert_success(
        &tolerant,
        "tolerant incan inspect codegraph should keep syntax facts for directory typecheck diagnostics",
    );
    let records = parse_jsonl_stdout(&tolerant)?;
    assert_codegraph_v04_record_contract(&records);
    assert_eq!(records[0]["degraded"], serde_json::json!(true));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("declaration")
            && record["name"] == serde_json::json!("bad")
            && record["provenance"] == serde_json::json!("syntax")
    }));
    assert!(records.iter().any(|record| {
        record["record"] == serde_json::json!("diagnostic")
            && record["code"] == serde_json::json!("INCAN-T0001")
            && record["phase"] == serde_json::json!("typecheck")
            && record["message"] == serde_json::json!("Unknown symbol 'missing'")
    }));

    Ok(())
}

#[test]
fn requires_incan_allows_compatible_project_commands() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "compatible_toolchain_guard"
version = "0.1.0"
requires-incan = ">=0.5.0-0,<0.6.0"

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
        project_root.join("incan.toml"),
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
        project_root.join("incan.toml"),
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

    let manifest = fs::read_to_string(project_dir.join("incan.toml"))?;
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
fn lock_generates_lockfile_for_manifest_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_lock_project", "")?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan lock");
    let lock = fs::read_to_string(tmp.path().join("incan.lock"))?;
    assert!(lock.contains("# Auto-generated by Incan - do not edit manually"));
    assert!(lock.contains("[incan]"));
    assert!(
        !lock.contains("generated ="),
        "incan.lock must not include volatile generation timestamps"
    );
    assert!(lock.contains("deps-fingerprint = \"sha256:"));
    assert!(lock.contains("[cargo]"));
    assert!(lock.contains("[[package]]"));

    let second_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&second_output, "second incan lock");
    let second_lock = fs::read_to_string(tmp.path().join("incan.lock"))?;
    assert_eq!(lock, second_lock, "relocking unchanged inputs must be deterministic");
    Ok(())
}

#[test]
fn lock_preheats_dependency_graph_for_path_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("preheat_helper");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"preheat_helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(helper_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 1 }\n")?;

    let main_path = write_minimal_project(
        tmp.path(),
        "cli_lock_preheat_project",
        r#"
[rust-dependencies.preheat_helper]
path = "preheat_helper"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::preheat_helper import value

def main() -> None:
  println(str(value()))
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan lock with dependency preheat");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("preheating Cargo dependencies for generated test harnesses"),
        "lock should explain dependency preheat work, got:\n{stderr}"
    );
    assert!(
        tmp.path()
            .join("target/incan_lock/.incan_dependency_preheat_fingerprint")
            .is_file(),
        "dependency preheat should write a fingerprint stamp"
    );
    Ok(())
}

#[test]
fn build_lib_preheats_dependency_graph_for_generated_library_target() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("library_preheat_helper");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"library_preheat_helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(helper_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 7 }\n")?;

    let _main_path = write_minimal_project(
        tmp.path(),
        "cli_library_preheat_project",
        r#"
[rust-dependencies.library_preheat_helper]
path = "library_preheat_helper"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("lib.incn"),
        r#"from rust::library_preheat_helper import value

pub def exported_value() -> int:
  return value()
"#,
    )?;

    let first = run_incan(tmp.path(), &["build", "--lib"])?;
    assert_success(&first, "first incan build --lib with dependency preheat");
    let first_stderr = String::from_utf8_lossy(&first.stderr);
    assert!(
        first_stderr.contains("preheating Cargo dependencies for generated library builds"),
        "build --lib should explain generated-library dependency preheat work, got:\n{first_stderr}"
    );
    assert!(
        tmp.path()
            .join("target/incan_lock/.incan_library_dependency_preheat_fingerprint")
            .is_file(),
        "generated-library dependency preheat should write a fingerprint stamp"
    );

    let second = run_incan(tmp.path(), &["build", "--lib"])?;
    assert_success(&second, "second incan build --lib with dependency preheat");
    let second_stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        second_stderr.contains("generated library dependency preheat: up-to-date"),
        "second build --lib should report generated-library dependency preheat reuse, got:\n{second_stderr}"
    );
    Ok(())
}

#[test]
fn build_lib_recreates_dependency_preheat_workspace_from_existing_lock() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("library_preheat_existing_lock_helper");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"library_preheat_existing_lock_helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(helper_dir.join("src").join("lib.rs"), "pub fn value() -> i64 { 11 }\n")?;

    let _main_path = write_minimal_project(
        tmp.path(),
        "cli_library_preheat_existing_lock_project",
        r#"
[rust-dependencies.library_preheat_existing_lock_helper]
path = "library_preheat_existing_lock_helper"
"#,
    )?;
    let lib_path = tmp.path().join("src").join("lib.incn");
    fs::write(
        &lib_path,
        r#"from rust::library_preheat_existing_lock_helper import value

pub def exported_value() -> int:
  return value()
"#,
    )?;

    let lock = run_incan(
        tmp.path(),
        &["lock", lib_path.to_str().ok_or("lib path was not valid UTF-8")?],
    )?;
    assert_success(&lock, "incan lock for library preheat existing-lock fixture");
    fs::remove_dir_all(tmp.path().join("target").join("incan_lock"))?;

    let build = run_incan(tmp.path(), &["build", "--lib"])?;
    assert_success(
        &build,
        "incan build --lib should recreate dependency preheat workspace from committed incan.lock",
    );
    let stderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        stderr.contains("preheating Cargo dependencies for generated library builds"),
        "build --lib should preheat after recreating the missing dependency workspace, got:\n{stderr}"
    );
    assert!(
        tmp.path().join("target/incan_lock/Cargo.toml").is_file(),
        "generated-library dependency preheat should recreate the missing lock workspace"
    );
    assert!(
        tmp.path()
            .join("target/incan_lock/.incan_library_dependency_preheat_fingerprint")
            .is_file(),
        "generated-library dependency preheat should write a fingerprint stamp after workspace recreation"
    );
    Ok(())
}

fn stale_lockfile_without_changing_cargo_payload(root: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let lock_path = root.join("incan.lock");
    let original = fs::read_to_string(&lock_path)?;
    let stale = original.replace("deps-fingerprint = \"sha256:", "deps-fingerprint = \"sha256:stale");
    fs::write(lock_path, &stale)?;
    Ok(stale)
}

#[test]
fn build_reuses_stale_lockfile_without_rewriting_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_default_stale_lock_build_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before default build");
    let stale_lock = stale_lockfile_without_changing_cargo_payload(tmp.path())?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&build_output, "incan build with stale lockfile by default");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("warning: incan.lock is out of date; using the existing lock payload without rewriting it"),
        "default build should warn instead of silently refreshing the stale lockfile, got:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("incan.lock"))?,
        stale_lock,
        "default build must not rewrite an existing stale incan.lock"
    );
    Ok(())
}

#[test]
fn build_assert_string_inequality_in_list_loop_issue739() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "list_str_loop_assert_compare"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"
def validate(values: list[str], target: str) -> None:
    for value in values:
        assert value != target, "duplicate"


def main() -> None:
    validate(["a"], "b")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build for assert string inequality in list loop");
    Ok(())
}

#[test]
fn build_union_widening_converts_generated_wrappers_issue741() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "union_widening_conversion"
version = "0.1.0"
"#,
    )?;
    let main_path = src_dir.join("main.incn");
    fs::write(
        &main_path,
        r#"
pub model A:
    value: str


pub model B:
    value: str


pub model Holder:
    value: Extended


pub type Base = Union[A, B]
pub type Extra = Union[int, A]
pub type Extended = Union[Base, Extra, B]


pub def make_base() -> Base:
    return A(value="x")


pub def accept_extended(value: Extended) -> Extended:
    return value


pub def widen_argument(value: Base) -> Extended:
    return accept_extended(value)


pub def widen_assignment(value: Base) -> Extended:
    widened: Extended = value
    return widened


pub def widen_field(value: Base) -> Extended:
    holder = Holder(value=value)
    return holder.value


pub def widen_list_item(value: Base) -> None:
    values: list[Extended] = [value]
    return


pub def widen_return() -> Extended:
    return make_base()


pub def base_from_alias_pattern(value: Extended) -> Base:
    match value:
        Base(expr) => return expr
        int(number) => return A(value=f"{number}")


pub def keep_base(value: Base) -> bool:
    return true


pub def base_from_guarded_alias_pattern(value: Extended) -> Base:
    match value:
        case Base(expr) if keep_base(expr):
            return expr
        case Base(expr):
            return expr
        case int(number):
            return A(value=f"{number}")


pub def base_from_explicit_variants(value: Extended) -> Base:
    match value:
        A(expr) => return expr
        B(expr) => return expr
        int(number) => return A(value=f"{number}")


pub def base_from_fallback_binding(value: Extended) -> Base:
    match value:
        int(number) => return A(value=f"{number}")
        other => return other


pub def main() -> None:
    source = make_base()
    accept_extended(source)
    accept_extended(make_base())
    accept_extended(widen_argument(source))
    accept_extended(widen_assignment(source))
    accept_extended(widen_field(source))
    widen_list_item(source)
    accept_extended(widen_return())
    accept_extended(base_from_alias_pattern(source))
    accept_extended(base_from_guarded_alias_pattern(source))
    accept_extended(base_from_explicit_variants(source))
    accept_extended(base_from_fallback_binding(source))
    return
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for union widening generated wrapper conversion",
    );

    let generated_main = fs::read_to_string(tmp.path().join("target/incan/union_widening_conversion/src/main.rs"))?;
    assert!(
        generated_main.contains("match make_base()"),
        "expected generated Rust to convert call-result union wrappers through a match, got:\n{generated_main}"
    );
    assert!(
        generated_main.contains("__incan_union_value"),
        "expected generated Rust to rebuild the wider union wrapper variant-by-variant, got:\n{generated_main}"
    );

    let imported_root = tmp.path().join("union_imported_alias");
    let imported_src = imported_root.join("src");
    fs::create_dir_all(&imported_src)?;
    fs::write(
        imported_root.join("incan.toml"),
        r#"[project]
name = "union_imported_alias"
version = "0.1.0"
"#,
    )?;
    fs::write(
        imported_src.join("types.incn"),
        r#"
pub model A:
    value: str


pub model B:
    value: str


pub type Base = Union[A, B]
"#,
    )?;
    fs::write(
        imported_src.join("normalizer.incn"),
        r#"
from types import A, Base


pub type Input = Union[Base, int]


pub def normalize(value: Input) -> Base:
    match value:
        int(number) => return A(value=f"{number}")
        expr => return expr
"#,
    )?;
    fs::write(
        imported_src.join("main.incn"),
        r#"
from normalizer import normalize
from types import A


pub def main() -> None:
    normalize(A(value="x"))
    normalize(1)
    return
"#,
    )?;
    let imported_main = imported_src.join("main.incn");
    let imported_build = run_incan(
        &imported_root,
        &[
            "build",
            imported_main
                .to_str()
                .ok_or("imported alias main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &imported_build,
        "incan build for imported alias fallback union narrowing issue741",
    );

    let producer_root = tmp.path().join("union_lib");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "union_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("defs.incn"),
        r#"
pub model A:
    value: str


pub model B:
    value: str


pub type Base = Union[A, B]
pub type Extra = Union[int, A]
pub type Extended = Union[Base, Extra, B]


pub def make_base() -> Base:
    return A(value="x")


pub def accept_extended(value: Extended) -> Extended:
    return value
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from defs import accept_extended, make_base
"#,
    )?;
    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for public union widening issue741",
    );

    let consumer_root = tmp.path().join("union_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "union_consumer",
        r#"
[dependencies]
union_lib = { path = "../union_lib" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::union_lib import accept_extended, make_base


def main() -> None:
    accept_extended(make_base())
    return
"#,
    )?;
    let consumer_build = run_incan(
        &consumer_root,
        &[
            "build",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_build, "pub consumer build for public union widening issue741");

    let generated_consumer = fs::read_to_string(consumer_root.join("target/incan/union_consumer/src/main.rs"))?;
    assert!(
        generated_consumer.contains("match union_lib::make_base()"),
        "expected public consumer to convert dependency-owned union call results through a match, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("union_lib::__IncanUnion"),
        "expected public consumer union conversion to use dependency-owned wrapper paths, got:\n{generated_consumer}"
    );
    Ok(())
}

#[test]
fn build_pub_helper_wraps_union_call_result_as_option_payload_issue745() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("querykit");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "querykit"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("defs.incn"),
        r#"
pub model IntExpr:
    value: int


pub model TextExpr:
    value: str


pub type Value = Union[IntExpr, TextExpr]


pub def lit(value: int) -> Value:
    return IntExpr(value=value)


pub def fallback() -> Value:
    return TextExpr(value="fallback")


pub def accept_optional(value: Option[Value] = None) -> Value:
    return fallback()


pub def combine(first: Value, second: Option[Value] = None) -> Value:
    return first
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from defs import accept_optional, combine, fallback, lit
"#,
    )?;
    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for optional union helper issue745",
    );

    let consumer_root = tmp.path().join("consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "optional_union_consumer",
        r#"
[dependencies]
querykit = { path = "../querykit" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::querykit import accept_optional, combine, lit


def main() -> None:
    accept_optional(lit(2))
    combine(lit(1), lit(2))
    combine(lit(1), second=lit(3))
    return
"#,
    )?;
    let consumer_build = run_incan(
        &consumer_root,
        &[
            "build",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_build, "pub consumer build for optional union helper issue745");

    let generated_consumer =
        fs::read_to_string(consumer_root.join("target/incan/optional_union_consumer/src/main.rs"))?;
    assert!(
        generated_consumer.contains("querykit::accept_optional(Some(querykit::lit(2)))"),
        "expected public optional helper call to wrap the dependency-owned union result in Some, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("querykit::combine(querykit::lit(1), Some(querykit::lit(2)))"),
        "expected positional optional union argument to be wrapped in Some, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("querykit::combine(querykit::lit(1), Some(querykit::lit(3)))"),
        "expected named optional union argument to be wrapped in Some, got:\n{generated_consumer}"
    );
    Ok(())
}

#[test]
fn build_pub_method_accepts_dependency_owned_union_alias_payload_issue755() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("union_provider");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "union_provider"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("surface.incn"),
        r#"
pub model ColumnRefExpr:
    name: str


pub model NumberColumnExpr:
    expr: ColumnRefExpr


pub model SortExpr:
    expr: ColumnRefExpr


pub type ColumnExpr = Union[ColumnRefExpr, NumberColumnExpr, SortExpr]
pub type NumberValueOrColumn = Union[ColumnRefExpr, NumberColumnExpr, int]


pub model Frame:
    source: str

    def filter(self, predicate: ColumnExpr) -> Self:
        return self

    def order_by(self, columns: list[ColumnExpr]) -> Self:
        return self


pub def frame() -> Frame:
    return Frame(source="orders")


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)


pub def add(left: NumberValueOrColumn, right: NumberValueOrColumn) -> NumberColumnExpr:
    return NumberColumnExpr(expr=col("sum"))


pub def desc(expr: ColumnExpr) -> ColumnExpr:
    return SortExpr(expr=col("sorted"))
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from surface import ColumnExpr, ColumnRefExpr, Frame, NumberColumnExpr, NumberValueOrColumn, SortExpr, add, col, desc, frame
"#,
    )?;
    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for dependency-owned union boundary issue755",
    );

    let consumer_root = tmp.path().join("union_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "union_consumer",
        r#"
[dependencies]
union_provider = { path = "../union_provider" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::union_provider import add as __incan_vocab_helper_union_provider_add
from pub::union_provider import col as __incan_vocab_helper_union_provider_col
from pub::union_provider import desc as __incan_vocab_helper_union_provider_desc
from pub::union_provider import frame as __incan_vocab_helper_union_provider_frame


def main() -> None:
    __incan_vocab_helper_union_provider_frame().filter(
        __incan_vocab_helper_union_provider_add(__incan_vocab_helper_union_provider_col("amount"), 5),
    )
    __incan_vocab_helper_union_provider_frame().order_by([
        __incan_vocab_helper_union_provider_desc(__incan_vocab_helper_union_provider_col("amount")),
    ])
    return
"#,
    )?;
    let consumer_build = run_incan(
        &consumer_root,
        &[
            "build",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &consumer_build,
        "pub consumer build for dependency-owned union boundary issue755",
    );

    let generated_consumer = fs::read_to_string(consumer_root.join("target/incan/union_consumer/src/main.rs"))?;
    assert!(
        generated_consumer.contains("union_provider::__IncanUnion"),
        "expected public method call to use dependency-owned wrapper paths, got:\n{generated_consumer}"
    );
    assert!(
        generated_consumer.contains("union_provider::desc(union_provider::__IncanUnion"),
        "expected public union-return helper call to use dependency-owned wrapper paths, got:\n{generated_consumer}"
    );
    assert!(
        !generated_consumer.contains("crate::__IncanUnion"),
        "expected public consumer not to re-own dependency union wrappers, got:\n{generated_consumer}"
    );
    assert!(
        !generated_consumer.contains("pub enum __IncanUnion"),
        "expected public consumer not to emit local duplicate dependency union wrappers, got:\n{generated_consumer}"
    );
    Ok(())
}

#[test]
fn build_narrowed_union_fallback_helper_calls_issue743() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "narrowed_fallback_call", "")?;
    fs::write(
        &main_path,
        r#"
pub model A:
    value: str


pub model B:
    value: str


pub model C:
    value: str


pub type Expr = Union[A, B, C]


pub def describe(expr: Expr) -> str:
    return "expr"


pub def combine(left: Expr, right: Expr) -> str:
    return "both"


pub def fallback_describe(expr: Expr) -> str:
    match expr:
        A(value) => return value.value
        _ => return describe(expr)


pub def fallback_binding_describe(expr: Expr) -> str:
    match expr:
        A(value) => return value.value
        other => return combine(expr, other)


pub def main() -> None:
    fallback_describe(B(value="b"))
    fallback_describe(C(value="c"))
    fallback_binding_describe(B(value="b"))
    fallback_binding_describe(C(value="c"))
    return
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build for narrowed fallback helper calls issue743");
    Ok(())
}

#[test]
fn test_reuses_stale_lockfile_without_rewriting_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_default_stale_lock_test_project", "")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import assert_eq

def test_smoke() -> None:
  assert_eq(1, 1)
"#,
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before default test");
    let stale_lock = stale_lockfile_without_changing_cargo_payload(tmp.path())?;

    let test_output = run_incan(tmp.path(), &["test"])?;

    assert_success(&test_output, "incan test with stale lockfile by default");
    let stderr = String::from_utf8_lossy(&test_output.stderr);
    assert!(
        stderr.contains("warning: incan.lock is out of date; using the existing lock payload without rewriting it"),
        "default test should warn instead of silently refreshing the stale lockfile, got:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("incan.lock"))?,
        stale_lock,
        "default test must not rewrite an existing stale incan.lock"
    );
    Ok(())
}

#[test]
fn test_lock_with_path_rust_dependency_stays_fresh_for_test_issue505() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_lock_path_dep_fresh_for_test_project",
        r#"

[rust-dependencies]
tiny_helper = { path = "rust/tiny_helper" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_helper import plus_one

pub def value() -> int:
  return plus_one(0)

def main() -> None:
  println(value())
"#,
    )?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.testing import assert_eq
from crate.main import value

def test_value() -> None:
  assert_eq(value(), 1)
"#,
    )?;
    let helper_src = tmp.path().join("rust").join("tiny_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "tiny_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn plus_one(value: i64) -> i64 { value + 1 }\n",
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock with path Rust dependency");

    let test_output = run_incan(tmp.path(), &["test"])?;

    assert_success(&test_output, "incan test after lock with path Rust dependency");
    let stderr = String::from_utf8_lossy(&test_output.stderr);
    assert!(
        !stderr.contains("incan.lock is out of date"),
        "fresh lock should not warn as stale for path Rust dependencies, got:\n{stderr}"
    );
    Ok(())
}

#[test]
fn multi_entrypoint_lock_covers_project_scripts_and_tests_issue505() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_multi_entry_lock_freshness_project",
        r#"
extra = "src/extra.incn"

[rust-dependencies]
tiny_helper = { path = "rust/tiny_helper" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"pub def value() -> int:
  return 1

def main() -> None:
  println(value())
"#,
    )?;
    let extra_path = tmp.path().join("src").join("extra.incn");
    fs::write(
        &extra_path,
        r#"from rust::tiny_helper import plus_one

def main() -> None:
  println(plus_one(1))
"#,
    )?;

    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_main.incn"),
        r#"from std.serde.json import Serialize
from std.testing import assert_eq
from crate.main import value

model Event with Serialize:
  id: int

def test_value() -> None:
  event = Event(id=1)
  assert_eq(event.to_json(), "{\"id\":1}")
  assert_eq(value(), 1)
"#,
    )?;

    let helper_src = tmp.path().join("rust").join("tiny_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "tiny_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn plus_one(value: i64) -> i64 { value + 1 }\n",
    )?;

    let assert_no_stale_warning = |output: &Output, context: &str| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("incan.lock is out of date"),
            "{context} should not warn that incan.lock is stale, got:\n{stderr}"
        );
    };

    let default_lock_output = run_incan(tmp.path(), &["lock"])?;
    assert_success(&default_lock_output, "default incan lock");

    let test_after_default_lock = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_after_default_lock, "incan test after default lock");
    assert_no_stale_warning(&test_after_default_lock, "incan test after default lock");

    let extra_after_default_lock = run_incan(
        tmp.path(),
        &["run", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_after_default_lock, "incan run extra after default lock");
    assert_no_stale_warning(&extra_after_default_lock, "incan run extra after default lock");

    let extra_lock_output = run_incan(
        tmp.path(),
        &["lock", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_lock_output, "incan lock extra");

    let extra_after_extra_lock = run_incan(
        tmp.path(),
        &["run", extra_path.to_str().ok_or("extra path was not valid UTF-8")?],
    )?;
    assert_success(&extra_after_extra_lock, "incan run extra after extra lock");
    assert_no_stale_warning(&extra_after_extra_lock, "incan run extra after extra lock");

    let test_after_extra_lock = run_incan(tmp.path(), &["test"])?;
    assert_success(&test_after_extra_lock, "incan test after extra lock");
    assert_no_stale_warning(&test_after_extra_lock, "incan test after extra lock");

    Ok(())
}

#[test]
fn run_accepts_generic_rust_param_scenarios_share_one_generated_project() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_generic_rust_param_scenarios",
        r#"

[rust-dependencies]
arc_callback = { path = "rust/arc_callback" }
borrow_helper = { path = "rust/borrow_helper" }
decode_helper = { path = "rust/decode_helper" }
decode_trait_helper = { path = "rust/decode_trait_helper" }
prost = { path = "rust/prost" }
prost-types = { path = "rust/prost-types" }
reexport_identity = { path = "rust/reexport_identity" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from arc_callback import arc_callback_case
from borrowed_generic import borrowed_generic_case
from by_value_decode import by_value_decode_case
from cross_crate_decode import cross_crate_decode_case
from reexport_identity import reexport_identity_case
from trait_by_value_decode import trait_by_value_decode_case

def main() -> None:
  println(arc_callback_case())
  println(borrowed_generic_case())
  println(by_value_decode_case())
  println(trait_by_value_decode_case())
  println(cross_crate_decode_case())
  println(reexport_identity_case())
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("arc_callback.incn"),
        r#"from rust::arc_callback import CallbackError, ColumnarValue, DataType, ScalarFunctionImplementation, SliceCallback, Volatility, create_udf, create_udf_full
from rust::std::sync import Arc

def callback(args: list[ColumnarValue]) -> Result[ColumnarValue, CallbackError]:
  return Ok(args[0].clone())

def inline_arc_callback_value() -> int:
  match create_udf(callback=Arc.from((args) => callback(args.to_vec())), name="inline"):
    Ok(value) => return value.value()
    Err(_) => return -1

def inline_datafusion_shaped_callback_value() -> int:
  match create_udf_full(
    name="sha1",
    input_types=[DataType.Utf8],
    return_type=DataType.Utf8,
    volatility=Volatility.Immutable,
    fun=Arc.from((args) => callback(args.to_vec())),
  ):
    Ok(value) => return value.value()
    Err(_) => return -1

pub def arc_callback_case() -> str:
  implementation: SliceCallback = Arc.from((args) => callback(args.to_vec()))
  match create_udf(callback=implementation, name="assigned"):
    Ok(value) => return f"arc_callback:{value.value()}:{inline_arc_callback_value()}:{inline_datafusion_shaped_callback_value()}"
    Err(_) => return "arc_callback:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("borrowed_generic.incn"),
        r#"from rust::borrow_helper import takes_ref

model Payload:
  name: str

pub def borrowed_generic_case() -> str:
  payload = Payload(name="demo")
  return f"borrowed:{takes_ref(payload)}"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("by_value_decode.incn"),
        r#"from rust::decode_helper import FileDescriptorSet
from rust::std::io import Cursor

pub def by_value_decode_case() -> str:
  mut cursor = Cursor.new(b"abc")
  match FileDescriptorSet.decode(cursor):
    Ok(_) => return "by_value:ok"
    Err(_) => return "by_value:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("trait_by_value_decode.incn"),
        r#"from rust::decode_trait_helper import FileDescriptorSet, Message

pub def trait_by_value_decode_case() -> str:
  encoded = b"abc"
  match FileDescriptorSet.decode(encoded.as_slice()):
    Ok(_) => return "trait_by_value:ok"
    Err(_) => return "trait_by_value:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("cross_crate_decode.incn"),
        r#"from rust::prost import Message
from rust::prost_types import FileDescriptorSet, ProducerPlan

pub def cross_crate_decode_case() -> str:
  producer = ProducerPlan.new()
  encoded = producer.encode_to_vec()
  match FileDescriptorSet.decode(encoded):
    Ok(_) => return "cross_crate:ok"
    Err(_) => return "cross_crate:err"
"#,
    )?;
    fs::write(
        tmp.path().join("src").join("reexport_identity.incn"),
        r#"from rust::reexport_identity import Expr as RustExpr, ScalarFunction as RustScalarFunction, registry

pub def reexport_identity_case() -> str:
  state = registry()
  udf = state.udf()
  args: list[RustExpr] = []
  _ = RustExpr.ScalarFunction(RustScalarFunction.new_udf(udf, args))
  return "reexport_identity:ok"
"#,
    )?;

    // Keep this fixture DataFusion-shaped but crate-light. The real DataFusion crate is far too expensive for a
    // compiler regression test; the behavior under test is the Rust metadata shape:
    // `ScalarFunctionImplementation -> SliceCallback -> Arc<dyn Fn(...)>`.
    let helper_src = tmp.path().join("rust").join("arc_callback").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("arc_callback src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "arc_callback"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"use std::sync::Arc;

#[derive(Clone)]
pub struct ColumnarValue {
    value: i64,
}

impl ColumnarValue {
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn value(&self) -> i64 {
        self.value
    }
}

pub struct CallbackError;

pub type SliceCallback = Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue, CallbackError> + Send + Sync>;
pub type ScalarFunctionImplementation = crate::SliceCallback;

#[derive(Clone)]
pub enum DataType {
    Utf8,
}

#[derive(Clone)]
pub enum Volatility {
    Immutable,
}

pub fn invoke(callback: SliceCallback) -> Result<ColumnarValue, CallbackError> {
    let args = vec![ColumnarValue::new(7)];
    callback(&args)
}

pub fn create_udf(name: &str, callback: crate::SliceCallback) -> Result<ColumnarValue, CallbackError> {
    let _ = name;
    let args = vec![ColumnarValue::new(11)];
    callback(&args)
}

pub fn create_udf_full(
    name: &str,
    input_types: Vec<DataType>,
    return_type: DataType,
    volatility: Volatility,
    fun: crate::ScalarFunctionImplementation,
) -> Result<ColumnarValue, CallbackError> {
    let _ = name;
    let _ = input_types;
    let _ = return_type;
    let _ = volatility;
    let args = vec![ColumnarValue::new(13)];
    fun(&args)
}
"#,
    )?;
    let helper_src = tmp.path().join("rust").join("borrow_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "borrow_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        "pub fn takes_ref<TValue>(_value: &TValue) -> i64 { 1 }\n",
    )?;
    let helper_src = tmp.path().join("rust").join("decode_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "decode_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub trait DecodeBuf {}

impl DecodeBuf for std::io::Cursor<Vec<u8>> {}

pub struct DecodeError;

pub struct FileDescriptorSet;

impl FileDescriptorSet {
    pub fn decode<T: DecodeBuf>(_buf: T) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;
    let helper_src = tmp.path().join("rust").join("decode_trait_helper").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("helper src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "decode_trait_helper"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"pub trait DecodeBuf {}

impl DecodeBuf for &[u8] {}

pub struct DecodeError;

pub struct FileDescriptorSet;

pub trait Message: Sized {
    fn decode(_buf: impl DecodeBuf) -> Result<Self, DecodeError>;
}

impl Message for FileDescriptorSet {
    fn decode(_buf: impl DecodeBuf) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;
    let prost_src = tmp.path().join("rust").join("prost").join("src");
    fs::create_dir_all(&prost_src)?;
    fs::write(
        prost_src.parent().ok_or("prost src has no parent")?.join("Cargo.toml"),
        r#"[package]
name = "prost"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        prost_src.join("lib.rs"),
        r#"pub trait Buf {}

impl Buf for &[u8] {}

pub struct DecodeError;

pub trait Message: Sized {
    fn decode(_buf: impl Buf) -> Result<Self, DecodeError>;
}
"#,
    )?;
    let prost_types_src = tmp.path().join("rust").join("prost-types").join("src");
    fs::create_dir_all(&prost_types_src)?;
    fs::write(
        prost_types_src
            .parent()
            .ok_or("prost-types src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "prost-types"
version = "0.1.0"
edition = "2021"

[dependencies]
prost = { path = "../prost" }
"#,
    )?;
    fs::write(
        prost_types_src.join("lib.rs"),
        r#"pub struct ProducerPlan;

impl ProducerPlan {
    pub fn new() -> Self {
        Self
    }

    pub fn encode_to_vec(&self) -> Vec<u8> {
        b"abc".to_vec()
    }
}

pub struct FileDescriptorSet;

impl prost::Message for FileDescriptorSet {
    fn decode(_buf: impl prost::Buf) -> Result<Self, prost::DecodeError> {
        Ok(Self)
    }
}
"#,
    )?;
    let reexport_identity_src = tmp.path().join("rust").join("reexport_identity").join("src");
    fs::create_dir_all(&reexport_identity_src)?;
    fs::write(
        reexport_identity_src
            .parent()
            .ok_or("reexport_identity src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "reexport_identity"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        reexport_identity_src.join("lib.rs"),
        r#"use std::sync::Arc;

pub mod udf {
    pub struct ScalarUDF;
}

pub use udf::ScalarUDF;

pub struct FunctionRegistry;

pub fn registry() -> FunctionRegistry {
    FunctionRegistry
}

impl FunctionRegistry {
    pub fn udf(&self) -> Arc<udf::ScalarUDF> {
        Arc::new(udf::ScalarUDF)
    }
}

pub struct Expr;
pub struct ScalarFunction;

impl ScalarFunction {
    pub fn new_udf(_udf: Arc<ScalarUDF>, _args: Vec<Expr>) -> Self {
        Self
    }
}

impl Expr {
    #[allow(non_snake_case)]
    pub fn ScalarFunction(_function: ScalarFunction) -> Self {
        Self
    }
}
"#,
    )?;

    let output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;

    assert_success(&output, "incan run with batched generic Rust param scenarios");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "arc_callback:11:11:13\nborrowed:1\nby_value:ok\ntrait_by_value:ok\ncross_crate:ok\nreexport_identity:ok",
        "expected batched generic Rust param output, got:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_types_rust_callback_closures_in_every_match_arm_issue733() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "cli_rust_match_arm_callback_context",
        r#"

[rust-dependencies]
arc_match_callback = { path = "rust/arc_match_callback" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::arc_match_callback import CallbackError, ColumnarValue, DataType, ScalarUDF, Volatility, create_udf
from rust::std::sync import Arc


@derive(Clone)
enum ReproFunction(str):
  First = "first"
  Second = "second"


def callback(args: list[ColumnarValue]) -> Result[ColumnarValue, CallbackError]:
  return Ok(args[0].clone())


def make_udf(function: ReproFunction) -> ScalarUDF:
  match function:
    ReproFunction.First =>
      return create_udf(
        name=function.value(),
        input_types=[DataType.Utf8],
        return_type=DataType.Utf8,
        volatility=Volatility.Immutable,
        fun=Arc.from((args) => callback(args.to_vec())),
      )
    ReproFunction.Second =>
      return create_udf(
        name=function.value(),
        input_types=[DataType.Utf8],
        return_type=DataType.Utf8,
        volatility=Volatility.Immutable,
        fun=Arc.from((args) => callback(args.to_vec())),
      )


def main() -> None:
  first = make_udf(ReproFunction.First)
  second = make_udf(ReproFunction.Second)
  println(f"match-callback:{first.value()}:{second.value()}")
"#,
    )?;

    // Keep the regression DataFusion-shaped without compiling DataFusion. The issue is the metadata contract for a
    // transitive callback alias used by an inspected Rust function parameter.
    let helper_src = tmp.path().join("rust").join("arc_match_callback").join("src");
    fs::create_dir_all(&helper_src)?;
    fs::write(
        helper_src
            .parent()
            .ok_or("arc_match_callback src has no parent")?
            .join("Cargo.toml"),
        r#"[package]
name = "arc_match_callback"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        helper_src.join("lib.rs"),
        r#"use std::sync::Arc;

#[derive(Clone)]
pub struct ColumnarValue {
    value: i64,
}

impl ColumnarValue {
    pub fn new(value: i64) -> Self {
        Self { value }
    }

    pub fn value(&self) -> i64 {
        self.value
    }
}

pub struct CallbackError;

pub type SliceCallback = Arc<dyn Fn(&[ColumnarValue]) -> Result<ColumnarValue, CallbackError> + Send + Sync>;
pub type ScalarFunctionImplementation = crate::SliceCallback;

#[derive(Clone)]
pub struct ScalarUDF {
    value: i64,
}

impl ScalarUDF {
    pub fn value(&self) -> i64 {
        self.value
    }
}

#[derive(Clone)]
pub enum DataType {
    Utf8,
}

#[derive(Clone)]
pub enum Volatility {
    Immutable,
}

pub fn create_udf(
    name: &str,
    input_types: Vec<DataType>,
    return_type: DataType,
    volatility: Volatility,
    fun: crate::ScalarFunctionImplementation,
) -> ScalarUDF {
    let _ = name;
    let _ = input_types;
    let _ = return_type;
    let _ = volatility;
    let args = vec![ColumnarValue::new(13)];
    let value = fun(&args).map(|value| value.value()).unwrap_or(-1);
    ScalarUDF { value }
}
"#,
    )?;

    let output = run_incan(tmp.path(), &["run"])?;
    assert_success(&output, "rust callback closure context inside match arms");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "match-callback:13:13",
        "unexpected callback output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn test_runner_prefers_project_sibling_import_over_unimported_stdlib_stub_type()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "stdhash_sibling_collision"
version = "0.1.0"
"#,
    )?;

    let src_dir = project_root.join("src");
    let functions_dir = src_dir.join("functions");
    let hashing_dir = functions_dir.join("hashing");
    let session_dir = src_dir.join("session");
    let tests_dir = project_root.join("tests");
    fs::create_dir_all(&hashing_dir)?;
    fs::create_dir_all(&session_dir)?;
    fs::create_dir_all(&tests_dir)?;

    fs::write(
        hashing_dir.join("expr.incn"),
        r#"pub model Expr:
    pub value: int
"#,
    )?;
    fs::write(
        hashing_dir.join("sha224.incn"),
        r#"from functions.hashing.expr import Expr

pub def sha224(expr: Expr) -> Expr:
    return expr
"#,
    )?;
    fs::write(
        hashing_dir.join("sha2.incn"),
        r#"from functions.hashing.expr import Expr
from functions.hashing.sha224 import sha224

pub def sha2(expr: Expr) -> Expr:
    return sha224(expr)
"#,
    )?;
    fs::write(
        functions_dir.join("mod.incn"),
        r#"pub from functions.hashing.expr import Expr
pub from functions.hashing.sha224 import sha224
pub from functions.hashing.sha2 import sha2
"#,
    )?;
    fs::write(
        session_dir.join("bridge.incn"),
        r#"from std.hash import sha1 as std_sha1

pub def digest(data: bytes) -> bytes:
    return std_sha1.digest(data)
"#,
    )?;
    fs::write(
        session_dir.join("mod.incn"),
        r#"pub from session.bridge import digest
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub from functions import Expr, sha224, sha2
pub from session import digest
"#,
    )?;
    fs::write(
        tests_dir.join("test_collision.incn"),
        r#"from functions import Expr, sha2
from session import digest

def test_collision__sibling_import_wins() -> None:
    payload = Expr(value=1)
    assert len(digest(b"abc")) > 0
    assert sha2(payload).value == 1
"#,
    )?;

    let output = run_incan(project_root, &["test", "tests"])?;
    assert_success(
        &output,
        "incan test should keep project sibling imports ahead of unimported stdlib stub helper types",
    );
    Ok(())
}

#[test]
fn test_runner_resolves_imported_stdlib_enum_patterns_from_enum_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("incan.toml"),
        r#"[project]
name = "stdlib_enum_pattern_metadata"
version = "0.1.0"
"#,
    )?;

    let src_dir = project_root.join("src");
    let substrait_dir = src_dir.join("substrait");
    let session_dir = src_dir.join("session");
    let tests_dir = project_root.join("tests");
    fs::create_dir_all(&substrait_dir)?;
    fs::create_dir_all(&session_dir)?;
    fs::create_dir_all(&tests_dir)?;

    fs::write(
        substrait_dir.join("schema.incn"),
        r#"pub enum PrimitiveKind(str):
    Bool = "bool"
    String = "string"
"#,
    )?;
    fs::write(
        session_dir.join("json_schema.incn"),
        r#"from std.json import JsonKind, JsonValue
from substrait.schema import PrimitiveKind

pub def primitive_kind() -> PrimitiveKind:
    return PrimitiveKind.Bool

pub def schema_name(value: JsonValue) -> str:
    match value.kind():
        JsonKind.Bool => return "BOOLEAN"
        JsonKind.String => return "STRING"
        _ => return "OTHER"
"#,
    )?;
    fs::write(
        session_dir.join("mod.incn"),
        r#"pub from session.json_schema import primitive_kind, schema_name
"#,
    )?;
    fs::write(
        src_dir.join("lib.incn"),
        r#"pub from session import primitive_kind, schema_name
"#,
    )?;
    fs::write(
        tests_dir.join("test_json_schema.incn"),
        r#"from session import primitive_kind, schema_name
from std.json import JsonValue

def test_stdlib_enum_patterns_survive_colliding_project_variants() -> None:
    assert primitive_kind().value() == "bool"
    assert schema_name(JsonValue.bool(True)) == "BOOLEAN"
    assert schema_name(JsonValue.string("x")) == "STRING"
"#,
    )?;

    let output = run_incan(project_root, &["test", "tests"])?;
    assert_success(
        &output,
        "incan test should resolve imported stdlib enum patterns from enum-owned metadata",
    );
    Ok(())
}

#[test]
fn build_locked_rejects_stale_lockfile() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_locked_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before locked build");

    fs::write(
        tmp.path().join("incan.toml"),
        r#"[project]
name = "cli_locked_project"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"

[rust-dependencies.serde]
version = "1.0"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::serde import Serialize

def main() -> None:
  println("cli lifecycle ok")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_failure(&build_output, "incan build --locked with stale lockfile");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("incan.lock is out of date"),
        "locked build should report stale lockfile, got:\n{stderr}"
    );
    assert!(
        stderr.contains("incan lock"),
        "locked build should tell users how to refresh the lockfile"
    );
    Ok(())
}

#[test]
fn build_frozen_rejects_missing_lockfile() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_frozen_project", "")?;

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--frozen",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_failure(&build_output, "incan build --frozen without lockfile");
    let stderr = String::from_utf8_lossy(&build_output.stderr);
    assert!(
        stderr.contains("incan.lock is missing; run `incan lock`"),
        "frozen build should report missing lockfile, got:\n{stderr}"
    );
    assert!(
        !tmp.path().join("incan.lock").exists(),
        "frozen build must not create incan.lock after rejecting a missing lockfile"
    );
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
        project_dir.join("incan.toml"),
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
        project_dir.join("incan.toml"),
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
    name: str
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

fn write_order_summary_bundle(project_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let contract_dir = project_dir.join("contracts");
    fs::create_dir_all(&contract_dir)?;
    fs::write(
        contract_dir.join("order_summary.json"),
        r#"{
  "schema_version": 1,
  "stable_model_id": "orders.summary",
  "logical_type_name": "OrderSummary",
  "publishable": true,
  "fields": [
    {
      "name": "order_id",
      "type": "str",
      "alias": "orderId",
      "description": "Stable order identifier"
    },
    {
      "name": "total_cents",
      "type": "int"
    },
    {
      "name": "coupon_code",
      "type": "str",
      "nullable": true
    }
  ]
}
"#,
    )?;
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
        project_dir.join("incan.toml"),
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
        project_dir.join("incan.toml"),
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

#[test]
fn run_generic_reflection_calls_issue712() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_reflection_issue712", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("generic_reflection_helpers.incn"),
        r#"pub def imported_field_count[T](value: T) -> int:
    return len(value.__fields__())


pub def imported_class_name[T](value: T) -> str:
    return str(value.__class_name__())
"#,
    )?;
    fs::write(
        &main_path,
        r#"from generic_reflection_helpers import imported_class_name, imported_field_count


model Row:
    name: str


class Bare:
    value: int


def reflected_field_count[T](value: T) -> int:
    return len(value.__fields__())


def reflected_class_name[T](value: T) -> str:
    return str(value.__class_name__())


def main() -> None:
    row = Row(name="Ada")
    println(reflected_class_name(row))
    println(reflected_field_count(row))
    println(imported_class_name(row))
    println(imported_field_count(row))
    bare = Bare(value=1)
    println(bare.__class_name__())
    println(len(bare.__fields__()))
    println(reflected_class_name(bare))
    println(reflected_field_count(bare))
    println(imported_class_name(bare))
    println(imported_field_count(bare))
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &["--check", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&check_output, "incan --check for generic reflection issue712");

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for generic reflection issue712");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["Row", "1", "Row", "1", "Bare", "1", "Bare", "1", "Bare", "1"],
        "unexpected generic reflection output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_type_parameter_reflection_calls_issue715() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "type_parameter_reflection_issue715", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("schema_helpers.incn"),
        r#"pub def class_name_for[T]() -> str:
    return T.__class_name__()


pub def field_count_for[T]() -> int:
    return len(T.__fields__())


pub def print_schema[T]() -> None:
    println(str(T.__class_name__()))
    for info in T.__fields__():
        println(f"{info.name}|{info.wire_name}|{info.type_name}|{info.has_default}")
"#,
    )?;
    fs::write(
        &main_path,
        r#"from schema_helpers import class_name_for, field_count_for, print_schema


model MySchema:
    id [description="Stable id"]: int
    status [alias="state"]: str = "new"


class BareSchema:
    value: int


def local_field_count[T]() -> int:
    return len(T.__fields__())


def main() -> None:
    println(class_name_for[MySchema]())
    println(field_count_for[MySchema]())
    println(local_field_count[MySchema]())
    print_schema[MySchema]()
    println(class_name_for[BareSchema]())
    println(field_count_for[BareSchema]())
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &["--check", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&check_output, "incan --check for type-parameter reflection issue715");

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for type-parameter reflection issue715");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "MySchema",
            "2",
            "2",
            "MySchema",
            "id|id|int|false",
            "status|state|str|true",
            "BareSchema",
            "1",
        ],
        "unexpected type-parameter reflection output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_generic_value_field_reflection_calls_issue819() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_value_field_reflection_issue819", "")?;
    fs::write(
        &main_path,
        r#"model Row:
    id: int
    status: str
    paid: bool


def summarize_lookup[T](rows: list[T]) -> str:
    mut parts: list[str] = []
    if len(rows) == 0:
        return ",".join(parts)
    for field in T.__fields__():
        match rows[0].__field_value__(str(field.wire_name)):
            Some(value) =>
                parts.append(f"{field.wire_name}={value}")
            None =>
                parts.append(f"{field.wire_name}=<missing>")
    return "|".join(parts)


def summarize_items[T](rows: list[T]) -> str:
    mut parts: list[str] = []
    if len(rows) == 0:
        return ",".join(parts)
    for name, value in rows[0].__field_items__():
        parts.append(f"{name}={value}")
    return "|".join(parts)


def main() -> None:
    rows = [Row(id=1, status="paid", paid=true)]
    println(summarize_lookup[Row](rows))
    println(summarize_items[Row](rows))
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &["--check", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &check_output,
        "incan --check for generic value field reflection issue819",
    );

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for generic value field reflection issue819");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["id=1|status=paid|paid=true", "id=1|status=paid|paid=true"],
        "unexpected generic value reflection output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_optional_scalar_generic_value_reflection_issue819() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "generic_value_field_reflection_optional_scalar_issue819",
        "",
    )?;
    fs::write(
        &main_path,
        r#"model ProbeRow:
    id: int
    score: float
    active: bool
    label: str
    optional_label: Option[str]


def show_items[T](row: T) -> None:
    for name, value in row.__field_items__():
        println(f"{name}={value}")


def main() -> None:
    show_items[ProbeRow](ProbeRow(id=7, score=3.5, active=true, label="paid", optional_label=None))
    show_items[ProbeRow](ProbeRow(id=8, score=4.25, active=false, label="late", optional_label=Some("x")))
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &["--check", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &check_output,
        "incan --check for optional generic value field reflection issue819",
    );

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &run_output,
        "incan run for optional generic value field reflection issue819",
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "id=7",
            "score=3.5",
            "active=true",
            "label=paid",
            "optional_label=None",
            "id=8",
            "score=4.25",
            "active=false",
            "label=late",
            "optional_label=x",
        ],
        "unexpected optional generic value reflection output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_generic_value_reflection_in_method_owned_type_parameter_issue819() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(
        tmp.path(),
        "generic_value_reflection_method_owned_type_parameter_issue819",
        "",
    )?;
    fs::write(
        &main_path,
        r#"model ProbeRow:
    id: int
    label: str


class InlineSession:
    def reflected_summary[T](self, rows: list[T]) -> str:
        mut names: list[str] = []
        for field in T.__fields__():
            names.append(str(field.wire_name))
        if len(rows) == 0:
            return f"{T.__class_name__()}:{','.join(names)}:<empty>"
        match rows[0].__field_value__("label"):
            Some(value) => return f"{T.__class_name__()}:{','.join(names)}:{value}"
            None => return f"{T.__class_name__()}:{','.join(names)}:<missing>"


def main() -> None:
    session = InlineSession()
    println(session.reflected_summary[ProbeRow]([ProbeRow(id=7, label="paid")]))
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &["--check", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &check_output,
        "incan --check for method-owned generic value reflection issue819",
    );

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &run_output,
        "incan run for method-owned generic value reflection issue819",
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["ProbeRow:id,label:paid"],
        "unexpected method-owned generic value reflection output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_primitive_type_parameter_class_names_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "primitive_type_parameter_names_issue750", "")?;
    fs::write(
        &main_path,
        r#"pub def primitive_name[T]() -> str:
    return str(T.__class_name__())


pub def primitive_marker[T]() -> str:
    name = str(T.__class_name__())
    if name == "int":
        return "integer"
    if name == "float":
        return "floating"
    if name == "str":
        return "string"
    if name == "bool":
        return "boolean"
    return "other"


def main() -> None:
    println(primitive_name[int]())
    println(primitive_name[float]())
    println(primitive_name[str]())
    println(primitive_name[bool]())
    println(primitive_marker[int]())
    println(primitive_marker[float]())
    println(primitive_marker[str]())
    println(primitive_marker[bool]())
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &run_output,
        "incan run for primitive type-parameter class names issue750",
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "int", "float", "str", "bool", "integer", "floating", "string", "boolean",
        ],
        "unexpected primitive type-parameter metadata output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_pub_decorated_primitive_type_parameter_class_names_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("primitive_tokens");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "primitive_tokens"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("type_names.incn"),
        r#"def register[F]() -> (F) -> F:
    return (func) => func


pub def primitive_name[T]() -> str:
    return str(T.__class_name__())


pub def primitive_marker[T]() -> str:
    name = str(T.__class_name__())
    if name == "int":
        return "integer"
    if name == "float":
        return "floating"
    if name == "str":
        return "string"
    if name == "bool":
        return "boolean"
    return "other"


@register()
pub def decorated_primitive_marker[T]() -> str:
    return primitive_marker[T]()
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from type_names import decorated_primitive_marker, primitive_marker, primitive_name
"#,
    )?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for primitive type-parameter metadata issue750",
    );

    let consumer_root = tmp.path().join("primitive_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "primitive_consumer",
        r#"
[dependencies]
primitive_tokens = { path = "../primitive_tokens" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::primitive_tokens import decorated_primitive_marker, primitive_marker, primitive_name


def main() -> None:
    println(primitive_name[str]())
    println(primitive_marker[int]())
    println(decorated_primitive_marker[bool]())
"#,
    )?;

    let consumer_run = run_incan(
        &consumer_root,
        &[
            "run",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &consumer_run,
        "pub consumer run for primitive type-parameter metadata issue750",
    );
    let stdout = String::from_utf8_lossy(&consumer_run.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["str", "integer", "boolean"],
        "unexpected public primitive type-parameter metadata output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_primitive_type_token_overload_cast_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "primitive_type_token_cast_issue750", "")?;
    fs::write(
        &main_path,
        r#"pub model ColumnExpr:
    name: str


pub model IntColumnExpr:
    source: str


pub model FloatColumnExpr:
    source: str


pub model StringColumnExpr:
    source: str


pub type NumberColumnExpr = Union[IntColumnExpr, FloatColumnExpr]


pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)


pub def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return IntColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: Type[str]) -> StringColumnExpr:
    return StringColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return ColumnExpr(name=f"{expr.name}:{target}")


pub safe_cast = alias cast


pub def mul(left: NumberColumnExpr, right: NumberColumnExpr) -> FloatColumnExpr:
    return FloatColumnExpr(source="mul")


def main() -> None:
    amount: IntColumnExpr = cast(col("amount"), int)
    unit_price: NumberColumnExpr = cast(col("unit_price"), float)
    total: FloatColumnExpr = mul(cast(col("unit_price"), float), cast(col("qty"), float))
    fallback: ColumnExpr = cast(col("amount"), "decimal(10,2)")
    safe: FloatColumnExpr = safe_cast(col("safe"), float)
    println(amount.source)
    println(safe.source)
    println(total.source)
    println(fallback.name)
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for primitive type-token overload cast issue750");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["amount", "safe", "mul", "amount:decimal(10,2)"],
        "unexpected primitive type-token cast output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_pub_primitive_type_token_overload_cast_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("typed_casts");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "typed_casts"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("casts.incn"),
        r#"pub model ColumnExpr:
    name: str


pub model IntColumnExpr:
    source: str


pub model FloatColumnExpr:
    source: str


pub model StringColumnExpr:
    source: str


pub type NumberColumnExpr = Union[IntColumnExpr, FloatColumnExpr]


pub static registered_casts: list[str] = []


def register_cast_float[F]() -> ((F) -> F):
    return (func) => remember_cast_float[F](func)


def register_cast_string[F]() -> ((F) -> F):
    return (func) => remember_cast_string[F](func)


def remember_cast_float[F](func: F) -> F:
    registered_casts.append(func.__name__)
    return func


def remember_cast_string[F](func: F) -> F:
    registered_casts.append(func.__name__)
    return func


pub def col(name: str) -> ColumnExpr:
    return ColumnExpr(name=name)


pub def cast(expr: ColumnExpr, target: Type[int]) -> IntColumnExpr:
    return IntColumnExpr(source=expr.name)


@register_cast_float()
pub def cast(expr: ColumnExpr, target: Type[float]) -> FloatColumnExpr:
    return FloatColumnExpr(source=expr.name)


pub def cast(expr: ColumnExpr, target: Type[str]) -> StringColumnExpr:
    return StringColumnExpr(source=expr.name)


@register_cast_string()
pub def cast(expr: ColumnExpr, target: str) -> ColumnExpr:
    return ColumnExpr(name=f"{expr.name}:{target}")


pub def mul(left: NumberColumnExpr, right: NumberColumnExpr) -> FloatColumnExpr:
    return FloatColumnExpr(source="mul")


pub def registered_cast_count() -> int:
    return len(registered_casts)


pub def registered_cast_at(index: int) -> str:
    return registered_casts[index]
"#,
    )?;
    fs::write(
        producer_src.join("safe_alias.incn"),
        r#"from casts import cast


pub safe_cast = alias cast
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from casts import ColumnExpr, FloatColumnExpr, IntColumnExpr, NumberColumnExpr, cast, col, mul, registered_cast_at, registered_cast_count
pub from safe_alias import safe_cast
"#,
    )?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for primitive type-token cast issue750",
    );

    let producer_tests = producer_root.join("tests");
    fs::create_dir_all(&producer_tests)?;
    fs::write(
        producer_tests.join("test_safe_cast.incn"),
        r#"from lib import ColumnExpr, FloatColumnExpr, col, registered_cast_at, registered_cast_count, safe_cast


def test_cross_module_alias_preserves_overload_set() -> None:
    typed: FloatColumnExpr = safe_cast(col("safe"), float)
    fallback: ColumnExpr = safe_cast(col("safe"), "float64")
    assert typed.source == "safe"
    assert fallback.name == "safe:float64"
    assert registered_cast_count() == 2
    assert registered_cast_at(0) == "cast"
    assert registered_cast_at(1) == "cast"
"#,
    )?;
    let producer_test = run_incan(&producer_root, &["test", "tests"])?;
    assert_success(
        &producer_test,
        "producer incan test for cross-module overloaded alias issue750",
    );

    let consumer_root = tmp.path().join("typed_cast_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "typed_cast_consumer",
        r#"
[dependencies]
typed_casts = { path = "../typed_casts" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::typed_casts import ColumnExpr, FloatColumnExpr, IntColumnExpr, NumberColumnExpr, cast, col, mul, registered_cast_at, registered_cast_count, safe_cast


def main() -> None:
    amount: IntColumnExpr = cast(col("amount"), int)
    unit_price: NumberColumnExpr = cast(col("unit_price"), float)
    total: FloatColumnExpr = mul(cast(col("unit_price"), float), cast(col("qty"), float))
    fallback: ColumnExpr = cast(col("amount"), "decimal(10,2)")
    safe: FloatColumnExpr = safe_cast(col("safe"), float)
    println(amount.source)
    println(safe.source)
    println(total.source)
    println(fallback.name)
    println(str(registered_cast_count()))
    println(registered_cast_at(0))
    println(registered_cast_at(1))
"#,
    )?;

    let consumer_run = run_incan(
        &consumer_root,
        &[
            "run",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_run, "pub consumer run for primitive type-token cast issue750");
    let stdout = String::from_utf8_lossy(&consumer_run.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec!["amount", "safe", "mul", "amount:decimal(10,2)", "2", "cast", "cast",],
        "unexpected public primitive type-token cast output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_decorated_type_parameter_reflection_calls_issue715() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "decorated_type_parameter_reflection_issue715", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("reflection_helpers.incn"),
        r#"def requires_clone[T with Clone]() -> str:
    return "clone"


pub def reflected_schema_marker[T]() -> str:
    return f"{T.__class_name__()}:{len(T.__fields__())}:{requires_clone[T]()}"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from reflection_helpers import reflected_schema_marker


static decorated_names: list[str] = []


def register[F]() -> ((F) -> F):
    return (func) => remember[F](func)


def remember[F](func: F) -> F:
    decorated_names.append(func.__name__)
    return func


@register()
def class_name_for[T]() -> str:
    return str(T.__class_name__())


@register()
def field_count_for[T]() -> int:
    return len(T.__fields__())


def requires_clone[T with Clone]() -> str:
    return "clone"


@register()
def clone_marker_for[T]() -> str:
    return requires_clone[T]()


@register()
def imported_reflection_for[T]() -> str:
    return reflected_schema_marker[T]()


model MySchema:
    id: int
    status: str


def main() -> None:
    println(class_name_for[MySchema]())
    println(field_count_for[MySchema]())
    println(clone_marker_for[MySchema]())
    println(imported_reflection_for[MySchema]())
    println(imported_reflection_for[MySchema]())
    println(decorated_names[0])
    println(decorated_names[1])
    println(decorated_names[2])
    println(decorated_names[3])
    println(len(decorated_names))
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &run_output,
        "incan run for decorated type-parameter reflection issue715",
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "MySchema",
            "2",
            "clone",
            "MySchema:2:clone",
            "MySchema:2:clone",
            "class_name_for",
            "field_count_for",
            "clone_marker_for",
            "imported_reflection_for",
            "4",
        ],
        "unexpected decorated type-parameter reflection output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_model_type_token_value_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "model_type_token_value_issue750", "")?;
    fs::write(
        &main_path,
        r#"model MySchema:
    id: int
    status: str


def accepts_schema_type(value: Type[MySchema]) -> str:
    return "schema-token"


def main() -> None:
    println(accepts_schema_type(MySchema))
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for model type-token value issue750");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    assert_eq!(stdout.lines().collect::<Vec<_>>(), vec!["schema-token"]);
    Ok(())
}

#[test]
fn build_inline_fstring_rust_str_argument_issue716() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "inline_fstring_rust_str_argument_issue716", "")?;
    fs::write(
        &main_path,
        r#"from rust::incan_stdlib::errors import raise_value_error


def fail_inline(value: str) -> int:
    return raise_value_error(f"bad value `{value}`")


def fail_local(value: str) -> int:
    message = f"bad value `{value}`"
    return raise_value_error(message)


def main() -> None:
    fail_inline("x")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for inline f-string Rust &str argument issue716",
    );
    Ok(())
}

#[test]
fn build_inline_fstring_rust_string_variant_issue716() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_error");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_error\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub enum TinyError {
    Execution(String),
}

pub fn consume(err: TinyError) -> i64 {
    match err {
        TinyError::Execution(message) => message.len() as i64,
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "inline_fstring_rust_string_variant_issue716",
        r#"
[rust-dependencies]
tiny_error = { path = "rust/tiny_error" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_error import TinyError, consume


def make_error(value: str) -> int:
    return consume(TinyError.Execution(f"bad value `{value}`"))


def main() -> None:
    println(str(make_error("x")))
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for inline f-string Rust String enum variant issue716",
    );
    Ok(())
}

#[test]
fn build_static_str_const_rust_string_struct_field() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tiny_option");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tiny_option\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct FunctionOption {
    pub name: String,
    pub enabled: bool,
}

pub fn option_name(option: FunctionOption) -> String {
    option.name
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "static_str_const_rust_string_struct_field",
        r#"
[rust-dependencies]
tiny_option = { path = "rust/tiny_option" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tiny_option import FunctionOption, option_name


pub const OPTION_NAME: str = "sketch_family"


def main() -> None:
    option = FunctionOption(name=OPTION_NAME, enabled=True)
    println(option_name(option))
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for static str const into Rust String struct field",
    );
    Ok(())
}

#[test]
fn build_metadata_free_into_bound_tokenizer_encode_issue804() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let helper_dir = tmp.path().join("rust").join("tokenizers");
    fs::create_dir_all(helper_dir.join("src"))?;
    fs::write(
        helper_dir.join("Cargo.toml"),
        "[package]\nname = \"tokenizers\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        helper_dir.join("src").join("lib.rs"),
        r#"pub struct Tokenizer;

pub struct EncodeInput<'a>(&'a str);

impl<'a> From<&'a str> for EncodeInput<'a> {
    fn from(value: &'a str) -> Self {
        Self(value)
    }
}

impl Tokenizer {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<'a, E>(&self, value: E, _add_special_tokens: bool) -> Result<(), ()>
    where
        E: Into<EncodeInput<'a>>,
    {
        let _ = value.into();
        Ok(())
    }
}
"#,
    )?;
    let main_path = write_minimal_project(
        tmp.path(),
        "metadata_free_into_bound_tokenizer_encode_issue804",
        r#"
[rust-dependencies]
tokenizers = { path = "rust/tokenizers" }
"#,
    )?;
    fs::write(
        &main_path,
        r#"from rust::tokenizers import Tokenizer

def main() -> None:
    tokenizer = Tokenizer.new()
    literal = tokenizer.encode("literal", False)
    text = "variable"
    variable = tokenizer.encode(text, False)
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for metadata-free Into-bound tokenizer encode issue804",
    );
    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/metadata_free_into_bound_tokenizer_encode_issue804/src/main.rs"),
    )?;
    assert!(
        generated.contains("tokenizer.encode(\"literal\", false)"),
        "literal must preserve its direct &str shape, got:\n{generated}"
    );
    assert!(
        generated.contains("tokenizer.encode((text).as_str(), false)"),
        "owned Incan strings must become &str for the Into-bound method, got:\n{generated}"
    );
    Ok(())
}

#[test]
fn build_public_alias_of_imported_item_reexports_original_path_issue617() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "public_alias_import_reexport", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("helper.incn"),
        r#"pub def target(value: int) -> int:
    """Return one incremented value."""
    return value + 1
"#,
    )?;
    fs::write(
        &main_path,
        r#"from helper import target as target_builder


pub public_target = alias target_builder


def main() -> None:
    """Exercise public alias re-export of an imported public function."""
    assert public_target(1) == 2
"#,
    )?;

    let output_dir = tmp.path().join("out");
    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
            output_dir.to_str().ok_or("output path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&build_output, "public alias of imported item build");

    let generated_main = fs::read_to_string(output_dir.join("src/main.rs"))?;
    assert!(
        !generated_main.contains("pub use target_builder as public_target;"),
        "public alias should not re-export the private local import binding, got:\n{generated_main}"
    );
    assert!(
        generated_main.contains("pub use crate::helper::target as public_target;")
            || generated_main.contains("pub use helper::target as public_target;"),
        "public alias should re-export the original imported path, got:\n{generated_main}"
    );
    Ok(())
}

#[test]
fn build_pub_consumer_imports_public_alias_of_imported_item_issue617() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("alias_lib");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "alias_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("helper.incn"),
        r#"pub def target(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        producer_src.join("functions.incn"),
        r#"from helper import target as target_impl

pub public_target = alias target_impl
"#,
    )?;
    fs::write(
        producer_src.join("lib.incn"),
        r#"pub from functions import public_target
"#,
    )?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(&producer_build, "producer build --lib for public alias issue617");

    let manifest_path = producer_root.join("target").join("lib").join("alias_lib.incnlib");
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    assert!(
        manifest.pointer("/exports/aliases/0/projected_function").is_some(),
        "callable alias export should include function projection metadata, got:\n{manifest}"
    );

    let consumer_root = tmp.path().join("alias_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "alias_consumer",
        r#"
[dependencies]
alias_lib = { path = "../alias_lib" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::alias_lib import public_target


def main() -> None:
    assert public_target(1) == 2
"#,
    )?;

    let consumer_check = run_incan(
        &consumer_root,
        &[
            "--check",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_check, "pub consumer check for public alias issue617");
    Ok(())
}

#[test]
fn build_lib_materializes_facade_decorator_metadata_projection_issue695() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("metadata_registry");
    let src = producer_root.join("src");
    let operators = src.join("functions").join("operators");
    fs::create_dir_all(&operators)?;
    fs::write(
        producer_root.join("incan.toml"),
        r#"[project]
name = "metadata_registry"
version = "0.1.0"
"#,
    )?;
    fs::write(
        src.join("registry.incn"),
        r#"pub def registered[F](spec: str) -> ((F) -> F):
    return (func) => func
"#,
    )?;
    fs::write(
        operators.join("eq.incn"),
        r#"from registry import registered

pub model ColumnExpr:
    pub name: str

@registered("equal")
pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return left
"#,
    )?;
    fs::write(
        operators.join("mod.incn"),
        "pub from functions.operators.eq import eq\n",
    )?;
    fs::write(src.join("lib.incn"), "pub from functions.operators.mod import eq\n")?;

    let producer_build = run_incan(&producer_root, &["build", "--lib"])?;
    assert_success(
        &producer_build,
        "producer build --lib for decorator metadata projection issue695",
    );

    let manifest_path = producer_root
        .join("target")
        .join("lib")
        .join("metadata_registry.incnlib");
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    assert!(
        manifest.pointer("/exports/aliases/0/projected_function").is_some(),
        "reexport-only facade should materialize callable alias projection in manifest exports, got:\n{manifest}"
    );
    let api_modules = manifest
        .pointer("/contract_metadata/api/modules")
        .and_then(|value| value.as_array())
        .ok_or("expected checked API modules in manifest")?;
    let lib_alias = api_modules
        .iter()
        .flat_map(|module| {
            module
                .pointer("/declarations")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
        })
        .find(|decl| {
            decl.pointer("/kind").and_then(|value| value.as_str()) == Some("alias")
                && decl.pointer("/name").and_then(|value| value.as_str()) == Some("eq")
                && decl.pointer("/projected_function").is_some()
        })
        .ok_or("expected projected eq alias declaration in checked API metadata")?;
    assert_eq!(
        lib_alias
            .pointer("/projected_function/callable/name")
            .and_then(|value| value.as_str()),
        Some("eq")
    );
    assert_eq!(
        lib_alias
            .pointer("/projected_function/source_path")
            .and_then(|value| value.as_array())
            .map(|values| values.iter().filter_map(|value| value.as_str()).collect::<Vec<_>>()),
        Some(vec!["functions", "operators", "eq", "eq"])
    );
    assert!(
        lib_alias
            .pointer("/projected_function/decorators/0/decorated_callable/name")
            .and_then(|value| value.as_str())
            == Some("eq"),
        "projected decorator metadata should carry decorated callable identity/signature, got:\n{lib_alias}"
    );
    Ok(())
}

#[test]
fn test_accepts_public_alias_of_imported_item_issue631() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "public_alias_test_reexport", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("helper.incn"),
        r#"pub def target() -> int:
    return 1
"#,
    )?;
    fs::write(
        src_dir.join("functions.incn"),
        r#"from helper import target as target_builder

pub public_target = alias target_builder
"#,
    )?;
    fs::write(
        &main_path,
        r#"from functions import public_target


def main() -> None:
    assert public_target() == 1
"#,
    )?;
    fs::write(
        tests_dir.join("test_alias.incn"),
        r#"from functions import public_target


def test_alias() -> None:
    assert public_target() == 1
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&build_output, "incan build for public alias issue631");

    let test_path = tests_dir.join("test_alias.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for public alias issue631");
    Ok(())
}

#[test]
fn test_imported_public_partial_presets_keep_projected_call_surface_issue698() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "imported_public_partial_preset", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("presets.incn"),
        r#"pub model Spec:
    pub namespace: str
    pub policy: str
    pub klass: str
    pub lifecycle: str


"""Build a core portable spec."""
pub core_spec = partial Spec(namespace="core", policy="portable")
"#,
    )?;
    fs::write(
        tests_dir.join("test_imported_partial.incn"),
        r#"from presets import core_spec


def test_imported_partial_preset_keeps_presets() -> None:
    spec = core_spec(klass="scalar", lifecycle="v1")
    assert spec.namespace == "core"
    assert spec.policy == "portable"
    assert spec.klass == "scalar"
    assert spec.lifecycle == "v1"
"#,
    )?;

    let test_path = tests_dir.join("test_imported_partial.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for imported public partial issue698");
    Ok(())
}

#[test]
fn test_imported_partial_preset_defaults_survive_decorator_argument_issue698() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "imported_partial_decorator_argument", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("function_registry.incn"),
        r#"pub model FunctionSpec:
    pub namespace: str
    pub deterministic: bool
    pub lifecycle: str


pub static registered_names: list[str] = []
pub static registered_namespaces: list[str] = []


pub def capture(func: (int) -> int) -> ((int) -> int):
    registered_names.append(func.__name__)
    return func


pub def add(spec: FunctionSpec) -> (((int) -> int) -> ((int) -> int)):
    registered_namespaces.append(spec.namespace)
    return capture


pub deterministic_spec = partial FunctionSpec(namespace="core", deterministic=true)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from function_registry import add, deterministic_spec


@add(deterministic_spec(lifecycle="stable"))
pub def normalize(value: int) -> int:
    return value
"#,
    )?;
    fs::write(
        src_dir.join("registry_facade.incn"),
        r#"pub from function_registry import add, deterministic_spec
"#,
    )?;
    fs::write(
        src_dir.join("facade_helpers.incn"),
        r#"from registry_facade import add, deterministic_spec


@add(deterministic_spec(lifecycle="stable"))
pub def facade_normalize(value: int) -> int:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_registry_intent.incn"),
        r#"from function_registry import registered_names, registered_namespaces
from helpers import normalize
from facade_helpers import facade_normalize


def test_decorator_can_infer_name_with_imported_partial_spec() -> None:
    assert normalize(7) == 7
    assert registered_names[0] == "normalize"
    assert registered_namespaces[0] == "core"


def test_decorator_can_use_reexported_partial_spec() -> None:
    assert facade_normalize(8) == 8
    assert registered_names[1] == "facade_normalize"
    assert registered_namespaces[1] == "core"
"#,
    )?;

    let test_path = tests_dir.join("test_registry_intent.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(
        &test_output,
        "incan test for imported partial in decorator argument issue698",
    );
    Ok(())
}

#[test]
fn test_imported_partial_default_symbols_survive_decorator_argument_issue701() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "imported_partial_default_symbols_decorator", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub const DEFAULT_NAMESPACE: str = "core"


pub enum Policy(str):
    Portable = "portable"


pub model Spec:
    pub namespace: str
    pub policy: Policy
    pub lifecycle: str


pub static namespaces: list[str] = []
pub static names: list[str] = []


pub spec = partial Spec(namespace=DEFAULT_NAMESPACE, policy=Policy.Portable)


pub def capture(func: (int) -> int) -> ((int) -> int):
    names.append(func.__name__)
    return func


pub def add(spec_value: Spec) -> (((int) -> int) -> ((int) -> int)):
    namespaces.append(spec_value.namespace)
    return capture
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import add, spec


@add(spec(lifecycle="v1"))
pub def sample(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        tests_dir.join("test_partial_default_symbols.incn"),
        r#"from helpers import sample
from registry import names, namespaces


def test_partial_default_symbols_in_decorator() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
    assert namespaces[0] == "core"
"#,
    )?;

    let test_path = tests_dir.join("test_partial_default_symbols.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for imported partial default symbols issue701");
    Ok(())
}

#[test]
fn test_partial_constructor_presets_materialize_const_metadata_issue753() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "partial_constructor_const_metadata", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("metadata.incn"),
        r#"pub model Policy:
    pub family: FrozenStr
    pub role: FrozenStr
    pub enabled: bool


pub policy = partial Policy(family="hyperloglog", enabled=true)


pub const CONSTRUCT_POLICY: Policy = policy(role="construct")
pub const MERGE_POLICY: Policy = policy(role="merge", enabled=false)


pub def construct_enabled() -> bool:
    return CONSTRUCT_POLICY.enabled


pub def merge_enabled() -> bool:
    return MERGE_POLICY.enabled
"#,
    )?;
    fs::write(
        src_dir.join("runtime_consumer.incn"),
        r#"from metadata import policy


pub def runtime_policy_enabled() -> bool:
    return policy(role="runtime").enabled
"#,
    )?;
    fs::write(
        &main_path,
        r#"from metadata import Policy, construct_enabled, merge_enabled, policy
from runtime_consumer import runtime_policy_enabled


const IMPORTED_POLICY: Policy = policy(role="imported")


def main() -> None:
    assert construct_enabled()
    assert not merge_enabled()
    assert IMPORTED_POLICY.enabled
    assert runtime_policy_enabled()
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for partial constructor const metadata issue753",
    );
    Ok(())
}

#[test]
fn test_qualified_partial_constructor_presets_cross_package_const_metadata_issue699()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let provider_root = tmp.path().join("partialkit_provider");
    fs::create_dir_all(provider_root.join("src"))?;
    fs::write(
        provider_root.join("incan.toml"),
        "[project]\nname = \"partialkit\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        provider_root.join("src/models.incn"),
        r#"pub model Policy:
    pub family: FrozenStr
    pub role: FrozenStr
    pub enabled: bool
"#,
    )?;
    fs::write(
        provider_root.join("src/lib.incn"),
        r#"import models
pub from models import Policy


pub policy = partial models.Policy(family="cross-package", enabled=true)
"#,
    )?;

    let provider_output = run_incan(&provider_root, &["build", "--lib"])?;
    assert_success(
        &provider_output,
        "provider build for qualified partial constructor metadata issue699",
    );

    let consumer_root = tmp.path().join("consumer");
    fs::create_dir_all(consumer_root.join("src"))?;
    fs::write(
        consumer_root.join("incan.toml"),
        "[project]\nname = \"consumer\"\n\n[dependencies]\npartialkit = { path = \"../partialkit_provider\" }\n",
    )?;
    let main_path = consumer_root.join("src/main.incn");
    fs::write(
        &main_path,
        r#"from pub::partialkit import Policy, policy


const DEFAULT_POLICY: Policy = policy(role="consumer")


def main() -> None:
    assert DEFAULT_POLICY.enabled
"#,
    )?;

    let consumer_output = run_incan(
        &consumer_root,
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &consumer_output,
        "consumer build for qualified partial constructor metadata issue699",
    );
    Ok(())
}

#[test]
fn test_decorated_functions_preserve_default_argument_calls_issue703() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "decorated_default_argument_calls", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    fs::write(
        src_dir.join("columns.incn"),
        r#"pub model ColumnExpr:
    pub value: str


pub model Ref:
    pub name: str


pub model Literal:
    pub value: int


pub type Expr = Union[Ref, Literal]


pub def col(value: str) -> ColumnExpr:
    return ColumnExpr(value=value)


pub def union_col(name: str) -> Expr:
    return Ref(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("defaults.incn"),
        r#"pub model Ref:
    pub name: str


pub model Literal:
    pub value: int


pub type Expr = Union[Ref, Literal]


pub def col(name: str) -> Expr:
    return Ref(name=name)


def identity(func: (Expr) -> int) -> (Expr) -> int:
    return func


@identity
pub def decorated_default(expr: Expr = col("")) -> int:
    return 1
"#,
    )?;
    fs::write(
        src_dir.join("test_consumer.incn"),
        r#"from defaults import decorated_default


def test_imported_decorated_default_call() -> None:
    assert decorated_default() == 1
"#,
    )?;
    fs::write(
        src_dir.join("facade.incn"),
        r#"pub from defaults import decorated_default
"#,
    )?;
    fs::write(
        src_dir.join("facade_chain.incn"),
        r#"pub from facade import decorated_default
"#,
    )?;
    fs::write(
        src_dir.join("facade_alias.incn"),
        r#"pub from defaults import decorated_default as public_decorated_default
"#,
    )?;
    fs::write(
        src_dir.join("test_facade_consumer.incn"),
        r#"from facade import decorated_default


def test_reexported_decorated_default_call() -> None:
    assert decorated_default() == 1
"#,
    )?;
    fs::write(
        src_dir.join("test_facade_chain_consumer.incn"),
        r#"from facade_chain import decorated_default


def test_chained_reexported_decorated_default_call() -> None:
    assert decorated_default() == 1
"#,
    )?;
    fs::write(
        src_dir.join("test_facade_alias_consumer.incn"),
        r#"from facade_alias import public_decorated_default


def test_aliased_reexported_decorated_default_call() -> None:
    assert public_decorated_default() == 1
"#,
    )?;
    let functions_dir = src_dir.join("functions");
    let aggregates_dir = functions_dir.join("aggregates");
    fs::create_dir_all(&aggregates_dir)?;
    fs::write(
        aggregates_dir.join("count.incn"),
        r#"from defaults import Expr, col


def identity(func: (Expr) -> int) -> (Expr) -> int:
    return func


@identity
pub def count(expr: Expr = col("")) -> int:
    return 1
"#,
    )?;
    fs::write(
        functions_dir.join("mod.incn"),
        r#"pub from functions.aggregates.count import count
"#,
    )?;
    fs::write(
        src_dir.join("test_nested_facade_consumer.incn"),
        r#"from functions import count


def test_nested_reexported_decorated_default_call() -> None:
    assert count() == 1
"#,
    )?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        tests_dir.join("test_decorated_default_probe.incn"),
        r#"from columns import ColumnExpr, Expr, col, union_col


def identity(func: (int) -> int) -> ((int) -> int):
    return func


class Box:
    value: int

    @method_identity
    def decorated_method_default(self, value: int = 11) -> int:
        return value


def method_identity(func: (&Box, int) -> int) -> ((&Box, int) -> int):
    return func


@identity
def decorated_default(value: int = 7) -> int:
    return value


def count_identity(func: (ColumnExpr) -> int) -> ((ColumnExpr) -> int):
    return func


@count_identity
def count(expr: ColumnExpr = col("")) -> int:
    return 1


def union_count_identity(func: (Expr) -> int) -> ((Expr) -> int):
    return func


@union_count_identity
def union_count(expr: Expr = union_col("")) -> int:
    return 1


def adapted_impl(value: str) -> int:
    return 7


def string_adapter(func: (int) -> int) -> ((str) -> int):
    return adapted_impl


@string_adapter
def surface_changed(value: int = 7) -> int:
    return value


def plain_default(value: int = 7) -> int:
    return value


def plain_union_default(expr: Expr = union_col("")) -> int:
    return 1


def test_decorated_default_probe() -> None:
    assert plain_default() == 7
    assert plain_union_default() == 1
    assert plain_union_default(union_col("orders")) == 1
    assert decorated_default() == 7
    assert decorated_default(3) == 3
    box = Box(value=1)
    assert box.decorated_method_default() == 11
    assert box.decorated_method_default(5) == 5
    assert count() == 1
    assert count(col("orders")) == 1
    assert union_count() == 1
    assert union_count(union_col("orders")) == 1
    assert surface_changed("changed") == 7
"#,
    )?;

    let test_path = tmp.path().join("tests/test_decorated_default_probe.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for decorated default arguments issue703");

    let consumer_path = src_dir.join("test_consumer.incn");
    let consumer_output = run_incan(
        tmp.path(),
        &[
            "test",
            consumer_path.to_str().ok_or("consumer path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &consumer_output,
        "incan test for imported decorated default arguments issue703",
    );

    let facade_consumer_path = src_dir.join("test_facade_consumer.incn");
    let facade_consumer_output = run_incan(
        tmp.path(),
        &[
            "test",
            facade_consumer_path
                .to_str()
                .ok_or("facade consumer path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &facade_consumer_output,
        "incan test for re-exported decorated default arguments issue703",
    );

    let facade_chain_consumer_path = src_dir.join("test_facade_chain_consumer.incn");
    let facade_chain_consumer_output = run_incan(
        tmp.path(),
        &[
            "test",
            facade_chain_consumer_path
                .to_str()
                .ok_or("facade chain consumer path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &facade_chain_consumer_output,
        "incan test for chained re-exported decorated default arguments issue703",
    );

    let facade_alias_consumer_path = src_dir.join("test_facade_alias_consumer.incn");
    let facade_alias_consumer_output = run_incan(
        tmp.path(),
        &[
            "test",
            facade_alias_consumer_path
                .to_str()
                .ok_or("facade alias consumer path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &facade_alias_consumer_output,
        "incan test for aliased re-exported decorated default arguments issue703",
    );

    let nested_facade_consumer_path = src_dir.join("test_nested_facade_consumer.incn");
    let nested_facade_consumer_output = run_incan(
        tmp.path(),
        &[
            "test",
            nested_facade_consumer_path
                .to_str()
                .ok_or("nested facade consumer path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &nested_facade_consumer_output,
        "incan test for nested re-exported decorated default arguments issue703",
    );
    Ok(())
}

#[test]
fn test_facade_reexport_preserves_declared_source_import_alias_target_issue57() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "facade_reexport_import_alias_target", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let references_dir = src_dir.join("functions").join("references");
    let aggregates_dir = src_dir.join("functions").join("aggregates");
    fs::create_dir_all(&references_dir)?;
    fs::create_dir_all(&aggregates_dir)?;
    fs::write(
        src_dir.join("projection_builders.incn"),
        r#"pub model ColumnRefExpr:
    pub name: str


pub model ScalarFunctionExpr:
    pub name: str


pub type ColumnExpr = Union[ColumnRefExpr, ScalarFunctionExpr]


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("aggregate_builders.incn"),
        r#"from projection_builders import ColumnExpr, ScalarFunctionExpr


pub model AggregateMeasure:
    pub has_expr: bool


pub def col(name: str) -> ColumnExpr:
    return ScalarFunctionExpr(name=name)


pub def count(expr: Option[ColumnExpr] = None) -> AggregateMeasure:
    if let Some(_) = expr:
        return AggregateMeasure(has_expr=true)
    return AggregateMeasure(has_expr=false)
"#,
    )?;
    fs::write(
        references_dir.join("col.incn"),
        r#"from projection_builders import ColumnRefExpr, col as col_builder


pub def col(name: str) -> ColumnRefExpr:
    return col_builder(name)
"#,
    )?;
    fs::write(
        aggregates_dir.join("count.incn"),
        r#"from aggregate_builders import AggregateMeasure, count as count_builder
from projection_builders import ColumnExpr


pub def count(expr: Option[ColumnExpr] = None) -> AggregateMeasure:
    return count_builder(expr)


pub def count_expr(expr: ColumnExpr) -> AggregateMeasure:
    return count(expr)
"#,
    )?;
    fs::write(
        src_dir.join("functions.incn"),
        r#"pub from functions.references.col import col
pub from functions.aggregates.count import count, count_expr
"#,
    )?;

    let facade_path = src_dir.join("functions.incn");
    let emit_output = run_incan(
        tmp.path(),
        &[
            "--emit-rust",
            facade_path.to_str().ok_or("facade path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &emit_output,
        "emit-rust for facade re-export with colliding source import alias target",
    );
    Ok(())
}

#[test]
fn test_facade_reexport_preserves_decorated_helper_signature_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "facade_decorated_helper_signature", "")?;
    let src_dir = main_path.parent().ok_or("main path did not have a parent")?;
    let functions_dir = src_dir.join("functions");
    let operators_dir = functions_dir.join("operators");
    let references_dir = functions_dir.join("references");
    fs::create_dir_all(&operators_dir)?;
    fs::create_dir_all(&references_dir)?;
    fs::write(
        src_dir.join("projection_builders.incn"),
        r#"pub model ColumnRefExpr:
    pub name: str


pub model StringLiteralExpr:
    pub value: str


pub type ColumnExpr = Union[ColumnRefExpr, StringLiteralExpr]


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub def register[F]() -> (F) -> F:
    return (func) => func
"#,
    )?;
    fs::write(
        src_dir.join("filter_builders.incn"),
        r#"from projection_builders import ColumnExpr


pub def eq(left: ColumnExpr, right: ColumnExpr) -> ColumnExpr:
    return left
"#,
    )?;
    fs::write(
        src_dir.join("functions").join("inputs.incn"),
        r#"from projection_builders import ColumnExpr


pub type ScalarValueOrColumn = Union[ColumnExpr, str]
"#,
    )?;
    fs::write(
        references_dir.join("col.incn"),
        r#"from projection_builders import ColumnRefExpr, col as col_builder
from registry import register


@register()
pub def col(name: str) -> ColumnRefExpr:
    return col_builder(name)
"#,
    )?;
    fs::write(
        operators_dir.join("eq.incn"),
        r#"from functions.inputs import ScalarValueOrColumn
from registry import register


@register()
pub def eq(left: ScalarValueOrColumn, right: ScalarValueOrColumn) -> None:
    return
"#,
    )?;
    fs::write(
        src_dir.join("functions").join("mod.incn"),
        r#"pub from functions.inputs import ScalarValueOrColumn
pub from functions.references.col import col
pub from functions.operators.eq import eq
pub from filter_builders import eq as filter_eq
"#,
    )?;
    let scratch_dir = tmp.path().join(".agents").join("tmp");
    fs::create_dir_all(&scratch_dir)?;
    let scratch_path = scratch_dir.join("repro_facade_eq.incn");
    fs::write(
        &scratch_path,
        r#"from functions import col, eq


pub def repro() -> None:
    eq(col("status"), "paid")
"#,
    )?;

    let check_output = run_incan(
        tmp.path(),
        &[
            "--check",
            scratch_path.to_str().ok_or("scratch path was not valid UTF-8")?,
        ],
    )?;
    assert_success(
        &check_output,
        "incan check for facade re-export preserving decorated helper signature",
    );
    Ok(())
}

#[test]
fn test_incan_call_widens_list_elements_to_union_argument_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "incan_list_element_union_arg", "")?;
    fs::write(
        &main_path,
        r#"pub model ColumnRefExpr:
    pub name: str


pub model StringColumnExpr:
    pub name: str


pub type ColumnExpr = Union[ColumnRefExpr, StringColumnExpr]


pub def registered_application(arguments: list[ColumnExpr]) -> ColumnExpr:
    return arguments[0]


pub def str_col(name: str) -> StringColumnExpr:
    return StringColumnExpr(name=name)


pub def concat(first: str) -> ColumnExpr:
    mut arguments = [str_col(first)]
    arguments.append(str_col("tail"))
    return registered_application(arguments)


pub def concat_direct(first: str) -> ColumnExpr:
    return registered_application([str_col(first)])


def main() -> None:
    concat("name")
    concat_direct("name")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for list element union widening at an Incan call boundary",
    );
    Ok(())
}

#[test]
fn test_incan_call_widens_imported_list_elements_to_union_argument_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "incan_imported_list_element_union_arg", "")?;
    let src_dir = main_path.parent().ok_or("main path did not have a parent")?;
    fs::write(
        src_dir.join("types.incn"),
        r#"pub model A:
    pub value: str


pub model B:
    pub value: str


pub type U = Union[A, B]


pub type Outer = Union[U, int]
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from types import A


pub def a(value: str) -> A:
    return A(value=value)
"#,
    )?;
    fs::write(
        &main_path,
        r#"from helpers import a
from types import Outer, U


pub def repro(name: str) -> int:
    return takes([a(name)])


pub def repro_nested(name: str) -> int:
    return takes_nested([a(name)])


pub def takes(values: list[U]) -> int:
    return len(values)


pub def takes_nested(values: list[Outer]) -> int:
    return len(values)


def main() -> None:
    repro("name")
    repro_nested("name")
"#,
    )?;

    let build_output = run_incan(
        tmp.path(),
        &["build", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(
        &build_output,
        "incan build for imported list element union widening at an Incan call boundary",
    );
    Ok(())
}

#[test]
fn test_multi_file_test_batch_keeps_file_local_import_scopes_issue57() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "test_batch_file_local_import_scopes", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("projection_builders.incn"),
        r#"pub model ColumnRefExpr:
    pub name: str


pub model ScalarFunctionExpr:
    pub name: str


pub type ColumnExpr = Union[ColumnRefExpr, ScalarFunctionExpr]


pub def col(name: str) -> ColumnRefExpr:
    return ColumnRefExpr(name=name)
"#,
    )?;
    fs::write(
        src_dir.join("aggregate_builders.incn"),
        r#"from projection_builders import ColumnExpr, ScalarFunctionExpr


pub def col(name: str) -> ColumnExpr:
    return ScalarFunctionExpr(name=name)
"#,
    )?;
    fs::write(
        tests_dir.join("test_projection_col.incn"),
        r#"from projection_builders import ColumnRefExpr, col


def test_projection_col_keeps_concrete_return_type() -> None:
    ref: ColumnRefExpr = col("customer_id")
    assert ref.name == "customer_id"
"#,
    )?;
    fs::write(
        tests_dir.join("test_aggregate_col.incn"),
        r#"from aggregate_builders import col
from projection_builders import ColumnExpr


def test_aggregate_col_keeps_union_return_type() -> None:
    expr: ColumnExpr = col("customer_id")
    assert true
"#,
    )?;

    let test_output = run_incan(tmp.path(), &["test", "tests"])?;
    assert_success(
        &test_output,
        "incan test multi-file batch with same local import name from different modules",
    );
    let test_batches_dir = tmp.path().join("target").join("incan_tests");
    let isolated_projection_module = fs::read_dir(&test_batches_dir)?.filter_map(Result::ok).any(|entry| {
        entry
            .path()
            .join("src")
            .join("tests")
            .join("test_projection_col.rs")
            .exists()
    });
    let isolated_aggregate_module = fs::read_dir(&test_batches_dir)?.filter_map(Result::ok).any(|entry| {
        entry
            .path()
            .join("src")
            .join("tests")
            .join("test_aggregate_col.rs")
            .exists()
    });
    assert!(
        isolated_projection_module && isolated_aggregate_module,
        "multi-file test batch should emit each test file as its own Rust module"
    );
    Ok(())
}

#[test]
fn test_decorator_callable_exposes_source_name_issue694() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "decorator_callable_name", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        &main_path,
        r#"def main() -> None:
    pass
"#,
    )?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture(func: (int) -> int) -> ((int) -> int):
    names.append(func.__name__)
    return func


pub def registered() -> (((int) -> int) -> ((int) -> int)):
    return capture
"#,
    )?;
    fs::write(
        src_dir.join("registry_facade.incn"),
        r#"pub from registry import names, registered
"#,
    )?;
    fs::write(
        tests_dir.join("test_callable_name.incn"),
        r#"from registry import names, registered
from registry_facade import registered as facade_registered


@registered()
pub def sample(value: int) -> int:
    return value + 1


@facade_registered()
pub def facade_sample(value: int) -> int:
    return value + 2


def test_decorator_can_read_specific_callable_name() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
    assert facade_sample(1) == 3
    assert names[1] == "facade_sample"
"#,
    )?;

    let test_path = tests_dir.join("test_callable_name.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for decorator callable name issue694");
    Ok(())
}

#[test]
fn test_generic_decorator_callable_exposes_source_name_issue694() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_decorator_callable_name", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture[F](func: F) -> F:
    names.append(func.__name__)
    return func


pub def registered[F]() -> ((F) -> F):
    return (func) => capture[F](func)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import names, registered


@registered[(int) -> int]()
pub def sample(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        tests_dir.join("test_generic_callable_name.incn"),
        r#"from registry import names
from helpers import sample


def test_generic_decorator_can_read_callable_name() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
"#,
    )?;

    let test_path = tests_dir.join("test_generic_callable_name.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(&test_output, "incan test for generic decorator callable name issue694");
    Ok(())
}

#[test]
fn test_generic_decorator_callable_name_accepts_imported_alias_union_issue701() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_callable_name_imported_alias_union", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("types.incn"),
        r#"pub model A:
    pub value: int


pub model B:
    pub value: int


pub type Expr = Union[A, B]
"#,
    )?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture[F](func: F) -> F:
    names.append(func.__name__)
    return func


pub def register[F]() -> ((F) -> F):
    return (func) => capture[F](func)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import register
from types import Expr


@register[(Expr) -> Expr]()
pub def identity_expr(value: Expr) -> Expr:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_alias_union_callable_name.incn"),
        r#"from helpers import identity_expr
from registry import names
from types import A


def test_alias_union_callable_name() -> None:
    identity_expr(A(value=1))
    assert names[0] == "identity_expr"
"#,
    )?;

    let test_path = tests_dir.join("test_alias_union_callable_name.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(
        &test_output,
        "incan test for alias/union generic callable name issue701",
    );
    Ok(())
}

#[test]
fn test_generic_callable_name_planning_ignores_unrelated_async_signatures_issue701()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_callable_name_with_async_noise", "")?;
    let src_dir = main_path.parent().ok_or("main path had no parent")?;
    let tests_dir = tmp.path().join("tests");
    fs::create_dir_all(&tests_dir)?;
    fs::write(
        src_dir.join("registry.incn"),
        r#"pub static names: list[str] = []


pub def capture[F](func: F) -> F:
    names.append(func.__name__)
    return func


pub def register[F]() -> ((F) -> F):
    return (func) => capture[F](func)
"#,
    )?;
    fs::write(
        src_dir.join("helpers.incn"),
        r#"from registry import register


@register[(int) -> int]()
pub def sample(value: int) -> int:
    return value + 1
"#,
    )?;
    fs::write(
        src_dir.join("noise.incn"),
        r#"pub async def unrelated_async(delay: float) -> None:
    return


pub def unrelated_generic[T](value: T) -> T:
    return value
"#,
    )?;
    fs::write(
        tests_dir.join("test_scoped_callable_name_planning.incn"),
        r#"from helpers import sample
from registry import names


def test_generic_callable_name_ignores_unrelated_signatures() -> None:
    assert sample(1) == 2
    assert names[0] == "sample"
"#,
    )?;

    let test_path = tests_dir.join("test_scoped_callable_name_planning.incn");
    let test_output = run_incan(
        tmp.path(),
        &["test", test_path.to_str().ok_or("test path was not valid UTF-8")?],
    )?;
    assert_success(
        &test_output,
        "incan test for scoped generic callable-name planning issue701",
    );
    Ok(())
}

#[test]
fn build_frozen_uses_existing_lockfile_without_network() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "cli_frozen_existing_lock_project", "")?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock before frozen build");

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--frozen",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;

    assert_success(&build_output, "incan build --frozen with existing lockfile");
    let stdout = String::from_utf8_lossy(&build_output.stdout);
    assert!(
        stdout.contains("Build successful"),
        "frozen build should complete with the existing lockfile, got:\n{stdout}"
    );
    Ok(())
}
