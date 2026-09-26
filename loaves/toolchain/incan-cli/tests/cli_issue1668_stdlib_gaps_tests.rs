//! Issue #1668: the stdlib surfaces a command-line tool needs, driven end to end through Oven.
//!
//! One program covers `std.environ.args()`, the `str.encode` / `bytes.decode` text round trip, mutable
//! `Dict.contains_key`, and the emitter shapes the issue met on 0.5.1: indexed assignment and membership on a `mut
//! Dict` parameter, a `mut str` local reassigned inside a `match` arm, and `dict.keys()` / `dict.values()` outside a
//! comprehension. The built executable is run directly so the argument vector carries real arguments; `incan run`
//! has no program-argument passthrough.
//!
//! A second program covers the dict-view conversions the same tool tripped over: `sorted(dict.keys())` driving a
//! `for` loop (#1461) and `list(dict.keys())` / `list(dict.values())` materializing a list (#1464).

use std::fs;
use std::process::Command;

use incan_test_support::cli_project::{assert_success, run_explicit_oven_bake, run_incan, write_minimal_project};

/// The program under test; every line of its output is asserted below.
const MAIN_SOURCE: &str = r#"from std.environ import EnvironError, args

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

def main() -> Result[None, EnvironError]:
    arguments = args()?
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

    match text_round_trip(arguments[1]):
        Ok(_) => println("round-trip ok")
        Err(error) => println(f"{error}")
    return Ok(None)

def text_round_trip(word: str) -> Result[None, ValidationError]:
    payload = word.encode()
    println(len(payload))
    println(payload.decode()?)
    println(b"\xff".decode(errors="replace")?)
    println("UTF_8".encode("utf8").decode("UTF-8", "strict")?)
    match b"\xff".decode():
        Ok(text) => println(text)
        Err(error) => println(f"{error}")
    return Ok(None)
"#;

/// Build the program through Oven and run the executable with real arguments.
#[test]
fn stdlib_gaps_argv_text_codecs_and_dict_surfaces_issue1668() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_name = "stdlib_gaps_1668";
    write_minimal_project(tmp.path(), project_name, "")?;
    fs::write(tmp.path().join("src/main.incn"), MAIN_SOURCE)?;

    // Under the compiler suite the build resolves through the suite's stdlib Loaf and needs no Cargo; this root is
    // deliberately not in the explicit-bake registry (`OvenCompilerSuiteTargetCapabilities`). Outside the suite a
    // fresh project has no inspection authority until it is baked once, so the standalone run bakes first.
    if !incan_test_support::oven_compiler_suite_is_active() {
        let bake = run_explicit_oven_bake(tmp.path())?;
        assert_success(&bake, "prepare the #1668 stdlib-gaps fixture");
    }
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
            // str.encode / bytes.decode: "admit" is five UTF-8 bytes; U+FFFD replaces the lone 0xff under
            // errors="replace", and the strict default reports it as an Err whose message names the offset.
            "5",
            "admit",
            "\u{FFFD}",
            "UTF_8",
            "invalid-utf8: 'utf-8' codec can't decode bytes: invalid utf-8 sequence of 1 bytes from index 0",
            "round-trip ok",
        ],
        "the #1668 program must print every surface's expected value"
    );
    Ok(())
}

/// Dict views handed to `sorted()` and `list()`; every line of its output is asserted below.
const DICT_VIEWS_SOURCE: &str = r#"const TEXTS: FrozenList[str] = ["b", "a"]

def ordered_keys(values: Dict[str, int]) -> list[str]:
    return sorted(values.keys())

def key_list(values: Dict[str, int]) -> list[str]:
    return list(values.keys())

def value_total(values: Dict[str, int]) -> int:
    mut total = 0
    for value in list(values.values()):
        total += value
    return total

def appended(mut names: list[str]) -> list[str]:
    names.append("d")
    return list(names)

def main() -> None:
    values: Dict[str, int] = {"b": 2, "a": 1, "c": 3}
    for key in sorted(values.keys()):
        println(key)
    for value in sorted(values.values()):
        println(value)
    println(",".join(ordered_keys(values)))
    for key in list(values.keys()):
        println(key in values)
    println(",".join(sorted(key_list(values))))
    println(value_total(values))
    mut extra: list[str] = ["z"]
    println(",".join(appended(extra)))
    println(",".join(sorted(list(values))))
    println(len(list("abc")))
    println(",".join(sorted(list(TEXTS))))
    names: list[str] = list()
    println(len(names))
"#;

/// `sorted(dict.keys())` and `sorted(dict.values())` drive a `for` loop through a real build (#1461), and
/// `list(...)` over dict views, a dict, a list, a `mut` list parameter, text, a frozen list of text, and nothing at
/// all materializes a list (#1464): the emitted Rust must never sort or re-iterate the borrowed `Keys` / `Values`
/// iterator, nor call an undefined `list` function.
#[test]
fn sorted_and_listed_dict_views_issues1461_1464() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "dict_views_1461_1464", "")?;
    fs::write(tmp.path().join("src/main.incn"), DICT_VIEWS_SOURCE)?;

    // Same suite-versus-standalone split as the #1668 program above: bake only where no Loaf authority exists yet.
    if !incan_test_support::oven_compiler_suite_is_active() {
        let bake = run_explicit_oven_bake(tmp.path())?;
        assert_success(&bake, "prepare the #1461 / #1464 dict-views fixture");
    }
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run the #1461 / #1464 dict-views program");
    assert_eq!(
        String::from_utf8(run.stdout)?.lines().collect::<Vec<_>>(),
        vec![
            // sorted() over dict views driving loops and a join.
            "a", "b", "c", "1", "2", "3", "a,b,c",
            // list() over dict views: every listed key is a member, and the listed keys sort back to the same order.
            "true", "true", "true", "a,b,c", "6",
            // list() over a mut list parameter, a dict (its keys), text, a frozen list of text, and no source at all.
            "z,d", "a,b,c", "3", "a,b", "0",
        ],
        "sorted and listed dict views must yield the expected keys and values"
    );
    Ok(())
}
