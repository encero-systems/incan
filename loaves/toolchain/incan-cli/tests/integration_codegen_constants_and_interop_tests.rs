#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

mod codegen_tests {
    use incan_driver::backend::IrCodegen;
    use incan_frontend::{lexer, parser, typechecker};
    include!("support/integration_tests_codegen_tests.rs");

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
