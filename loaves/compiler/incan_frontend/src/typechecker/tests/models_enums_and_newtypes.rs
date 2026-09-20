//! Nominal type declarations: model and class definitions and construction, generic model field substitution,
//! `@derive(Validate)`, enums and value enums, enum-variant constructors, newtype trait adoption and associated types,
//! validated newtypes, and the #1370 type-parameter rule.

use super::*;

#[test]
fn test_newtype_class_name_magic_method_is_not_assumed() {
    let source = r#"
type UserId = newtype int

def describe(user_id: UserId) -> str:
  return user_id.__class_name__()
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_derive_validate_requires_validate_method() {
    let source = r#"
@derive(Validate)
model User:
  name: str
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_derive_validate_rejects_raw_constructor_call() {
    let source = r#"
@derive(Validate)
model User:
  name: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def main() -> int:
  let u = User(name="Ada")
  return 0
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_derive_validate_allows_new_constructor_call() {
    let source = r#"
@derive(Validate)
model User:
  name: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def build_user() -> Result[User, str]:
  return User.new(name="Ada")
"#;
    assert_check_ok(source);
}

#[test]
fn test_derive_validate_new_constructor_param_order_positional() {
    let source = r#"
@derive(Validate)
model User:
  id: int
  email: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def build_user() -> Result[User, str]:
  return User.new(42, "a@b.com")
"#;
    assert_check_ok(source);
}

#[test]
fn test_derive_validate_new_constructor_param_order_positional_mismatch() {
    let source = r#"
@derive(Validate)
model User:
  id: int
  email: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def build_user() -> Result[User, str]:
  # Wrong order: str then int should be rejected.
  return User.new("a@b.com", 42)
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_generic_model_field_access_returns_substituted_type() {
    let source = r#"
pub model Boxed[T]:
  pub value: T

pub def get_value[T](boxed: Boxed[T]) -> T:
  return boxed.value
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_explicit_generic_model_constructor_args_specialize_field_types() {
    let source = r#"
pub trait Iterator[T]:
  def __next__(mut self) -> Option[T]: ...

  def zip[U](self, other: Iterator[U]) -> Iterator[tuple[T, U]]:
    return ZipIterator[T, Self, U, Iterator[U]](left=self, right=other)

pub model ZipIterator[T, Left with Iterator[T], U, Right with Iterator[U]] with Iterator[tuple[T, U]]:
  pub left: Left
  pub right: Right

  def __next__(mut self) -> Option[tuple[T, U]]:
    return None
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_class_field_access_substitutes_nested_field_type() {
    let source = r#"
pub class Boxed[T]:
  pub values: List[T]

pub def get_values[T](boxed: Boxed[T]) -> List[T]:
  return boxed.values
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_model_field_type_params_shadow_value_variants() {
    let source = r#"
pub enum JoinSide:
  Left
  Right

pub trait Iterator[T]:
  def __next__(mut self) -> Option[T]: ...

  def zip[U](self, other: Iterator[U]) -> Iterator[tuple[T, U]]:
    return ZipIterator[T, Self, U, Iterator[U]](left=self, right=other)

pub model ZipIterator[T, Left with Iterator[T], U, Right with Iterator[U]] with Iterator[tuple[T, U]]:
  pub left: Left
  pub right: Right

  def __next__(mut self) -> Option[tuple[T, U]]:
    return None
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_model_definition() {
    let source = r#"
model User:
  name: str
  age: int
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_model_instantiation() {
    let source = r#"
model Point:
  x: int
  y: int

def make_point() -> Point:
  return Point(x=0, y=0)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_class_definition() {
    let source = r#"
class Counter:
  value: int

  def get(self) -> int:
    return self.value
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_enum_definition() {
    let source = r#"
enum Color:
  Red
  Green
  Blue
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_enum_instance_method_typechecks() {
    let source = r#"
enum Color:
  Red
  Blue

  def label(self) -> str:
    return "color"

def label_red() -> str:
  return Red.label()
"#;
    assert_check_ok(source);
}

#[test]
fn test_enum_associated_method_typechecks() {
    let source = r#"
enum Status:
  Ok
  Failed

  def fallback() -> Status:
    return Failed

def choose() -> Status:
  return Status.fallback()
"#;
    assert_check_ok(source);
}

#[test]
fn test_enum_explicit_trait_adoption_typechecks() {
    let source = r#"
trait Labelled:
  def label(self) -> str: ...

enum Color with Labelled:
  Red
  Blue

  def label(self) -> str:
    return "color"

def render(color: Color) -> str:
  return color.label()
"#;
    assert_check_ok(source);
}

#[test]
fn test_enum_missing_trait_method_is_rejected() {
    let source = r#"
trait Labelled:
  def label(self) -> str: ...

enum Color with Labelled:
  Red
  Blue
"#;
    let errs = check_str_err(source, "enum should satisfy abstract trait methods");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("requires method") && e.message.contains("label")),
        "expected missing enum trait method diagnostic, got {errs:?}"
    );
}

#[test]
fn test_newtype_explicit_trait_adoption_typechecks() {
    let source = r#"
trait Labelled:
  def label(self) -> str: ...

type UserId = newtype int with Labelled:
  def label(self) -> str:
    return "user"

def render(user_id: UserId) -> str:
  return user_id.label()
"#;
    assert_check_ok(source);
}

#[test]
fn test_newtype_missing_trait_method_is_rejected() {
    let source = r#"
trait Labelled:
  def label(self) -> str: ...

type UserId = newtype int with Labelled
"#;
    let errs = check_str_err(source, "newtype should satisfy abstract trait methods");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("requires method") && e.message.contains("label")),
        "expected missing newtype trait method diagnostic, got {errs:?}"
    );
}

#[test]
fn test_newtype_unknown_trait_adoption_is_rejected() {
    let source = r#"
type UserId = newtype int with MissingTrait
"#;
    let errs = check_str_err(source, "newtype should reject unknown adopted traits");
    assert!(
        has_unknown_symbol_error(&errs, "MissingTrait"),
        "expected unknown trait diagnostic, got {errs:?}"
    );
}

#[test]
fn test_newtype_method_trait_targets_disambiguate_same_name_obligations() {
    let source = r#"
trait ToInt:
  def convert(self) -> int: ...

trait ToStr:
  def convert(self) -> str: ...

type Value = newtype int with ToInt, ToStr:
  def convert(self) for ToInt -> int:
    return 1

  def convert(self) for ToStr -> str:
    return "value"
"#;
    assert_check_ok(source);
}

#[test]
fn test_newtype_associated_type_resolves_trait_target_and_rhs() {
    let source = r#"
trait Add[T]:
  def add(self, rhs: T) -> Self: ...

type UserId = newtype int with Add[int]:
  type Output for Add[int] = UserId

  def add(self, rhs: int) -> Self:
    return self
"#;
    assert_check_ok(source);
}

#[test]
fn test_enum_generic_trait_adoption_arity_is_checked() {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

enum Token with Boxed[int, str]:
  Number

  def get(self) -> int:
    return 1
"#;
    let errs = check_str_err(source, "enum generic trait adoption should validate arity");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expects 1") || e.message.contains("arity")),
        "expected enum trait adoption arity diagnostic, got {errs:?}"
    );
}

#[test]
fn test_enum_satisfies_explicit_trait_bound() {
    let source = r#"
trait Labelled:
  def label(self) -> str: ...

enum Color with Labelled:
  Red
  Blue

  def label(self) -> str:
    return "color"

def keep_labelled[T with Labelled](value: T) -> T:
  return value

def keep_red() -> Color:
  return keep_labelled(Red)
"#;
    assert_check_ok(source);
}

#[test]
fn test_value_enum_str_generated_surface_typechecks() {
    let source = r#"
enum Env(str):
  Dev = "development"
  Prod = "production"
  Production = alias Prod

def raw(env: Env) -> str:
  return env.value()

def parse() -> Option[Env]:
  return Env.Production
"#;
    assert_check_ok(source);
}

#[test]
fn test_value_enum_variant_aliases_validate_target() {
    let source = r#"
enum Env(str):
  Dev = "development"
  Local = alias Missing
"#;
    let errs = check_str_err(source, "value enum alias with missing target should fail");
    assert!(
        errs.iter().any(|e| e.message.contains("Unknown symbol 'Missing'")),
        "expected unknown alias target diagnostic, got {errs:?}"
    );
}

#[test]
fn test_value_enum_int_generated_surface_typechecks() {
    let source = r#"
enum HttpStatus(int):
  Ok = 200
  NotFound = 404

def raw(status: HttpStatus) -> int:
  return status.value()

def parse() -> Option[HttpStatus]:
  return HttpStatus.from_value(404)
"#;
    assert_check_ok(source);
}

#[test]
fn test_value_enum_duplicate_raw_values_rejected() {
    let source = r#"
enum Env(str):
  Dev = "local"
  Local = "local"
"#;
    let errs = check_str_err(source, "duplicate value enum raw values should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Duplicate value enum value") && e.message.contains("Dev")),
        "expected duplicate value enum diagnostic, got {errs:?}"
    );
}

#[test]
fn test_value_enum_generated_names_reserved() {
    let source = r#"
enum Env(str):
  value = "value"
  Prod = "production"
"#;
    let errs = check_str_err(source, "generated value enum helper names should be reserved");
    assert!(
        errs.iter().any(|e| e.message.contains("generated member name 'value'")),
        "expected reserved generated member diagnostic, got {errs:?}"
    );
}

#[test]
fn test_value_enum_type_params_rejected() {
    let source = r#"
enum Box[T](str):
  Value = "value"
"#;
    let errs = check_str_err(source, "generic value enum should be rejected");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("cannot declare type parameters")),
        "expected generic value enum diagnostic, got {errs:?}"
    );
}

/// Issue #1370: a newtype has exactly its underlying type, so a declared parameter the underlying type does not
/// mention has nowhere to live; the checker refuses it at the parameter's span instead of the generated Rust failing.
#[test]
fn issue1370_newtype_type_param_not_in_underlying_type_is_refused() -> Result<(), String> {
    let source = r#"
type Tag[T] = newtype str:
  def label(self) -> str:
    return self.0
"#;
    let errs = check_str_err(source, "expected the phantom newtype parameter to be refused");
    let refusal = errs
        .iter()
        .find(|e| e.message == "Type parameter 'T' of newtype 'Tag' is not used by its underlying type")
        .ok_or_else(|| format!("expected the type_param_not_stored diagnostic, got {errs:?}"))?;
    assert_eq!(
        refusal.hints,
        vec!["Use 'T' in the underlying type, for example `newtype list[T]`, or remove it"]
    );
    let param_offset = source
        .find("[T]")
        .ok_or_else(|| "fixture must declare [T]".to_string())?
        + 1;
    assert_eq!(
        refusal.span.start, param_offset,
        "the diagnostic points at the parameter, got {:?}",
        refusal.span
    );

    let rusttype = r#"
from rust::std::collections import HashMap

type Index[K, V] = rusttype HashMap[K, str]
"#;
    let errs = check_str_err(rusttype, "expected the unused rusttype parameter to be refused");
    assert!(
        errs.iter()
            .any(|e| e.message == "Type parameter 'V' of newtype 'Index' is not used by its underlying type"),
        "a rusttype takes the same rule, and only the unused parameter is named; got {errs:?}"
    );
    assert!(
        !errs.iter().any(|e| e.message.contains("'K' of newtype")),
        "a parameter the underlying type mentions is accepted; got {errs:?}"
    );
    Ok(())
}

/// Issue #1370: a newtype whose parameter appears anywhere in the underlying type is the accepted shape.
#[test]
fn issue1370_newtype_type_param_in_underlying_type_is_accepted() {
    assert_check_ok(
        r#"
type Bare[T] = newtype T

type Many[T] = newtype list[T]:
  def count(self) -> int:
    return len(self.0)

type Pair[K, V] = newtype (K, list[V])
"#,
    );
}

/// Issue #1370: an enum stores its parameters in variant payloads; a parameter no payload mentions is refused at the
/// parameter's span rather than reaching rustc as an unused type parameter.
#[test]
fn issue1370_enum_type_param_not_in_any_payload_is_refused() -> Result<(), String> {
    let source = r#"
enum Slot[T]:
  Filled
  Empty

  def describe(self) -> str:
    return "slot"
"#;
    let errs = check_str_err(source, "expected the phantom enum parameter to be refused");
    let refusal = errs
        .iter()
        .find(|e| e.message == "Type parameter 'T' of enum 'Slot' is not used by any variant payload")
        .ok_or_else(|| format!("expected the type_param_not_stored diagnostic, got {errs:?}"))?;
    assert_eq!(
        refusal.hints,
        vec!["Give a variant a payload that mentions 'T', for example `Some(T)`, or remove it"]
    );
    let param_offset = source
        .find("[T]")
        .ok_or_else(|| "fixture must declare [T]".to_string())?
        + 1;
    assert_eq!(
        refusal.span.start, param_offset,
        "the diagnostic points at the parameter, got {:?}",
        refusal.span
    );

    let partly_stored = r#"
enum Outcome[T, E]:
  Done(T)
  Pending
"#;
    let errs = check_str_err(partly_stored, "expected the unused enum parameter to be refused");
    assert!(
        errs.iter()
            .any(|e| e.message == "Type parameter 'E' of enum 'Outcome' is not used by any variant payload"),
        "only the parameter no payload mentions is named; got {errs:?}"
    );
    assert!(
        !errs.iter().any(|e| e.message.contains("'T' of enum")),
        "a parameter some payload mentions is accepted; got {errs:?}"
    );
    Ok(())
}

/// Issue #1370: an enum whose parameter appears in at least one payload, at any nesting, is the accepted shape.
#[test]
fn issue1370_enum_type_param_in_a_payload_is_accepted() {
    assert_check_ok(
        r#"
enum Maybe[T]:
  Some(T)
  Nothing

enum Batch[T, E]:
  Items(list[T])
  Failed(str, E)
  Empty

  def is_empty(self) -> bool:
    match self:
      Batch.Empty => return true
      _ => return false
"#,
    );
}

#[test]
fn test_value_enum_from_value_argument_type_checked() {
    let source = r#"
enum Env(str):
  Dev = "development"

def parse() -> Option[Env]:
  return Env.from_value(1)
"#;
    let errs = check_str_err(source, "from_value should require the value enum backing type");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expected 'str'") && e.message.contains("found 'int'")),
        "expected from_value argument type mismatch, got {errs:?}"
    );
}

#[test]
fn test_value_enum_from_value_arity_checked() {
    let source = r#"
enum Env(str):
  Dev = "development"

def parse() -> Option[Env]:
  return Env.from_value()
"#;
    let errs = check_str_err(source, "from_value should require one argument");
    assert!(
        errs.iter().any(|e| e.message.contains("expects 1 argument")),
        "expected from_value arity diagnostic, got {errs:?}"
    );
}

#[test]
fn test_value_enum_value_arity_checked() {
    let source = r#"
enum Env(str):
  Dev = "development"

def raw(env: Env) -> str:
  return env.value(1)
"#;
    let errs = check_str_err(source, "value should not accept arguments");
    assert!(
        errs.iter().any(|e| e.message.contains("expects 0 argument")),
        "expected value arity diagnostic, got {errs:?}"
    );
}

#[test]
fn test_value_enum_from_value_requires_type_receiver() {
    let source = r#"
enum Env(str):
  Dev = "development"

def parse(env: Env) -> Option[Env]:
  return env.from_value("development")
"#;
    let errs = check_str_err(source, "from_value should require an enum type receiver");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("from_value") || e.message.contains("Unknown")),
        "expected receiver-shape diagnostic for from_value, got {errs:?}"
    );
}

#[test]
fn test_value_enum_remains_distinct_from_primitive() {
    let source = r#"
enum Env(str):
  Dev = "development"

def raw(env: Env) -> str:
  return env
"#;
    let errs = check_str_err(
        source,
        "value enum should not be assignable to its backing primitive type",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expected 'str'") && e.message.contains("found 'Env'")),
        "expected nominal value enum mismatch, got {errs:?}"
    );
}

#[test]
fn test_generic_newtype_static_builder_typechecks() {
    let source = r#"
type Box[T] = newtype T:
  @staticmethod
  def wrap(value: T) -> Self:
    return Box(value)

  def duplicate(self) -> Tuple[T, T]:
    return (self.0, self.0)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_enum_variant_constructor_is_a_first_class_callable() {
    let source = r#"
enum StreamError:
  Fetch(str)

def apply_error[E](detail: str, constructor: (str) -> E) -> E:
  return constructor(detail)

def build_error() -> StreamError:
  return apply_error("unavailable", StreamError.Fetch)
"#;

    assert_check_ok(source);
}

#[test]
fn test_validated_newtype_implicit_coercions_are_recorded() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Attempts, ValidationError]:
    return Ok(Attempts(n))

type RetryAttempts = newtype Attempts

model Job:
  attempts: Attempts

def take_attempts(a: Attempts) -> None:
  return

def main() -> None:
  take_attempts(3)
  attempts: Attempts = 4
  retry: RetryAttempts = 5
  job = Job(attempts=6)
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let coercions = &checker.type_info().expressions.validated_newtype_coercions;

    assert!(
        coercions.values().any(|info| {
            info.target_type == ResolvedType::Named("Attempts".to_string())
                && info.steps.len() == 1
                && info.steps[0].newtype_name == "Attempts"
                && info.steps[0].ctor.as_deref() == Some("from_underlying")
        }),
        "expected direct Attempts coercion, got {coercions:?}"
    );
    assert!(
        coercions.values().any(|info| {
            info.target_type == ResolvedType::Named("RetryAttempts".to_string())
                && info
                    .steps
                    .iter()
                    .map(|step| step.newtype_name.as_str())
                    .collect::<Vec<_>>()
                    == vec!["Attempts", "RetryAttempts"]
        }),
        "expected transitive RetryAttempts coercion, got {coercions:?}"
    );
    Ok(())
}

#[test]
fn test_validated_newtype_implicit_coercion_does_not_parse_primitives() {
    let source = r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Attempts, ValidationError]:
    return Ok(Attempts(n))

def take_attempts(a: Attempts) -> None:
  return

def main() -> None:
  take_attempts("3")
"#;
    let errors = check_str_err(source, "expected str-to-newtype coercion to fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected 'Attempts'") && error.message.contains("found 'str'")),
        "unexpected errors: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_validated_newtype_hook_requires_validation_error() {
    let source = r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Attempts, str]:
    return Ok(Attempts(n))
"#;
    let errors = check_str_err(source, "expected malformed from_underlying hook to fail");
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("Invalid 'Attempts.from_underlying' validation hook")
                && error.message.contains("ValidationError")
        }),
        "unexpected errors: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_validated_newtype_hook_allows_self_return() -> Result<(), Vec<CompileError>> {
    let source = r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Self, ValidationError]:
    return Ok(Attempts(n))

def take_attempts(value: Attempts) -> None:
  return

def main() -> None:
  take_attempts(1)
"#;
    check_str(source)
}

#[test]
fn test_explicit_validated_newtype_constructor_checks_underlying_type() {
    let source = r#"
type Attempts = newtype int:
  def from_underlying(n: int) -> Result[Attempts, ValidationError]:
    return Ok(Attempts(n))

def main() -> None:
  attempts = Attempts("3")
"#;
    let errors = check_str_err(
        source,
        "expected explicit newtype constructor to reject wrong underlying type",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected 'int'") && error.message.contains("found 'str'")),
        "unexpected errors: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_validated_newtype_reassignment_is_not_implicit_coercion_site() {
    let source = r#"
type Attempts = newtype int

def main() -> None:
  mut attempts: Attempts = Attempts(1)
  attempts = 2
"#;
    let errors = check_str_err(source, "expected reassignment to reject implicit newtype coercion");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("expected 'Attempts'") && error.message.contains("found 'int'")),
        "unexpected errors: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_validated_newtype_constrained_underlying_records_generated_validation() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
type PositiveInt = newtype int[gt=0]

def take_positive(value: PositiveInt) -> None:
  return

def main() -> None:
  take_positive(1)
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let coercions = &checker.type_info().expressions.validated_newtype_coercions;
    assert!(
        coercions.values().any(|info| {
            info.target_type == ResolvedType::Named("PositiveInt".to_string())
                && info.steps.len() == 1
                && info.steps[0].newtype_name == "PositiveInt"
                && info.steps[0].ctor.is_none()
                && info.steps[0]
                    .constraints
                    .iter()
                    .any(|constraint| matches!(constraint.key, TypeConstraintKey::Gt) && constraint.value == 0)
        }),
        "expected generated constrained newtype validation metadata, got {coercions:?}"
    );
    Ok(())
}

#[test]
fn test_validated_newtype_no_implicit_coercion_rejects_site() {
    let source = r#"
@no_implicit_coercion
type Attempts = newtype int

def take_attempts(value: Attempts) -> None:
  return

def main() -> None:
  take_attempts(1)
"#;
    let errors = check_str_err(source, "expected @no_implicit_coercion to reject implicit site");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Implicit coercion into newtype 'Attempts' is disabled")),
        "unexpected errors: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_validated_newtype_underlying_cycle_is_rejected() {
    let source = r#"
type A = newtype B
type B = newtype A
"#;
    let errors = check_str_err(source, "expected newtype cycle to be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Validated-newtype coercion cycle detected")),
        "unexpected errors: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}
