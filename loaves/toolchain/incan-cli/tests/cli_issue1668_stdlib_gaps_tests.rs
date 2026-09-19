//! Issue #1668: the stdlib surfaces a command-line tool needs, driven end to end through Oven.
//!
//! One program covers `std.environ.args()`, the `str.encode` / `bytes.decode` text round trip, mutable
//! `Dict.contains_key`, and the emitter shapes the issue met on 0.5.1: indexed assignment and membership on a `mut
//! Dict` parameter, a `mut str` local reassigned inside a `match` arm, and `dict.keys()` / `dict.values()` outside a
//! comprehension. The built executable is run directly so the argument vector carries real arguments; `incan run`
//! has no program-argument passthrough.

use std::fs;
use std::process::Command;

use incan_test_support::cli_project::{assert_success, run_explicit_oven_bake, run_incan, write_minimal_project};

/// The program under test; every line of its output is asserted below.
const MAIN_SOURCE: &str = r#"from std.environ import args

enum Verdict:
    Admitted
    Rejected(str)

def record(mut files: Dict[str, str], key: str, value: str) -> None:
    files[key] = value

def seen(mut files: Dict[str, str], key: str) -> bool:
    return key in files

def lookup(mut files: Dict[str, str], key: str) -> str:
    if key in files:
        return files[key]
    return "missing"

def count(mut hits: Dict[str, int], key: str) -> int:
    return hits.get(key).copied().unwrap_or(0)

def describe(verdict: Verdict) -> str:
    mut label = "pending"
    match verdict:
        Verdict.Admitted => label = "admitted"
        Verdict.Rejected(reason) => label = reason
    return label

def sorted_keys(files: Dict[str, str]) -> list[str]:
    return sorted(files.keys())

def main() -> None:
    arguments = args()
    println(len(arguments) - 1)
    for argument in arguments[1:]:
        println(argument)

    mut files: Dict[str, str] = {}
    record(files, "b.incn", "beta")
    record(files, "a.incn", "alpha")
    println(files.contains_key("a.incn"))
    println(files.contains_key("missing"))
    println(seen(files, "b.incn"))
    println(lookup(files, "a.incn"))
    mut hits: Dict[str, int] = {}
    hits["a.incn"] = 3
    println(count(hits, "a.incn"))
    println("a.incn" in files.keys())
    println(",".join(sorted_keys(files)))
    println(len(files.values()))
    mut total = 0
    for value in files.values():
        total += len(value)
    println(total)

    println(describe(Verdict.Admitted))
    println(describe(Verdict.Rejected("no manifest")))

    payload = arguments[1].encode()
    println(len(payload))
    println(payload.decode())
    println(b"\xff".decode(errors="replace"))
    println("UTF_8".encode("utf8").decode("UTF-8", "strict"))
"#;

/// Build the program through Oven and run the executable with real arguments.
#[test]
fn stdlib_gaps_argv_text_codecs_and_dict_surfaces_issue1668() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = "stdlib_gaps_1668";
    write_minimal_project(tmp.path(), project_name, "")?;
    fs::write(tmp.path().join("src/main.incn"), MAIN_SOURCE)?;

    // Outside the compiler suite a fresh project has no inspection authority until it is baked once; under the suite
    // the bake is the same explicit publisher boundary every other build-based root crosses.
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare the #1668 stdlib-gaps fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build the #1668 stdlib-gaps program");

    let binary = tmp
        .path()
        .join("target/incan")
        .join(project_name)
        .join("oven/release")
        .join(project_name);
    assert!(
        binary.is_file(),
        "expected Oven to produce the #1668 executable at {}\nstdout:\n{}\nstderr:\n{}",
        binary.display(),
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new(&binary).args(["admit", "héllo"]).output()?;
    assert!(
        run.status.success(),
        "expected the #1668 executable to run.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout)?.lines().collect::<Vec<_>>(),
        vec![
            // std.environ.args(): the program name is skipped, the two real arguments follow in order.
            "2",
            "admit",
            "héllo",
            // Mutable Dict.contains_key, and `in` / `.get()` / indexed assignment on a `mut Dict` parameter.
            "true",
            "false",
            "true",
            "alpha",
            "3",
            // dict.keys() / dict.values() outside a comprehension.
            "true",
            "a.incn,b.incn",
            "2",
            "9",
            // mut str local reassigned inside match arms.
            "admitted",
            "no manifest",
            // str.encode / bytes.decode: "admit" is five UTF-8 bytes; U+FFFD replaces the lone 0xff.
            "5",
            "admit",
            "\u{FFFD}",
            "UTF_8",
        ],
        "the #1668 program must print every surface's expected value"
    );
    Ok(())
}
