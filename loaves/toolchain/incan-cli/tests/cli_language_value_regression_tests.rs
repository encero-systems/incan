//! Language and codegen regressions reproduced through `incan build`, `run`, and `test`.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_language_regression_tests_root.rs");

#[test]
fn run_synchronous_result_main_issue843() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "synchronous_result_main", "")?;
    fs::write(
        &main_path,
        r#"def run() -> Result[None, str]:
  println("fallible entrypoint")
  return Ok(None)

def main() -> Result[None, str]:
  run()?
  return Ok(None)
"#,
    )?;

    let output = run_incan(tmp.path(), &["run", main_path.to_str().ok_or("non-utf8 main path")?])?;
    assert_success(&output, "incan run with a synchronous Result-returning main");
    assert_eq!(String::from_utf8(output.stdout)?, "fallible entrypoint\n");
    Ok(())
}

/// Infer owned strings from later list members through a real build and runtime loop.
#[test]
fn nested_empty_first_list_runs_without_a_caller_annotation_issue1471() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "nested_empty_first_list", "")?;
    fs::write(
        tmp.path().join("src/main.incn"),
        fs::read_to_string(incan_test_support::fixture("nested_list_loop_1471.incn"))?,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare nested empty-first list fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build inferred nested string lists");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run nested-list count and content assertions");
    Ok(())
}

/// `Result[None, toml.TomlError]` reaches Rust as a type path when `toml` is a stdlib module binding (#1437).
///
/// The unit-level reproducers cover a project-local module. This is the shape the issue reports, and the only one
/// that resolves the member through the SDK provider inventory rather than the source module registry. The generated
/// Rust is inspected first, because the defect was an emitter panic on the dotted spelling; the program is then baked
/// and run so the emitted path is proven to link against the provider.
#[test]
fn module_qualified_stdlib_type_annotation_emits_and_runs_issue1437() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "qualified_stdlib_type", "")?;
    fs::write(
        &main_path,
        r#"from std import toml

pub def result() -> Result[None, toml.TomlError]:
    return Ok(None)

def main() -> None:
    match result():
        Ok(_) => println("qualified ok")
        Err(_) => println("qualified err")
"#,
    )?;

    let emitted = run_incan(tmp.path(), &["--emit-rust", "src/main.incn"])?;
    assert_success(&emitted, "emit-rust with a module-qualified stdlib type annotation");
    let rust = String::from_utf8(emitted.stdout)?;
    assert!(
        rust.contains("__incan_std::toml::TomlError"),
        "the qualified annotation must emit the provider type by its module path:\n{rust}"
    );
    assert!(
        !rust.contains("toml.TomlError"),
        "no dotted spelling may survive into generated Rust:\n{rust}"
    );

    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "bake a module-qualified stdlib type annotation");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run a module-qualified stdlib type annotation");
    assert_eq!(String::from_utf8(run.stdout)?, "qualified ok\n");
    Ok(())
}

/// Named constructor arguments evaluate in the order they were written, however the model declares its fields. The
/// emitter assembles the construction in declaration order, so `evidence=Evidence(source=source)` moved `source`
/// before `intent=inspect(source)` had read it, and the generated Rust failed with E0382 (#1462).
#[test]
fn named_constructor_arguments_evaluate_in_written_order_issue1462() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "constructor_argument_order", "")?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"model Evidence:
    source: str

model Document:
    evidence: Evidence
    intent: str

def inspect(source: str) -> str:
    return f"inspect:{source}"

def main() -> None:
    source = "manifest"
    result = Document(intent=inspect(source), evidence=Evidence(source=source))
    println(result.intent)
    println(result.evidence.source)
"#,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare the reordered constructor fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(
        &build,
        "build a construction whose arguments are written out of declaration order",
    );
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run the reordered constructor program");
    assert_eq!(
        String::from_utf8(run.stdout)?.lines().collect::<Vec<_>>(),
        vec!["inspect:manifest", "manifest"],
        "both arguments must observe `source`, in written order"
    );
    Ok(())
}

/// A `for` over `list[list[str]]` binds its variable by reference, so returning it as an `Ok` payload needs the
/// owned value. The payload used to bypass ownership planning and reached rustc as `&Vec<String>` (E0308, #1489),
/// while an owned local returned from the same loop must still move rather than clone.
#[test]
fn returning_a_loop_variable_from_a_list_of_lists_issue1489() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "loop_variable_returned", "")?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"def require_member(values: list[list[str]], name: str) -> Result[list[str], str]:
    for row in values:
        if row[0] == name:
            return Ok(row)
    return Err("absent")

def first_named(names: list[str], name: str) -> Result[str, str]:
    for candidate in names:
        if candidate == name:
            return Ok(candidate)
    return Err("absent")

def collect_named(names: list[str], name: str) -> Result[list[str], str]:
    mut found: list[str] = []
    for candidate in names:
        if candidate == name:
            found.append(candidate)
            return Ok(found)
    return Err("absent")

def main() -> None:
    match require_member([["a", "b"], ["c", "d"]], "c"):
        Ok(row) => println(",".join(row))
        Err(reason) => println(reason)
    match require_member([["a"]], "z"):
        Ok(row) => println(",".join(row))
        Err(reason) => println(reason)
    match first_named(["a", "b"], "b"):
        Ok(found) => println(found)
        Err(reason) => println(reason)
    match collect_named(["a", "b"], "b"):
        Ok(found) => println(len(found))
        Err(reason) => println(reason)
"#,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare the returned loop variable fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build a function returning its loop variable as an Ok payload");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run the returned loop variable program");
    assert_eq!(
        String::from_utf8(run.stdout)?.lines().collect::<Vec<_>>(),
        vec!["c,d", "absent", "b", "1"],
        "each shape must return the owned value it found"
    );
    Ok(())
}

/// A string literal passed to a collection method reaches the `String` parameter owned (#1494). The live failure was
/// `Deque[str].from_iter(...)` followed by `append("default")`: the type application lost its argument, the receiver
/// reached emission as `Deque[Unknown]`, and the literal passed as `&str` (E0308). #1529 fixed that in the type
/// resolver; the in-process harness had passed all along because it takes the module path, so this pins the packaged
/// path through a real build, alongside the builtin list, set and dict receivers.
#[test]
fn string_literals_reach_collection_methods_owned_issue1494() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "collection_method_literals", "")?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"from std.collections import Deque

def probe(requested: list[str]) -> int:
    mut pending = Deque[str].from_iter(requested)
    pending.append("default")
    pending.appendleft("first")
    mut names: list[str] = []
    names.append("default")
    mut seen: set[str] = set()
    seen.add("default")
    seen.add("default")
    mut labels: Dict[str, str] = {}
    labels["default"] = "value"
    labels.insert("first", "value")
    return len(pending) + len(names) + len(seen) + len(labels)

def main() -> None:
    println(probe(["a"]))
"#,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare the collection-method literal fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build collection methods receiving string literals");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run the collection-method literal program");
    assert_eq!(
        String::from_utf8(run.stdout)?.trim(),
        "7",
        "three deque items, one list item, one set item and two dict entries"
    );
    Ok(())
}

/// Comparing a `list[str]` against an empty literal, in either order and with either equality operator, compiles and
/// runs with the JSON dependency linked (#1476). That cohort brings extra `String: PartialEq<_>` impls into scope,
/// which is what turned an untyped `vec![]` operand into E0283; the operand now carries its element type from the
/// checker, so the generated Rust spells `Vec::<String>::new()`.
#[test]
fn empty_list_equality_operands_build_with_json_cohort_issue1476() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "empty_list_equality", "")?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"from std.json import JsonValue

def is_empty(values: list[str]) -> bool:
    return values == []

def is_not_empty(values: list[str]) -> bool:
    return [] != values

def empty_first(values: list[str]) -> bool:
    return [] == values

def nonempty_first(values: list[str]) -> bool:
    return values != []

def main() -> None:
    value = JsonValue.null()
    assert value.is_null()
    assert is_empty([])
    assert not is_empty(["value"])
    assert is_not_empty(["value"])
    assert not is_not_empty([])
    assert empty_first([])
    assert not empty_first(["value"])
    assert nonempty_first(["value"])
    assert not nonempty_first([])
    println("empty-list comparisons ok")
"#,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare the empty-list equality fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build empty-list comparisons alongside the JSON dependency");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run the empty-list comparison assertions");
    assert_eq!(String::from_utf8(run.stdout)?.trim(), "empty-list comparisons ok");
    Ok(())
}

/// `{{` and `}}` are the f-string escapes for one literal brace, as in Python, so `f"{{{name}}}"` renders `{x}`. The
/// lexer collapsed them correctly; emission then brace-escaped every literal segment again for a `format!` string it
/// no longer builds, and both characters reached the output.
#[test]
fn fstring_doubled_braces_render_one_literal_brace() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_minimal_project(tmp.path(), "fstring_braces", "")?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"def main() -> None:
    name = "x"
    assert f"{{" == "{"
    assert f"}}" == "}"
    assert f"{{{name}}}" == "{x}"
    assert f"{{name}}" == "{name}"
    assert f'{{"name": "{name}"}}' == '{"name": "x"}'
    println("braces ok")
"#,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare f-string brace fixture");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run f-string brace assertions");
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("braces ok"),
        "expected the program to reach its final line:\n{}",
        String::from_utf8_lossy(&run.stdout)
    );
    Ok(())
}

#[test]
fn build_assert_string_inequality_in_list_loop_issue739() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
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

/// Inline and bound model comparisons agree without moving operands or evaluating their fields twice.
#[test]
fn model_constructor_comparison_runs_equivalent_assertions() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname = \"model_constructor_comparison\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"from std.testing import assert_eq, assert_ne, assert_false

@derive(Eq)
model Pair:
    x: str

def observed(tag: str) -> str:
    println(tag)
    return "same"

def failure_message() -> str:
    println("unexpected failure message")
    return "model comparison failed"

pub def main() -> None:
    value = Pair(x="same")
    expected = Pair(x="same")
    assert value == Pair(x="same")
    assert Pair(x="same") == value
    assert Pair(x="same") == Pair(x="same")
    assert value != Pair(x="different")
    assert Pair(x="different") != value
    assert (value == Pair(x="same")) == (Pair(x="same") == value)
    assert not (value != Pair(x="same"))
    assert_eq(value, Pair(x="same"))
    assert_eq(Pair(x="same"), value)
    assert_ne(value, Pair(x="different"))
    assert_ne(Pair(x="different"), value)
    assert_false(value != Pair(x="same"))
    assert value == expected
    assert value == Pair(x="same"), failure_message()
    assert Pair(x=observed("left")) == Pair(x=observed("right"))
    println(value.x)
    println(expected.x)
    println("ok")
"#,
    )?;

    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare inline model comparison fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build inline model comparison assertions");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run inline model comparison assertions");
    assert_eq!(String::from_utf8(run.stdout)?, "left\nright\nsame\nsame\nok\n");
    Ok(())
}

/// Unequal inline models still fail at runtime and evaluate the failure message exactly once.
#[test]
fn model_constructor_comparison_preserves_assertion_failure() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::create_dir_all(tmp.path().join("src"))?;
    fs::write(
        tmp.path().join("loaf.toml"),
        "[project]\nname = \"model_constructor_comparison_failure\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(
        tmp.path().join("src/main.incn"),
        r#"@derive(Eq)
model Pair:
    x: str

def observed(tag: str) -> str:
    println(tag)
    return tag

def failure_message() -> str:
    println("failure message")
    return "model comparison failed"

pub def main() -> None:
    assert Pair(x=observed("left")) == Pair(x=observed("right")), failure_message()
    println("unreachable")
"#,
    )?;

    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare failing inline model comparison fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build failing inline model comparison assertion");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_failure(&run, "unequal inline models must fail their assertion");
    assert_eq!(String::from_utf8(run.stdout)?, "left\nright\nfailure message\n");
    let stderr = String::from_utf8(run.stderr)?;
    assert!(
        stderr.contains("AssertionError: model comparison failed; left != right"),
        "the normal assertion failure must survive native execution: {stderr}"
    );
    Ok(())
}
