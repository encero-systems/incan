//! Callables at the call site: parameter lists and callable defaults, variadic rest parameters, fixed and variadic call
//! unpacking, borrowed (`&`) parameter shapes, RFC 022 decorator resolution and user-defined decorators, builtin
//! functions, and module or import shadowing of builtins (#1116).

use super::*;

fn callable_default_errors(source: &str) -> Result<Vec<CompileError>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .err()
        .ok_or_else(|| std::io::Error::other("expected callable default typechecking to fail"))
        .map_err(Into::into)
}

#[test]
fn owned_value_does_not_satisfy_incan_shared_ref_parameter() {
    let source = r#"
def borrowed(data: &bytes) -> None:
  return

def main(data: bytes) -> None:
  borrowed(data)
"#;
    let errs = check_str_err(source, "owned bytes should not satisfy an Incan &bytes parameter");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Argument 'data' of 'borrowed' has type mismatch: expected '&bytes', found 'bytes'")),
        "expected call argument type mismatch, got {errs:?}"
    );
    assert!(
        errs.iter().any(|err| err
            .notes
            .iter()
            .any(|note| note.contains("Parameter 'data' of 'borrowed' is declared as '&bytes'"))),
        "expected call argument context note, got {errs:?}"
    );
}

#[test]
fn callable_default_function_records_contextual_literal_and_call_facts() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def fallback() -> int:
  return 2

def choose(limit: u8 = 7, value: int = fallback()) -> int:
  return value
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

    let literal_start = source.find("7,").ok_or("missing contextual literal default")?;
    assert_eq!(
        checker
            .type_info()
            .expr_type(Span::new(literal_start, literal_start + 1)),
        Some(&ResolvedType::Numeric(NumericTypeId::U8)),
        "a numeric default must retain the declared parameter type as its canonical expression fact"
    );
    let call_start = source.rfind("fallback()").ok_or("missing callable default")?;
    assert_eq!(
        checker
            .type_info()
            .expr_type(Span::new(call_start, call_start + "fallback()".len())),
        Some(&ResolvedType::Int),
        "a declaration-visible default call must retain its exact root expression fact"
    );

    Ok(())
}

#[test]
fn callable_default_generic_method_records_literal_and_call_facts() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def fallback() -> str:
  return "label"

model Shelf[T]:
  def label[U](self, owner_items: list[T] = [], method_items: list[U] = [], suffix: str = "", fallback_label: str = fallback()) -> str:
    return suffix
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

    let literal_start = source.find("\"\"").ok_or("missing method literal default")?;
    assert_eq!(
        checker
            .type_info()
            .expr_type(Span::new(literal_start, literal_start + "\"\"".len())),
        Some(&ResolvedType::Str),
        "method literal defaults must retain type facts after generic owner and method contexts are installed"
    );
    let owner_list_start = source.find("[]").ok_or("missing generic owner list default")?;
    assert_eq!(
        checker
            .type_info()
            .expr_type(Span::new(owner_list_start, owner_list_start + "[]".len())),
        Some(&ResolvedType::Generic(
            "List".to_string(),
            vec![ResolvedType::Named("T".to_string())],
        )),
        "a generic owner default must retain the owner type parameter in its root fact"
    );
    let method_list_start = source.rfind("[]").ok_or("missing generic method list default")?;
    assert_eq!(
        checker
            .type_info()
            .expr_type(Span::new(method_list_start, method_list_start + "[]".len())),
        Some(&ResolvedType::Generic(
            "List".to_string(),
            vec![ResolvedType::Named("U".to_string())],
        )),
        "a generic method default must retain the method type parameter in its root fact"
    );
    let call_start = source.rfind("fallback()").ok_or("missing method call default")?;
    assert_eq!(
        checker
            .type_info()
            .expr_type(Span::new(call_start, call_start + "fallback()".len())),
        Some(&ResolvedType::Str),
        "method call defaults must retain exact root type facts after generic owner and method contexts are installed"
    );

    Ok(())
}

#[test]
fn callable_default_function_type_mismatch_uses_the_default_expression_span() -> Result<(), Box<dyn std::error::Error>>
{
    let source = "def choose(value: int = \"wrong\") -> int:\n  return value\n";
    let errors = callable_default_errors(source)?;
    let default_start = source.find("\"wrong\"").ok_or("missing function mismatch default")?;
    let default_span = Span::new(default_start, default_start + "\"wrong\"".len());
    assert!(
        errors.iter().any(|error| {
            error.span == default_span && error.message.contains("Type mismatch: expected 'int', found 'str'")
        }),
        "expected an exact default-expression mismatch diagnostic, got: {errors:?}"
    );

    Ok(())
}

#[test]
fn callable_default_method_type_mismatch_uses_the_default_expression_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Label:\n  def choose(self, value: str = 1) -> str:\n    return value\n";
    let errors = callable_default_errors(source)?;
    let default_start = source.find("1)").ok_or("missing method mismatch default")?;
    let default_span = Span::new(default_start, default_start + 1);
    assert!(
        errors.iter().any(|error| {
            error.span == default_span && error.message.contains("Type mismatch: expected 'str', found 'int'")
        }),
        "expected an exact method-default mismatch diagnostic, got: {errors:?}"
    );

    Ok(())
}

#[test]
fn callable_defaults_cannot_read_callable_frame_bindings() -> Result<(), Box<dyn std::error::Error>> {
    let function_source = "def choose(first: str, value: str = first) -> str:\n  return value\n";
    let function_errors = callable_default_errors(function_source)?;
    let first_start = function_source.rfind("first").ok_or("missing parameter default read")?;
    let first_span = Span::new(first_start, first_start + "first".len());
    assert!(
        function_errors
            .iter()
            .any(|error| error.span == first_span && error.message.contains("Unknown symbol 'first'")),
        "the function default must be checked before callable parameters are bound: {function_errors:?}"
    );

    let method_source =
        "model Label:\n  text: str\n\n  def choose(self, value: str = self.text) -> str:\n    return value\n";
    let method_errors = callable_default_errors(method_source)?;
    let self_start = method_source.find("self.text").ok_or("missing receiver default read")?;
    let self_span = Span::new(self_start, self_start + "self".len());
    assert!(
        method_errors
            .iter()
            .any(|error| error.span == self_span && error.message.contains("Unknown symbol 'self'")),
        "the method default must be checked before self is bound: {method_errors:?}"
    );

    let bare_field_source =
        "model Label:\n  text: str\n\n  def choose(self, value: str = text) -> str:\n    return value\n";
    let bare_field_errors = callable_default_errors(bare_field_source)?;
    let text_start = bare_field_source
        .rfind("text")
        .ok_or("missing bare field default read")?;
    let text_span = Span::new(text_start, text_start + "text".len());
    assert!(
        bare_field_errors
            .iter()
            .any(|error| error.span == text_span && error.message.contains("Unknown symbol 'text'")),
        "the method default must not resolve an enclosing instance field: {bare_field_errors:?}"
    );

    let bare_property_source = "model Label:\n  text: str\n\n  property display -> str:\n    return self.text\n\n  def choose(self, value: str = display) -> str:\n    return value\n";
    let bare_property_errors = callable_default_errors(bare_property_source)?;
    let display_start = bare_property_source
        .rfind("display")
        .ok_or("missing bare property default read")?;
    let display_span = Span::new(display_start, display_start + "display".len());
    assert!(
        bare_property_errors
            .iter()
            .any(|error| error.span == display_span && error.message.contains("Unknown symbol 'display'")),
        "the method default must not resolve an enclosing computed property: {bare_property_errors:?}"
    );

    Ok(())
}

#[test]
fn callable_default_newtype_records_root_type_and_checked_coercion() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Attempts, ValidationError]:
    return Ok(Attempts(n))

def choose(value: Attempts = 3) -> Attempts:
  return value
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("typecheck failed: {errs:?}")))?;

    let default_start = source.rfind("3)").ok_or("missing newtype default")?;
    let default_span = Span::new(default_start, default_start + 1);
    assert_eq!(
        checker.type_info().expr_type(default_span),
        Some(&ResolvedType::Int),
        "the default retains its source expression type rather than pretending to be the newtype"
    );
    let coercion = checker
        .type_info()
        .validated_newtype_coercion(default_span)
        .ok_or("missing callable-default validated-newtype coercion")?;
    assert_eq!(coercion.target_type, ResolvedType::Named("Attempts".to_string()));
    assert_eq!(coercion.steps.len(), 1);
    assert_eq!(coercion.steps[0].newtype_name, "Attempts");
    assert_eq!(coercion.steps[0].ctor.as_deref(), Some("from_underlying"));
    let ctor_identity = coercion.steps[0]
        .ctor_identity
        .as_ref()
        .ok_or("checked coercion must retain the selected hook identity")?;
    assert_eq!(
        ctor_identity.kind,
        incan_semantics_core::SemanticSourceTargetKind::Method
    );
    assert_eq!(ctor_identity.declaration_name, "from_underlying");

    Ok(())
}

#[test]
fn test_generic_function_reference_rejected_in_value_position() {
    let source = r#"
def id[T](x: T) -> T:
  return x

def accept(f: (int) -> int) -> int:
  return f(42)

def main() -> None:
  _ = accept(id)
"#;
    let err = check_str_err(source, "generic function name in value position should fail");
    assert!(
        err.iter()
            .any(|e| e.message.contains("generic function") && e.message.contains("'id'")),
        "unexpected errors: {:?}",
        err.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_decorator_resolution_canonical_path() {
    // Canonical @std.web.routing.route with fully qualified path
    let source = r#"
from std.web.routing import GET
import std.async

@std.web.routing.route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorator_resolution_module_alias() {
    // Aliased @web.route after `import std.web.routing as web`
    let source = r#"
import std.web.routing as web
from std.web.routing import GET
import std.async

@web.route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorator_resolution_from_import() {
    // Bare @route after `from std.web import route` (prelude re-export)
    let source = r#"
from std.web import route, GET
import std.async

@route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorator_resolution_colcolon_path() {
    // `::` separator variant: @std::web::routing::route
    let source = r#"
from std.web.routing import GET
import std.async

@std::web::routing::route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_unknown_decorator_path() {
    let source = r#"
@std.web.missing
def foo() -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_user_defined_plain_decorator_updates_function_binding_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def parse(value: int) -> int:
  return value

def as_int(func: (int) -> str) -> (int) -> int:
  return parse

@as_int
def label(value: int) -> str:
  return "value"

def main() -> int:
  return label(1)
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    let symbol = checker
        .lookup_symbol("label")
        .ok_or_else(|| "expected decorated label binding".to_string())?;
    let SymbolKind::Variable(info) = &symbol.kind else {
        return Err(format!("expected decorated binding to be a value, got {:?}", symbol.kind).into());
    };
    let ResolvedType::Function(_, ret) = &info.ty else {
        return Err(format!("expected decorated binding to stay callable, got {:?}", info.ty).into());
    };
    assert_eq!(**ret, ResolvedType::Int);
    Ok(())
}

#[test]
fn test_function_callable_name_metadata_typechecks_issue694() {
    let source = r#"
def capture(func: (int) -> int) -> ((int) -> int):
  name: str = func.__name__
  return func

def registered() -> (((int) -> int) -> ((int) -> int)):
  return capture

@registered()
pub def sample(value: int) -> int:
  return value + 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_defined_decorator_factory_and_stacking_apply_bottom_up() {
    let source = r#"
def keep(func: (int) -> str) -> (int) -> str:
  return func

def parse(value: int) -> int:
  return value

def as_int(func: (int) -> str) -> (int) -> int:
  return parse

def named(label: str) -> Callable[(int) -> str, (int) -> str]:
  return keep

@as_int
@named(label="inner")
def label(value: int) -> str:
  return "value"

def main() -> int:
  return label(1)
"#;
    assert_check_ok(source);
}

#[test]
fn test_generic_decorator_factory_with_explicit_function_type_arg_preserves_binding_type() {
    let source = r#"
model ColumnExpr:
  name: str

def registered[F](name: str) -> ((F) -> F):
  return (func) => func

@registered[(str) -> ColumnExpr]("incql.functions.col")
def col(name: str) -> ColumnExpr:
  return ColumnExpr(name=name)

def main() -> ColumnExpr:
  return col("id")
"#;
    assert_check_ok(source);
}

#[test]
fn test_generic_decorator_factory_infers_decorated_function_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model ColumnExpr:
  name: str

def registered[F](name: str) -> ((F) -> F):
  return (func) => func

@registered("incql.functions.col")
def col(name: str) -> ColumnExpr:
  return ColumnExpr(name=name)

def main() -> ColumnExpr:
  return col("id")
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    let symbol = checker
        .lookup_symbol("col")
        .ok_or_else(|| "expected decorated col binding".to_string())?;
    let SymbolKind::Variable(info) = &symbol.kind else {
        return Err(format!("expected decorated binding to be a value, got {:?}", symbol.kind).into());
    };
    let ResolvedType::Function(params, ret) = &info.ty else {
        return Err(format!("expected decorated binding to stay callable, got {:?}", info.ty).into());
    };
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].ty, ResolvedType::Str);
    assert_eq!(**ret, ResolvedType::Named("ColumnExpr".to_string()));
    Ok(())
}

#[test]
fn test_user_defined_decorator_on_async_def_is_kept_as_candidate() {
    let source = r#"
import std.async

def keep(func: () -> int) -> () -> int:
  return func

@keep
async def fetch() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_defined_method_decorator_updates_method_binding_type() {
    let source = r#"
class Box:
  value: int

  @as_int
  def label(self, value: int) -> str:
    return "value"

def parse(box: &Box, value: int) -> int:
  return value

def as_int(func: (&Box, int) -> str) -> (&Box, int) -> int:
  return parse

def main(box: Box) -> int:
  return box.label(1)
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_defined_trait_method_decorator_is_checked() {
    let source = r#"
trait Service:
  @keep
  def read(self) -> int

def keep(func: (&Service) -> int) -> (&Service) -> int:
  return func
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_defined_decorator_on_unsupported_target_is_rejected() {
    let errors = check_str_err(
        r#"
def keep(func: () -> int) -> () -> int:
  return func

@keep
model Bad:
  value: int
"#,
        "user-defined model decorator should be rejected",
    );
    assert!(
        errors.iter().any(|err| err
            .message
            .contains("User-defined decorator '@keep' cannot be used on model declarations")),
        "expected unsupported target diagnostic, got {errors:?}"
    );
}

#[test]
fn test_user_defined_decorator_on_mutable_method_is_checked() {
    let source = r#"
class Counter:
  value: int

  @keep
  def bump(mut self) -> int:
    self.value = self.value + 1
    return self.value

def keep(func: (&mut Counter) -> int) -> (&mut Counter) -> int:
  return func
"#;
    assert_check_ok(source);
}

#[test]
fn test_user_defined_decorator_rejects_non_callable_and_factory_result() {
    let non_callable = check_str_err(
        r#"
const count: int = 1

@count
def label() -> int:
  return 1
"#,
        "non-callable decorator should be rejected",
    );
    assert!(
        non_callable
            .iter()
            .any(|err| err.message.contains("decorator 'count' is not callable")),
        "expected non-callable decorator diagnostic, got {non_callable:?}"
    );

    let bad_factory = check_str_err(
        r#"
def count_factory() -> int:
  return 1

@count_factory()
def label() -> int:
  return 1
"#,
        "factory returning non-callable should be rejected",
    );
    assert!(
        bad_factory
            .iter()
            .any(|err| err.message.contains("'count_factory(...)' does not return a callable")),
        "expected non-callable factory diagnostic, got {bad_factory:?}"
    );

    let bad_result = check_str_err(
        r#"
def count(func: () -> int) -> int:
  return 1

@count
def label() -> int:
  return 1
"#,
        "decorator returning non-callable should be rejected",
    );
    assert!(
        bad_result
            .iter()
            .any(|err| err.message.contains("decorator 'count' must return a callable")),
        "expected non-callable decorator result diagnostic, got {bad_result:?}"
    );
}

#[test]
fn test_function_call() {
    let source = r#"
def add(a: int, b: int) -> int:
  return a + b

def foo() -> int:
  return add(1, 2)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_variadic_rest_params_typecheck_and_bind_local_container_types() {
    let source = r#"
def collect(prefix: str, *items: int, **labels: str) -> int:
  first: int = items[0]
  label: str = labels["name"]
  return first

def main(xs: list[int], kw: dict[str, str]) -> int:
  return collect("x", 1, *xs, name="demo", **kw)
"#;
    assert_check_ok(source);
}

#[test]
fn test_fixed_call_unpack_accepts_shaped_positional_sources() {
    let source = r#"
def pair(a: int, b: str) -> str:
  return b

def collect(a: int, b: str, *rest: int) -> int:
  return a + rest[0]

def main() -> int:
  xy: tuple[int, str] = (1, "v")
  left = pair(*(1, "x"))
  right = pair(*[2, "y"])
  named = pair(*xy)
  return collect(*[3, "z", 4])
"#;
    assert_check_ok(source);
}

#[test]
fn test_fixed_call_unpack_accepts_shaped_keyword_sources() {
    let source = r#"
def user(name: str, age: int) -> str:
  return name

def collect(name: str, **labels: str) -> str:
  return labels["city"]

def main() -> str:
  left = user(**{"name": "Ada", "age": 36})
  return collect(**{"name": "Ada", "city": "London"})
"#;
    assert_check_ok(source);
}

#[test]
fn test_fixed_call_unpack_reports_invalid_positional_cases() {
    let source = r#"
def pair(a: int, b: str) -> str:
  return b

def main(xs: list[int]) -> str:
  missing = pair(*(1,))
  wrong_type = pair(*(2, 3))
  unshaped = pair(*xs)
  return wrong_type
"#;
    let errs = check_str_err(source, "expected invalid fixed positional unpack cases");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Missing required argument 'b' when calling 'pair'")),
        "expected missing fixed parameter diagnostic, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|msg| msg.contains("expected 'str', found 'int'")),
        "expected shaped positional item type mismatch, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|msg| msg.contains("Cannot use `*` unpacking")),
        "expected unshaped fixed positional unpack rejection, got: {messages:?}"
    );
}

#[test]
fn test_fixed_call_unpack_reports_invalid_keyword_cases() {
    let source = r#"
def user(name: str, age: int) -> str:
  return name

def main(kw: dict[str, int]) -> str:
  duplicate = user(name="Ada", **{"name": "Grace", "age": 37})
  missing = user(**{"name": "Ada"})
  unknown = user(**{"name": "Ada", "age": 36, "city": "London"})
  wrong_type = user(**{"name": "Ada", "age": "old"})
  unshaped = user(**kw)
  return duplicate
"#;
    let errs = check_str_err(source, "expected invalid fixed keyword unpack cases");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Duplicate argument 'name' when calling 'user'")),
        "expected duplicate fixed keyword diagnostic, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Missing required argument 'age' when calling 'user'")),
        "expected missing fixed keyword diagnostic, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Unexpected keyword argument 'city' when calling 'user'")),
        "expected unknown fixed keyword diagnostic, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|msg| msg.contains("expected 'int', found 'str'")),
        "expected shaped keyword value type mismatch, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|msg| msg.contains("Cannot use `**` unpacking")),
        "expected unshaped fixed keyword unpack rejection, got: {messages:?}"
    );
}

#[test]
fn test_variadic_unpack_requires_matching_rest_param() {
    let source = r#"
def fixed(value: int) -> int:
  return value

def main(xs: list[int], kw: dict[str, str]) -> int:
  return fixed(*xs, **kw)
"#;
    let errs = check_str_err(source, "expected unpacking into fixed function to fail");
    assert!(
        errs.iter().any(|err| err.message.contains("Cannot use `*` unpacking")),
        "expected positional unpack diagnostic, got: {errs:?}"
    );
    assert!(
        errs.iter().any(|err| err.message.contains("Cannot use `**` unpacking")),
        "expected keyword unpack diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_variadic_rest_type_mismatch_reports_element_and_container_shapes() {
    let source = r#"
def collect(*items: int, **labels: str) -> int:
  return 0

def main(xs: list[str], kw: dict[str, int]) -> int:
  return collect(1.0, *xs, name=2, **kw)
"#;
    let errs = check_str_err(source, "expected rest argument type mismatches");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        messages.iter().any(|msg| msg.contains("expected 'int', found 'float'")),
        "expected direct rest positional mismatch, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("expected 'List[int]', found 'List[str]'")),
        "expected positional unpack container mismatch, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|msg| msg.contains("expected 'str', found 'int'")),
        "expected direct keyword rest mismatch, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("expected 'Dict[str, str]', found 'Dict[str, int]'")),
        "expected keyword unpack container mismatch, got: {messages:?}"
    );
}

#[test]
fn test_variadic_rest_params_preserved_through_function_values() {
    let source = r#"
def collect(*items: int, **labels: str) -> int:
  return 0

def main(xs: list[int], kw: dict[str, str]) -> int:
  f = collect
  return f(1, *xs, name="demo", **kw)
"#;
    assert_check_ok(source);
}

#[test]
fn test_invalid_rest_parameter_declarations_report_targeted_errors() {
    let source = r#"
def normal_after_args(*items: int, value: int) -> int:
  return value

def duplicate_args(*left: int, *right: int) -> int:
  return 0

def args_after_kwargs(**labels: str, *items: int) -> int:
  return 0

def duplicate_kwargs(**left: str, **right: str) -> int:
  return 0

def rest_with_default(*items: int = []) -> int:
  return 0
"#;
    let errs = check_str_err(source, "expected invalid rest parameter declarations");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Normal parameters cannot appear after a rest parameter")),
        "expected normal-after-rest diagnostic, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Only one `*args` rest parameter")),
        "expected duplicate *args diagnostic, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("`*args` must appear before `**kwargs`")),
        "expected *args-after-**kwargs diagnostic, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Only one `**kwargs` rest parameter")),
        "expected duplicate **kwargs diagnostic, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("Rest parameter 'items' cannot declare a default value")),
        "expected rest-default diagnostic, got: {messages:?}"
    );
}

#[test]
fn test_normal_after_kwargs_reports_single_specific_rest_order_error() {
    let source = r#"
def invalid(**labels: str, value: int) -> int:
  return value
"#;
    let errs = check_str_err(source, "expected normal parameter after **kwargs to fail");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    let specific_count = messages
        .iter()
        .filter(|msg| msg.contains("Normal parameters cannot appear after a `**kwargs` rest parameter"))
        .count();
    let generic_count = messages
        .iter()
        .filter(|msg| **msg == "Normal parameters cannot appear after a rest parameter")
        .count();
    assert_eq!(
        specific_count, 1,
        "expected one **kwargs-specific diagnostic, got: {messages:?}"
    );
    assert_eq!(
        generic_count, 0,
        "expected no duplicate generic diagnostic, got: {messages:?}"
    );
}

#[test]
fn test_builtin_len() {
    let source = r#"
def foo() -> int:
  x = [1, 2, 3]
  return len(x)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_builtin_sum() {
    let source = r#"
def foo() -> int:
  x = [True, False, True]
  return sum(x)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn builtin_json_stringify_requires_exactly_one_operand_at_the_call_span() -> Result<(), String> {
    for (source, call, expected) in [
        (
            "def main() -> str:\n  return json_stringify()\n",
            "json_stringify()",
            "json_stringify() expects 1 argument(s), got 0",
        ),
        (
            "def main() -> str:\n  return std.builtins.json_stringify(1, 2)\n",
            "std.builtins.json_stringify(1, 2)",
            "json_stringify() expects 1 argument(s), got 2",
        ),
    ] {
        let errors = check_str(source)
            .err()
            .ok_or_else(|| "json_stringify arity mismatch should fail".to_string())?;
        let error = errors
            .iter()
            .find(|error| error.message == expected)
            .ok_or_else(|| format!("expected {expected:?}, got {errors:?}"))?;
        let actual_span = source
            .get(error.span.start..error.span.end)
            .ok_or_else(|| format!("invalid json_stringify diagnostic span {:?}", error.span))?;
        assert_eq!(
            actual_span, call,
            "the arity diagnostic must own the complete source call"
        );
    }
    Ok(())
}

#[test]
fn local_function_named_json_stringify_remains_an_ordinary_lexical_binding() -> Result<(), String> {
    let source = r#"
def json_stringify(value: int) -> int:
  return value + 1

def main() -> int:
  return json_stringify(41)
"#;
    check_str(source).map_err(|errors| format!("local json_stringify binding should typecheck: {errors:?}"))?;
    Ok(())
}

#[test]
fn test_local_function_named_sum_shadows_builtin_sum() {
    let source = r#"
def sum(value: str) -> str:
  return value

def foo() -> str:
  return sum("ok")
"#;
    assert!(check_str(source).is_ok());
}

/// Issue #1116: a module binding takes precedence over an ambient core builtin, while `std.builtins` remains an
/// explicit route to the builtin.
#[test]
fn module_len_shadowing_and_explicit_builtin_selection_are_supported_issue1116() {
    let source = r#"
def len(value: int) -> int:
  return value + 1

def shadowed_len_call() -> int:
  return len(4)

def explicit_builtin_len_call() -> int:
  return std.builtins.len([10, 20, 30])
"#;
    assert_check_ok(source);
}

/// Issue #1116: an explicitly imported source function is a normal lexical binding, not a request for ambient
/// builtin dispatch with the same spelling.
#[test]
fn imported_sum_shadowing_is_supported_issue1116() -> Result<(), String> {
    let provider = parse_program(
        r#"
pub def sum(value: int) -> int:
  return value + 1
"#,
        "issue1116 sum provider",
    );
    let consumer = parse_program(
        r#"
from aggregates import sum

def imported_sum_call() -> int:
  return sum(4)
"#,
        "issue1116 sum consumer",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer, &[("aggregates", &provider)])
        .map_err(|errors| format!("imported `sum` should shadow the builtin: {errors:?}"))?;
    Ok(())
}

#[test]
fn test_local_function_named_sleep_ms_shadows_surface_helper() {
    let source = r#"
def sleep_ms(value: str) -> str:
  return value

def foo() -> str:
  return sleep_ms("ok")
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_function_named_some_shadows_option_constructor() {
    let source = r#"
def Some(value: str) -> str:
  return value

def foo() -> str:
  return Some("ok")
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_function_named_list_shadows_collection_helper() {
    let source = r#"
def list(value: str) -> str:
  return value

def foo() -> str:
  return list("ok")
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorated_function_named_sum_shadows_builtin_sum_in_inline_module_tests() {
    let source = r#"
model IntExpr:
  value: int

model Measure:
  kind: str

def registered[F](function_ref: str) -> ((F) -> F):
  return (func) => func

def expr(value: int) -> IntExpr:
  return IntExpr(value=value)

@registered("demo.sum")
def sum(value: IntExpr) -> Measure:
  return Measure(kind="local")

module tests:
  def test_inline_sum() -> None:
    measure = sum(expr(1))
    assert measure.kind == "local"
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_std_builtins_sum_call() {
    let source = r#"
def foo() -> int:
  x = [1, 2, 3]
  return std.builtins.sum(x)
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_std_builtins_len_call() {
    let source = r#"
def foo() -> int:
  names = ["a", "b"]
  return std.builtins.len(names)
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_std_builtins_unknown_member_is_rejected() {
    let source = r#"
def foo() -> int:
  return std.builtins.not_real([1, 2, 3])
"#;
    let Err(errs) = check_str(source) else {
        panic!("unknown std.builtins member should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Type 'std.builtins' has no method 'not_real(...)'")),
        "Expected missing-method diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_root_sum_shadowing_preserved_but_explicit_std_builtins_bypasses_shadow() {
    let source = r#"
def sum(value: str) -> str:
  return value

def root_call() -> str:
  return sum("ok")

def explicit_call() -> int:
  x = [1, 2, 3]
  return std.builtins.sum(x)
"#;
    assert_check_ok(source);
}

// ---- #1725: `print` has no rendering for a tuple ----

#[test]
fn print_of_a_tuple_value_is_refused_issue1725() {
    // The program from #1725: the tuple has no printed form, so the checker refuses it where the build would have.
    let source = r#"
def get_coordinates() -> tuple[int, int]:
    return (10, 20)

def main() -> None:
    coords: tuple[int, int] = get_coordinates()
    print(coords)
    println(get_coordinates())
"#;
    let errors = check_str_err(source, "printing a tuple must be refused");
    let refused = errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0103"))
        .map(|error| (error.message.as_str(), error.hints.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        refused.len(),
        2,
        "both call spellings refuse the tuple, got: {refused:?}"
    );
    assert_eq!(refused[0].0, "'print' cannot print the tuple 'coords'");
    assert!(
        refused[0]
            .1
            .iter()
            .any(|hint| hint.contains("print(coords[0], coords[1])")),
        "the hint spells the element-by-element form, got: {:?}",
        refused[0].1
    );
    assert_eq!(refused[1].0, "'println' cannot print the tuple 'value'");
}

#[test]
fn print_of_tuple_elements_and_unpacked_names_is_accepted_issue1725() {
    assert_check_ok(
        r#"
def get_coordinates() -> tuple[int, int]:
    return (10, 20)

def main() -> None:
    coords: tuple[int, int] = get_coordinates()
    print(coords[0], coords[1])
    x, y = coords
    println(x, y)
    println(f"{coords[0]},{coords[1]}")
"#,
    );
}
