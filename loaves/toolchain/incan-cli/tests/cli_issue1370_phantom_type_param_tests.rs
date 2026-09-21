//! Issue #1370: a generic model whose type parameter appears only in method signatures builds and runs.
//!
//! `incan check` accepted the issue's `Column[T]` while the generated Rust failed under rustc with E0392 (`T` never
//! used) and E0282 at every construction site, so the defect is only visible on a real build. Lowering now records
//! the phantom parameter on the struct and emission carries it as a `PhantomData` marker, initializes the marker at
//! every struct literal, and threads the explicit constructor type argument. The built executable is run so the
//! operator method's output is asserted, not only compilation, and the `Debug` rendering is printed to prove the
//! marker stays out of it.

use std::fs;
use std::process::Command;

use incan_test_support::cli_project::{assert_success, run_explicit_oven_bake, run_incan, write_minimal_project};

/// The issue's program plus one construction whose binding carries no annotation, so only the explicit type
/// argument names `T` for Rust, and one private field (`note`) that only the `Debug` rendering reads: the emitter
/// gives such a field a dead-code expectation, and the hand-written `Debug` impl must count as a derive for that
/// expectation to stay fulfilled.
const MAIN_SOURCE: &str = r#"@derive(Debug)
model Column[T]:
  sql: str
  note: str = "n"

  def __mul__(self, other: T) -> Column[T]:
    return Column[T](sql=f"({self.sql} * {other})")

def col[T](name: str) -> Column[T]:
  return Column[T](sql=name)

def main() -> None:
  amount: Column[float] = col("amount")
  total = amount * 2.0
  println(f"{total.sql}")
  label = Column[str](sql="label")
  println(f"{(label * "x").sql}")
  println(f"{amount:?}")
"#;

/// Build the phantom-parameter program through Oven and run the executable.
#[test]
fn phantom_type_parameter_model_builds_and_runs_issue1370() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = "phantom_type_param_1370";
    write_minimal_project(tmp.path(), project_name, "")?;
    fs::write(tmp.path().join("src/main.incn"), MAIN_SOURCE)?;

    // Under the compiler suite the build resolves through the suite's stdlib Loaf and needs no Cargo. Outside the
    // suite a fresh project has no inspection authority until it is baked once, so the standalone run bakes first.
    if !incan_test_support::oven_compiler_suite_is_active() {
        let bake = run_explicit_oven_bake(tmp.path())?;
        assert_success(&bake, "prepare the #1370 phantom-type-parameter fixture");
    }
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build the #1370 phantom-type-parameter program");
    let build_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(
        !build_output.contains("unfulfilled"),
        "the hand-written Debug impl must not leave a private field's dead-code expectation unfulfilled:\n{build_output}"
    );

    let binary = tmp
        .path()
        .join("target/incan")
        .join(project_name)
        .join("oven/release")
        .join(project_name);
    assert!(
        binary.is_file(),
        "expected Oven to produce the #1370 executable at {}\nstdout:\n{}\nstderr:\n{}",
        binary.display(),
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&binary).output()?;
    assert!(
        run.status.success(),
        "expected the #1370 executable to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout)?.lines().collect::<Vec<_>>(),
        vec!["(amount * 2)", "(label * x)", "Column { sql: \"amount\", note: \"n\" }"],
        "the #1370 program must print both operator results and a Debug rendering without the marker"
    );
    Ok(())
}
