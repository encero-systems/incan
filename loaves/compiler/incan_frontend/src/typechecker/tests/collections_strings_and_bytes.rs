//! Builtin container and text surfaces: `list` / `set` constructor identity (#951, #1464), list literals and spreads,
//! slicing, element-bound methods, `dict.contains_key`, string methods and f-string spans, and `str.encode` /
//! `bytes.decode` codecs (#1668).

use super::*;

#[test]
fn set_constructor_calls_record_canonical_collection_identity_issue951() -> Result<(), String> {
    let ast = parse_program(
        r#"
def main(values: List[str]) -> None:
  lower = set(values)
  canonical = Set(values)
"#,
        "issue951 set constructor identity",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("set constructors should typecheck: {errors:?}"))?;

    let constructors = checker
        .type_info()
        .calls
        .resolved_collection_constructors
        .values()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        constructors,
        vec![CollectionTypeId::Set, CollectionTypeId::Set],
        "both accepted spellings should resolve through the canonical Set identity"
    );
    Ok(())
}

#[test]
fn user_defined_set_call_does_not_record_collection_constructor_issue951() -> Result<(), String> {
    let ast = parse_program(
        r#"
def set(values: List[str]) -> int:
  return len(values)

def main(values: List[str]) -> None:
  count = set(values)
"#,
        "issue951 shadowed set function",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("shadowing source function should typecheck: {errors:?}"))?;

    assert!(
        checker.type_info().calls.resolved_collection_constructors.is_empty(),
        "a user-defined set function must not be lowered as the Set collection constructor"
    );
    Ok(())
}

#[test]
fn set_constructor_rejects_more_than_one_source_collection_issue951() {
    let errors = check_str_err(
        r#"
def main(left: List[str], right: List[str]) -> None:
  invalid = set(left, right)
"#,
        "set() should reject multiple source collections",
    );

    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("set() expects at most 1 argument(s), got 2")),
        "expected a source diagnostic before lowering, got {errors:?}"
    );
}

/// `list(source)` records the canonical List constructor identity and types its elements by the loop-header
/// iteration rule, so lowering never treats the call as an ordinary function named `list` (#1464).
#[test]
fn list_constructor_calls_record_canonical_collection_identity_issue1464() -> Result<(), String> {
    let source = r#"
def main(values: Dict[str, int], names: list[str], text: str) -> None:
  keys = list(values.keys())
  counts = list(values.values())
  copied = list(names)
  key_view = list(values)
  characters = list(text)
  empty: list[int] = list()
"#;
    let ast = parse_program(source, "issue1464 list constructor identity");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("list constructors should typecheck: {errors:?}"))?;

    let constructors = checker
        .type_info()
        .calls
        .resolved_collection_constructors
        .values()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        constructors,
        vec![CollectionTypeId::List; 6],
        "every accepted list() spelling should resolve through the canonical List identity"
    );

    let expr_type = |call: &str| -> Result<ResolvedType, String> {
        let start = source.find(call).ok_or_else(|| format!("missing `{call}`"))?;
        checker
            .type_info()
            .expr_type(Span::new(start, start + call.len()))
            .cloned()
            .ok_or_else(|| format!("`{call}` should retain a checked type"))
    };
    let list_of = crate::typechecker::helpers::list_ty;
    assert_eq!(expr_type("list(values.keys())")?, list_of(ResolvedType::Str));
    assert_eq!(expr_type("list(values.values())")?, list_of(ResolvedType::Int));
    assert_eq!(expr_type("list(names)")?, list_of(ResolvedType::Str));
    assert_eq!(
        expr_type("list(values)")?,
        list_of(ResolvedType::Str),
        "a dict yields its keys"
    );
    assert_eq!(expr_type("list(text)")?, list_of(ResolvedType::Str));
    assert_eq!(
        expr_type("list()")?,
        list_of(ResolvedType::Int),
        "an empty list() adopts the annotated element type"
    );
    Ok(())
}

#[test]
fn list_constructor_rejects_sources_iteration_rejects_issue1464() {
    let errors = check_str_err(
        r#"
def main(count: int, pair: (int, str)) -> None:
  from_scalar = list(count)
  from_tuple = list(pair)
"#,
        "list() should reject sources a loop cannot iterate",
    );

    assert!(
        errors.iter().any(|error| error
            .message
            .contains("list() expects an iterable collection, str, bytes, or Iterator, got int")),
        "expected a source diagnostic for the scalar before lowering, got {errors:?}"
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("list() expects an iterable collection, str, bytes, or Iterator, got (int, str)")),
        "expected a source diagnostic for the tuple before lowering, got {errors:?}"
    );
}

#[test]
fn list_constructor_rejects_more_than_one_source_issue1464() {
    let errors = check_str_err(
        r#"
def main(left: List[str], right: List[str]) -> None:
  invalid = list(left, right)
"#,
        "list() should reject multiple sources",
    );

    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("list() expects at most 1 argument(s), got 2")),
        "expected a source diagnostic before lowering, got {errors:?}"
    );
}

#[test]
fn user_defined_list_call_does_not_record_collection_constructor_issue1464() -> Result<(), String> {
    let ast = parse_program(
        r#"
def list(values: List[str]) -> int:
  return len(values)

def main(values: List[str]) -> None:
  count = list(values)
"#,
        "issue1464 shadowed list function",
    );
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("shadowing source function should typecheck: {errors:?}"))?;

    assert!(
        checker.type_info().calls.resolved_collection_constructors.is_empty(),
        "a user-defined list function must not be lowered as the List collection constructor"
    );
    Ok(())
}

#[test]
fn test_fstring_unknown_symbol_span_points_to_interpolation() {
    let source = "def foo() -> str:\n  return f\"value: {unknown_var}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => {
            panic!("Expected typechecker error for unknown symbol in f-string interpolation")
        }
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_var'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_var; got: {errors:?}"),
    };

    let expected_start = match source.find("{unknown_var}") {
        Some(start) => start,
        None => panic!("Expected interpolation segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "{unknown_var}".len());
}

#[test]
fn test_fstring_nested_unknown_symbol_span_rebased() {
    let source = "def foo(x: int) -> str:\n  return f\"sum: {x + unknown_var}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => panic!("Expected typechecker error for nested unknown symbol in f-string interpolation"),
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_var'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_var; got: {errors:?}"),
    };

    let expected_start = match source.find("unknown_var") {
        Some(start) => start,
        None => panic!("Expected unknown symbol segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "unknown_var".len());
}

#[test]
fn test_fstring_unknown_symbol_span_in_index_method_chain() {
    let source = "def foo(users: List[str]) -> str:\n  return f\"value: {users[unknown_idx].upper()}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => panic!("Expected typechecker error for unknown symbol in index interpolation"),
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_idx'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_idx; got: {errors:?}"),
    };

    let expected_start = match source.find("unknown_idx") {
        Some(start) => start,
        None => panic!("Expected unknown symbol segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "unknown_idx".len());
}

#[test]
fn test_fstring_unknown_symbol_span_in_list_comp_filter_call() {
    let source = "def foo(items: List[int]) -> str:\n  return f\"value: {[x for x in items if unknown_pred(x)]}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => panic!("Expected typechecker error for unknown symbol in list comp interpolation"),
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_pred'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_pred; got: {errors:?}"),
    };

    let expected_start = match source.find("unknown_pred") {
        Some(start) => start,
        None => panic!("Expected unknown symbol segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "unknown_pred".len());
}

#[test]
fn test_string_return() {
    let source = r#"
def foo() -> str:
  return "hello"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_string_concat() {
    let source = r#"
def foo() -> str:
  return "hello" + " world"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_slice_rejects_non_int_bounds_and_step() {
    let source = r#"
def main() -> None:
  xs: List[int] = [1, 2, 3]
  _a = xs["bad":]
  _b = xs[:1.2]
  _c = xs[0:2:"nope"]
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_list_slice_accepts_int_bounds_and_step() {
    let source = r#"
def main() -> None:
  xs: List[int] = [1, 2, 3]
  _a = xs[0:]
  _b = xs[:2]
  _c = xs[0:2:1]
"#;
    assert!(check_str(source).is_ok());
}

// FIXME(#121): `List[Mutex].append(value)` should become valid once implicit ownership
// inference can choose move/borrow over Clone-by-default for external Rust types.
#[test]
fn test_list_append_requires_clone_for_external_type() {
    let source = r#"
from rust::std::sync import Mutex

def add(mut xs: List[Mutex], value: Mutex) -> None:
  xs.append(value)
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter().any(|e| {
            e.message.contains("List.append requires element type")
                && e.message.contains("Mutex")
                && e.message.contains(incan_lang::lang::traits::as_str(
                    incan_lang::lang::traits::TraitId::Clone,
                ))
        }),
        "expected List.append / Clone diagnostic for Rust element type; got {errs:?}"
    );
}

#[test]
fn test_list_append_accepts_clone_bound_type_param() {
    let source = r#"
def add_item[T with Clone](mut items: List[T], item: T) -> None:
  items.append(item)
"#;
    assert_check_ok(source);
}

#[test]
fn test_list_repeat_infers_list_element_type() {
    let source = r#"
def main() -> None:
  xs: List[int] = list.repeat(-1, 3)
  ys: list[str] = list.repeat("seed", 2)
  zs: list[int] = list.repeat(count=2, value=7)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_repeat_u8_can_initialize_bytes() {
    let source = r#"
def zeros(size: int) -> bytes:
  zero: u8 = 0
  return list.repeat(zero, size)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_repeat_rejects_wrong_arity() {
    let source = r#"
def main() -> None:
  xs = list.repeat(1)
"#;
    let errors = check_str_err(source, "expected list.repeat arity error");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("list.repeat") && err.message.contains("expects 2")),
        "expected list.repeat arity diagnostic; got {errors:?}"
    );
}

#[test]
fn test_list_repeat_rejects_non_int_count() {
    let source = r#"
def main() -> None:
  xs = list.repeat(1, "two")
"#;
    let errors = check_str_err(source, "expected list.repeat count type error");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("expected 'int'") && err.message.contains("found 'str'")),
        "expected count type mismatch diagnostic; got {errors:?}"
    );
}

#[test]
fn test_list_repeat_requires_clone_for_external_type() {
    let source = r#"
from rust::std::sync import Mutex

def make(value: Mutex) -> List[Mutex]:
  return list.repeat(value, 2)
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter().any(|e| {
            e.message.contains("list.repeat requires element type")
                && e.message.contains("Mutex")
                && e.message.contains(incan_lang::lang::traits::as_str(
                    incan_lang::lang::traits::TraitId::Clone,
                ))
        }),
        "expected list.repeat / Clone diagnostic for Rust element type; got {errs:?}"
    );
}

#[test]
fn test_list_concat_requires_clone_for_external_type() {
    let source = r#"
from rust::std::sync import Mutex

def combine(a: List[Mutex], b: List[Mutex]) -> List[Mutex]:
  return a + b
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter().any(|e| {
            e.message.contains("List concatenation requires element type")
                && e.message.contains("Mutex")
                && e.message.contains(incan_lang::lang::traits::as_str(
                    incan_lang::lang::traits::TraitId::Clone,
                ))
        }),
        "expected List + List / Clone diagnostic for Rust element type; got {errs:?}"
    );
}

#[test]
fn test_list_extend_requires_clone_for_external_type() {
    let source = r#"
from rust::std::sync import Mutex

def extend_into(mut xs: List[Mutex], other: List[Mutex]) -> None:
  xs.extend(other)
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter().any(|e| {
            e.message.contains("List.extend requires element type")
                && e.message.contains("Mutex")
                && e.message.contains(incan_lang::lang::traits::as_str(
                    incan_lang::lang::traits::TraitId::Clone,
                ))
        }),
        "expected List.extend / Clone diagnostic for Rust element type; got {errs:?}"
    );
}

#[test]
fn test_list_clone_accepts_clone_element_type() {
    let source = r#"
@derive(Clone)
model Node:
  id: int

def clone_nodes(nodes: List[Node]) -> List[Node]:
  return nodes.clone()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_clone_accepts_clone_bound_type_param() {
    let source = r#"
def clone_items[T with Clone](items: List[T]) -> List[T]:
  return items.clone()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_clone_requires_clone_for_external_type() {
    let source = r#"
from rust::std::sync import Mutex

def clone_mutexes(xs: List[Mutex]) -> List[Mutex]:
  return xs.clone()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter().any(|e| {
            e.message.contains("List.clone requires element type")
                && e.message.contains("Mutex")
                && e.message.contains(shadowed_trait_name().as_str())
        }),
        "expected List.clone / Clone diagnostic for Rust element type; got {errs:?}"
    );
}

#[test]
fn test_list_literal() {
    let source = r#"
def foo() -> List[int]:
  return [1, 2, 3]
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_collection_literal_spreads_typecheck() {
    let source = r#"
def values(xs: list[int]) -> list[int]:
  xy: tuple[int, int] = (2, 3)
  return [1, *xs, *xy, *(5, 6)]

def headers(defaults: dict[str, str], overrides: dict[str, str]) -> dict[str, str]:
  return {**defaults, "trace": "enabled", **overrides}
"#;
    assert_check_ok(source);
}

#[test]
fn test_collection_literal_spread_type_mismatches_are_reported() {
    let list_source = r#"
def bad_list(xs: list[str]) -> list[int]:
  return [1, *xs]
"#;
    let list_errs = check_str_err(list_source, "expected list spread type mismatch");
    let list_messages: Vec<&str> = list_errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        list_messages
            .iter()
            .any(|msg| msg.contains("expected 'int', found 'str'")),
        "expected list spread element mismatch, got: {list_messages:?}"
    );

    let value_source = r#"
def bad_dict_values(headers: dict[str, int]) -> dict[str, str]:
  return {"accept": "json", **headers}
"#;
    let value_errs = check_str_err(value_source, "expected dict spread value mismatch");
    let value_messages: Vec<&str> = value_errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        value_messages
            .iter()
            .any(|msg| msg.contains("expected 'str', found 'int'")),
        "expected dict spread value mismatch, got: {value_messages:?}"
    );

    let key_source = r#"
def bad_dict_keys(headers: dict[int, str]) -> dict[str, str]:
  return {"accept": "json", **headers}
"#;
    let key_errs = check_str_err(key_source, "expected dict spread key mismatch");
    let key_messages: Vec<&str> = key_errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        key_messages
            .iter()
            .any(|msg| msg.contains("expected 'str', found 'int'")),
        "expected dict spread key mismatch, got: {key_messages:?}"
    );
}

#[test]
fn test_collection_literal_spread_requires_matching_container_shape() {
    let source = r#"
def bad_list(xs: dict[str, str]) -> list[int]:
  return [1, *xs]

def bad_dict(xs: list[int]) -> dict[str, str]:
  return {**xs}

def bad_frozen_list(xs: FrozenList[int]) -> list[int]:
  return [*xs]

def bad_frozen_dict(xs: FrozenDict[FrozenStr, int]) -> dict[str, int]:
  return {**xs}
"#;
    let errs = check_str_err(source, "expected spread shape mismatches");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    assert!(
        messages
            .iter()
            .any(|msg| msg.contains("expected 'List[_] or tuple[...]'")),
        "expected list spread container diagnostic, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|msg| msg.contains("expected 'Dict[_, _]'")),
        "expected dict spread container diagnostic, got: {messages:?}"
    );
}

#[test]
fn test_collection_literal_spread_invalid_markers_are_targeted() {
    let list_errs = check_str_err(
        "def f(xs: list[int]) -> None:\n  values = [**xs]\n",
        "expected invalid list marker diagnostic",
    );
    assert!(
        list_errs
            .iter()
            .any(|err| err.message.contains("Invalid list spread marker `**`")),
        "expected invalid list spread marker diagnostic, got: {list_errs:?}"
    );

    let dict_errs = check_str_err(
        "def f(xs: list[int]) -> None:\n  values = {*xs}\n",
        "expected invalid dict marker diagnostic",
    );
    assert!(
        dict_errs
            .iter()
            .any(|err| err.message.contains("Invalid dictionary spread marker `*`")),
        "expected invalid dictionary spread marker diagnostic, got: {dict_errs:?}"
    );
}

#[test]
fn test_empty_list() {
    let source = r#"
def foo() -> List[int]:
  let x: List[int] = []
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_empty_list_matches_typed_call_parameter() {
    let source = r#"
def takes_names(names: List[str]) -> int:
  return len(names)

def foo() -> int:
  return takes_names([])
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_concatenation_with_plus() {
    let source = r#"
def foo() -> List[int]:
  a: List[int] = [1, 2]
  b: List[int] = [3, 4]
  return a + b
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_extend_method() {
    let source = r#"
def foo(mut a: List[int], b: List[int]) -> None:
  a.extend(b)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_self_accepts_explicit_owner_instances() {
    let source = r#"
pub class Boxed[T]:
  pub value: T

  def pair(self) -> List[Self]:
    return [Boxed(value=self.value), Boxed(value=self.value)]
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_string_methods_typecheck() {
    let source = r#"
def foo() -> str:
  return "hello world".upper().strip()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_str_encode_and_bytes_decode_accept_utf8_round_trip_issue1668() {
    let source = r#"
const GREETING: FrozenStr = "héllo"
const RAW: FrozenBytes = b"raw"

def encode_forms(text: str, label: str) -> int:
    plain: bytes = text.encode()
    explicit: bytes = text.encode("utf-8")
    named: bytes = text.encode(encoding="UTF_8")
    runtime: bytes = text.encode(label)
    frozen: bytes = GREETING.encode()
    return len(plain) + len(explicit) + len(named) + len(runtime) + len(frozen)

def decode_forms(data: bytes, label: str, policy: str) -> Result[str, ValidationError]:
    plain: str = data.decode()?
    lossy: str = data.decode(errors="replace")?
    positional: str = data.decode("utf8", "strict")?
    runtime: str = data.decode(label, errors=policy)?
    frozen: str = RAW.decode()?
    attempt: Result[str, ValidationError] = data.decode()
    match attempt:
        Ok(text) => println(text)
        Err(error) => println(f"{error}")
    return Ok(plain + lossy + positional + runtime + frozen)
"#;
    assert_check_ok(source);
}

#[test]
fn test_bytes_decode_returns_result_not_str_issue1668() {
    let errors = check_str_err(
        r#"
def text(data: bytes) -> str:
    decoded: str = data.decode()
    return decoded
"#,
        "bytes.decode() returns Result[str, ValidationError], not str",
    );
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("expected 'str', found 'Result[str, ValidationError]'")),
        "expected a Result mismatch diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_str_encode_rejects_unsupported_literal_encoding_issue1668() {
    let errors = check_str_err(
        r#"
def payload(text: str) -> bytes:
    return text.encode("latin-1")
"#,
        "str.encode with a non-UTF-8 literal must fail typechecking",
    );
    assert!(
        errors.iter().any(
            |error| error.message.contains("str.encode() supports only UTF-8") && error.message.contains("latin-1")
        ),
        "expected an unsupported-encoding diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_bytes_decode_rejects_unsupported_literal_encoding_issue1668() {
    let errors = check_str_err(
        r#"
def text(data: bytes) -> str:
    return data.decode("latin-1")
"#,
        "bytes.decode with a non-UTF-8 literal must fail typechecking",
    );
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("bytes.decode() supports only UTF-8")
                && error.message.contains("latin-1")),
        "expected an unsupported-encoding diagnostic, got: {:?}",
        errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_text_codec_calls_reject_foreign_keyword_and_duplicate_label_issue1668() {
    let keyword_errors = check_str_err(
        r#"
def payload(text: str) -> bytes:
    return text.encode(errors="strict")
"#,
        "str.encode has no errors policy",
    );
    assert!(
        keyword_errors
            .iter()
            .any(|error| error.message.contains("Unexpected keyword argument 'errors'")),
        "expected an unknown-keyword diagnostic, got: {:?}",
        keyword_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let duplicate_errors = check_str_err(
        r#"
def text(data: bytes) -> str:
    return data.decode("utf-8", encoding="utf-8")
"#,
        "bytes.decode must not bind encoding twice",
    );
    assert!(
        duplicate_errors
            .iter()
            .any(|error| error.message.contains("Duplicate argument 'encoding'")),
        "expected a duplicate-argument diagnostic, got: {:?}",
        duplicate_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_bytes_decode_rejects_bad_policy_keyword_and_arity_issue1668() {
    let policy_errors = check_str_err(
        r#"
def text(data: bytes) -> str:
    return data.decode(errors="ignore")
"#,
        "bytes.decode with an unsupported errors policy must fail typechecking",
    );
    assert!(
        policy_errors
            .iter()
            .any(|error| error.message.contains("errors must be") && error.message.contains("ignore")),
        "expected an unsupported-policy diagnostic, got: {:?}",
        policy_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let keyword_errors = check_str_err(
        r#"
def text(data: bytes) -> str:
    return data.decode(codec="utf-8")
"#,
        "bytes.decode with an unknown keyword must fail typechecking",
    );
    assert!(
        keyword_errors
            .iter()
            .any(|error| error.message.contains("Unexpected keyword argument 'codec'")),
        "expected an unknown-keyword diagnostic, got: {:?}",
        keyword_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let arity_errors = check_str_err(
        r#"
def payload(text: str) -> bytes:
    return text.encode("utf-8", "strict")
"#,
        "str.encode takes at most one argument",
    );
    assert!(
        arity_errors.iter().any(|error| error
            .message
            .contains("str.encode() expects at most 1 argument(s), got 2")),
        "expected a max-arity diagnostic, got: {:?}",
        arity_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let type_errors = check_str_err(
        r#"
def payload(text: str) -> bytes:
    return text.encode(8)
"#,
        "str.encode requires a text encoding label",
    );
    assert!(
        type_errors.iter().any(|error| error
            .message
            .contains("Argument 'encoding' of 'str.encode' has type mismatch")),
        "expected an argument type diagnostic, got: {:?}",
        type_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_dict_contains_key_typechecks_on_mutable_dict_issue1668() {
    let source = r#"
def has_manifest(files: Dict[str, str]) -> bool:
    return files.contains_key("loaf.toml")

def has_id(mut counts: Dict[int, int], id: int) -> bool:
    return counts.contains_key(id)

def keys_outside_comprehension(d: Dict[str, int]) -> int:
    names: list[str] = sorted(d.keys())
    present: bool = "a" in d.keys()
    values: list[int] = d.values()
    return len(names) + len(values)
"#;
    assert_check_ok(source);
}

#[test]
fn test_dict_contains_key_rejects_arity_and_key_type_issue1668() {
    let arity_errors = check_str_err(
        r#"
def has_manifest(files: Dict[str, str]) -> bool:
    return files.contains_key()
"#,
        "Dict.contains_key requires exactly one key argument",
    );
    assert!(
        arity_errors.iter().any(|error| error
            .message
            .contains("Dict.contains_key() expects 1 argument(s), got 0")),
        "expected an arity diagnostic, got: {:?}",
        arity_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let type_errors = check_str_err(
        r#"
def has_manifest(files: Dict[str, str]) -> bool:
    return files.contains_key(7)
"#,
        "Dict.contains_key rejects a probe outside the key type",
    );
    assert!(
        type_errors.iter().any(|error| error
            .message
            .contains("Argument to 'Dict.contains_key' has type mismatch")),
        "expected a key type diagnostic, got: {:?}",
        type_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );

    let named_errors = check_str_err(
        r#"
def has_manifest(files: Dict[str, str]) -> bool:
    return files.contains_key(key="loaf.toml")
"#,
        "Dict.contains_key takes its probe positionally",
    );
    assert!(
        named_errors
            .iter()
            .any(|error| error.message.contains("Unexpected keyword argument 'key'")),
        "expected an unknown-keyword diagnostic, got: {:?}",
        named_errors.iter().map(|error| &error.message).collect::<Vec<_>>()
    );
}

// ---- #1717: a tuple annotation spells its element types ----

#[test]
fn bare_tuple_annotation_is_refused_with_the_parameterised_spelling_issue1717() {
    // The program from #1717, plus the lowercase spelling and a parameter position: each bare occurrence is refused
    // once, with a hint in the author's casing.
    let source = r#"
def first(pair: tuple) -> int:
    return pair[0]

def main() -> None:
    pair: tuple[int, str] = (1, "one")
    multiple: Tuple = ("a", pair, "b", 42)
    println("ok")
"#;
    let errors = check_str_err(source, "a bare tuple annotation must be refused");
    let refused = errors
        .iter()
        .filter(|error| error.stable_code() == Some("INCAN-T0104"))
        .map(|error| (error.message.clone(), error.hints.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        refused.iter().map(|(message, _)| message.as_str()).collect::<Vec<_>>(),
        vec![
            "Tuple annotation 'tuple' is missing its element types",
            "Tuple annotation 'Tuple' is missing its element types",
        ],
        "one report per bare occurrence, in source order"
    );
    assert!(
        refused[0].1.iter().any(|hint| hint.contains("'tuple[int, str]'"))
            && refused[1].1.iter().any(|hint| hint.contains("'Tuple[int, str]'")),
        "the hint keeps the author's casing, got: {:?}",
        refused.iter().map(|(_, hints)| hints).collect::<Vec<_>>()
    );
}

#[test]
fn parameterised_tuple_annotations_and_a_shadowing_tuple_type_are_accepted_issue1717() {
    assert_check_ok(
        r#"
def main() -> None:
    pair: tuple[int, str] = (1, "one")
    triple: Tuple[int, int, int] = (1, 2, 3)
    nested: tuple[tuple[int, str], tuple[int, int, int]] = (pair, triple)
    println(nested[0][0])
"#,
    );
    // A source type that takes the spelling for itself is an ordinary nominal, not the builtin family.
    assert_check_ok(
        r#"
model Tuple:
    left: int
    right: int

def sum(pair: Tuple) -> int:
    return pair.left + pair.right
"#,
    );
}
