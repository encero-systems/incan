//! RFC 089 `std.environ` accessors driven end to end through `incan run`.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

use std::fs;

use incan_test_support::cli_project::*;

/// One normal command covers the passing `std.environ` access matrix.
///
/// These formerly independent fixtures each created a fresh project and repeated the same direct-rustc journey. Their
/// accessor contracts are orthogonal but composable, so failures still name the precise assertion without paying for
/// four copies of normal-command setup.
#[test]
fn run_std_environ_passing_accessors_share_one_program_issues557_rfc089() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "std_environ_access_matrix", "")?;
    fs::write(
        &main_path,
        r#"from std.environ import EnvironError, get, get_as, get_optional, get_or
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

def require[T with TryFrom[str]](key: str) -> Result[T, EnvironError]:
  match get_as[T](key)?:
    Some(value) => return Ok(value)
    None => return Err(EnvironError.missing(key))

def read_primitive_values() -> Result[None, EnvironError]:
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

type DefaultPort = newtype int:
  def from_underlying(value: int) -> Result[Self, ValidationError]:
    if value < 1 or value > 65535:
      return Err(ValidationError("port out of range"))
    return Ok(DefaultPort(value))

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

  match read_primitive_values():
    Ok(_) => pass
    Err(error) => println(error.message())

  match get_as[int]("INCAN_ENVIRON_DEFAULT_MISSING", 8080):
    Ok(value) => println(f"positional:{value}")
    Err(error) => println(f"positional:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_DEFAULT_PRESENT", default=8080):
    Ok(value) => println(f"keyword:{value}")
    Err(error) => println(f"keyword:{error.kind_name()}")

  match get_as[int]("INCAN_ENVIRON_DEFAULT_INVALID", 8080):
    Ok(value) => println(f"invalid:unexpected:{value}")
    Err(error) => println(f"invalid:{error.kind_name()}")

  match get_as[DefaultPort]("INCAN_ENVIRON_DEFAULT_PORT", 5432):
    Ok(port) => println(f"newtype:{port.0}")
    Err(error) => println(f"newtype:{error.kind_name()}")
"#,
    )?;

    let run_output = run_incan_with_env_and_removed(
        tmp.path(),
        &["run", main_path.to_str().ok_or("non-utf8 main path")?],
        &[
            ("INCAN_ENVIRON_PRESENT", "present-value"),
            ("INCAN_ENVIRON_PORT", "5432"),
            ("INCAN_ENVIRON_PORT_BAD", "70000"),
            ("INCAN_ENVIRON_LABEL", "worker"),
            ("INCAN_ENVIRON_MODE", "prod"),
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
            ("INCAN_ENVIRON_DEFAULT_PRESENT", "9090"),
            ("INCAN_ENVIRON_DEFAULT_INVALID", "not-an-int"),
        ],
        &[
            "INCAN_ENVIRON_MISSING_TEST",
            "INCAN_ENVIRON_MISSING_PORT",
            "INCAN_ENVIRON_MISSING_INTEGER",
            "INCAN_ENVIRON_DEFAULT_MISSING",
            "INCAN_ENVIRON_DEFAULT_PORT",
        ],
    )?;
    assert_success(&run_output, "incan run for the std.environ passing access matrix");

    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout),
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
            "present:5432\n",
            "missing:missing\n",
            "invalid:invalid_value:INCAN_ENVIRON_PORT_BAD\n",
            "empty:invalid_key:\n",
            "label:worker\n",
            "mode:prod\n",
            "42:3.5:true:hello\n",
            "-8\n",
            "f32:1.25\n",
            "widths:-16:-32:-64:-128:-7:8:16:32:64:128:7:6.25\n",
            "bool:false\n",
            "i8-overflow:invalid_value\n",
            "u8-negative:invalid_value\n",
            "environment variable `INCAN_ENVIRON_BAD_F64` could not be parsed or validated as `the requested type`\n",
            "bool-invalid:invalid_value\n",
            "invalid_value:INCAN_ENVIRON_BAD_INTEGER\n",
            "environment variable `INCAN_ENVIRON_REDACTED` could not be parsed or validated as `the requested type`\n",
            "missing\n",
            "positional:8080\n",
            "keyword:9090\n",
            "invalid:invalid_value\n",
            "newtype:5432\n",
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
