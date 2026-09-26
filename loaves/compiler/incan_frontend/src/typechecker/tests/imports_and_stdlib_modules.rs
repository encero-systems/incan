//! Source-module and stdlib imports: module-style item imports (#1407), `check_with_imports` upcasts and cyclic
//! interfaces, reserved root namespaces, which `std.*` modules and members exist, annotation-only imports (#902),
//! prelude re-exports, `std.math` / `std.graph` / `std.regex` surfaces, and the unknown-module hint.

use super::*;

#[test]
fn stdlib_module_function_calls_accept_default_arguments() -> Result<(), String> {
    let source = r#"
from std.encoding import hex
from std.io import BytesIO

def main(payload: bytes) -> None:
  target = BytesIO()
  encoded = hex.encode(payload, target)
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn test_from_import_accepts_public_source_enum_variant_export() -> Result<(), Box<dyn std::error::Error>> {
    let library = parse_program(
        r#"
pub enum Status(str):
  Active = "active"
  Disabled = "disabled"
"#,
        "enum variant import library",
    );
    let consumer = parse_program(
        r#"
from statuses import Active, Status

def current() -> Status:
  return Active
"#,
        "enum variant import consumer",
    );

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("statuses", &library)])
        .map_err(|errs| format!("consumer should import public source enum variants by name: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_dependency_overload_cache_keeps_module_local_symbols_when_spans_collide()
-> Result<(), Box<dyn std::error::Error>> {
    let left = parse_program(
        r#"
pub model Alpha:
  pub value: str

pub model Bravo:
  pub value: str

pub def choose(value: Type[Alpha]) -> Alpha:
  return Alpha(value="a")

pub def choose(value: Type[Bravo]) -> Bravo:
  return Bravo(value="b")
"#,
        "left overload dependency",
    );
    let right = parse_program(
        r#"
pub model Gamma:
  pub value: str

pub model Delta:
  pub value: str

pub def choose(value: Type[Gamma]) -> Gamma:
  return Gamma(value="g")

pub def choose(value: Type[Delta]) -> Delta:
  return Delta(value="d")
"#,
        "right overload dependency",
    );
    let consumer = parse_program(
        r#"
from left import Alpha, choose as choose_left
from right import Gamma, choose as choose_right

def use() -> None:
  choose_left(Alpha)
  choose_right(Gamma)
"#,
        "overload span collision consumer",
    );

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("left", &left), ("right", &right)])
        .map_err(|errs| format!("consumer should import both overload groups independently: {errs:?}"))?;

    let left_path = ImportPath {
        is_absolute: false,
        parent_levels: 0,
        segments: vec!["left".to_string()],
    };
    let right_path = ImportPath {
        is_absolute: false,
        parent_levels: 0,
        segments: vec!["right".to_string()],
    };
    let left_symbol = checker
        .dependency_member_symbol_for_path(&left_path, "choose")
        .ok_or_else(|| "expected left.choose to be present in dependency member cache".to_string())?;
    let SymbolKind::FunctionOverloads(left_overloads) = left_symbol else {
        return Err(format!("expected left.choose to be cached as function overloads, got {left_symbol:?}").into());
    };
    let right_symbol = checker
        .dependency_member_symbol_for_path(&right_path, "choose")
        .ok_or_else(|| "expected right.choose to be present in dependency member cache".to_string())?;
    let SymbolKind::FunctionOverloads(right_overloads) = right_symbol else {
        return Err(format!("expected right.choose to be cached as function overloads, got {right_symbol:?}").into());
    };

    let left_return_types = left_overloads
        .iter()
        .map(|overload| overload.info.return_type.to_string())
        .collect::<Vec<_>>();
    let right_return_types = right_overloads
        .iter()
        .map(|overload| overload.info.return_type.to_string())
        .collect::<Vec<_>>();
    assert_eq!(left_return_types, vec!["Alpha", "Bravo"]);
    assert_eq!(right_return_types, vec!["Gamma", "Delta"]);

    let left_emitted_names = left_overloads
        .iter()
        .filter_map(|overload| overload.info.emitted_name.as_deref())
        .collect::<Vec<_>>();
    let right_emitted_names = right_overloads
        .iter()
        .filter_map(|overload| overload.info.emitted_name.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(left_emitted_names.len(), 2);
    assert_eq!(right_emitted_names.len(), 2);
    assert!(
        left_emitted_names
            .iter()
            .all(|name| name.starts_with("choose_overload_")),
        "left overloads should keep deterministic emitted names, got {left_emitted_names:?}"
    );
    assert!(
        right_emitted_names
            .iter()
            .all(|name| name.starts_with("choose_overload_")),
        "right overloads should keep deterministic emitted names, got {right_emitted_names:?}"
    );
    assert_ne!(
        left_emitted_names, right_emitted_names,
        "same-span overload declarations from different modules must not collapse to one emitted-name set"
    );
    Ok(())
}

#[test]
fn test_reserved_root_namespace_std() {
    // `std` is a reserved root namespace, so `def std() -> int: return 1` is rejected.
    let source = r#"
def std() -> int:
  return 1
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_reserved_root_namespace_rust_import_alias() {
    // Aliasing a std import to `rust` (a different reserved root) is rejected.
    let source = r#"
import std.web as rust
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn module_style_item_import_carries_the_declaration_signature_issue1407() -> Result<(), Box<dyn std::error::Error>> {
    // `import dep::give_float` bound its last segment as if it named a module: the name resolved, so nothing
    // reported an error, but the binding carried no signature and the call inferred `Unknown`. Arithmetic on the
    // result then failed with `expected 'numeric', found '? + ?'`, pointing at the arithmetic rather than at the
    // import — `examples/advanced/multifile` had not typechecked for exactly this reason.
    let dep_source = r#"
pub def give_float(n: int) -> float:
  return float(n)
"#;
    let module_style = r#"
import dep::give_float

def main() -> None:
  a = give_float(1)
  b = give_float(2)
  total = a + b
  println(f"{total}")
"#;
    // The same program in the other spelling, which already worked. Both are checked here so the two import
    // surfaces cannot drift apart again without this failing.
    let from_style = r#"
from dep import give_float

def main() -> None:
  a = give_float(1)
  b = give_float(2)
  total = a + b
  println(f"{total}")
"#;

    // The reference documents `import utils::format_currency as fmt`, so the aliased spelling has to carry the
    // signature too, under the alias.
    let aliased_module_style = r#"
import dep::give_float as give

def main() -> None:
  a = give(1)
  b = give(2)
  total = a + b
  println(f"{total}")
"#;

    for (label, source) in [
        ("module style", module_style),
        ("aliased module style", aliased_module_style),
        ("from style", from_style),
    ] {
        let dep_tokens =
            lexer::lex(dep_source).map_err(|errs| std::io::Error::other(format!("lex dep failed: {errs:?}")))?;
        let dep_ast =
            parser::parse(&dep_tokens).map_err(|errs| std::io::Error::other(format!("parse dep failed: {errs:?}")))?;
        let tokens =
            lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{label}: lex failed: {errs:?}")))?;
        let ast =
            parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{label}: parse failed: {errs:?}")))?;
        let mut checker = TypeChecker::new();
        checker
            .check_with_imports(&ast, &[("dep", &dep_ast)])
            .map_err(|errs| std::io::Error::other(format!("{label}: {errs:?}")))?;
    }
    Ok(())
}

#[test]
fn test_std_web_type_requires_import() {
    // async needs to be imported to use the Query type and asyc keyword.
    let source = r#"
async def search(params: Query[int]) -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_web_type_import_ok() {
    // async needs to be imported to use the Query type and asyc keyword.
    let source = r#"
from std.web import Query
import std.async

async def search(params: Query[int]) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_async_type_requires_import() {
    let source = r#"
def queue(handle: JoinHandle[int]) -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_async_type_import_ok() {
    let source = r#"
from std.async.task import JoinHandle

def queue(handle: JoinHandle[int]) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_async_function_requires_import() {
    let source = r#"
async def foo():
  await sleep(1.0)
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_async_function_import_ok() {
    let source = r#"
from std.async.time import sleep

async def foo() -> None:
  await sleep(1.0)
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_reflection_type_requires_import() {
    let source = r#"
def foo(fields: List[FieldInfo]) -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_reflection_type_import_ok() {
    let source = r#"
from std.reflection import FieldInfo

def foo(fields: List[FieldInfo]) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_reserved_root_namespace_std_import_alias_allowed() {
    // Import aliases may use reserved roots — only declarations are rejected.
    let source = r#"
import std.web as std
"#;
    assert_check_ok(source);
}

#[test]
fn test_check_with_imports_concrete_and_supertrait_upcasts() -> Result<(), Box<dyn std::error::Error>> {
    let dependency_source = r#"
pub trait DataSet[T]:
  def filter(self, _p: bool) -> Self: ...

pub trait BoundedDataSet[T] with DataSet[T]:
  def bounded_marker(self) -> T: ...

pub class DataFrame[T] with BoundedDataSet:
  _row_schema_marker: T

  def filter(self, _p: bool) -> Self:
    return self

  def bounded_marker(self) -> T:
    return self._row_schema_marker
"#;
    let consumer_source = r#"
from dataset import DataFrame, BoundedDataSet, DataSet

def upcast_data_frame[T](v: DataFrame[T]) -> BoundedDataSet[T]:
  return v

def upcast_bounded[T](v: BoundedDataSet[T]) -> DataSet[T]:
  return v
"#;

    let dep_tokens = lexer::lex(dependency_source).map_err(|errs| format!("dependency lex failed: {errs:?}"))?;
    let dep_ast = parser::parse(&dep_tokens).map_err(|errs| format!("dependency parse failed: {errs:?}"))?;
    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| format!("consumer lex failed: {errs:?}"))?;
    let consumer_ast = parser::parse(&consumer_tokens).map_err(|errs| format!("consumer parse failed: {errs:?}"))?;

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer_ast, &[("dataset", &dep_ast)])
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_check_with_imports_preserves_cyclic_dependency_interface_result_types() -> Result<(), Box<dyn std::error::Error>>
{
    let dataset_source = r#"
from session import SessionError

pub class DataFrame[T]:
  def clone(self) -> Self:
    return self

pub class LazyFrame[T]:
  def clone(self) -> Self:
    return self

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Err(str("not implemented"))
"#;
    let session_source = r#"
from dataset import LazyFrame

pub class Session:
  def read_csv[T](self) -> Result[LazyFrame[T], SessionError]:
    return Err(str("not implemented"))

pub model SessionError:
  pub message: str
"#;
    let consumer_source = r#"
from session import Session, SessionError

def main() -> Result[None, SessionError]:
  session = Session()
  lines = session.read_csv[int]()?
  df = lines.clone().collect()?
  df.clone()
  return Ok(None)
"#;

    let dataset_tokens = lexer::lex(dataset_source).map_err(|errs| format!("dataset lex failed: {errs:?}"))?;
    let dataset_ast = parser::parse(&dataset_tokens).map_err(|errs| format!("dataset parse failed: {errs:?}"))?;
    let session_tokens = lexer::lex(session_source).map_err(|errs| format!("session lex failed: {errs:?}"))?;
    let session_ast = parser::parse(&session_tokens).map_err(|errs| format!("session parse failed: {errs:?}"))?;
    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| format!("consumer lex failed: {errs:?}"))?;
    let consumer_ast = parser::parse(&consumer_tokens).map_err(|errs| format!("consumer parse failed: {errs:?}"))?;

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer_ast, &[("dataset", &dataset_ast), ("session", &session_ast)])
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    let mut reverse_checker = TypeChecker::new();
    reverse_checker
        .check_with_imports(&consumer_ast, &[("session", &session_ast), ("dataset", &dataset_ast)])
        .map_err(|errs| format!("reverse-order typecheck failed: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_check_with_imports_preserves_imported_generic_method_bounds_for_local_derived_types()
-> Result<(), Box<dyn std::error::Error>> {
    let dataset_source = r#"
from session import SessionError

pub class LazyFrame[T with Clone]:
  def clone(self) -> Self:
    return self

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))

pub class DataFrame[T with Clone]:
  def clone(self) -> Self:
    return self
"#;
    let session_source = r#"
from dataset import LazyFrame

pub model SessionError:
  pub message: str

pub class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def read_csv[T with Clone](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#;
    let consumer_source = r#"
from session import Session, SessionError

@derive(Clone)
pub model OrderLine:
  pub sku: str

def main() -> Result[None, SessionError]:
  session = Session.default()
  lines = session.read_csv[OrderLine](str("orders"), str("input.csv"))?
  df = lines.clone().collect()?
  df.clone()
  return Ok(None)
"#;

    let dataset_tokens = lexer::lex(dataset_source).map_err(|errs| format!("dataset lex failed: {errs:?}"))?;
    let dataset_ast = parser::parse(&dataset_tokens).map_err(|errs| format!("dataset parse failed: {errs:?}"))?;
    let session_tokens = lexer::lex(session_source).map_err(|errs| format!("session lex failed: {errs:?}"))?;
    let session_ast = parser::parse(&session_tokens).map_err(|errs| format!("session parse failed: {errs:?}"))?;
    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| format!("consumer lex failed: {errs:?}"))?;
    let consumer_ast = parser::parse(&consumer_tokens).map_err(|errs| format!("consumer parse failed: {errs:?}"))?;

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer_ast, &[("dataset", &dataset_ast), ("session", &session_ast)])
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_unknown_stdlib_module_from_import() {
    // `from std.f64.consts import PI` should be rejected — user meant `from rust::std::f64::consts import PI`.
    let source = "from std.f64.consts import PI\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: std.f64.consts is not a known Incan stdlib module");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
        "Expected unknown stdlib module error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_known_stdlib_module_rejects_unknown_annotation_only_import() {
    let source = r#"
from std.testing import NotExported

def accepts_marker(value: NotExported) -> None:
  pass
"#;
    let errs = check_str_err(source, "unknown stdlib import used only as an annotation should fail");
    assert!(
        errs.iter().any(|e| {
            e.message
                .contains("Cannot import `NotExported` from stdlib module `std.testing`")
                && e.message.contains("not exported")
        }),
        "Expected not-exported diagnostic for std.testing.NotExported; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_unbound_source_type_annotations_are_rejected_issue902() {
    let cases = [
        (
            "callable annotation",
            r#"
def accepts(value: Count) -> Count:
  return value

def main() -> None:
  print(accepts(7))
"#,
        ),
        (
            "nested annotation",
            r#"
def accepts(values: list[Count]) -> None:
  pass
"#,
        ),
        (
            "model field annotation",
            r#"
model Counter:
  value: Count
"#,
        ),
        (
            "local annotation",
            r#"
def main() -> None:
  value: Count = 7
  print(value)
"#,
        ),
        (
            "transparent alias target",
            r#"
type Counts = list[Count]

def main() -> None:
  pass
"#,
        ),
        (
            "abstract trait method annotation",
            r#"
trait Counter:
  def count(self, value: Count) -> Count: ...
"#,
        ),
        (
            "abstract trait property annotation",
            r#"
trait Counter:
  property count -> Count
"#,
        ),
        (
            "trait required-field annotation",
            r#"
@requires(count: Count)
trait Counter:
  def value(self) -> int: ...
"#,
        ),
        (
            "supertrait type argument",
            r#"
trait Parent[T]:
  def value(self) -> T: ...

trait Counter with Parent[Count]:
  def count(self) -> int: ...
"#,
        ),
        (
            "trait type-parameter bound argument",
            r#"
trait Parent[T]:
  def value(self) -> T: ...

trait Counter[T with Parent[Count]]:
  def count(self, value: T) -> T: ...
"#,
        ),
        (
            "abstract method type-parameter bound argument",
            r#"
trait Parent[T]:
  def value(self) -> T: ...

trait Counter:
  def count[T with Parent[Count]](self, value: T) -> T: ...
"#,
        ),
        (
            "transparent alias type-parameter bound argument",
            r#"
trait Parent[T]:
  def value(self) -> T: ...

type Counter[T with Parent[Count]] = T
"#,
        ),
        (
            "newtype type-parameter bound argument",
            r#"
trait Parent[T]:
  def value(self) -> T: ...

type Counter[T with Parent[Count]] = newtype T
"#,
        ),
    ];

    for (context, source) in cases {
        let errors = check_str_err(source, context);
        assert!(
            has_unknown_symbol_error(&errors, "Count"),
            "expected an Incan unknown-type diagnostic for {context}, got: {errors:?}"
        );
    }
}

#[test]
fn test_unbound_qualified_source_type_annotation_is_rejected_issue902() {
    let errors = check_str_err(
        r#"
def accepts(value: missing::Count) -> missing::Count:
  return value
"#,
        "unbound qualified annotation",
    );
    assert!(
        has_unknown_symbol_error(&errors, "missing"),
        "expected the unbound qualified root to receive an Incan diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_revisited_annotation_emits_one_unknown_symbol_diagnostic_issue902() {
    let errors = check_str_err(
        r#"
model Counter:
  value: Count
"#,
        "revisited model field annotation",
    );
    let count = errors
        .iter()
        .filter(|error| error.message.contains("Unknown symbol 'Count'"))
        .count();
    assert_eq!(count, 1, "one source annotation must emit one diagnostic: {errors:?}");
}

#[test]
fn test_declared_annotation_names_remain_valid_issue902() {
    let explicit_generic = r#"
def accepts[Count](value: Count) -> Count:
  return value

def main() -> None:
  print(accepts(7))
"#;
    assert!(
        check_str(explicit_generic).is_ok(),
        "explicit function type parameters must remain valid"
    );

    let forward_declared = r#"
def accepts(value: Count) -> Count:
  return value

model Count:
  value: int

def main() -> None:
  print(accepts(Count(value=7)).value)
"#;
    assert!(
        check_str(forward_declared).is_ok(),
        "two-pass collection must keep forward-declared nominal types valid"
    );

    let generic_alias = r#"
type Counts[T] = list[T]

def identity[T](values: Counts[T]) -> Counts[T]:
  return values
"#;
    assert!(
        check_str(generic_alias).is_ok(),
        "declared generic parameters in transparent aliases must remain valid"
    );

    let generic_class = r#"
class Box[T]:
  value: T
"#;
    assert!(
        check_str(generic_class).is_ok(),
        "declared generic class parameters must remain valid in field annotations"
    );

    let generic_trait = r#"
@requires(value: T)
trait Parent[T]:
  property current -> T
  def keep[U](self, value: U) -> T: ...

trait Child[T] with Parent[T]:
  def child(self, value: T) -> T: ...
"#;
    assert!(
        check_str(generic_trait).is_ok(),
        "trait and method generic parameters must remain valid across declaration-only trait surfaces"
    );

    let inline_test_forward_declared = r#"
module tests:
  def accepts(value: Count) -> Count:
    return value

  model Count:
    value: int
"#;
    assert!(
        check_str(inline_test_forward_declared).is_ok(),
        "inline test modules must preserve the same forward-declaration phase as the source module"
    );

    let qualified_rust_path = r#"
import rust::std::path as path

def accepts_path(value: path::PathBuf) -> path::PathBuf:
  return value
"#;
    assert!(
        check_str(qualified_rust_path).is_ok(),
        "qualified paths rooted in an imported Rust module must remain valid"
    );

    let dependency = parse_program(
        r#"
pub model Count:
  pub value: int
"#,
        "issue902 imported dependency",
    );
    let consumer = parse_program(
        r#"
from provider import Count

def accepts(value: Count) -> Count:
  return value
"#,
        "issue902 imported consumer",
    );
    let mut checker = TypeChecker::new();
    assert!(
        checker
            .check_with_imports(&consumer, &[("provider", &dependency)])
            .is_ok(),
        "imported source types must be established before annotation validation"
    );

    let runtime_error_token = r#"
from std.testing import assert_raises

def raises_value_error() -> None:
  pass

def main() -> None:
  assert_raises[ValueError](raises_value_error)
"#;
    assert!(
        check_str(runtime_error_token).is_ok(),
        "registered runtime error names must remain valid testing type tokens"
    );

    let inferred_closure_param = r#"
def keep_error(value: Result[int, str]) -> Result[int, str]:
  return value.map_err((error) => error)
"#;
    assert!(
        check_str(inferred_closure_param).is_ok(),
        "compiler-managed inferred closure parameter annotations must remain valid"
    );
}

#[test]
fn test_stdlib_prelude_reexports_stdlib_imported_types() {
    let source = r#"
from std.fs import IoError

def main() -> None:
  err = IoError(kind="invalid_input", detail="bad input")
  print(err.kind)
  print(err.detail)
"#;
    assert_check_ok(source);
}

#[test]
fn test_stdlib_internal_imports_do_not_become_public_reexports() {
    let source = r#"
from std.io import Error

def main() -> None:
  pass
"#;
    let errs = check_str_err(
        source,
        "private stdlib implementation imports should not be public reexports",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Cannot import `Error` from stdlib module `std.io`")),
        "Expected std.io.Error to stay private; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_stdlib_import_only_facades_reexport_imported_types() {
    let source = r#"
from std.datetime.civil import Date, TimeDelta
from std.datetime.error import DateTimeError

def main() -> Result[None, DateTimeError]:
  renewal = Date.fromisoformat("2026-04-14")? + TimeDelta.days(30)
  print(renewal.isoformat())
  return Ok(None)
"#;
    assert_check_ok(source);
}

#[test]
fn test_non_stdlib_annotation_only_import_keeps_placeholder_fallback() {
    let source = r#"
from app.types import ExternalOnly

def accepts_external(value: ExternalOnly) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_math_constant_import_ok() {
    let source = r#"
from std.math import PI

def circle_constant() -> float:
  return PI
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_math_module_constants_uppercase_ok() {
    let source = r#"
import std.math

def constants() -> float:
  return math.PI + math.E + math.TAU + math.INFINITY + math.NAN
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_math_lowercase_constant_aliases_rejected() {
    let source = r#"
import std.math

def constants() -> float:
  return math.pi
"#;
    let Err(errs) = check_str(source) else {
        panic!("legacy lowercase std.math aliases should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("missing field") || e.message.contains("has no field")),
        "Expected missing-field diagnostic for lowercase alias; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_math_module_extended_functions_ok() {
    let source = r#"
import std.math

def value(x: float, y: float, a: int, b: int) -> float:
  ints = math.gcd(a, b) + math.lcm(a, b)
  return math.round(x) + math.log2(x) + math.atan2(y, x) + math.hypot(x, y) + float(ints)
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_math_unknown_member_is_rejected() {
    let source = r#"
import std.math

def broken() -> float:
  return math.not_real
"#;
    let Err(errs) = check_str(source) else {
        panic!("unknown std.math member should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("missing field") || e.message.contains("has no field")),
        "Expected missing-field diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_math_unknown_function_is_rejected() {
    let source = r#"
import std.math

def broken() -> float:
  return math.not_real(1.0)
"#;
    let Err(errs) = check_str(source) else {
        panic!("unknown std.math function should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Type 'math' has no method 'not_real(...)'")),
        "Expected missing-method diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_std_math_arity_is_checked() {
    let source = r#"
import std.math

def broken() -> float:
  return math.round(1.0, 2.0)
"#;
    let Err(errs) = check_str(source) else {
        panic!("wrong std.math arity should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("math.round() expects 1 argument(s), got 2")),
        "Expected math arity diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_known_stdlib_module_is_accepted() {
    // `from std.testing import fail` should not trigger unknown-module diagnostic.
    let source = "from std.testing import fail\ndef main() -> None:\n    fail(\"oops\")\n";
    // This may error for other reasons (e.g. fail not found if stdlib stubs aren't available),
    // but it must NOT error with "Unknown stdlib module".
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.testing should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_std_graph_imports_and_direct_constructors_typecheck() {
    let source = r#"
from std.graph import DiGraph, Dag, MultiDiGraph, NodeId, EdgeId, GraphError

def exercise() -> None:
    mut graph = DiGraph[str]()
    a: NodeId = graph.add_node("a")
    b: NodeId = graph.add_node("b")
    edge_result: Result[None, GraphError] = graph.add_edge(a, b)
    removed: Result[None, GraphError] = graph.remove_edge(a, b)
    successors: Result[list[NodeId], GraphError] = graph.successors(a)
    topo: Result[list[NodeId], GraphError] = graph.topological_order()

    mut dag = Dag[str]()
    root: NodeId = dag.add_node("root")
    leaf: NodeId = dag.add_node("leaf")
    dag_edge: Result[None, GraphError] = dag.add_edge(root, leaf)
    dag_order: list[NodeId] = dag.topological_order()

    mut multi = MultiDiGraph[str]()
    left: NodeId = multi.add_node("left")
    right: NodeId = multi.add_node("right")
    multi_edge: Result[EdgeId, GraphError] = multi.add_edge(left, right)
    between: Result[list[EdgeId], GraphError] = multi.edges_between(left, right)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_std_regex_rfc059_surface_typechecks() {
    let source = r#"
from std.regex import Captures, Match, Regex, RegexError

def replacement(caps: Captures) -> str:
    match caps.group("word"):
        Some(word) => return f"[{word}]"
        None => return "[]"

def exercise(text: str) -> Result[None, RegexError]:
    word_re: Regex = Regex("^(?P<word>\\w+)(?:-(\\d+))?$", ignore_case=true, multiline=true, dotall=false, verbose=false)?
    matched: bool = word_re.is_match(text)
    maybe_match: Option[Match] = word_re.find(text)
    maybe_captures: Option[Captures] = word_re.captures(text)
    maybe_full: Option[Captures] = word_re.full_match(text)

    for found in word_re.find_iter(text):
        found_text: str = found.as_str()
        found_start: int = found.start()
        found_end: int = found.end()
        found_span: Tuple[int, int] = found.span()

    for captures in word_re.captures_iter(text):
        indexed_zero: Option[str] = captures.group(0)
        named_word: Option[str] = captures.group("word")
        word_span: Option[Tuple[int, int]] = captures.span("word")
        indexed_groups: list[Option[str]] = captures.groups()
        named_groups: Dict[str, Option[str]] = captures.groupdict()

    for part in word_re.split(text):
        split_part: str = part

    for part in word_re.splitn(text, 2):
        splitn_part: str = part

    literal_once: str = word_re.replace(text, "literal")
    literal_all: str = word_re.replace_all(text, "literal")
    indexed_replacement: str = word_re.replace_all(text, "$1")
    named_replacement: str = word_re.replace_all(text, "${word}")
    callable_replacement: str = word_re.replacen(text, 1, replacement)
    return Ok(None)
"#;
    check_str(source).unwrap_or_else(|errs| {
        panic!(
            "std.regex RFC 059 surface should typecheck; got: {:?}",
            errs.iter().map(|err| &err.message).collect::<Vec<_>>()
        )
    });
}

#[test]
fn test_known_stdlib_web_submodule_is_accepted() {
    let source = "from std.web.app import App\n";
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.web.app should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_known_stdlib_async_prelude_is_accepted() {
    let source = "from std.async.prelude import sleep\n";
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.async.prelude should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_known_stdlib_fs_module_is_accepted() {
    let source = "from std.fs import Path, File\n";
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.fs should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_unknown_stdlib_module_hint_includes_registry_entries() {
    let source = "from std.f64.consts import PI\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: std.f64.consts is not a known Incan stdlib module");
    };
    let Some(err) = errs.iter().find(|e| e.message.contains("Unknown stdlib module")) else {
        panic!(
            "Expected unknown stdlib module error; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    };
    assert!(
        err.hints.iter().any(|h| h.contains("std.derives")),
        "Expected hint to include std.derives; hints: {:?}",
        err.hints
    );
    assert!(
        err.hints.iter().any(|h| h.contains("std.fs")),
        "Expected hint to include std.fs; hints: {:?}",
        err.hints
    );
    assert!(
        err.hints.iter().any(|h| h.contains("std.web.app")),
        "Expected hint to include std.web.app; hints: {:?}",
        err.hints
    );
}

/// #1767: the std root exports its submodules only. A prelude trait imported from it is refused, and the hint names
/// the module that declares it, found through the builtin trait registry (`Debug`), the callable registry
/// (`Callable1`) or the trait family's own modules (`Add`, `Index`).
#[test]
fn std_root_import_of_a_prelude_trait_is_refused_naming_its_module_issue1767() -> Result<(), String> {
    let source = "from std import Debug, Add, Index, Callable1\n\ndef main() -> None:\n    println(\"ok\")\n";
    let Err(errors) = check_str(source) else {
        return Err("a prelude trait imported from the std root must be refused".to_string());
    };
    for (name, module) in [
        ("Debug", "std.derives.string"),
        ("Add", "std.traits.ops"),
        ("Index", "std.traits.indexing"),
        ("Callable1", "std.traits.callable"),
    ] {
        let refusal = errors
            .iter()
            .find(|error| error.message.contains(&format!("Cannot import `{name}` from `std`")))
            .ok_or_else(|| format!("no refusal names `{name}`: {errors:?}"))?;
        assert!(
            refusal
                .hints
                .iter()
                .any(|hint| hint.contains(&format!("from {module} import {name}"))),
            "the refusal of `{name}` must name `{module}`: {refusal:?}"
        );
    }
    Ok(())
}

/// #1767: a name the std root does not bind at all is refused the same way, with the root's modules as the hint.
#[test]
fn std_root_import_of_an_unknown_name_is_refused_issue1767() -> Result<(), String> {
    let Err(errors) = check_str("from std import mathh\n\ndef main() -> None:\n    println(\"ok\")\n") else {
        return Err("an unknown name imported from the std root must be refused".to_string());
    };
    let refusal = errors
        .iter()
        .find(|error| error.message.contains("Cannot import `mathh` from `std`"))
        .ok_or_else(|| format!("no refusal names `mathh`: {errors:?}"))?;
    assert!(
        refusal.hints.iter().any(|hint| hint.contains("std.math")),
        "{refusal:?}"
    );
    Ok(())
}

/// #1767: the std root still binds its submodules.
#[test]
fn std_root_import_of_a_submodule_binds_the_module_issue1767() -> Result<(), String> {
    check_str("from std import math, toml\n\ndef main() -> None:\n    println(\"ok\")\n")
        .map_err(|errors| format!("a std submodule imported from the root must bind: {errors:?}"))
}
