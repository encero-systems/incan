//! Issue #1372: an integral-valued `float` renders with its decimal point, as Python spells it.
//!
//! Rust's `Display for f64` prints `100.0` as `100`, so f-string interpolation, `str()`, `println()`, and computed
//! results all wrote an integer where the program meant a float -- a SQL predicate came out as `> 100` rather than
//! `> 100.0`. The rendering now belongs to the runtime (`incan_std_core::strings::float_to_string`), and this root
//! runs the issue's own program through Oven to prove every display position agrees. Debug interpolation keeps
//! Rust's formatting, which was already Python-shaped for these values.

use std::fs;
use std::process::Command;

use incan_test_support::cli_project::{assert_success, run_explicit_oven_bake, run_incan, write_minimal_project};

/// The issue's reproduction, extended with the exponent, non-finite, and `str()`-concatenation cases.
const MAIN_SOURCE: &str = r#"def main() -> None:
    a: float = 100.0
    b: float = 1.5
    c: float = 0.0
    d: float = -2.0
    e: float = 1e10
    big: float = 1e16
    tiny: float = 1.5e-7
    println(f"interp integral : {a}")
    println(f"interp fraction : {b}")
    println(f"interp zero     : {c}")
    println(f"interp negative : {d}")
    println(f"interp big      : {e}")
    println(f"interp exponent : {big} {tiny}")
    println(f"interp inf      : {float('inf')} {float('-inf')} {float('nan')}")
    println(f"debug integral  : {a:?}")
    println(str(a))
    println(f"arith           : {a / 4.0}")
    println("concat          : " + str(a) + " " + str(b))
    println(a, b)
"#;

/// Build the program through Oven and compare every printed line.
#[test]
fn integral_floats_render_with_their_decimal_point_issue1372() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = "float_display_1372";
    write_minimal_project(tmp.path(), project_name, "")?;
    fs::write(tmp.path().join("src/main.incn"), MAIN_SOURCE)?;

    // Under the compiler suite the build resolves through the suite's stdlib Loaf and needs no Cargo. Outside it a
    // fresh project has no inspection authority until it is baked once, so the standalone run bakes first.
    if !incan_test_support::oven_compiler_suite_is_active() {
        let bake = run_explicit_oven_bake(tmp.path())?;
        assert_success(&bake, "prepare the #1372 float-display fixture");
    }
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build the #1372 float-display program");

    let binary = tmp
        .path()
        .join("target/incan")
        .join(project_name)
        .join("oven/release")
        .join(project_name);
    let run = Command::new(&binary).output()?;
    assert!(
        run.status.success(),
        "expected the #1372 executable to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout)?.lines().collect::<Vec<_>>(),
        vec![
            "interp integral : 100.0",
            "interp fraction : 1.5",
            "interp zero     : 0.0",
            "interp negative : -2.0",
            "interp big      : 10000000000.0",
            "interp exponent : 1e+16 1.5e-07",
            "interp inf      : inf -inf nan",
            "debug integral  : 100.0",
            "100.0",
            "arith           : 25.0",
            "concat          : 100.0 1.5",
            "100.0 1.5",
        ]
    );
    Ok(())
}
