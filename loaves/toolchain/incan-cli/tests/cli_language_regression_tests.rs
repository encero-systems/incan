//! Language and codegen regressions reproduced through `incan build`, `run`, and `test`.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

use std::fs;

use incan_test_support::cli_project::*;

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
        fs::read_to_string(incan_test_support::repo_root().join("tests/fixtures/nested_list_loop_1471.incn"))?,
    )?;
    let bake = run_explicit_oven_bake(tmp.path())?;
    assert_success(&bake, "prepare nested empty-first list fixture");
    let build = run_incan(tmp.path(), &["build", "src/main.incn"])?;
    assert_success(&build, "build inferred nested string lists");
    let run = run_incan(tmp.path(), &["run", "src/main.incn"])?;
    assert_success(&run, "run nested-list count and content assertions");
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

#[test]
fn build_union_widening_converts_generated_wrappers_issue741() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let src_dir = tmp.path().join("src");
    fs::create_dir_all(&src_dir)?;
    fs::write(
        tmp.path().join("loaf.toml"),
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
    pub value: str


pub model B:
    pub value: str


pub model Holder:
    pub value: Extended


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

    let generated_main = read_generated_rust(&tmp.path().join("target/incan/union_widening_conversion/src/main.rs"))?;
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
        imported_root.join("loaf.toml"),
        r#"[project]
name = "union_imported_alias"
version = "0.1.0"
"#,
    )?;
    fs::write(
        imported_src.join("types.incn"),
        r#"
pub model A:
    pub value: str


pub model B:
    pub value: str


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
        producer_root.join("loaf.toml"),
        r#"[project]
name = "union_lib"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("defs.incn"),
        r#"
pub model A:
    pub value: str


pub model B:
    pub value: str


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
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(&producer_build, "explicit Oven bake for public union widening issue741");

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
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for public union widening consumer issue741",
    );
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
        producer_root.join("loaf.toml"),
        r#"[project]
name = "querykit"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("defs.incn"),
        r#"
pub model IntExpr:
    pub value: int


pub model TextExpr:
    pub value: str


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
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(&producer_build, "explicit Oven bake for optional union helper issue745");

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
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for optional union helper consumer issue745",
    );
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
        producer_root.join("loaf.toml"),
        r#"[project]
name = "union_provider"
version = "0.1.0"
"#,
    )?;
    fs::write(
        producer_src.join("surface.incn"),
        r#"
pub model ColumnRefExpr:
    pub name: str


pub model NumberColumnExpr:
    pub expr: ColumnRefExpr


pub model SortExpr:
    pub expr: ColumnRefExpr


pub type ColumnExpr = Union[ColumnRefExpr, NumberColumnExpr, SortExpr]
pub type NumberValueOrColumn = Union[ColumnRefExpr, NumberColumnExpr, int]


pub model Frame:
    pub source: str

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
    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(
        &producer_build,
        "explicit Oven bake for dependency-owned union boundary issue755",
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
    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for dependency-owned union consumer issue755",
    );
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
    // The dependency's wrapper reaches the emitter down two routes -- a caller-supplied crate qualifier, and the
    // carried native union's own recorded path -- and the two spell the same item relatively and absolutely. Both
    // resolve to the dependency's wrapper, which is what this regression is about, so accept either; asserting one
    // spelling made this fail on a generated file that was already correct.
    assert!(
        generated_consumer.contains("union_provider::desc(union_provider::__IncanUnion")
            || generated_consumer.contains("union_provider::desc(::union_provider::__IncanUnion"),
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
    pub value: str


pub model B:
    pub value: str


pub model C:
    pub value: str


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
fn test_runner_prefers_project_sibling_import_over_unimported_stdlib_stub_type()
-> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let project_root = tmp.path();
    fs::write(
        project_root.join("loaf.toml"),
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
        project_root.join("loaf.toml"),
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
fn run_generic_reflection_contracts_issues712_715_819() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "generic_reflection_contracts", "")?;
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
        src_dir.join("reflection_helpers.incn"),
        r#"def requires_clone[T with Clone]() -> str:
    return "clone"


pub def reflected_schema_marker[T]() -> str:
    return f"{T.__class_name__()}:{len(T.__fields__())}:{requires_clone[T]()}"
"#,
    )?;
    fs::write(
        &main_path,
        r#"from generic_reflection_helpers import imported_class_name, imported_field_count
from schema_helpers import class_name_for as schema_class_name_for, field_count_for as schema_field_count_for, print_schema
from reflection_helpers import reflected_schema_marker


model NamedRow:
    name: str


class Bare:
    value: int


model MySchema:
    id [description="Stable id"]: int
    status [alias="state"]: str = "new"


class BareSchema:
    value: int


model Row:
    id: int
    status: str
    paid: bool


model ProbeRow:
    id: int
    score: float
    active: bool
    label: str
    optional_label: Option[str]


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


def show_items[T](row: T) -> None:
    for name, value in row.__field_items__():
        println(f"{name}={value}")


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


static decorated_names: list[str] = []


def register[F]() -> ((F) -> F):
    return (func) => remember[F](func)


def remember[F](func: F) -> F:
    decorated_names.append(func.__name__)
    return func


@register()
def decorated_class_name_for[T]() -> str:
    return str(T.__class_name__())


@register()
def decorated_field_count_for[T]() -> int:
    return len(T.__fields__())


def requires_clone[T with Clone]() -> str:
    return "clone"


@register()
def clone_marker_for[T]() -> str:
    return requires_clone[T]()


@register()
def imported_reflection_for[T]() -> str:
    return reflected_schema_marker[T]()


def reflected_field_count[T](value: T) -> int:
    return len(value.__fields__())


def reflected_class_name[T](value: T) -> str:
    return str(value.__class_name__())


def local_field_count[T]() -> int:
    return len(T.__fields__())


def main() -> None:
    named = NamedRow(name="Ada")
    println(reflected_class_name(named))
    println(reflected_field_count(named))
    println(imported_class_name(named))
    println(imported_field_count(named))
    bare = Bare(value=1)
    println(bare.__class_name__())
    println(len(bare.__fields__()))
    println(reflected_class_name(bare))
    println(reflected_field_count(bare))
    println(imported_class_name(bare))
    println(imported_field_count(bare))
    println(schema_class_name_for[MySchema]())
    println(schema_field_count_for[MySchema]())
    println(local_field_count[MySchema]())
    print_schema[MySchema]()
    println(schema_class_name_for[BareSchema]())
    println(schema_field_count_for[BareSchema]())
    rows = [Row(id=1, status="paid", paid=true)]
    println(summarize_lookup[Row](rows))
    println(summarize_items[Row](rows))
    show_items[ProbeRow](ProbeRow(id=7, score=3.5, active=true, label="paid", optional_label=None))
    show_items[ProbeRow](ProbeRow(id=8, score=4.25, active=false, label="late", optional_label=Some("x")))
    session = InlineSession()
    println(session.reflected_summary[ProbeRow]([ProbeRow(id=7, score=3.5, active=true, label="paid", optional_label=None)]))
    println(decorated_class_name_for[MySchema]())
    println(decorated_field_count_for[MySchema]())
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
        "incan run for generic reflection contracts issues712/715/819",
    );
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "NamedRow",
            "1",
            "NamedRow",
            "1",
            "Bare",
            "0",
            "Bare",
            "0",
            "Bare",
            "0",
            "MySchema",
            "2",
            "2",
            "MySchema",
            "id|id|int|false",
            "status|state|str|true",
            "BareSchema",
            "0",
            "id=1|status=paid|paid=true",
            "id=1|status=paid|paid=true",
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
            "ProbeRow:id,score,active,label,optional_label:paid",
            "MySchema",
            "2",
            "clone",
            "MySchema:2:clone",
            "MySchema:2:clone",
            "decorated_class_name_for",
            "decorated_field_count_for",
            "clone_marker_for",
            "imported_reflection_for",
            "4",
        ],
        "unexpected generic reflection contracts output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_direct_type_token_contracts_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let main_path = write_minimal_project(tmp.path(), "direct_type_token_contracts_issue750", "")?;
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


pub model ColumnExpr:
    pub name: str


pub model IntColumnExpr:
    pub source: str


pub model FloatColumnExpr:
    pub source: str


pub model StringColumnExpr:
    pub source: str


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


model MySchema:
    id: int
    status: str


def accepts_schema_type(value: Type[MySchema]) -> str:
    return "schema-token"


def main() -> None:
    println(primitive_name[int]())
    println(primitive_name[float]())
    println(primitive_name[str]())
    println(primitive_name[bool]())
    println(primitive_marker[int]())
    println(primitive_marker[float]())
    println(primitive_marker[str]())
    println(primitive_marker[bool]())
    amount: IntColumnExpr = cast(col("amount"), int)
    unit_price: NumberColumnExpr = cast(col("unit_price"), float)
    total: FloatColumnExpr = mul(cast(col("unit_price"), float), cast(col("qty"), float))
    fallback: ColumnExpr = cast(col("amount"), "decimal(10,2)")
    safe: FloatColumnExpr = safe_cast(col("safe"), float)
    println(amount.source)
    println(safe.source)
    println(total.source)
    println(fallback.name)
    println(accepts_schema_type(MySchema))
"#,
    )?;

    let run_output = run_incan(
        tmp.path(),
        &["run", main_path.to_str().ok_or("main path was not valid UTF-8")?],
    )?;
    assert_success(&run_output, "incan run for direct type-token contracts issue750");
    let stdout = String::from_utf8_lossy(&run_output.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "int",
            "float",
            "str",
            "bool",
            "integer",
            "floating",
            "string",
            "boolean",
            "amount",
            "safe",
            "mul",
            "amount:decimal(10,2)",
            "schema-token",
        ],
        "unexpected direct type-token contracts output:\n{stdout}"
    );
    Ok(())
}

#[test]
fn run_pub_type_token_contracts_issue750() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let producer_root = tmp.path().join("type_token_provider");
    let producer_src = producer_root.join("src");
    fs::create_dir_all(&producer_src)?;
    fs::write(
        producer_root.join("loaf.toml"),
        r#"[project]
name = "type_token_provider"
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
pub from casts import ColumnExpr, FloatColumnExpr, IntColumnExpr, NumberColumnExpr, cast, col, mul, registered_cast_at, registered_cast_count
pub from safe_alias import safe_cast
"#,
    )?;
    fs::write(
        producer_src.join("casts.incn"),
        r#"pub model ColumnExpr:
    pub name: str


pub model IntColumnExpr:
    pub source: str


pub model FloatColumnExpr:
    pub source: str


pub model StringColumnExpr:
    pub source: str


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

    let producer_build = run_explicit_oven_bake(&producer_root)?;
    assert_success(
        &producer_build,
        "explicit Oven bake for public type-token contracts issue750",
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
        "provider test batch for cross-module overloaded alias issue750",
    );

    let consumer_root = tmp.path().join("primitive_consumer");
    let consumer_main = write_minimal_project(
        &consumer_root,
        "type_token_consumer",
        r#"
[dependencies]
type_token_provider = { path = "../type_token_provider" }
"#,
    )?;
    fs::write(
        &consumer_main,
        r#"from pub::type_token_provider import ColumnExpr, FloatColumnExpr, IntColumnExpr, NumberColumnExpr, cast, col, decorated_primitive_marker, mul, primitive_marker, primitive_name, registered_cast_at, registered_cast_count, safe_cast


def main() -> None:
    println(primitive_name[str]())
    println(primitive_marker[int]())
    println(decorated_primitive_marker[bool]())
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

    let consumer_bake = run_explicit_oven_bake(&consumer_root)?;
    assert_success(
        &consumer_bake,
        "explicit Oven bake for type-token contracts consumer issue750",
    );

    let consumer_run = run_incan(
        &consumer_root,
        &[
            "run",
            consumer_main.to_str().ok_or("consumer main path was not valid UTF-8")?,
        ],
    )?;
    assert_success(&consumer_run, "public consumer run for type-token contracts issue750");
    let stdout = String::from_utf8_lossy(&consumer_run.stdout);
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines,
        vec![
            "str",
            "integer",
            "boolean",
            "amount",
            "safe",
            "mul",
            "amount:decimal(10,2)",
            "2",
            "cast",
            "cast",
        ],
        "unexpected public type-token contracts output:\n{stdout}"
    );
    Ok(())
}

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
