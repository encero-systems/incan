//! Language and codegen regressions reproduced through `incan build`, `run`, and `test`.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_language_regression_tests_root.rs");

#[test]
fn build_locked_map_err_string_literal_closure_issue880() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "map_err_string_literal_closure_issue880", "")?;
    fs::write(
        &main_path,
        r#"from std.json import JsonValue

def parse(source: str) -> Result[JsonValue, str]:
    return JsonValue.parse(source).map_err((_error) => "malformed_json")

def main() -> None:
    match parse("{"):
        Ok(_) => println("unexpected")
        Err(error) => println(error)
"#,
    )?;

    let lock_output = run_incan(
        tmp.path(),
        &["lock", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&lock_output, "incan lock for map_err closure issue880");

    let build_output = run_incan(
        tmp.path(),
        &[
            "build",
            "--locked",
            main_path.to_str().ok_or("main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&build_output, "incan build --locked for map_err closure issue880");

    let generated = fs::read_to_string(
        tmp.path()
            .join("target/incan/map_err_string_literal_closure_issue880/src/main.rs"),
    )?;
    assert!(
        generated.contains("map_err(|_error| \"malformed_json\".to_string())"),
        "expected generated Rust to own the map_err closure literal, got:\n{generated}"
    );
    Ok(())
}

/// Verifies that a direct zero-argument call in an f-string survives the complete CLI pipeline.
#[test]
fn fstring_interpolation_zero_arg_function_call_issue979() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "fstring_zero_arg_call_issue979", "")?;
    fs::write(
        &main_path,
        r#"def enabled() -> bool:
  return True

def main() -> None:
  println(f"enabled:{enabled()}")
"#,
    )?;

    let main_arg = main_path.to_str().ok_or("main path was not valid UTF-8")?;
    let output = run_incan(tmp.path(), &["run", main_arg, "--sdk-profile", "minimal"])?;
    assert_success(&output, "incan run with a direct f-string zero-argument call");
    assert_eq!(String::from_utf8(output.stdout)?, "enabled:true\n");
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
