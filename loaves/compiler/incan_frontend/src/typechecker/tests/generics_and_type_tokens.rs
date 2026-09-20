//! Generic functions and methods: `Self` substitution at call sites (#237, #388), bounds enforced at call sites,
//! explicit call type arguments and RFC 054 inference placeholders, the #1373 hints, local inference after factory
//! calls, `Type[...]` tokens as values, and reflection magic methods.

use super::*;

#[test]
fn test_type_name_value_requires_type_token_expected_context() {
    let source = r#"
def accepts_any[T](value: T) -> None:
  return

def use() -> None:
  accepts_any(int)
"#;
    let errs = check_str_err(
        source,
        "bare primitive type value should require Type[T] expected context",
    );
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Cannot use type 'int' as a value")),
        "expected type-name-as-value diagnostic, got {errs:?}"
    );
}

#[test]
fn test_generic_type_token_parameter_accepts_type_name_value() {
    let source = r#"
def accepts_type[T](value: Type[T]) -> str:
  return "ok"

def use() -> str:
  return accepts_type(int)
"#;
    let result = check_str(source);
    assert!(
        result.is_ok(),
        "expected generic Type[T] parameter to accept primitive type token, got {result:?}"
    );
}

#[test]
fn test_reflection_magic_methods_record_surface_types() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model User:
  name: str

def describe(u: User) -> None:
  class_name = u.__class_name__()
  fields = u.__fields__()
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.expressions
            .expr_types
            .values()
            .any(|t| matches!(t, ResolvedType::Str)),
        "expected __class_name__() to resolve to str, got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|t| {
            matches!(
                t,
                ResolvedType::FrozenList(inner)
                    if matches!(inner.as_ref(), ResolvedType::Named(name) if name == "FieldInfo")
            )
        }),
        "expected __fields__() to resolve to FrozenList[FieldInfo], got {:?}",
        info.expressions.expr_types
    );
    Ok(())
}

#[test]
fn test_generic_reflection_magic_methods_record_surface_types() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def reflected_field_count[T](value: T) -> int:
  fields = value.__fields__()
  return len(fields)

def reflected_class_name[T](value: T) -> str:
  return value.__class_name__()

def reflected_field_value[T](value: T) -> Option[str]:
  return value.__field_value__("name")

def reflected_field_items[T](value: T) -> list[tuple[str, str]]:
  return value.__field_items__()
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.expressions
            .expr_types
            .values()
            .any(|ty| matches!(ty, ResolvedType::Str)),
        "expected generic __class_name__() to resolve to str, got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|ty| {
            matches!(
                ty,
                ResolvedType::FrozenList(inner)
                    if matches!(inner.as_ref(), ResolvedType::Named(name) if name == "FieldInfo")
            )
        }),
        "expected generic __fields__() to resolve to FrozenList[FieldInfo], got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|ty| {
            matches!(
                ty,
                ResolvedType::Generic(name, args)
                    if collection_types::from_str(name.as_str()) == Some(CollectionTypeId::Option)
                        && matches!(args.as_slice(), [ResolvedType::Str])
            )
        }),
        "expected generic __field_value__() to resolve to Option[str], got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|ty| {
            matches!(
                ty,
                ResolvedType::Generic(name, args)
                    if collection_types::from_str(name.as_str()) == Some(CollectionTypeId::List)
                        && matches!(
                            args.as_slice(),
                            [ResolvedType::Tuple(items)]
                                if matches!(items.as_slice(), [ResolvedType::Str, ResolvedType::Str])
                        )
            )
        }),
        "expected generic __field_items__() to resolve to list[tuple[str, str]], got {:?}",
        info.expressions.expr_types
    );
    Ok(())
}

#[test]
fn test_type_parameter_reflection_magic_methods_record_surface_types() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def reflected_field_count[T]() -> int:
  fields = T.__fields__()
  return len(fields)

def reflected_class_name[T]() -> str:
  return T.__class_name__()
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.expressions
            .expr_types
            .values()
            .any(|ty| matches!(ty, ResolvedType::Str)),
        "expected type-parameter __class_name__() to resolve to str, got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|ty| {
            matches!(
                ty,
                ResolvedType::FrozenList(inner)
                    if matches!(inner.as_ref(), ResolvedType::Named(name) if name == "FieldInfo")
            )
        }),
        "expected type-parameter __fields__() to resolve to FrozenList[FieldInfo], got {:?}",
        info.expressions.expr_types
    );
    Ok(())
}

#[test]
fn test_model_type_name_is_type_token_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model User:
  name: str

def accepts_user_type(value: Type[User]) -> str:
  return "ok"

def main() -> None:
  accepts_user_type(User)
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.expressions.expr_types.values().any(|ty| {
            matches!(
                ty,
                ResolvedType::TypeToken(inner) if matches!(inner.as_ref(), ResolvedType::Named(name) if name == "User")
            )
        }),
        "expected model type name to resolve as Type[User], got {:?}",
        info.expressions.expr_types
    );
    Ok(())
}

#[test]
fn test_type_token_does_not_satisfy_model_value_context() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model User:
  name: str

def accepts_user(value: User) -> str:
  return value.name

def main() -> None:
  accepts_user(User)
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        return Err(std::io::Error::other("expected bare User type name to be rejected as a value").into());
    };
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Cannot use type 'User' as a value")),
        "expected type-name-as-value diagnostic, got {errs:?}"
    );
    Ok(())
}

#[test]
fn test_reflection_fieldinfo_members_typecheck_without_explicit_import() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model User:
  name [alias="display_name"]: str = "Alice"

def describe(u: User) -> None:
  for info in u.__fields__():
    type_name = info.type_name
    alias = info.alias
    extra = info.extra
    has_default = info.has_default
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.expressions
            .expr_types
            .values()
            .any(|t| matches!(t, ResolvedType::FrozenStr)),
        "expected FieldInfo.name/type_name access to resolve to FrozenStr, got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|t| {
            matches!(
                t,
                ResolvedType::Generic(name, args)
                    if crate::typechecker::helpers::collection_type_id(name.as_str())
                        == Some(CollectionTypeId::Option)
                        && args.len() == 1
                        && matches!(args.first(), Some(ResolvedType::FrozenStr))
            )
        }),
        "expected FieldInfo.alias access to resolve to Option[FrozenStr], got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions.expr_types.values().any(|t| {
            matches!(
                t,
                ResolvedType::FrozenDict(key, value)
                    if matches!(key.as_ref(), ResolvedType::FrozenStr)
                        && matches!(value.as_ref(), ResolvedType::FrozenStr)
            )
        }),
        "expected FieldInfo.extra access to resolve to FrozenDict[FrozenStr, FrozenStr], got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.expressions
            .expr_types
            .values()
            .any(|t| matches!(t, ResolvedType::Bool)),
        "expected FieldInfo.has_default access to resolve to bool, got {:?}",
        info.expressions.expr_types
    );
    Ok(())
}

/// Regression for #237: `-> Self` on a generic class method must type as the instantiated receiver at the call site,
/// not bare `Self`, so annotations and chaining against `Carrier[Order]` succeed.
#[test]
fn test_issue_237_self_return_substituted_at_call_site() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Carrier[T]:
  _m: T

  def filter(self, _p: bool) -> Self:
    return self

model Order:
  id: int

def use_filter(x: Carrier[Order]) -> Carrier[Order]:
  return x.filter(true)

def use_annotated_local(x: Carrier[Order]) -> Carrier[Order]:
  y: Carrier[Order] = x.filter(true)
  return y
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    Ok(())
}

/// `Self` in non-receiver parameters must use the same call-site substitution as the return type (#237 follow-up).
#[test]
fn test_self_param_substituted_at_call_site_for_method_args() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Carrier[T]:
  _m: T

  def join(self, other: Self, cond: bool) -> Self:
    return self

model Order:
  id: int

def use_join(left: Carrier[Order], right: Carrier[Order]) -> Carrier[Order]:
  return left.join(right, true)
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    Ok(())
}

#[test]
fn test_issue_388_generic_classmethod_cls_constructor_typechecks() {
    let source = r#"
@derive(Clone)
class Box[T with Clone]:
  value: T

  @classmethod
  def make(cls, value: T) -> Self:
    return cls(value=value)
"#;

    assert_check_ok(source);
}

/// Trait **default** methods are not copied into `ClassInfo.methods`; dispatch goes through the trait branch of
/// `resolve_named_method`. Call-site `Self` substitution must still apply (#237).
#[test]
fn test_issue_237_self_substitution_trait_default_methods_not_on_class_map() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def filter(self, _p: bool) -> Self:
    return self

  def join(self, other: Self, cond: bool) -> Self:
    return self

class Carrier[T] with DataSet:
  _m: T

model Order:
  id: int

def use_filter(x: Carrier[Order]) -> Carrier[Order]:
  return x.filter(true)

def use_annotated_local(x: Carrier[Order]) -> Carrier[Order]:
  y: Carrier[Order] = x.filter(true)
  return y

def use_join(left: Carrier[Order], right: Carrier[Order]) -> Carrier[Order]:
  return left.join(right, true)
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    Ok(())
}

#[test]
fn test_generic_bound_enforced_at_callsite_negative() {
    let source = r#"
@requires(message: str)
trait Displayable:
  def display(self) -> str:
    return self.message

class NotDisplayable:
  value: int

def show[T with Displayable](value: T) -> T:
  return value

def main() -> None:
  _ = show(NotDisplayable(value=1))
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected generic bound failure");
    };
    assert!(errs.iter().any(|e| e.message.contains("violates generic bound")));
}

#[test]
fn test_generic_bound_enforced_at_callsite_positive() {
    let source = r#"
@requires(message: str)
trait Displayable:
  def display(self) -> str:
    return self.message

class User with Displayable:
  message: str

def show[T with Displayable](value: T) -> T:
  return value

def main() -> None:
  _ = show(User(message="ok"))
"#;
    assert_check_ok(source);
}

#[test]
fn test_generic_bound_propagates_through_nested_generic_call() {
    let source = r#"
trait Reader:
  def read_bytes(self, size: int) -> bytes: ...

model Buffer with Reader:
  data: bytes

  def read_bytes(self, _size: int) -> bytes:
    return self.data

def feed[R with Reader](reader: R) -> bytes:
  return reader.read_bytes(1)

def outer[R with Reader](reader: R) -> bytes:
  return feed(reader)

def main() -> bytes:
  return outer(Buffer(data=b"abc"))
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_preserves_method_result_field_access_after_factory_call() {
    let source = r#"
class Backend:
  pub enable_optimizer: bool

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session = Session.default()
  let backend = session.backend()
  let enabled = backend.enable_optimizer
  let _ = enabled
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_preserves_result_match_after_factory_call() {
    let source = r#"
@derive(Clone)
class Source:
  value: str

model SessionError:
  kind: str

class Session:
  regs: list[Source]

  @staticmethod
  def default() -> Session:
    return Session(regs=[])

  def register(mut self, logical_name: str, source: Source) -> Result[None, SessionError]:
    self.regs.append(source)
    return Ok(None)

def main() -> None:
  mut session = Session.default()
  match session.register("x", Source(value="y")):
    Ok(_) => pass
    Err(err) => pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_preserves_generic_result_match_after_factory_call() {
    let source = r#"
model SessionError:
  kind: str

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def table[T with Clone](self, logical_name: str, marker: T) -> Result[T, SessionError]:
    return Ok(marker)

def main() -> None:
  let session = Session.default()
  match session.table("x", 1):
    Ok(value) => pass
    Err(err) => pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_annotation_control_still_typechecks() {
    let source = r#"
class Backend:
  pub enable_optimizer: bool

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session: Session = Session.default()
  let backend: Backend = session.backend()
  let _ = backend.enable_optimizer
"#;
    assert_check_ok(source);
}

#[test]
fn test_direct_construction_method_result_field_access_control_typechecks() {
    let source = r#"
class Backend:
  pub enable_optimizer: bool

class Session:
  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session = Session()
  let backend = session.backend()
  let _ = backend.enable_optimizer
"#;
    assert_check_ok(source);
}

#[test]
fn test_direct_construction_with_static_factory_present_still_typechecks() {
    let source = r#"
class Backend:
  pub enable_optimizer: bool

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session = Session()
  let backend = session.backend()
  let _ = backend.enable_optimizer
"#;
    assert_check_ok(source);
}

#[test]
fn explicit_call_type_args_specialize_generic_function_params() {
    assert_check_ok(
        r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  return id[int](1)
"#,
    );
}

#[test]
fn explicit_call_type_args_enforce_function_type_arg_arity() {
    let source = r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  return id[int, str](1)
"#;
    let errs = check_str_err(source, "expected explicit type arg arity error");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expects 1 explicit type argument(s), got 2")),
        "expected explicit type argument arity diagnostic, got {errs:?}"
    );
}

/// Issue #1373 (1): a `with` bound naming a type is not a trait, so the hint must not ask for an implementation of
/// `float`, and it must not point at the value arguments when the explicit type argument is what mismatched.
#[test]
fn issue1373_type_bound_violation_hint_names_the_type_not_an_implementation() -> Result<(), String> {
    let source = r#"
def cast[T with float](x: int) -> float:
  return 1.0

def main() -> None:
  a = cast[int](1)
"#;
    let errs = check_str_err(source, "expected the float bound to reject cast[int]");
    let bound_error = errs
        .iter()
        .find(|e| {
            e.message == "Call to 'cast' violates generic bound: type parameter 'T' requires 'float' but got 'int'"
        })
        .ok_or_else(|| format!("expected the bound-violation error, got {errs:?}"))?;
    let hint = bound_error.hints.join("\n");
    assert!(
        hint.contains("'float' is a type, not a trait") && hint.contains("declare the parameter as 'float'"),
        "the hint must explain that a type in bound position cannot be implemented, got {hint:?}"
    );
    assert!(
        !hint.contains("implements 'float'") && !hint.contains("the argument type"),
        "the hint must not ask to implement a type or blame the value arguments, got {hint:?}"
    );
    Ok(())
}

/// Issue #1373 (1): with a trait bound, an explicit type argument that fails it is named as the type argument.
#[test]
fn issue1373_explicit_type_argument_bound_violation_hint_names_the_type_argument() -> Result<(), String> {
    let source = r#"
@requires(message: str)
trait Displayable:
  def display(self) -> str:
    return self.message

class NotDisplayable:
  value: int

def show[T with Displayable](value: T) -> T:
  return value

def main() -> None:
  _ = show[NotDisplayable](NotDisplayable(value=1))
"#;
    let errs = check_str_err(source, "expected the Displayable bound to reject show[NotDisplayable]");
    let bound_error = errs
        .iter()
        .find(|e| e.message.contains("violates generic bound"))
        .ok_or_else(|| format!("expected the bound-violation error, got {errs:?}"))?;
    let hint = bound_error.hints.join("\n");
    assert!(
        hint.contains("Type argument 'NotDisplayable' for 'T' must implement 'Displayable'"),
        "an explicit type argument is named as such, got {hint:?}"
    );
    assert!(
        !hint.contains("the argument type"),
        "the value arguments are not what mismatched, got {hint:?}"
    );
    Ok(())
}

/// Issue #1373 (2): a call that no overload accepts lists every candidate and what each one wanted, instead of only
/// the first-declared candidate's bound.
#[test]
fn issue1373_overload_set_rejection_lists_every_candidate() -> Result<(), String> {
    let source = r#"
def cast[T with float](x: int) -> float:
  return 1.0

def cast[T with int](x: int) -> int:
  return 1

def main() -> None:
  c = cast[str](1)
"#;
    let errs = check_str_err(source, "expected cast[str] to match no overload");
    let summary = errs
        .iter()
        .find(|e| e.message == "Call to 'cast' matches none of its 2 overloads")
        .ok_or_else(|| format!("expected the overload summary diagnostic, got {errs:?}"))?;
    assert_eq!(
        summary.notes,
        vec![
            "candidate `cast[T with float](x: int) -> float` rejected: Call to 'cast' violates generic bound: type \
             parameter 'T' requires 'float' but got 'str'",
            "candidate `cast[T with int](x: int) -> int` rejected: Call to 'cast' violates generic bound: type \
             parameter 'T' requires 'int' but got 'str'",
        ],
        "each candidate is listed with the reason it was rejected"
    );
    Ok(())
}

/// Issue #1373 (3): an unknown name in a signature's type position is most likely an undeclared type parameter, so
/// the hint shows the declaration rather than sending the reader to their imports.
#[test]
fn issue1373_undeclared_type_parameter_suggests_declaring_it() -> Result<(), String> {
    let source = r#"
model Column[T]:
  name: str

def widen(x: Column[U]) -> None:
  println(f"{x.name}")
"#;
    let errs = check_str_err(source, "expected the undeclared U to be rejected");
    let unknown = errs
        .iter()
        .find(|e| e.message == "Unknown symbol 'U'")
        .ok_or_else(|| format!("expected the unknown-symbol error, got {errs:?}"))?;
    assert_eq!(
        unknown.hints.first().map(String::as_str),
        Some("'U' is not declared as a type parameter of 'widen'; did you mean `def widen[U](...)`?"),
        "the first hint shows where the declaration goes, got {:?}",
        unknown.hints
    );
    assert!(
        !unknown.hints.iter().any(|hint| hint.contains("forget to import")),
        "the generic import hint must not lead, got {:?}",
        unknown.hints
    );

    let generic_owner = r#"
def pair[T](x: T, y: U) -> T:
  return x
"#;
    let errs = check_str_err(generic_owner, "expected the undeclared U to be rejected");
    let unknown = errs
        .iter()
        .find(|e| e.message == "Unknown symbol 'U'")
        .ok_or_else(|| format!("expected the unknown-symbol error, got {errs:?}"))?;
    assert_eq!(
        unknown.hints.first().map(String::as_str),
        Some("'U' is not declared as a type parameter of 'pair'; did you mean `def pair[T, U](...)`?"),
        "declared parameters are kept ahead of the missing one, got {:?}",
        unknown.hints
    );
    Ok(())
}

/// Issue #1373 (4): RFC 054 keeps an explicit bracket list arity-complete, so a short list names the parameters it
/// left unbound and shows the `_` placeholder that infers them, rather than only counting.
#[test]
fn issue1373_partial_explicit_type_arguments_hint_shows_the_inference_placeholder() -> Result<(), String> {
    let source = r#"
def convert[T, U](x: U) -> T:
  return x

def main() -> None:
  a: float = convert[float](1)
"#;
    let errs = check_str_err(source, "expected the partial bracket list to be rejected");
    let arity = errs
        .iter()
        .find(|e| e.message == "convert expects 2 explicit type argument(s), got 1")
        .ok_or_else(|| format!("expected the arity error, got {errs:?}"))?;
    assert_eq!(
        arity.notes,
        vec!["'convert' declares type parameters [T, U]; an explicit list binds every one of them in that order"],
        "the note names the declared parameters"
    );
    assert_eq!(
        arity.hints,
        vec!["Write `_` for a parameter the value arguments determine (U): convert[float, _](...)"],
        "the hint completes the written list with the inference placeholder"
    );
    Ok(())
}

#[test]
fn explicit_method_type_args_specialize_generic_method_params() {
    assert_check_ok(
        r#"
class Box:
  def get[T](self, value: T) -> T:
    return value

def run() -> int:
  let b = Box()
  return b.get[int](1)
"#,
    );
}

#[test]
fn explicit_method_type_args_enforce_generic_contract() {
    let source = r#"
class Box:
  def get[T](self, value: T) -> T:
    return value

def run() -> int:
  let b = Box()
  return b.get[int](str("x"))
"#;
    let errs = check_str_err(source, "expected explicit method type arg mismatch");
    assert!(
        errs.iter().any(|e| e.message.contains("expected 'int', found 'str'")),
        "expected type mismatch after explicit method type specialization, got {errs:?}"
    );
}

#[test]
fn explicit_call_type_args_infer_placeholder_filled_from_value_args() {
    assert_check_ok(
        r#"
def pair_map[T, U](x: T, y: U) -> int:
  return 0

def run() -> int:
  return pair_map[int, _](1, 2)
"#,
    );
}

#[test]
fn explicit_call_type_args_all_infer_placeholders_filled_from_value_args() {
    assert_check_ok(
        r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  return id[_](1)
"#,
    );
}

#[test]
fn explicit_call_type_args_infer_placeholder_reports_when_unresolved() {
    let source = r#"
def mystery[T]() -> int:
  return 0

def run() -> int:
  return mystery[_]()
"#;
    let errs = check_str_err(source, "expected inference unresolved when no value args bind T");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Could not infer type parameter")),
        "expected call-site `_` unresolved diagnostic, got {errs:?}"
    );
}

#[test]
fn explicit_call_type_args_rejected_on_builtin_callee() {
    let source = r#"
def run() -> int:
  return len[int]([1, 2])
"#;
    let errs = check_str_err(source, "expected unsupported explicit type args on builtin");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not supported for this call form")),
        "expected unsupported call-site type args diagnostic, got {errs:?}"
    );
}

#[test]
fn explicit_call_type_args_rejected_on_indirect_function_value_call() {
    let source = r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  let f = id
  return f[int](1)
"#;
    let errs = check_str_err(source, "expected unsupported explicit type args on indirect call");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not supported for this call form")),
        "expected unsupported call-site type args diagnostic, got {errs:?}"
    );
}
