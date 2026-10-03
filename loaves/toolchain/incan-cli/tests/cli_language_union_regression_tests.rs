//! Language and codegen regressions reproduced through `incan build`, `run`, and `test`.
//!
//! One of nine roots split out of the former `tests/cli_integration.rs`. The shared command surface lives in
//! `incan_test_support::cli_project`.

include!("support/cli_language_regression_tests_root.rs");

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
        r#"from pub::union_provider import add as vocab_helper_union_provider_add
from pub::union_provider import col as vocab_helper_union_provider_col
from pub::union_provider import desc as vocab_helper_union_provider_desc
from pub::union_provider import frame as vocab_helper_union_provider_frame


def main() -> None:
    vocab_helper_union_provider_frame().filter(
        vocab_helper_union_provider_add(vocab_helper_union_provider_col("amount"), 5),
    )
    vocab_helper_union_provider_frame().order_by([
        vocab_helper_union_provider_desc(vocab_helper_union_provider_col("amount")),
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
