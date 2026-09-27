//! Sum types and how they are taken apart: union member checks and `isinstance` / `is None` narrowing, transparent type
//! aliases (#562), match and `if let` patterns and pattern node types (#1245), enum-variant resolution against the
//! scrutinee, `Option` / `Result` payloads, the `?` operator, RFC 070 combinators, and RFC 068 truthiness.

use super::*;

#[test]
fn test_union_member_values_satisfy_explicit_union_return_type() {
    let source = r#"
def parse_value(flag: bool) -> int | str:
  if flag:
    return 42
  return "fallback"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_rejects_return_value_outside_member_set() {
    let source = r#"
def parse_value() -> int | str:
  return true
"#;
    let errors = check_str_err(source, "bool should not satisfy int | str");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Union") && error.message.contains("bool")),
        "expected union type mismatch diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_union_assignment_canonicalizes_none_through_option() {
    let source = r#"
def maybe_name(flag: bool) -> str | None:
  if flag:
    return "Ada"
  return None
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_isinstance_narrows_branch_type() {
    let source = r#"
def normalize(value: int | str) -> str:
  if isinstance(value, str):
    return value.upper()
  return "number"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_isinstance_narrows_else_branch_for_two_member_union() {
    let source = r#"
def normalize(value: int | str) -> str:
  if isinstance(value, int):
    return "number"
  else:
    return value.upper()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_isinstance_narrows_wider_else_branch_to_remaining_union() {
    let source = r#"
def normalize(value: int | str | bool) -> str:
  if isinstance(value, int):
    return "number"
  else:
    match value:
      bool(flag) =>
        if flag:
          return "true"
        return "false"
      str(text) =>
        return text.upper()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_isinstance_narrows_elif_chain() {
    let source = r#"
def normalize(value: int | str | bool) -> str:
  if isinstance(value, int):
    return "number"
  elif isinstance(value, str):
    return value.upper()
  else:
    if value:
      return "true"
    return "false"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_collection_literal_requires_explicit_union_annotation() {
    let source = r#"
def values() -> List[int | str]:
  items: List[int | str] = [1, "two"]
  return items
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_collection_literal_does_not_synthesize_implicit_union() {
    let source = r#"
def values() -> None:
  items = [1, "two"]
"#;
    let errors = check_str_err(source, "mixed list literal should require an explicit union annotation");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("int") && error.message.contains("str")),
        "expected mixed list element diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_union_is_not_none_narrows_option_canonicalized_union() {
    let source = r#"
def normalize(value: str | None) -> str:
  if value is not None:
    return value.upper()
  return "missing"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_is_none_narrows_else_branch_to_option_inner() {
    let source = r#"
def normalize(value: str | None) -> str:
  if value is None:
    return "missing"
  else:
    return value.upper()
"#;
    assert!(check_str(source).is_ok());
}

/// The callable parameters and the resolved type the checker recorded for one call site.
type CallSiteFacts = (Option<Vec<CallableParam>>, Option<ResolvedType>);

/// The callable fact recorded for the `Some(...)` call whose text is `call`, and the type recorded for it.
fn some_call_facts(
    info: &TypeCheckInfo,
    source: &str,
    call: &str,
) -> Result<CallSiteFacts, Box<dyn std::error::Error>> {
    let start = source
        .find(call)
        .ok_or_else(|| format!("missing `{call}` in the source"))?;
    let span = Span::new(start, start + call.len());
    Ok((
        info.call_site_callable_params(span).map(<[CallableParam]>::to_vec),
        info.expr_type(span).cloned(),
    ))
}

/// Regression for #1724: `Some(member)` checked against `Option[union]` is the constructor instantiated at the
/// union. The checker records that parameter as the call's callable fact (lowering carries it as the call's
/// signature, so the payload is injected into the wrapper at the argument) and types the call as `Option[union]`.
#[test]
fn some_payload_admitted_into_an_option_union_records_the_instantiated_parameter_issue1724()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
@derive(Clone)
type LocalPath = newtype str

const FROZEN_TEXT: FrozenStr = "frozen text"

def describe(value: Option[LocalPath | str]) -> str:
  return "described"

def frozen_option_union_kind(value: Option[FrozenStr | int]) -> str:
  return "kind"

def frozen_or_int(pick_text: bool) -> FrozenStr | int:
  if pick_text:
    return FROZEN_TEXT
  return 1

def main() -> None:
  println(describe(Some("plain")))
  println(describe(Some(LocalPath("p"))))
  println(frozen_option_union_kind(Some(FROZEN_TEXT)))
  println(frozen_option_union_kind(Some(1)))
  println(frozen_option_union_kind(Some(frozen_or_int(true))))
  local: Option[LocalPath | str] = Some("local")
  println(describe(local))
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "Some payloads into Option[union]")?;
    let path_or_str = union_ty(vec![ResolvedType::Named("LocalPath".to_string()), ResolvedType::Str]);
    let frozen_or_int = union_ty(vec![ResolvedType::FrozenStr, ResolvedType::Int]);
    let option_of = |inner: &ResolvedType| {
        ResolvedType::Generic(
            collection_types::as_str(CollectionTypeId::Option).to_string(),
            vec![inner.clone()],
        )
    };

    for (call, union) in [
        ("Some(\"plain\")", &path_or_str),
        ("Some(LocalPath(\"p\"))", &path_or_str),
        ("Some(FROZEN_TEXT)", &frozen_or_int),
        ("Some(1)", &frozen_or_int),
        ("Some(\"local\")", &path_or_str),
    ] {
        let (params, ty) = some_call_facts(&info, source, call)?;
        assert_eq!(
            params,
            Some(vec![CallableParam::positional(union.clone())]),
            "`{call}` must record the union as the constructor's parameter"
        );
        assert_eq!(
            ty,
            Some(option_of(union)),
            "`{call}` is the constructor instantiated at the union"
        );
    }

    // A payload that already carries the union needs no injection: no fact, and the call keeps the payload's type.
    let (params, ty) = some_call_facts(&info, source, "Some(frozen_or_int(true))")?;
    assert_eq!(params, None, "a union-typed payload records no parameter fact");
    assert_eq!(ty, Some(option_of(&frozen_or_int)));
    Ok(())
}

/// The fact is specific to a union destination: `Some(member)` against a plain `Option[T]` records nothing and keeps
/// its payload-derived type, so ordinary option constructors lower exactly as before.
#[test]
fn some_payload_into_a_plain_option_records_no_parameter_fact() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def describe(value: Option[str]) -> str:
  return "described"

def main() -> None:
  println(describe(Some("plain")))
"#;
    let info = typecheck_info_for_module(source, vec!["main".to_string()], "Some payload into Option[str]")?;
    let (params, ty) = some_call_facts(&info, source, "Some(\"plain\")")?;
    assert_eq!(params, None);
    assert_eq!(
        ty,
        Some(ResolvedType::Generic(
            collection_types::as_str(CollectionTypeId::Option).to_string(),
            vec![ResolvedType::Str],
        ))
    );
    Ok(())
}

#[test]
fn test_union_isinstance_narrows_option_wrapped_union_else_branch() {
    let source = r#"
def normalize(value: int | str | None) -> str:
  if isinstance(value, int):
    return "number"
  else:
    if value is None:
      return "missing"
    else:
      return value.upper()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn explicit_builtin_isinstance_narrows_union_and_option_branches() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def normalize_union(value: int | str) -> str:
  if std.builtins.isinstance(value, str):
    return value.upper()
  return "number"

def normalize_option(value: int | str | None) -> str:
  if std.builtins.isinstance(value, int):
    return "number"
  else:
    if value is None:
      return "missing"
    else:
      return value.upper()
"#;
    check_str(source).map_err(|errors| {
        std::io::Error::other(format!(
            "the explicit builtin identity must drive the same union and option narrowing as the ambient spelling: {errors:?}"
        ))
    })?;
    Ok(())
}

#[test]
fn test_union_match_type_patterns_bind_narrowed_values() {
    let source = r#"
def normalize(value: int | str) -> str:
  match value:
    int(n) =>
      return "number"
    str(s) =>
      return s.upper()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_match_wildcard_arm_narrows_remaining_member() {
    let source = r#"
def normalize(value: int | str) -> str:
  match value:
    int(n) =>
      return "number"
    _ =>
      return value.upper()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_issue562_type_aliases_are_transparent_for_dict_and_union_surfaces() -> Result<(), String> {
    let source = r#"
type FieldValue = str | bool | int | float | None
type Fields = Dict[str, FieldValue]

model Logger:
  fields: Fields = {}

  def copy_fields(self, extra: Fields) -> Fields:
    mut merged: Fields = {}
    for key in self.fields.keys():
      merged[key] = self.fields[key]
    for key in extra.keys():
      merged[key] = extra[key]
    return merged

def to_text(value: FieldValue) -> str:
  match value:
    str(text) =>
      return text
    bool(flag) =>
      if flag:
        return "true"
      return "false"
    int(number) =>
      return str(number)
    float(number) =>
      return str(number)
    None =>
      return "none"
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn test_generic_type_alias_expands_in_dict_contexts() -> Result<(), String> {
    let source = r#"
type NamedValues[T] = Dict[str, T]

def build() -> NamedValues[int]:
  mut values: NamedValues[int] = {}
  values["count"] = 1
  return values
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn test_type_alias_expands_in_narrowing_type_positions() -> Result<(), String> {
    let source = r#"
type Text = str
type MaybeText = Text | int | None

def normalize(value: MaybeText) -> str:
  if isinstance(value, Text):
    return value.upper()
  return "missing"

def describe(value: MaybeText) -> str:
  match value:
    Text(text) =>
      return text.upper()
    int(number) =>
      return str(number)
    None =>
      return "missing"
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn test_nested_union_aliases_flatten_for_match_narrowing() -> Result<(), String> {
    let source = r#"
model A:
  value: str

model B:
  value: str

type Base = Union[A, B]
type Input = Union[Base, int]

def from_alias(value: Input) -> Base:
  match value:
    Base(expr) =>
      return expr
    int(number) =>
      return A(value=str(number))

def keep_base(value: Base) -> bool:
  return true

def from_guarded_alias(value: Input) -> Base:
  match value:
    case Base(expr) if keep_base(expr):
      return expr
    case Base(expr):
      return expr
    case int(number):
      return A(value=str(number))

def from_fallback(value: Input) -> Base:
  match value:
    int(number) =>
      return A(value=str(number))
    other =>
      return other
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn test_guarded_union_alias_patterns_do_not_satisfy_exhaustiveness() {
    let source = r#"
model A:
  value: str

model B:
  value: str

type Base = Union[A, B]
type Input = Union[Base, int]

def keep_base(value: Base) -> bool:
  return true

def guarded_only(value: Input) -> Base:
  match value:
    case Base(expr) if keep_base(expr):
      return expr
    case int(number):
      return A(value=str(number))
"#;
    let errors = check_str_err(source, "guarded union alias patterns should not prove coverage");
    assert!(
        errors
            .iter()
            .any(|error| error.message.to_lowercase().contains("non-exhaustive")),
        "expected non-exhaustive union match diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_union_match_requires_exhaustive_type_patterns() {
    let source = r#"
def normalize(value: int | str) -> str:
  match value:
    int(n) =>
      return "number"
  return "fallback"
"#;
    let errors = check_str_err(source, "missing union match arm should be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("non-exhaustive") || error.message.contains("str")),
        "expected non-exhaustive union match diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_union_clone_method_typechecks_when_members_are_cloneable() {
    let source = r#"
@derive(Clone)
model Leaf:
  value: int

@derive(Clone)
model Pair:
  args: List[Expr]

type Expr = Union[Leaf, Pair]

def clone_expr(expr: Expr) -> Expr:
  return expr.clone()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_union_model_variants_reject_direct_recursive_payload_without_indirection() {
    let source = r#"
@derive(Clone)
model Leaf:
  value: int

@derive(Clone)
model Pair:
  left: Expr
  right: Expr

type Expr = Union[Leaf, Pair]
"#;
    let errors = check_str_err(source, "direct recursive union model payload should be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("direct recursive") && error.message.contains("Pair")),
        "expected direct recursive model diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_match_pattern_alternation_typechecks_and_counts_exhaustiveness() {
    let source = r#"
enum Status:
  Pending
  Retrying
  Done

def label(status: Status) -> str:
  match status:
    Status.Pending | Status.Retrying =>
      return "waiting"
    Status.Done =>
      return "done"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_if_let_pattern_alternation_typechecks_common_binding() {
    let source = r#"
def first(result: Result[int, int]) -> int:
  if let Ok(value) | Err(value) = result:
    return value
  return 0
"#;
    assert!(check_str(source).is_ok());
}

/// The checked type of every pattern node is recorded at that node's span (#1245), so lowering can read a
/// destructured binding's declared payload type back instead of re-deriving it. One test covers the three
/// constructs that share the pattern walk plus the `assert value is P` subset, which defines its binding apart.
#[test]
fn pattern_nodes_record_their_checked_type_at_their_span_issue1245() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
enum Shape:
  Circle(int)
  Label(str)

def size(s: Shape, o: Option[str]) -> int:
  match s:
    case Shape.Circle(radius):
      return radius
    case Shape.Label(text):
      return len(text)
  if let Some(found) = o:
    return len(found)
  assert o is Some(asserted)
  return len(asserted)
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();

    let binding_span = |name: &str| -> Result<Span, Box<dyn std::error::Error>> {
        let needle = format!("({name})");
        let start = source
            .find(&needle)
            .ok_or_else(|| format!("fixture must spell `{needle}`"))?
            + 1;
        Ok(Span::new(start, start + name.len()))
    };
    for (name, expected) in [
        ("radius", ResolvedType::Int),
        ("text", ResolvedType::Str),
        ("found", ResolvedType::Str),
        ("asserted", ResolvedType::Str),
    ] {
        assert_eq!(
            info.expr_type(binding_span(name)?),
            Some(&expected),
            "the checked payload type must be recorded at `{name}`'s own span"
        );
    }
    // The constructor node itself carries the scrutinee type, so nested sub-patterns can be checked against it.
    let circle_start = source
        .find("Shape.Circle(radius)")
        .ok_or("fixture must spell the Circle pattern")?;
    assert_eq!(
        info.expr_type(Span::new(circle_start, circle_start + "Shape.Circle(radius)".len())),
        Some(&ResolvedType::Named("Shape".to_string())),
        "a constructor pattern node records the type it was checked against"
    );
    Ok(())
}

/// A tuple pattern binds its names over both tuple spellings: the `(A, B)` form that infers `ResolvedType::Tuple`
/// and the written `tuple[A, B]` annotation that resolves as `Generic("Tuple", …)`. Before #1714 only the first
/// spelling was destructured, so the bindings of a `match` over a `tuple[int, str]`-typed value were never
/// defined; the element types recorded at each binding's own span prove the sub-patterns are now visited.
#[test]
fn tuple_pattern_binds_over_a_written_tuple_annotation_issue1714() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def describe(pair: tuple[int, str]) -> str:
  match pair:
    (0, _) =>
      return "zero"
    (number, word) =>
      return f"{number + 1} {word.upper()}"
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();

    let pattern_start = source
        .find("(number, word)")
        .ok_or("fixture must spell the tuple pattern")?;
    for (name, offset, expected) in [("number", 1, ResolvedType::Int), ("word", 9, ResolvedType::Str)] {
        let start = pattern_start + offset;
        assert_eq!(
            info.expr_type(Span::new(start, start + name.len())),
            Some(&expected),
            "the element type must be recorded at `{name}`'s own span"
        );
    }
    Ok(())
}

/// A model or class pattern that names a subset of the fields records the canonical fields it leaves unnamed at
/// the constructor name's span, in declaration order and with aliases resolved (#1708); a pattern that names every
/// field records nothing.
#[test]
fn partial_constructor_pattern_records_its_rest_fields_issue1708() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
model Account:
  type_ [alias="type"]: str
  tier: int
  name: str

def describe(a: Account) -> str:
  match a:
    Account(type="premium") =>
      return "premium"
    Account(tier=1, name=n, type=t) =>
      return t
    _ =>
      return "other"
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();

    let name_span = |pattern: &str| -> Result<Span, Box<dyn std::error::Error>> {
        let start = source
            .find(pattern)
            .ok_or_else(|| format!("fixture must spell `{pattern}`"))?;
        Ok(Span::new(start, start + "Account".len()))
    };
    assert_eq!(
        info.pattern_rest_fields(name_span("Account(type=\"premium\")")?),
        Some(["tier".to_string(), "name".to_string()].as_slice()),
        "the fields the partial pattern leaves unnamed are recorded in declaration order"
    );
    assert_eq!(
        info.pattern_rest_fields(name_span("Account(tier=1, name=n, type=t)")?),
        None,
        "a pattern naming every field, aliases included, records no rest"
    );
    Ok(())
}

/// A partial pattern whose unnamed fields include a private field it may not name, one outside a method of the owning
/// model, is recorded as such (#1740), so lowering covers its rest without naming that field. Inside the owner's own
/// method the same field is nameable, and a rest of public fields is never recorded as private.
#[test]
fn partial_constructor_pattern_records_a_rest_holding_a_private_field_issue1740()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
pub model Account:
  pub kind: str
  pub tier: int
  _secret: int

  def shares_secret_with(self, other: Account) -> bool:
    match other:
      Account(_secret=0) =>
        return true
      _ =>
        return false

pub model Plan:
  pub name: str
  pub seats: int

def describe(account: Account) -> str:
  match account:
    Account(kind="premium") =>
      return "premium"
    _ =>
      return "other"

def plan_name(plan: Plan) -> str:
  match plan:
    Plan(name="team") =>
      return "team"
    _ =>
      return "other"
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();

    let name_span = |pattern: &str, name: &str| -> Result<Span, Box<dyn std::error::Error>> {
        let start = source
            .find(pattern)
            .ok_or_else(|| format!("fixture must spell `{pattern}`"))?;
        Ok(Span::new(start, start + name.len()))
    };
    let outside = name_span("Account(kind=\"premium\")", "Account")?;
    assert_eq!(
        info.pattern_rest_fields(outside),
        Some(["tier".to_string(), "_secret".to_string()].as_slice()),
        "the rest still lists every unnamed field in declaration order"
    );
    assert!(
        info.pattern_rest_has_private_fields(outside),
        "outside the owner's methods, a rest holding a private field is recorded as one"
    );

    let inside = name_span("Account(_secret=0)", "Account")?;
    assert_eq!(
        info.pattern_rest_fields(inside),
        Some(["kind".to_string(), "tier".to_string()].as_slice())
    );
    assert!(
        !info.pattern_rest_has_private_fields(inside),
        "inside the owner's method the rest holds only public fields"
    );

    let public_rest = name_span("Plan(name=\"team\")", "Plan")?;
    assert_eq!(
        info.pattern_rest_fields(public_rest),
        Some(["seats".to_string()].as_slice())
    );
    assert!(
        !info.pattern_rest_has_private_fields(public_rest),
        "a rest of public fields is not recorded as private"
    );
    Ok(())
}

#[test]
fn test_pattern_alternation_rejects_missing_binding() {
    let source = r#"
def first(result: Result[int, int]) -> int:
  if let Ok(value) | Err(_) = result:
    return value
  return 0
"#;
    let errors = check_str_err(source, "pattern alternation with missing binding should be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Pattern alternation binding mismatch")),
        "expected binding mismatch diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pattern_alternation_rejects_different_binding_names() {
    let source = r#"
def first(result: Result[int, int]) -> int:
  if let Ok(value) | Err(error) = result:
    return value
  return 0
"#;
    let errors = check_str_err(
        source,
        "pattern alternation with different binding names should be rejected",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Pattern alternation binding mismatch")),
        "expected binding mismatch diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pattern_alternation_rejects_different_binding_types() {
    let source = r#"
def describe(value: int | str) -> str:
  match value:
    int(item) | str(item) =>
      return str(item)
"#;
    let errors = check_str_err(
        source,
        "pattern alternation with different binding types should be rejected",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("has incompatible types")),
        "expected binding type mismatch diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_try_on_non_result() {
    let source = r#"
def foo() -> Result[int, str]:
  x = 42
  y = x?
  return Ok(y)
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_try_requires_result_return_type() {
    let source = r#"
def foo() -> int:
  x: Result[int, str] = Ok(42)
  return x?
"#;
    let errors = check_str_err(source, "try in non-Result function should fail typechecking");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("enclosing function does not return Result")),
        "expected non-Result enclosing function diagnostic, got {errors:?}"
    );
}

#[test]
fn test_try_does_not_cross_closure_boundary() {
    let source = r#"
def parse_value() -> Result[int, str]:
  return Ok(42)

def foo() -> Result[int, str]:
  callback = () => parse_value()?
  return Ok(callback())
"#;
    let errors = check_str_err(
        source,
        "try in closure should not target enclosing Result-returning function",
    );
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("enclosing function does not return Result")),
        "expected closure boundary diagnostic, got {errors:?}"
    );
}

#[test]
fn test_if_let_rejects_impossible_pattern() {
    let source = r#"
def first(count: int) -> int:
  if let Some(value) = count:
    return value
  return 0
"#;
    let errs = check_str_err(source, "expected impossible `if let` pattern to fail");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Constructor pattern 'Some' does not resolve")),
        "unexpected errors: {errs:?}"
    );
}

#[test]
fn test_option_some() {
    let source = r#"
def foo() -> Option[int]:
  return Some(42)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_option_none() {
    let source = r#"
def foo() -> Option[int]:
  return None
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_none_ok_literal() {
    let source = r#"
def ping() -> Result[None, str]:
  return Ok(None)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_option_match_exhaustive_some_none() {
    let source = r#"
def foo(value: Option[int]) -> int:
  match value:
    case Some(n):
      return n
    case None:
      return 0
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_ok() {
    let source = r#"
def foo() -> Result[int, str]:
  return Ok(42)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_err() {
    let source = r#"
def foo() -> Result[int, str]:
  return Err("error")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_ok_reports_payload_type_mismatch() {
    let source = r#"
def foo() -> Result[int, str]:
  return Ok("hello")
"#;
    let Err(errs) = check_str(source) else {
        panic!("Ok payload type mismatch should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Result[int, str]") && e.message.contains("Result[str, str]")),
        "Expected Result payload mismatch; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_result_err_reports_payload_type_mismatch() {
    let source = r#"
def foo() -> Result[int, str]:
  return Err(1)
"#;
    let Err(errs) = check_str(source) else {
        panic!("Err payload type mismatch should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Result[int, str]") && e.message.contains("Result[int, int]")),
        "Expected Result payload mismatch; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_match_expression() {
    let source = r#"
def foo(x: int) -> str:
  match x:
    0 => "zero"
    1 => "one"
    _ => "other"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_match_unknown_incan_enum_variant_reports_constructor_resolution_error() {
    let source = r#"
enum Traffic:
  Red
  Amber

def f(x: Traffic) -> None:
  match x:
    Crimson() =>
      _ = 0
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors for unknown enum constructor pattern");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("does not resolve for this match")),
        "expected unknown_match_constructor_pattern, got {errs:?}"
    );
}

#[test]
fn test_match_qualified_incan_enum_variant_resolves_against_scrutinee() {
    let source = r#"
pub enum ConformanceRel:
  Read
  Filter
  Project

pub def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    ConformanceRel.Read =>
      return "ReadRel"
    ConformanceRel.Filter =>
      return "FilterRel"
    ConformanceRel.Project =>
      return "ProjectRel"
    _ =>
      return "UnknownRel"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_match_qualified_incan_enum_variant_with_wrong_qualifier_reports_resolution_error() {
    let source = r#"
enum ConformanceRel:
  Read
  Filter

enum OtherRel:
  Read

def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    OtherRel.Read =>
      return "ReadRel"
    _ =>
      return "UnknownRel"
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors for mismatched enum constructor qualifier");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("does not resolve for this match")),
        "expected unknown_match_constructor_pattern, got {errs:?}"
    );
}

#[test]
fn test_match_qualified_incan_enum_variant_stays_resolvable_with_duplicate_variant_names() {
    let source = r#"
enum ConformanceRel:
  Read
  Filter

enum OtherRel:
  Read

def relation_kind_name_from_conformance(rel: ConformanceRel) -> str:
  match rel:
    ConformanceRel.Read =>
      return "ReadRel"
    ConformanceRel.Filter =>
      return "FilterRel"
    _ =>
      return "UnknownRel"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_match_qualified_incan_enum_variant_uses_enum_owned_payload_metadata() {
    let source = r#"
enum Packet:
  Bool(bool)
  String(str)

enum OtherKind(str):
  Bool = "bool"
  String = "string"

def packet_name(packet: Packet) -> str:
  match packet:
    Packet.Bool(flag) =>
      if flag:
        return "true"
      return "false"
    Packet.String(value) =>
      return value
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_enum_variant_does_not_shadow_existing_same_scope_type_binding() {
    let source = r#"
class Sha256:
  @staticmethod
  def default() -> int:
    return 256

enum Algorithm(str):
  Sha256 = "sha256"
  Md5 = "md5"

def selected_algorithm_name(algorithm: Algorithm) -> str:
  match algorithm:
    Algorithm.Sha256 =>
      return "sha256"
    Algorithm.Md5 =>
      return "md5"

def default_value() -> int:
  return Sha256.default()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_rfc070_result_combinators_typecheck() -> Result<(), Vec<CompileError>> {
    let source = r#"
def double(value: int) -> int:
  return value * 2

def prefix_error(err: str) -> str:
  return "error: " + err

def keep_positive(value: int) -> Result[int, str]:
  if value > 0:
    return Ok(value)
  return Err("not positive")

def recover(_err: str) -> Result[int, int]:
  return Ok(0)

def observe_int(_value: int) -> None:
  pass

def observe_err(_err: str) -> None:
  pass

from std.traits.callable import Callable1

model Observer with Callable1[int, None]:
  def __call__(self, value: int) -> None:
    pass

def main(result: Result[int, str]) -> None:
  observer = Observer()
  mapped: Result[int, str] = result.map(double)
  mapped_err: Result[int, str] = result.map_err(prefix_error)
  chained: Result[int, str] = result.and_then(keep_positive)
  recovered: Result[int, int] = result.or_else(recover)
  inspected: Result[int, str] = result.inspect(observe_int).inspect(observer)
  inspected_err: Result[int, str] = result.inspect_err(observe_err)
"#;

    check_str(source)
}

#[test]
fn test_result_unwrap_helpers_typecheck() -> Result<(), Vec<CompileError>> {
    let source = r#"
def direct(result: Result[int, str]) -> int:
  return result.unwrap()

def fallback(result: Result[int, str]) -> int:
  return result.unwrap_or(0)
"#;

    check_str(source)
}

#[test]
fn test_option_copied_accepts_generic_reference_payloads() -> Result<(), Vec<CompileError>> {
    let source = r#"
def copy_placeholder[T](value: Option[&T]) -> Option[T]:
  return value.copied()
"#;

    check_str(source)
}

#[test]
fn test_rfc070_result_combinators_reject_bad_callbacks() {
    let source = r#"
def wrong_arg(value: str) -> int:
  return 1

def not_result(value: int) -> int:
  return value

def observes_with_value(value: int) -> int:
  return value

def main(result: Result[int, str]) -> None:
  _mapped = result.map(wrong_arg)
  _chained = result.and_then(not_result)
  _inspected = result.inspect(observes_with_value)
"#;

    let errs = check_str_err(source, "bad Result combinator callbacks should fail");
    for expected in [
        "expected 'str', found 'int'",
        "expected 'Result",
        "expected 'Unit', found 'int'",
    ] {
        assert!(
            errs.iter().any(|err| err.message.contains(expected)),
            "expected diagnostic containing {expected:?}, got: {errs:?}"
        );
    }
}

#[test]
fn test_rfc068_option_and_result_are_not_truthy() {
    let source = r#"
def maybe_value() -> Option[int]:
  return None

def parse_value() -> Result[int, str]:
  return Ok(1)

def main() -> None:
  if maybe_value():
    pass
  while parse_value():
    break
  maybe_bool = bool(maybe_value())
  result_bool = bool(parse_value())
"#;

    let errs = check_str_err(source, "expected Option/Result truthiness rejection");
    for expected in [
        "expected 'bool', found 'Option[int]'",
        "expected 'bool', found 'Result[int, str]'",
        "bool() does not support type Option[int]",
        "bool() does not support type Result[int, str]",
    ] {
        assert!(
            errs.iter().any(|err| err.message.contains(expected)),
            "expected diagnostic containing {expected:?}, got: {errs:?}"
        );
    }
}
