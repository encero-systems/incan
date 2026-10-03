#![allow(dead_code)] // Split test binaries intentionally use different parts of shared support fragments.

//! Integration tests for the Incan compiler frontend

include!("support/integration_tests_root.rs");

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
    // Keep the locked consumer path while avoiding a bake: this program needs no Rust-import inspection.
    let lock_output = incan_command()
        .args(["lock", "src/main.incn"])
        .current_dir(tmp.path())
        .env("INCAN_HOME", &oven_home)
        .output()?;
    assert!(
        lock_output.status.success(),
        "expected exact-float arithmetic inputs to lock.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&lock_output.stdout),
        String::from_utf8_lossy(&lock_output.stderr)
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
    // Same-carrier `f32` arithmetic keeps Rust's native spelling (`13`, `5`, ...); a mixed `f32`/`f64` or
    // `f32`/`int` operation is checked as ordinary `float`, which renders as Python spells it (#1372): `13.0`,
    // `2.0`, `1.0`, `6561.0`, `10.0`.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "13\n5\n36\n2.25\n2\n1\n6561\n13.0\n2.25\n2.0\n1.0\n6561.0\n9.5\n10.0\ntrue\nfalse\ntrue\n"
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
