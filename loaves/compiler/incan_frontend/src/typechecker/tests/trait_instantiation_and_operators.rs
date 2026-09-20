//! Multi-instantiation trait adoption and dispatch through it: explicit type arguments, return-type hints that
//! disambiguate (#955), cross-trait method-name collisions, RFC 028 operator dunders, RFC 068 structural protocol
//! hooks, callable-bound sources, and trait-typed mutable terminals.

use super::*;

#[test]
fn test_model_trait_adoption_rejects_wrong_explicit_type_argument_arity() {
    let source = r#"
from std.traits.convert import From

model UserId with From[int, str]:
  value: int

  @classmethod
  def from(cls, value: int) -> Self:
    return UserId(value=value)
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected trait adoption arity error");
    };
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Trait adoption 'From' expects 1 type argument(s), found 2")),
        "expected trait adoption arity diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_model_trait_adoption_instantiates_explicit_type_arguments_for_method_checks() {
    let source = r#"
from std.traits.convert import From

model UserId with From[str]:
  value: int

  @classmethod
  def from(cls, value: int) -> Self:
    return UserId(value=value)
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected trait method signature mismatch");
    };
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Trait 'From' requires 'UserId'::from to match its signature")),
        "expected trait conformance diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_multi_instantiation_trait_method_return_hint_disambiguates() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Into[T]:
  def into(self) -> T: ...

model Reading with Into[int], Into[float]:
  value: int

  def into(self) -> int:
    return self.value

  def into(self) -> float:
    return 1.0

def main() -> None:
  reading = Reading(value=1)
  precise: float = reading.into()
"#;

    check_str(source)
}

#[test]
fn test_nested_result_return_hint_disambiguates_trait_instantiation_issue955() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Read[T]:
  def read(self) -> Result[T, str]: ...

model Source with Read[u8], Read[u16]:
  value: int

  def read(self) -> Result[u8, str]:
    value: u8 = 1
    return Ok(value)

  def read(self) -> Result[u16, str]:
    value: u16 = 2
    return Ok(value)

def main() -> None:
  source = Source(value=1)
  precise: Result[u16, str] = source.read()
"#;

    check_str(source)
}

#[test]
fn test_nested_result_trait_method_without_hint_remains_ambiguous_issue955() -> Result<(), String> {
    let source = r#"
trait Read[T]:
  def read(self) -> Result[T, str]: ...

model Source with Read[u8], Read[u16]:
  value: int

  def read(self) -> Result[u8, str]:
    value: u8 = 1
    return Ok(value)

  def read(self) -> Result[u16, str]:
    value: u16 = 2
    return Ok(value)

def main() -> None:
  source = Source(value=1)
  precise = source.read()
"#;

    let errors = match check_str(source) {
        Err(errors) => errors,
        Ok(()) => {
            return Err("expected nested Result trait method call to remain ambiguous without a result hint".into());
        }
    };
    if errors
        .iter()
        .any(|error| error.message.contains("Ambiguous trait method call 'read'"))
    {
        Ok(())
    } else {
        Err(format!("expected ambiguity diagnostic, got {errors:?}"))
    }
}

#[test]
fn test_std_binary_read_uses_nested_result_hint_issue955() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.io import Endian, IoError, _BytesIO

def read_u32(reader: _BytesIO) -> Result[u32, IoError]:
  result: Result[u32, IoError] = reader.read(Endian.Big)
  return result

def read_u16(reader: _BytesIO) -> Result[u16, IoError]:
  result: Result[u16, IoError] = reader.read(Endian.Big)
  return result

def read_f64(reader: _BytesIO) -> Result[f64, IoError]:
  result: Result[f64, IoError] = reader.read(Endian.Big)
  return result
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let read_dispatches = checker
        .type_info()
        .calls
        .resolved_method_calls
        .values()
        .filter_map(|call| match &call.dispatch {
            ResolvedMethodDispatch::Trait {
                trait_name, type_args, ..
            } if call.method == "read" && trait_name == "BinaryRead" => Some(type_args),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(read_dispatches.len(), 3, "expected three exact BinaryRead dispatches");
    for expected in [
        ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::U32),
        ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::U16),
        ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::F64),
    ] {
        assert!(
            read_dispatches
                .iter()
                .any(|type_args| type_args.as_slice() == [expected.clone()]),
            "expected BinaryRead[{expected}] dispatch, got {read_dispatches:?}"
        );
    }
    Ok(())
}

#[test]
fn test_multi_instantiation_trait_method_without_hint_is_ambiguous() {
    let source = r#"
trait Into[T]:
  def into(self) -> T: ...

model Reading with Into[int], Into[float]:
  value: int

  def into(self) -> int:
    return self.value

  def into(self) -> float:
    return 1.0

def main() -> None:
  reading = Reading(value=1)
  precise = reading.into()
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected ambiguous trait method call");
    };
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Ambiguous trait method call 'into'")),
        "expected ambiguity diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_multi_instantiation_trait_method_named_argument_disambiguates() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Reader[T]:
  def read(self, value: T) -> int: ...

model Source with Reader[str], Reader[int]:
  label: str

  def read(self, value: str) -> int:
    return 1

  def read(self, value: int) -> int:
    return value

def main() -> int:
  source = Source(label="events")
  return source.read(value=2)
"#;

    check_str(source)
}

#[test]
fn test_rfc028_dunder_only_add_operator_records_resolution() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Money:
  cents: int

  def __add__(self, other: Money) -> Money:
    return Money(cents=self.cents + other.cents)

def main() -> None:
  total = Money(cents=100) + Money(cents=25)
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_operator_calls
            .values()
            .any(|call| call.method == "__add__" && call.kind == ResolvedOperatorKind::Binary),
        "expected + to resolve to __add__, got {:?}",
        checker.type_info().calls.resolved_operator_calls
    );
    Ok(())
}

#[test]
fn test_rfc028_trait_backed_operator_dispatch_typechecks() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Add[Rhs, Output]:
  def __add__(self, other: Rhs) -> Output: ...

model Money with Add[Money, Money]:
  cents: int

  def __add__(self, other: Money) -> Money:
    return Money(cents=self.cents + other.cents)

def main() -> Money:
  return Money(cents=100) + Money(cents=25)
"#;

    check_str(source)
}

#[test]
fn test_rfc028_multi_instantiation_operator_dispatch_uses_operand_type() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Add[Rhs, Output]:
  def __add__(self, other: Rhs) -> Output: ...

model Acc with Add[int, int], Add[str, str]:
  value: int

  def __add__(self, other: int) -> int:
    return self.value + other

  def __add__(self, other: str) -> str:
    return other

def main() -> None:
  acc = Acc(value=2)
  n: int = acc + 3
  s: str = acc + "x"
"#;

    check_str(source)
}

#[test]
fn test_rfc028_trait_dunder_signature_mismatch_is_rejected() {
    let source = r#"
trait Add[Rhs, Output]:
  def __add__(self, other: Rhs) -> Output: ...

model BadMoney with Add[BadMoney, int]:
  cents: int

  def __add__(self, other: BadMoney) -> BadMoney:
    return self
"#;

    let errs = check_str_err(source, "expected operator trait/dunder signature mismatch");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Trait 'Add' requires 'BadMoney'::__add__ to match its signature")),
        "expected operator trait conformance diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc028_multi_instantiation_operator_without_hint_is_ambiguous() {
    let source = r#"
trait Add[Rhs, Output]:
  def __add__(self, other: Rhs) -> Output: ...

model Acc with Add[int, int], Add[int, str]:
  value: int

  def __add__(self, other: int) -> int:
    return self.value + other

  def __add__(self, other: int) -> str:
    return "x"

def main() -> None:
  acc = Acc(value=2)
  result = acc + 3
"#;

    let errs = check_str_err(source, "expected ambiguous operator method call");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Ambiguous trait method call '__add__")),
        "expected operator ambiguity diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc028_missing_operator_hook_reports_missing_dunder() {
    let source = r#"
model Box:
  value: int

def main() -> None:
  value = Box(value=1) + Box(value=2)
"#;

    let errs = check_str_err(source, "expected missing __add__ hook");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("has no method '__add__(...)'")),
        "expected missing __add__ diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc028_compound_assignment_missing_operator_hook_reports_missing_dunder() {
    let source = r#"
model Box:
  value: int

def main() -> None:
  mut value = Box(value=1)
  value += Box(value=2)
"#;

    let errs = check_str_err(source, "expected missing compound assignment fallback hook");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("has no method '__add__(...)'")),
        "expected missing __add__ diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc028_compound_assignment_resolves_binary_dunder_fallback() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Box:
  value: int

  def __add__(self, other: Box) -> Box:
    return Box(value=self.value + other.value)

def main() -> None:
  mut value = Box(value=1)
  value += Box(value=2)
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_operator_calls
            .values()
            .any(|call| call.method == "__add__" && call.kind == ResolvedOperatorKind::Binary),
        "expected compound assignment to resolve to __add__, got {:?}",
        checker.type_info().calls.resolved_operator_calls
    );
    Ok(())
}

#[test]
fn test_rfc028_comparison_requires_exact_explicit_hook() {
    let source = r#"
model Rank:
  value: int

  def __lt__(self, other: Rank) -> bool:
    return self.value < other.value

def main() -> None:
  a = Rank(value=1)
  b = Rank(value=2)
  ok = a < b
  missing = a <= b
"#;

    let errs = check_str_err(source, "expected missing __le__ hook");
    assert!(
        errs.iter()
            .any(|err| err.message.contains("has no method '__le__(...)'")),
        "expected missing __le__ diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc028_indexing_resolves_getitem_dunder() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Row:
  value: int

  def __getitem__(self, index: int) -> int:
    return self.value + index

def main() -> int:
  row = Row(value=4)
  return row[3]
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_operator_calls
            .values()
            .any(|call| call.method == "__getitem__" && call.kind == ResolvedOperatorKind::Index),
        "expected indexing to resolve to __getitem__, got {:?}",
        checker.type_info().calls.resolved_operator_calls
    );
    Ok(())
}

#[test]
fn test_rfc028_index_assignment_resolves_setitem_dunder() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Row:
  value: int

  def __setitem__(self, index: int, value: int) -> None:
    pass

def main() -> None:
  row = Row(value=4)
  row[3] = 9
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_operator_calls
            .values()
            .any(|call| call.method == "__setitem__" && call.kind == ResolvedOperatorKind::IndexAssign),
        "expected index assignment to resolve to __setitem__, got {:?}",
        checker.type_info().calls.resolved_operator_calls
    );
    Ok(())
}

#[test]
fn test_rfc068_structural_protocol_hooks_resolve_for_syntax() -> Result<(), Vec<CompileError>> {
    let source = r#"
model Flag:
  ready: bool

  def __bool__(self) -> bool:
    return self.ready

model Bag:
  size: int

  def __len__(self) -> int:
    return self.size

  def __contains__(self, item: int) -> bool:
    return item == self.size

model CallableBox:
  seed: int

  def __call__(self, value: int) -> int:
    return self.seed + value

model CounterIter:
  value: int
  limit: int

  def __next__(self) -> Option[int]:
    if self.value < self.limit:
      return Some(self.value)
    return None

model Counter:
  limit: int

  def __iter__(self) -> CounterIter:
    return CounterIter(value=0, limit=self.limit)

def main() -> None:
  flag = Flag(ready=true)
  bag = Bag(size=3)
  callable = CallableBox(seed=4)
  if flag:
    pass
  while flag:
    break
  n = len(bag)
  present = 3 in bag
  absent = 4 not in bag
  called = callable(5)
  for item in Counter(limit=2):
    seen = item
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let calls: Vec<_> = checker
        .type_info()
        .calls
        .resolved_operator_calls
        .values()
        .map(|call| (call.method.as_str(), call.kind))
        .collect();
    for expected in [
        ("__bool__", ResolvedOperatorKind::Truthiness),
        ("__len__", ResolvedOperatorKind::Len),
        ("__contains__", ResolvedOperatorKind::Contains),
        ("__call__", ResolvedOperatorKind::Call),
    ] {
        assert!(
            calls.contains(&expected),
            "expected RFC 068 hook {:?}, got {:?}",
            expected,
            checker.type_info().calls.resolved_operator_calls
        );
    }
    assert!(
        checker
            .type_info()
            .protocols
            .iterations
            .values()
            .any(|info| info.iter_method == "__iter__"
                && info.next_method == "__next__"
                && info.item_type == ResolvedType::Int),
        "expected custom iteration metadata, got {:?}",
        checker.type_info().protocols.iterations
    );
    Ok(())
}

#[test]
fn test_source_callable_bound_accepts_capturing_closure_and_rejects_wrong_signature() -> Result<(), Vec<CompileError>> {
    let accepted = r#"
from std.traits.callable import Callable1

model Failure:
  kind: str

def apply[Mapper with (Clone, Callable1[Failure, str])](mapper: Mapper, value: Failure) -> str:
  return mapper(value)

def main() -> str:
  prefix = "item"
  return apply((error) => f"{prefix}:{error.kind}", Failure(kind="read"))
"#;
    assert_check_ok(accepted);

    let rejected = r#"
from std.traits.callable import Callable1

def apply[Mapper with Callable1[int, str]](mapper: Mapper, value: int) -> str:
  return mapper(value)

def wrong(value: str) -> str:
  return value

def main() -> str:
  return apply(wrong, 3)
"#;
    let errors = check_str_err(rejected, "Callable1 must enforce its declared input signature");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("requires 'Callable1[int, str]'")
                && error.message.contains("(str) -> str")),
        "expected callable signature mismatch, got {errors:?}"
    );
    Ok(())
}

#[test]
fn test_source_callable_bound_records_typed_rust_closure_boundary() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.traits.callable import Callable1

model Failure:
  kind: str

def apply[Mapper with Callable1[Failure, str]](mapper: Mapper, value: Failure) -> str:
  return mapper(value)

def main() -> str:
  prefix = "item"
  return apply((error) => f"{prefix}:{error.kind}", Failure(kind="read"))
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let closure = r#"(error) => f"{prefix}:{error.kind}""#;
    let start = source.find(closure).expect("fixture must contain closure");
    assert!(
        checker
            .type_info()
            .is_source_callable_closure(Span::new(start, start + closure.len())),
        "the frontend must preserve that Callable1 supplied the closure parameter type"
    );
    Ok(())
}

#[test]
fn test_source_callable_generic_invocation_records_nominal_hook_dispatch() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.traits.callable import Callable1

def apply[Mapper with Callable1[int, str]](mapper: Mapper, value: int) -> str:
  return mapper(value)
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_operator_calls
            .values()
            .any(|call| call.method == "__call__" && call.kind == ResolvedOperatorKind::Call),
        "generic source callables must retain nominal __call__ dispatch: {:?}",
        checker.type_info().calls.resolved_operator_calls
    );
    Ok(())
}

#[test]
fn test_source_callable_bound_infers_return_type_from_nominal_adoption() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.traits.callable import Callable1

trait MapperHost:
  def map[U, Mapper with Callable1[int, U]](self, mapper: Mapper) -> U: ...

model Source with MapperHost:
  def map[U, Mapper with Callable1[int, U]](self, mapper: Mapper) -> U:
    return mapper(3)

model Label with Callable1[int, str]:
  def __call__(self, value: int) -> str:
    return f"item:{value}"

def main() -> None:
  mapped = Source().map(Label())
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let expression = "Source().map(Label())";
    let start = source
        .find(expression)
        .expect("fixture must contain nominal callable call");
    let span = Span::new(start, start + expression.len());
    assert_eq!(
        checker.type_info().expr_type(span),
        Some(&ResolvedType::Str),
        "Callable1 adoption arguments must infer the method's open return type"
    );
    Ok(())
}

#[test]
fn test_trait_typed_receiver_contextualizes_callable_bound_with_concrete_owner_args() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.traits.callable import Callable1

model Failure:
  kind: str

trait ErrorStream[T, E]:
  def map_err[F with Clone, Mapper with (Clone, Callable1[E, F])](self, mapper: Mapper) -> F: ...

model Source with ErrorStream[int, Failure]:
  def map_err[F with Clone, Mapper with (Clone, Callable1[Failure, F])](self, mapper: Mapper) -> F:
    return mapper(Failure(kind="read"))

def stream() -> ErrorStream[int, Failure]:
  return Source()

def main() -> str:
  prefix = "io"
  return stream().map_err((error) => f"{prefix}:{error.kind}")
"#;

    assert_check_ok(source);
    Ok(())
}

#[test]
fn test_trait_typed_mutable_terminal_records_receiver_mutability() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait FallibleStream[T, E]:
  def collect(mut self) -> Result[list[T], E]: ...

model NumberStream with FallibleStream[int, str]:
  def collect(mut self) -> Result[list[int], str]:
    return Ok([])

def stream() -> FallibleStream[int, str]:
  return NumberStream()

def main() -> None:
  values = stream()
  _ = values.collect()
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    assert!(
        checker
            .type_info()
            .calls
            .resolved_method_calls
            .values()
            .any(|call| matches!(
                call.dispatch,
                ResolvedMethodDispatch::Trait {
                    receiver_is_mutable: true,
                    ..
                }
            )),
        "expected mut-self trait dispatch, got {:?}",
        checker.type_info().calls.resolved_method_calls
    );
    Ok(())
}

#[test]
fn test_same_trait_adapter_chain_preserves_defining_module_dispatch() {
    let receiver_span = Span::new(10, 20);
    let call_span = Span::new(10, 30);
    let mut info = TypeCheckInfo::default();
    info.record_resolved_method_call(
        receiver_span,
        "map",
        ResolvedMethodDispatch::Trait {
            trait_name: "Stream".to_string(),
            module_path: Some(vec!["streams".to_string()]),
            type_args: vec![ResolvedType::Int, ResolvedType::Str],
            implementation_type_params: Vec::new(),
            receiver_is_mutable: false,
        },
    );
    info.record_resolved_method_call(
        call_span,
        "map_err",
        ResolvedMethodDispatch::Trait {
            trait_name: "Stream".to_string(),
            module_path: None,
            type_args: vec![ResolvedType::Int, ResolvedType::Str],
            implementation_type_params: Vec::new(),
            receiver_is_mutable: false,
        },
    );

    info.inherit_same_trait_method_module(receiver_span, call_span);

    assert!(matches!(
        info.resolved_method_call(call_span).map(|call| &call.dispatch),
        Some(ResolvedMethodDispatch::Trait {
            trait_name,
            module_path: Some(module_path),
            ..
        }) if trait_name == "Stream" && module_path == &["streams".to_string()]
    ));
}

#[test]
fn test_rfc068_explicit_trait_adoption_supplies_protocol_hook() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Sized:
  def __len__(self) -> int: ...

model Bag with Sized:
  size: int

  def __len__(self) -> int:
    return self.size

def main() -> int:
  return len(Bag(size=4))
"#;

    check_str(source)
}

#[test]
fn test_rfc068_missing_protocol_hooks_are_rejected() {
    let source = r#"
model Box:
  value: int

def main() -> None:
  box_value = Box(value=1)
  if box_value:
    pass
  n = len(box_value)
  present = 1 in box_value
  called = box_value()
  for item in box_value:
    pass
"#;

    let errs = check_str_err(source, "expected missing RFC 068 protocol hooks");
    for method in ["__bool__", "__len__", "__contains__", "__call__", "__iter__"] {
        assert!(
            errs.iter()
                .any(|err| err.message.contains(&format!("has no method '{method}(...)'"))),
            "expected missing {method} diagnostic, got: {errs:?}"
        );
    }
}

#[test]
fn test_rfc068_incompatible_protocol_hooks_are_rejected() {
    let source = r#"
model BadFlag:
  value: int

  def __bool__(self) -> int:
    return self.value

model BadBag:
  value: int

  def __len__(self) -> bool:
    return true

  def __contains__(self, item: int) -> int:
    return item

model BadCounter:
  value: int

  def __iter__(self) -> BadCounter:
    return self

  def __next__(self) -> int:
    return self.value

model BadCallable:
  value: int

  def __call__(self, item: str) -> int:
    return self.value

def main() -> None:
  flag = BadFlag(value=1)
  bag = BadBag(value=1)
  counter = BadCounter(value=1)
  callable = BadCallable(value=1)
  if flag:
    pass
  n = len(bag)
  present = 1 in bag
  called = callable(1)
  for item in counter:
    pass
"#;

    let errs = check_str_err(source, "expected incompatible RFC 068 protocol hooks");
    for expected in [
        "expected 'bool', found 'int'",
        "expected 'int', found 'bool'",
        "expected 'Option[_]', found 'int'",
    ] {
        assert!(
            errs.iter().any(|err| err.message.contains(expected)),
            "expected diagnostic containing {expected:?}, got: {errs:?}"
        );
    }
    assert!(
        errs.iter()
            .any(|err| err.message.contains("expected 'str', found 'int'")),
        "expected __call__ argument mismatch diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_rfc028_extended_operator_glyphs_resolve_dunders() -> Result<(), Vec<CompileError>> {
    let source = r#"
model OpBox:
  value: int

  def __matmul__(self, other: OpBox) -> OpBox:
    return other

  def __pipe_forward__(self, other: OpBox) -> OpBox:
    return other

  def __pipe_backward__(self, other: OpBox) -> OpBox:
    return other

  def __and__(self, other: OpBox) -> OpBox:
    return other

  def __or__(self, other: OpBox) -> OpBox:
    return other

  def __xor__(self, other: OpBox) -> OpBox:
    return other

  def __lshift__(self, other: int) -> OpBox:
    return self

  def __rshift__(self, other: int) -> OpBox:
    return self

  def __invert__(self) -> OpBox:
    return self

def main() -> None:
  a = OpBox(value=1)
  b = OpBox(value=2)
  mat = a @ b
  forward = a |> b
  backward = a <| b
  anded = a & b
  ored = a | b
  xored = a ^ b
  left = a << 1
  right = a >> 1
  inverted = ~a
"#;

    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let resolved: Vec<_> = checker
        .type_info()
        .calls
        .resolved_operator_calls
        .values()
        .map(|call| call.method.as_str())
        .collect();
    for expected in [
        "__matmul__",
        "__pipe_forward__",
        "__pipe_backward__",
        "__and__",
        "__or__",
        "__xor__",
        "__lshift__",
        "__rshift__",
        "__invert__",
    ] {
        assert!(
            resolved.contains(&expected),
            "expected {expected} to resolve in {:?}",
            checker.type_info().calls.resolved_operator_calls
        );
    }

    Ok(())
}

#[test]
fn test_rfc028_primitive_bitwise_compound_assignment_typechecks() -> Result<(), Vec<CompileError>> {
    let source = r#"
def main() -> int:
  mut value = 8
  value &= 3
  value |= 4
  value ^= 1
  value <<= 2
  value >>= 1
  return value
"#;

    check_str(source)
}

#[test]
fn test_enum_multi_instantiation_trait_method_return_hint_disambiguates() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Convert[T]:
  def convert(self) -> T: ...

enum Token with Convert[int], Convert[float]:
  Number

  def convert(self) -> int:
    return 1

  def convert(self) -> float:
    return 1.0

def main() -> None:
  token: Token = Token.Number
  precise: float = token.convert()
"#;

    check_str(source)
}

#[test]
fn test_enum_multi_instantiation_trait_method_without_hint_is_ambiguous() {
    let source = r#"
trait Convert[T]:
  def convert(self) -> T: ...

enum Token with Convert[int], Convert[float]:
  Number

  def convert(self) -> int:
    return 1

  def convert(self) -> float:
    return 1.0

def main() -> None:
  token: Token = Token.Number
  precise = token.convert()
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected ambiguous enum trait method call");
    };
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Ambiguous trait method call 'convert'")),
        "expected enum ambiguity diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_enum_duplicate_identical_trait_instantiation_rejected() -> Result<(), String> {
    let source = r#"
trait Convert[T]:
  def convert(self) -> T: ...

enum Token with Convert[int], Convert[int]:
  Number

  def convert(self) -> int:
    return 1
"#;

    let Err(errs) = check_str(source) else {
        return Err("expected duplicate enum trait instantiation diagnostic".to_string());
    };
    let duplicate = errs
        .iter()
        .find(|err| {
            err.message
                .contains("Trait 'Convert' is adopted more than once with type arguments [int]")
        })
        .ok_or_else(|| format!("expected duplicate enum trait instantiation diagnostic, got: {errs:?}"))?;
    assert_eq!(
        duplicate.related_spans().len(),
        1,
        "duplicate trait adoption must retain the first adoption site"
    );
    assert_ne!(duplicate.related_spans()[0].span, duplicate.span);
    Ok(())
}

#[test]
fn test_enum_cross_trait_same_method_name_collision_rejected() {
    let source = r#"
trait JsonSerializable:
  def serialize(self) -> str: ...

trait YamlSerializable:
  def serialize(self) -> str: ...

enum Event with JsonSerializable, YamlSerializable:
  Created

  def serialize(self) -> str:
    return "created"
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected enum cross-trait method collision");
    };
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Ambiguous trait method 'serialize' from unrelated traits")),
        "expected enum cross-trait collision diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_cross_trait_same_method_name_collision_rejected() {
    let source = r#"
trait JsonSerializable:
  def serialize(self) -> str: ...

trait YamlSerializable:
  def serialize(self) -> str: ...

model Event with JsonSerializable, YamlSerializable:
  value: str

  def serialize(self) -> str:
    return self.value
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected cross-trait method collision");
    };
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Ambiguous trait method 'serialize' from unrelated traits")),
        "expected cross-trait collision diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_cross_trait_same_method_name_with_different_params_rejected_until_aliasing() {
    let source = r#"
trait ReadsInt:
  def read(self, value: int) -> int: ...

trait ReadsStr:
  def read(self, value: str) -> str: ...

model Source with ReadsStr, ReadsInt:
  label: str

  def read(self, value: str) -> str:
    return value

  def read(self, value: int) -> int:
    return value

def main() -> int:
  source = Source(label="events")
  return source.read(2)
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected cross-trait method collision");
    };
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Ambiguous trait method 'read' from unrelated traits")),
        "expected cross-trait collision diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_generic_type_parameter_bound_dispatches_through_instantiated_trait() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Serializable[F]:
  def serialize(self, format: F) -> bytes: ...

model JsonFormat:
  name: str

model Event with Serializable[JsonFormat]:
  value: str

  def serialize(self, format: JsonFormat) -> bytes:
    return b"ok"

def encode[F, T with Serializable[F]](value: T, format: F) -> bytes:
  return value.serialize(format)
"#;

    check_str(source)
}

#[test]
fn test_enum_generic_type_parameter_bound_dispatches_through_instantiated_trait() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Serializable[F]:
  def serialize(self, format: F) -> bytes: ...

model JsonFormat:
  name: str

enum Event with Serializable[JsonFormat]:
  Created

  def serialize(self, format: JsonFormat) -> bytes:
    return b"ok"

def encode[F, T with Serializable[F]](value: T, format: F) -> bytes:
  return value.serialize(format)

def main() -> bytes:
  return encode[JsonFormat, Event](Event.Created, JsonFormat(name="json"))
"#;

    check_str(source)
}

#[test]
fn test_generic_type_parameter_bound_checks_trait_type_arguments() {
    let source = r#"
trait Serializable[F]:
  def serialize(self, format: F) -> bytes: ...

model JsonFormat:
  name: str

model YamlFormat:
  name: str

model Event with Serializable[JsonFormat]:
  value: str

  def serialize(self, format: JsonFormat) -> bytes:
    return b"ok"

def encode[F, T with Serializable[F]](value: T, format: F) -> bytes:
  return value.serialize(format)

def main() -> bytes:
  return encode[YamlFormat, Event](Event(value="x"), YamlFormat(name="yaml"))
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected generic bound type-argument diagnostic");
    };
    assert!(
        errs.iter().any(|err| {
            err.message
                .contains("type parameter 'T' requires 'Serializable[YamlFormat]' but got 'Event'")
        }),
        "expected generic bound type-argument diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_enum_generic_type_parameter_bound_checks_trait_type_arguments() {
    let source = r#"
trait Serializable[F]:
  def serialize(self, format: F) -> bytes: ...

model JsonFormat:
  name: str

model YamlFormat:
  name: str

enum Event with Serializable[JsonFormat]:
  Created

  def serialize(self, format: JsonFormat) -> bytes:
    return b"ok"

def encode[F, T with Serializable[F]](value: T, format: F) -> bytes:
  return value.serialize(format)

def main() -> bytes:
  return encode[YamlFormat, Event](Event.Created, YamlFormat(name="yaml"))
"#;

    let Err(errs) = check_str(source) else {
        panic!("expected enum generic bound type-argument diagnostic");
    };
    assert!(
        errs.iter().any(|err| {
            err.message
                .contains("type parameter 'T' requires 'Serializable[YamlFormat]' but got 'Event'")
        }),
        "expected enum generic bound type-argument diagnostic, got: {errs:?}"
    );
}
