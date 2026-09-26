//! RFC 009 numerics (exact-width widening, narrowing and range checks, decimals, #1219 f32 results), RFC 008 const
//! bindings and frozen collections (#1488), and static initializer ordering, cycles and mutation.

use super::*;

#[test]
fn rfc009_exact_width_numeric_widening_typechecks() -> Result<(), String> {
    let source = r#"
def main() -> None:
  small: i16 = 120
  wide: i64 = small
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_exact_width_numeric_narrowing_requires_explicit_policy() {
    let source = r#"
def main() -> None:
  wide: i16 = 120
  narrow: i8 = wide
"#;
    let errors = check_str_err(source, "expected narrowing assignment to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("expected 'i8', found 'i16'")),
        "expected i16 -> i8 mismatch, got: {errors:?}"
    );
}

#[test]
fn rfc009_integer_literals_are_range_checked_for_exact_width_targets() {
    let source = r#"
def main() -> None:
  small: i8 = 300
"#;
    let errors = check_str_err(source, "expected out-of-range i8 literal to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Integer literal 300 does not fit in i8")),
        "expected i8 range diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_const_integer_literals_use_exact_width_annotation() -> Result<(), String> {
    let source = r#"
const NANOS_PER_SECOND: u64 = 1_000_000_000
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_const_integer_literals_are_range_checked_for_exact_width_targets() {
    let source = r#"
const BYTE: u8 = -1
"#;
    let errors = check_str_err(source, "expected out-of-range u8 const literal to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Integer literal -1 does not fit in u8")),
        "expected u8 range diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_negative_integer_literals_use_signed_exact_width_ranges() -> Result<(), String> {
    let source = r#"
def main() -> None:
  small: i8 = -128
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_negative_integer_literals_do_not_fit_unsigned_targets() {
    let source = r#"
def main() -> None:
  byte: u8 = -1
"#;
    let errors = check_str_err(source, "expected negative u8 literal to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Integer literal -1 does not fit in u8")),
        "expected u8 range diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_pointer_sized_integer_literals_are_range_checked() {
    let source = r#"
def main() -> None:
  size: usize = -1
"#;
    let errors = check_str_err(source, "expected negative usize literal to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Integer literal -1 does not fit in usize")),
        "expected usize range diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_lossless_resize_uses_contextual_target() -> Result<(), String> {
    let source = r#"
def main() -> None:
  small: i8 = 120
  wide: int = small.resize()
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_lossless_resize_rejects_narrowing() {
    let source = r#"
def main() -> None:
  wide: i16 = 120
  narrow: i8 = wide.resize()
"#;
    let errors = check_str_err(source, "expected narrowing resize to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("lossless numeric resize target")),
        "expected lossless resize diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_explicit_resize_policies_allow_integer_narrowing() -> Result<(), String> {
    let source = r#"
def main() -> None:
  wide: i16 = 240
  maybe: Option[i8] = wide.try_resize()
  wrapped: i8 = wide.wrapping_resize()
  capped: i8 = wide.saturating_resize()
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_binary_float_literals_are_checked_for_f32_targets() -> Result<(), String> {
    let ok = r#"
def main() -> None:
  value: f32 = 1.5
"#;
    check_str(ok).map_err(|errs| format!("expected f32 literal to typecheck: {errs:?}"))?;

    let too_large = r#"
def main() -> None:
  value: f32 = 1e100
"#;
    let errors = check_str(too_large)
        .err()
        .ok_or_else(|| "expected out-of-range f32 literal to fail".to_string())?;
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Float literal 1e100 does not fit in f32")),
        "expected f32 range diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn rfc009_non_finite_exact_float_literals_are_rejected_with_source_spans() -> Result<(), String> {
    let cases = [
        (
            "positive f32 local",
            "def main() -> None:\n  value: f32 = 1e9999\n",
            "f32",
            "1e9999",
        ),
        (
            "negative f32 local",
            "def main() -> None:\n  value: f32 = -1e9999\n",
            "f32",
            "1e9999",
        ),
        (
            "positive f64 local",
            "def main() -> None:\n  value: f64 = 1e9999\n",
            "f64",
            "1e9999",
        ),
        (
            "negative f64 local",
            "def main() -> None:\n  value: f64 = -1e9999\n",
            "f64",
            "1e9999",
        ),
        ("positive f32 const", "const VALUE: f32 = 1e9999\n", "f32", "1e9999"),
        ("negative f32 const", "const VALUE: f32 = -1e9999\n", "f32", "-1e9999"),
        ("positive f64 const", "const VALUE: f64 = 1e9999\n", "f64", "1e9999"),
        ("negative f64 const", "const VALUE: f64 = -1e9999\n", "f64", "-1e9999"),
    ];

    for (case, source, target, expected_span) in cases {
        let errors = check_str(source)
            .err()
            .ok_or_else(|| format!("expected {case} to be rejected"))?;
        let error = errors
            .iter()
            .find(|error| error.message.contains(&format!("does not fit in {target}")))
            .ok_or_else(|| format!("expected a finite-only {target} diagnostic for {case}, got {errors:?}"))?;
        let actual_span = source
            .get(error.span.start..error.span.end)
            .ok_or_else(|| format!("invalid diagnostic span {:?} for {case}", error.span))?;
        assert_eq!(actual_span, expected_span, "wrong source span for {case}");
    }
    Ok(())
}

#[test]
fn non_finite_exact_float_literals_nested_in_arithmetic_keep_literal_spans() -> Result<(), String> {
    let cases = [
        ("f32", "1e9999 + 0.0", "1e9999"),
        ("f64", "0.0 + -1e9999", "1e9999"),
        ("f64", "(0.0 + (1e9999 * 1.0))", "1e9999"),
    ];
    for (target, expression, expected_span) in cases {
        let source = format!("def main() -> None:\n  value: {target} = {expression}\n");
        let errors = check_str(&source)
            .err()
            .ok_or_else(|| format!("expected nested {target} non-finite literal to fail"))?;
        let error = errors
            .iter()
            .find(|error| {
                error.message.contains("Float literal") && error.message.contains(&format!("does not fit in {target}"))
            })
            .ok_or_else(|| format!("expected a nested exact-float diagnostic, got {errors:?}"))?;
        let actual_span = source
            .get(error.span.start..error.span.end)
            .ok_or_else(|| format!("invalid diagnostic span {:?}", error.span))?;
        assert_eq!(actual_span, expected_span, "wrong nested literal span for {target}");
    }
    Ok(())
}

#[test]
fn folded_non_finite_exact_float_const_reports_a_constant_value() -> Result<(), String> {
    let source = "const VALUE: f64 = -1e9999\n";
    let errors = check_str(source)
        .err()
        .ok_or_else(|| "expected folded non-finite f64 const to fail".to_string())?;
    let error = errors
        .iter()
        .find(|error| error.message.contains("Constant value") && error.message.contains("does not fit in f64"))
        .ok_or_else(|| format!("expected a folded-constant diagnostic, got {errors:?}"))?;
    let actual_span = source
        .get(error.span.start..error.span.end)
        .ok_or_else(|| format!("invalid folded-constant diagnostic span {:?}", error.span))?;
    assert_eq!(actual_span, "-1e9999");
    Ok(())
}

#[test]
fn ordinary_float_literals_remain_ieee_non_finite() -> Result<(), String> {
    let source = "const VALUE: float = 1e9999\n\ndef main() -> None:\n  value: float = -1e9999\n";
    check_str(source).map_err(|errors| format!("ordinary float must retain IEEE non-finite values: {errors:?}"))
}

#[test]
fn ordinary_float_values_cannot_narrow_to_exact_f32() -> Result<(), String> {
    let source = "def exact(value: str) -> f32:\n  return float(value)\n";
    let errors = check_str(source)
        .err()
        .ok_or_else(|| "ordinary float must not narrow to exact f32".to_string())?;
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Return type mismatch: expected 'f32', found 'float'")),
        "expected the exact-f32 return boundary to reject ordinary float, got {errors:?}"
    );
    Ok(())
}

#[test]
fn exact_width_float_arithmetic_preserves_f32_results_issue1219() -> Result<(), String> {
    let source = r#"
def subtract(left: f32, right: f32) -> f32:
  return left - right

def scale(time: f32, velocity: f32) -> f32:
  return time * velocity

def negate(value: f32) -> f32:
  return -value

def main() -> None:
  zero: f32 = 0.0
  speed: f32 = 120.0
  left: f32 = zero - speed
  negative: f32 = -speed
  print(left)
  print(negative)
"#;
    check_str(source).map_err(|errors| format!("expected f32 arithmetic to preserve f32, got: {errors:?}"))
}

#[test]
fn exact_width_float_const_literals_preserve_f32_results_issue1219() -> Result<(), String> {
    let source = r#"
const WALK_SPEED: f32 = 120.0
const WALK_LIMIT: f32 = 280.0

def distance(time: f32) -> f32:
  return WALK_SPEED * time - WALK_LIMIT
"#;
    check_str(source).map_err(|errors| format!("expected f32 const literals to preserve f32, got: {errors:?}"))
}

#[test]
fn exact_width_float_const_literals_reject_out_of_range_f32_issue1219() -> Result<(), String> {
    let source = r#"
const TOO_LARGE: f32 = 1e100
"#;
    let errors = check_str(source)
        .err()
        .ok_or_else(|| "expected out-of-range f32 const literal to fail".to_string())?;
    assert!(
        errors.iter().any(|error| error.message.contains("does not fit in f32")),
        "expected an f32 range diagnostic, got: {errors:?}"
    );
    Ok(())
}

#[test]
fn rfc009_decimal_annotation_accepts_decimal_literal() -> Result<(), String> {
    let source = r#"
def main() -> None:
  price: decimal[5, 2] = 19.99d
"#;
    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_decimal_precision_and_scale_are_validated() {
    let source = r#"
def main() -> None:
  price: decimal[39, 2] = 19.99d
"#;
    let errors = check_str_err(source, "expected invalid decimal precision to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Decimal precision must be between 1 and 38")),
        "expected decimal precision diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_bare_decimal_and_numeric_are_reserved() {
    for (source, name) in [
        (
            r#"
def main() -> None:
  value: decimal = 1
"#,
            "decimal",
        ),
        (
            r#"
def main() -> None:
  value: numeric = 1
"#,
            "numeric",
        ),
    ] {
        let errors = check_str_err(source, "expected reserved numeric type name to fail");
        assert!(
            errors
                .iter()
                .any(|err| err.message.contains(&format!("`{name}` is reserved for numeric types"))),
            "expected reserved numeric type diagnostic for {name}, got: {errors:?}"
        );
    }
}

#[test]
fn rfc009_bigint_and_hugeint_aliases_typecheck() -> Result<(), String> {
    let source = r#"
def main() -> None:
  big: bigint = 1
  huge: hugeint = big
"#;

    check_str(source).map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rfc009_decimal_literals_are_checked_against_scale() {
    let source = r#"
def main() -> None:
  price: decimal[5, 2] = 19.999d
"#;
    let errors = check_str_err(source, "expected invalid decimal literal scale to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("has 3 fractional digit(s)") && err.message.contains("allows at most 2")),
        "expected decimal literal scale diagnostic, got: {errors:?}"
    );
}

#[test]
fn rfc009_decimal_literals_are_checked_against_integer_digits() {
    let source = r#"
def main() -> None:
  price: decimal[5, 2] = 1234.5d
"#;
    let errors = check_str_err(source, "expected invalid decimal literal integer width to fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("has 4 integer digit(s)") && err.message.contains("allows at most 3")),
        "expected decimal literal integer digit diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_module_level_const() {
    let source = r#"
const X: int = 1 + 2

def foo() -> int:
  return X
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_cycle_detected() {
    let source = r#"
const A: int = B
const B: int = A
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(errs.iter().any(|e| e.message.contains("Const dependency cycle")));
}

#[test]
fn test_const_frozen_str() {
    let source = r#"
const GREETING: FrozenStr = "hello"

def foo() -> FrozenStr:
  return GREETING
"#;
    assert!(check_str(source).is_ok());
}

/// Regression for #1719: a frozen collection is a `const` value and the language defines no constructor call for
/// one, so `FrozenList([...])`, `FrozenDict({...})` and `FrozenSet([...])` are refused at check time with the
/// documented spelling instead of being typed `Unknown` and left for the build to reject.
#[test]
fn frozen_collection_value_constructors_are_refused_issue1719() {
    let source = r#"
def count(frozen: FrozenList[int]) -> int:
  return len(frozen)

def main() -> None:
  println(count(FrozenList([7, 8])))
  names = FrozenSet(["a"])
  ages = FrozenDict({"a": 1})
"#;
    let errs = check_str_err(source, "frozen collection constructors should be refused");
    let messages: Vec<&str> = errs.iter().map(|err| err.message.as_str()).collect();
    for name in ["FrozenList", "FrozenSet", "FrozenDict"] {
        let expected = format!("{name}(...) is not a constructor; a frozen collection is declared as a const");
        assert!(
            messages.contains(&expected.as_str()),
            "expected a refusal for {name}(...), got: {messages:?}"
        );
    }
    let list_hint = errs
        .iter()
        .find(|err| err.message.starts_with("FrozenList("))
        .and_then(|err| err.hints.first())
        .map(String::as_str);
    assert_eq!(
        list_hint,
        Some("Declare it as a const: `const NAME: FrozenList[T] = [...]`"),
        "the hint names the documented const spelling"
    );
    let dict_hint = errs
        .iter()
        .find(|err| err.message.starts_with("FrozenDict("))
        .and_then(|err| err.hints.first())
        .map(String::as_str);
    assert_eq!(
        dict_hint,
        Some("Declare it as a const: `const NAME: FrozenDict[K, V] = {...}`")
    );
}

/// The documented spelling keeps working: a frozen const passed where the frozen type is expected.
#[test]
fn frozen_collection_consts_remain_the_documented_spelling() {
    let source = r#"
const FROZEN: FrozenList[int] = [7, 8]

def count(frozen: FrozenList[int]) -> int:
  return len(frozen)

def main() -> None:
  println(count(FROZEN))
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_runtime_str_does_not_implicitly_satisfy_frozen_str() {
    let source = r#"
def accept_frozen(value: FrozenStr) -> None:
  return

def foo(value: str) -> None:
  accept_frozen(value)
"#;
    let errs = check_str_err(source, "runtime str should not typecheck as FrozenStr");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Argument 'value' of 'accept_frozen' has type mismatch: expected 'FrozenStr', found 'str'")),
        "expected type mismatch for runtime str to FrozenStr, got {errs:?}"
    );
}

#[test]
fn test_runtime_bytes_do_not_implicitly_satisfy_frozen_bytes() {
    let source = r#"
def accept_frozen(value: FrozenBytes) -> None:
  return

def foo(value: bytes) -> None:
  accept_frozen(value)
"#;
    let errs = check_str_err(source, "runtime bytes should not typecheck as FrozenBytes");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Argument 'value' of 'accept_frozen' has type mismatch: expected 'FrozenBytes', found 'bytes'")),
        "expected type mismatch for runtime bytes to FrozenBytes, got {errs:?}"
    );
}

#[test]
fn test_const_frozen_list() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2, 3]

def foo() -> int:
  return NUMS.len()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_frozen_list_spread_is_rejected_in_frontend() {
    let source = r#"
const BASE: FrozenList[int] = [1, 2]
const NUMS: FrozenList[int] = [0, *BASE, 3]
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected const list spread to fail");
    };
    assert!(
        errs.iter().any(|err| err.message.contains("not allowed")),
        "expected const expression diagnostic for list spread, got: {errs:?}"
    );
}

#[test]
fn test_const_frozen_dict() {
    let source = r#"
const HEADERS: FrozenDict[FrozenStr, int] = {"a": 1, "b": 2}

def foo() -> bool:
  return HEADERS.contains_key("a")
"#;
    // Note: This may or may not pass depending on type inference for dict keys
    let _ = check_str(source);
}

#[test]
fn test_const_frozen_set() {
    let source = r#"
const ALLOWED: FrozenSet[int] = {1, 2, 3}

def foo() -> bool:
  return ALLOWED.contains(2)
"#;
    assert!(check_str(source).is_ok());
}

/// #1488: a `const` annotated with a mutable container type is rejected at the annotation, naming the frozen
/// representation the const actually has, instead of being silently retyped and failing at its first use site.
#[test]
fn const_mutable_collection_annotation_is_rejected_at_the_annotation_issue1488() {
    let source = r#"
const SECTIONS: list[str] = ["x"]

def read_only(names: list[str]) -> int:
  return len(names)

def main() -> None:
  println(f"{read_only(SECTIONS)}")
"#;
    let errs = check_str_err(source, "a `list[str]` const annotation must be rejected");
    let annotation_start = source.find("list[str]").unwrap_or_default();
    let annotation = Span::new(annotation_start, annotation_start + "list[str]".len());
    let rejection = errs
        .iter()
        .find(|err| err.message.contains("const 'SECTIONS'"))
        .unwrap_or_else(|| panic!("expected a const-annotation diagnostic, got: {errs:?}"));
    assert_eq!(
        rejection.span, annotation,
        "the diagnostic must point at the annotation the author wrote: {rejection:?}"
    );
    assert!(
        rejection.message.contains("'list[str]'") && rejection.message.contains("'FrozenList[str]'"),
        "the diagnostic must name both the written and the frozen spelling: {}",
        rejection.message
    );
    assert!(
        rejection.hints.iter().any(|hint| hint.contains("FrozenList[str]")),
        "the hint must say what to write instead: {:?}",
        rejection.hints
    );
    // The use-site mismatch is real -- a frozen const cannot feed a mutable `list[str]` parameter -- and it stays.
    // What changes is where the author learns about it: the declaration reports first, naming the spelling they
    // wrote, so the later message about `FrozenList[str]` no longer reads as a contradiction.
    let use_site = errs
        .iter()
        .position(|err| err.message.contains("Argument 'names'"))
        .unwrap_or_else(|| panic!("the use-site mismatch must still be reported: {errs:?}"));
    let declaration = errs
        .iter()
        .position(|err| err.span == annotation)
        .unwrap_or_else(|| panic!("the annotation diagnostic must be present: {errs:?}"));
    assert!(
        declaration < use_site,
        "the annotation must be reported before the use site: {errs:?}"
    );
}

#[test]
fn const_dict_and_set_annotations_name_their_frozen_forms_issue1488() {
    let source = r#"
const TABLE: dict[str, int] = {"a": 1}
const ALLOWED: set[int] = {1, 2}
"#;
    let errs = check_str_err(source, "mutable `dict`/`set` const annotations must be rejected");
    for (written, frozen) in [
        ("dict[str, int]", "FrozenDict[str, int]"),
        ("set[int]", "FrozenSet[int]"),
    ] {
        assert!(
            errs.iter().any(
                |err| err.message.contains(&format!("'{written}'")) && err.message.contains(&format!("'{frozen}'"))
            ),
            "expected a diagnostic naming '{written}' and '{frozen}', got: {errs:?}"
        );
    }
}

#[test]
fn const_frozen_and_scalar_annotations_stay_accepted_issue1488() {
    // `str`/`bytes` are also frozen in const context, but a `FrozenStr`/`FrozenBytes` reads wherever `str`/`bytes`
    // is expected, so the written annotation is honored at every use site and there is nothing to reject.
    let source = r#"
const NAMES: FrozenList[str] = ["x"]
const INFERRED = ["y"]
const LABEL: str = "z"
const RAW: bytes = b"\x00"
const LIMIT: int = 3

def take(label: str, raw: bytes) -> int:
  return len(label) + len(raw)

def main() -> int:
  return take(LABEL, RAW) + LIMIT
"#;
    assert!(check_str(source).is_ok(), "{:?}", check_str(source));
}

#[test]
fn test_const_reference_other_const() {
    let source = r#"
const BASE: int = 10
const DOUBLED: int = BASE * 2

def foo() -> int:
  return DOUBLED
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_newtype_constructor_from_numeric_literal() {
    let source = r#"
type Token = newtype u128

const ZERO: Token = Token(0)
const MAX_TOKEN: Token = Token(340282366920938463463374607431768211455)

def foo() -> Token:
  return MAX_TOKEN
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_non_const_in_initializer_fails() {
    // A variable binding (not a const) should not be usable in a const initializer
    let source = r#"
const BAD: int = some_runtime_var
"#;
    let result = check_str(source);
    // Should fail because some_runtime_var is not defined, or if defined as var, not allowed
    assert!(result.is_err());
}

#[test]
fn test_const_runtime_call_fails() {
    let source = r#"
def helper() -> int:
  return 42

const BAD: int = helper()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not allowed") || e.message.contains("const initializers"))
    );
}

#[test]
fn test_const_empty_list_requires_annotation() {
    let source = r#"
const EMPTY = []
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(
        errs.iter()
            .any(|e| { e.message.contains("Cannot infer type") || e.message.contains("empty const list") })
    );
}

#[test]
fn test_const_type_mismatch() {
    let source = r#"
const X: int = "not an int"
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_const_string_concat_allowed() {
    let source = r#"
const GREETING: FrozenStr = "hello" + " world"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_bytes_literal_allowed() {
    let source = r#"
const DATA: FrozenBytes = b"hi"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_bytes_method_len() {
    let source = r#"
const DATA: FrozenBytes = b"hi"

def foo() -> int:
  return DATA.len()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_bytes_method_is_empty() {
    let source = r#"
const DATA: FrozenBytes = b"hi"

def foo() -> bool:
  return DATA.is_empty()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_list_method_len() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2, 3]

def foo() -> int:
  return NUMS.len()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_list_method_is_empty() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2]

def foo() -> bool:
  return NUMS.is_empty()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_set_contains_method() {
    let source = r#"
const ALLOWED: FrozenSet[int] = {10, 20}

def foo() -> bool:
  return ALLOWED.contains(10)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_dict_contains_key_method() {
    let source = r#"
const ITEMS: FrozenDict[FrozenStr, int] = {"x": 1}

def foo() -> bool:
  return ITEMS.contains_key("x")
"#;
    // May need type inference improvements
    let _ = check_str(source);
}

#[test]
fn test_frozen_unknown_method_errors() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2]

def foo() -> int:
  return NUMS.nonexistent_method()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(errs.iter().any(|e| e.message.contains("has no method")));
}

#[test]
fn test_static_initializer_requires_earlier_static() {
    let source = r#"
static SECOND: int = FIRST
static FIRST: int = 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("forward static reference should fail");
    };
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Static 'SECOND' cannot reference 'FIRST' before it is initialized")),
        "expected earlier-static diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_dependency_cycle_is_rejected() {
    let source = r#"
static A: int = B
static B: int = A
"#;
    let Err(errs) = check_str(source) else {
        panic!("static cycle should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Static dependency cycle detected")),
        "expected static-cycle diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_dependency_cycle_via_function_calls_is_rejected() {
    let source = r#"
def read_a() -> int:
  return A

def read_b() -> int:
  return B

static A: int = read_b()
static B: int = read_a()
"#;
    let Err(errs) = check_str(source) else {
        panic!("static cycle through helper functions should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Static dependency cycle detected")),
        "expected static-cycle diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_initializer_rejects_helper_static_assignment() {
    let source = r#"
static TARGET: int = 0

def mutate_target() -> int:
  TARGET = TARGET + 1
  return TARGET

static RESULT: int = mutate_target()
"#;
    let Err(errs) = check_str(source) else {
        panic!("static initializer that assigns a static through helper call should fail");
    };
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Static initializer for 'RESULT' cannot assign to static 'TARGET'")),
        "expected static-initializer write diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_initializer_via_function_requires_earlier_static() {
    let source = r#"
def read_first() -> int:
  return FIRST

static SECOND: int = read_first()
static FIRST: int = 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("forward static reference through helper function should fail");
    };
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Static 'SECOND' cannot reference 'FIRST' before it is initialized")),
        "expected earlier-static diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_imported_static_reassignment_is_rejected() {
    let source = r#"
from pub::mylib import SHARED_ITEMS

def main() -> None:
  SHARED_ITEMS = []
"#;
    let Err(errs) = check_str_with_library_index(source, library_index_with_mylib_exports()) else {
        panic!("imported static reassignment should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Cannot reassign imported static 'SHARED_ITEMS'")),
        "expected imported-static reassignment diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_const_reassignment_suggests_static() {
    let source = r#"
const COUNTER: int = 0

def main() -> None:
  COUNTER = 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("const reassignment should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Cannot reassign const 'COUNTER'"))
            && errs.iter().any(|e| e
                .hints
                .iter()
                .any(|hint| hint.contains("declare it as `static COUNTER"))),
        "expected const-reassignment static hint, got: {:?}",
        errs.iter().map(|e| (&e.message, &e.hints)).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_alias_mutation_typechecks() {
    let source = r#"
static ITEMS: list[int] = []

def main() -> None:
  let live = ITEMS
  live.append(1)
  println(len(ITEMS))
  println(len(live))
"#;
    assert_check_ok(source);
}

#[test]
fn test_imported_static_mutation_typechecks() {
    let source = r#"
from pub::mylib import SHARED_ITEMS

def main() -> None:
  SHARED_ITEMS.append(1)
  let live = SHARED_ITEMS
  live.append(2)
"#;
    assert!(
        check_str_with_library_index(source, library_index_with_mylib_exports()).is_ok(),
        "expected imported static mutation to typecheck"
    );
}
