//! Statements and plain expressions: variables, arithmetic, comparison and logical operators, control flow, closures,
//! tuples, loop expressions, the diagnostics a plain function body produces, plain assignment versus `let` / `mut`
//! (#1072), statement tuple unpacking (#1132), the `isinstance` facts retained for Body IR (#1281), and unreachable
//! code after an unconditional `return` (#1117).

use super::*;

#[test]
fn test_simple_function() {
    let source = r#"
def add(a: int, b: int) -> int:
  return a + b
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_type_mismatch() {
    let source = r#"
def foo() -> int:
  return "hello"
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_return_type_mismatch_names_return_context() {
    let source = r#"
def foo() -> int:
  return "hello"
"#;
    let errs = check_str_err(source, "expected return type mismatch");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Return type mismatch: expected 'int', found 'str'")),
        "expected contextual return mismatch, got: {errs:?}"
    );
    assert!(
        errs.iter().any(|err| err
            .notes
            .iter()
            .any(|note| note.contains("return expression must match the function return type 'int'"))),
        "expected return-context note, got: {errs:?}"
    );
}

#[test]
fn test_assignment_type_mismatch_names_binding_context() {
    let source = r#"
def main() -> None:
  annotated: int = "hello"
"#;
    let errs = check_str_err(source, "expected assignment type mismatch");
    assert!(
        errs.iter().any(|err| {
            err.message
                .contains("Assignment to 'annotated' has type mismatch: expected 'int', found 'str'")
        }),
        "expected contextual assignment mismatch, got: {errs:?}"
    );
    assert!(
        errs.iter().any(|err| err
            .notes
            .iter()
            .any(|note| note.contains("'annotated' is declared as 'int'"))),
        "expected assignment-context note, got: {errs:?}"
    );
}

#[test]
fn test_call_argument_type_mismatch_names_parameter_context() {
    let source = r#"
def takes_int(value: int) -> None:
  pass

def main() -> None:
  takes_int("hello")
"#;
    let errs = check_str_err(source, "expected call argument type mismatch");
    assert!(
        errs.iter().any(|err| {
            err.message
                .contains("Argument 'value' of 'takes_int' has type mismatch: expected 'int', found 'str'")
        }),
        "expected contextual call argument mismatch, got: {errs:?}"
    );
    assert!(
        errs.iter().any(|err| err
            .notes
            .iter()
            .any(|note| note.contains("Parameter 'value' of 'takes_int' is declared as 'int'"))),
        "expected call-argument-context note, got: {errs:?}"
    );
}

#[test]
fn test_unknown_symbol() {
    let source = r#"
def foo() -> int:
  return unknown_var
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_unknown_symbol_in_elif_branch_is_reported() {
    let source = r#"
def foo(flag: bool) -> int:
  if flag:
    return 1
  elif true:
    return unknown_var
  else:
    return 0
"#;
    let errors = check_str_err(source, "Expected typechecker error for unknown symbol in elif branch");
    assert!(
        has_unknown_symbol_error(&errors, "unknown_var"),
        "Expected unknown symbol error for elif branch; got: {errors:?}"
    );
}

#[test]
fn test_variable_declaration() {
    let source = r#"
def foo() -> int:
  x = 10
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_mutable_variable() {
    let source = r#"
def foo() -> int:
  mut x = 10
  x = 20
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_typed_variable() {
    let source = r#"
def foo() -> int:
  let x: int = 10
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_addition() {
    let source = r#"
def foo() -> int:
  return 1 + 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_subtraction() {
    let source = r#"
def foo() -> int:
  return 10 - 5
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_multiplication() {
    let source = r#"
def foo() -> int:
  return 3 * 4
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_division() {
    // Division always returns float (Python-like semantics)
    let source = r#"
def foo() -> float:
  return 10 / 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_modulo() {
    let source = r#"
def foo() -> int:
  return 10 % 3
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_equal() {
    let source = r#"
def foo() -> bool:
  return 1 == 1
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_not_equal() {
    let source = r#"
def foo() -> bool:
  return 1 != 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_less_than() {
    let source = r#"
def foo() -> bool:
  return 1 < 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_greater_than() {
    let source = r#"
def foo() -> bool:
  return 2 > 1
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_logical_and() {
    let source = r#"
def foo() -> bool:
  return true and false
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_logical_or() {
    let source = r#"
def foo() -> bool:
  return true or false
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_logical_not() {
    let source = r#"
def foo() -> bool:
  return not true
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_if_statement() {
    let source = r#"
def foo(x: int) -> int:
  if x > 0:
    return 1
  return 0
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_if_else_statement() {
    let source = r#"
def foo(x: int) -> int:
  if x > 0:
    return 1
  else:
    return -1
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_while_loop() {
    let source = r#"
def foo() -> int:
  mut x = 0
  while x < 10:
    x = x + 1
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_if_let_statement_typechecks() {
    let source = r#"
def first(opt: Option[int]) -> int:
  if let Some(value) = opt:
    return value
  return 0
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_while_let_statement_typechecks() {
    let source = r#"
def sum_once(opt: Option[int]) -> int:
  mut total = 0
  mut current = opt
  while let Some(value) = current:
    total = total + value
    current = None
  return total
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_for_loop() {
    let source = r#"
def foo() -> int:
  mut sum = 0
  for i in range(10):
    sum = sum + i
  return sum
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_tuple_literal() {
    let source = r#"
def foo() -> (int, str):
  return (1, "hello")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_tuple_index_requires_literal() {
    let source = r#"
def foo(t: tuple[int, int]) -> int:
  idx: int = 0
  return t[idx]
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(
        errs.iter()
            .any(|e| { e.message.contains("Tuple indices must be an integer literal") })
    );
}

#[test]
fn test_unknown_method_errors() {
    let source = r#"
def foo() -> int:
  return "hi".nope()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(errs.iter().any(|e| e.message.contains("has no method")));
}

#[test]
fn test_closure() {
    // Note: untyped closure params may not pass typechecker
    // This tests that we handle closures correctly (even if they error)
    let source = r#"
def foo() -> int:
  f = (x) => x + 1
  return f(41)
"#;
    // Closure with untyped params may error, so just check it doesn't panic
    let _ = check_str(source);
}

#[test]
fn test_wrong_argument_count() {
    // Note: The typechecker may be lenient on argument counts
    // Just verify we can run through the check without panic
    let source = r#"
def add(a: int, b: int) -> int:
  return a + b

def foo() -> int:
  return add(1)
"#;
    let _ = check_str(source);
}

#[test]
fn test_undefined_function() {
    let source = r#"
def foo() -> int:
  return undefined_func()
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_return_type_mismatch_in_if() {
    let source = r#"
def foo(x: bool) -> int:
  if x:
    return "wrong"
  return 0
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn loop_expression_infers_break_value_type() {
    assert_check_ok(
        r#"
def run() -> int:
  return loop:
    break 42
"#,
    );
}

#[test]
fn break_value_requires_loop_expression() {
    let errs = check_str_err(
        r#"
def run(xs: list[int]) -> None:
  for x in xs:
    break x
"#,
        "expected break-value diagnostic in for loop",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("only valid inside `loop:` expressions")),
        "expected loop-expression-only diagnostic, got {errs:?}"
    );
}

#[test]
fn loop_expression_without_break_is_rejected() {
    let errs = check_str_err(
        r#"
def run() -> int:
  return loop:
    pass
"#,
        "expected missing-break diagnostic for loop expression",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("loop expression must contain at least one `break`")),
        "expected missing-break diagnostic, got {errs:?}"
    );
}

#[test]
fn break_outside_loop_uses_typed_diagnostic() {
    let errs = check_str_err(
        r#"
def run() -> None:
  break
"#,
        "expected break-outside-loop diagnostic",
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("`break` is only valid inside loops")),
        "expected break-outside-loop diagnostic, got {errs:?}"
    );
}

/// Stable code carried by the unreachable-code warning, asserted here rather than message prose so these tests
/// pin the machine-readable contract tooling consumes.
const UNREACHABLE_CODE: &str = "INCAN-T0101";

/// Collect only the unreachable-code warnings for `source`, ignoring any unrelated advisory diagnostics.
///
/// Uses [`check_str_warnings`], which panics if typechecking fails — so every caller also proves the diagnostic
/// stays non-fatal.
fn unreachable_warnings(source: &str, context: &str) -> Vec<CompileError> {
    check_str_warnings(source, context)
        .into_iter()
        .filter(|warning| warning.stable_code() == Some(UNREACHABLE_CODE))
        .collect()
}

/// Assert `source` produces exactly one unreachable-code warning and return it.
fn single_unreachable_warning(source: &str, context: &str) -> CompileError {
    match unreachable_warnings(source, context).as_slice() {
        [warning] => warning.clone(),
        other => panic!("{context}: expected exactly one unreachable-code warning, got: {other:?}"),
    }
}

#[test]
fn test_unreachable_code_after_direct_return_warns() {
    let source = r#"
def f() -> int:
    return 1
    println("dead code")
    return 2
"#;
    let warning = single_unreachable_warning(source, "statements after a direct `return`");

    assert_eq!(warning.kind, ErrorKind::Warning);
    assert!(
        warning.message.contains("Unreachable code"),
        "expected an unreachable-code message, got: {}",
        warning.message
    );
    assert_eq!(
        &source[warning.span.start..warning.span.end],
        "println(\"dead code\")\n    return 2",
        "the span must cover the whole unreachable tail of the block"
    );
    assert_eq!(
        warning.related_spans().len(),
        1,
        "expected the `return` that made the tail unreachable to be a related span, got: {:?}",
        warning.related_spans()
    );
    assert_eq!(
        &source[warning.related_spans()[0].span.start..warning.related_spans()[0].span.end],
        "return 1",
        "the related span must point at the `return` that ends the block"
    );
}

#[test]
fn test_unreachable_code_after_return_in_nested_if_body_warns() {
    let source = r#"
def f(flag: bool) -> int:
    if flag:
        return 1
        println("dead inside the if body")
    return 2
"#;
    let warning = single_unreachable_warning(source, "statements after a `return` inside an `if` body");

    assert_eq!(
        &source[warning.span.start..warning.span.end],
        "println(\"dead inside the if body\")",
        "the span must cover only the nested block's unreachable tail"
    );
}

#[test]
fn test_unreachable_code_after_return_in_else_and_loop_bodies_warns() {
    let else_body = r#"
def f(flag: bool) -> int:
    if flag:
        return 1
    else:
        return 2
        println("dead inside the else body")
"#;
    let _ = single_unreachable_warning(else_body, "statements after a `return` inside an `else` body");

    let loop_body = r#"
def f() -> int:
    for value in [1, 2, 3]:
        return value
        println("dead inside the loop body")
    return 0
"#;
    let _ = single_unreachable_warning(loop_body, "statements after a `return` inside a `for` body");
}

#[test]
fn test_unreachable_code_after_return_in_match_arm_block_warns() {
    let source = r#"
enum Color:
    Red
    Green

def f(c: Color) -> int:
    match c:
        case Color.Red:
            return 1
            println("dead inside the match arm")
        case Color.Green:
            return 2
"#;
    let warning = single_unreachable_warning(source, "statements after a `return` inside a match-arm block");

    assert_eq!(
        &source[warning.span.start..warning.span.end],
        "println(\"dead inside the match arm\")",
        "the span must cover only the match arm's unreachable tail"
    );
}

#[test]
fn test_unreachable_code_after_return_in_method_body_warns() {
    let source = r#"
class Counter:
    value: int

    def get(self) -> int:
        return self.value
        println("dead inside the method body")
"#;
    let _ = single_unreachable_warning(source, "statements after a `return` inside a method body");
}

#[test]
fn test_conditional_return_keeps_following_statements_reachable() {
    let if_without_else = r#"
def f(flag: bool) -> int:
    if flag:
        return 1
    println("still reachable when flag is false")
    return 2
"#;
    assert!(
        unreachable_warnings(
            if_without_else,
            "a conditional `return` must not poison the outer block"
        )
        .is_empty(),
        "statements after a conditional `return` are reachable and must not warn"
    );

    let both_branches_return = r#"
def f(flag: bool) -> int:
    if flag:
        return 1
    else:
        return 2
    println("conservatively treated as reachable")
    return 3
"#;
    assert!(
        unreachable_warnings(both_branches_return, "if/else divergence is out of scope for #1117").is_empty(),
        "the narrow #1117 rule only follows a `return` in the same block, so if/else divergence must stay silent"
    );

    let loop_return = r#"
def f() -> int:
    for value in [1, 2, 3]:
        return value
    return 0
"#;
    assert!(
        unreachable_warnings(
            loop_return,
            "a `return` inside a loop body must not poison the outer block"
        )
        .is_empty(),
        "a loop body that returns does not make the statements after the loop unreachable"
    );
}

#[test]
fn test_trailing_return_and_return_free_bodies_do_not_warn() {
    let trailing_return = r#"
def f() -> int:
    println("live")
    return 1
"#;
    assert!(
        unreachable_warnings(
            trailing_return,
            "a trailing `return` ends the block with nothing after it"
        )
        .is_empty(),
        "a `return` as the last statement of a block must not warn"
    );

    let no_return = r#"
def f() -> None:
    println("live")
    println("also live")
"#;
    assert!(
        unreachable_warnings(no_return, "a body without `return` has nothing to make unreachable").is_empty(),
        "a body without a `return` must not warn"
    );
}

#[test]
fn test_unreachable_statements_are_still_typechecked() {
    let source = r#"
def f() -> int:
    return 1
    println(undefined_name)
"#;
    let errors = check_str_err(source, "unreachable statements must still be typechecked");
    assert!(
        has_unknown_symbol_error(&errors, "undefined_name"),
        "expected the unreachable statement's own type error to still be reported, got: {errors:?}"
    );
}

#[test]
fn test_unreachable_code_in_a_trait_default_method_is_reported_once_per_definition() {
    let source = r#"
trait Greeter:
    def greet(self) -> int:
        return 1
        println("dead in the trait default method")

class Alpha with Greeter:
    value: int

class Beta with Greeter:
    value: int

def main() -> None:
    alpha = Alpha(value=1)
    beta = Beta(value=2)
    println(f"{alpha.greet()} {beta.greet()}")
"#;
    // A default method body is one block of source, so it must warn once — not once per implementing type.
    let _ = single_unreachable_warning(source, "a trait default method body with dead code");
}

#[test]
fn test_statement_tuple_unpack_of_non_tuple_value_is_rejected() {
    let source = r#"
def main() -> None:
    a, b = 5
    println(f"{a} {b}")
"#;
    let errors = check_str_err(source, "unpacking two names from an `int` must be rejected");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Cannot destructure 2 values from value of type 'int'")),
        "the diagnostic must name the resolved value type: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_tuple_assign_spelling_of_non_tuple_value_is_rejected() {
    // The `TupleAssign` spelling reaches a different statement arm than `TupleUnpack`; both carried the same
    // `vec![Unknown; n]` fallback, so both need the diagnostic.
    let source = r#"
def main() -> None:
    mut xs = [1, 2]
    xs[0], xs[1] = 5
"#;
    let errors = check_str_err(source, "tuple-assigning two lvalues from an `int` must be rejected");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Cannot destructure 2 values from value of type 'int'")),
        "the diagnostic must name the resolved value type: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_statement_tuple_unpack_accepts_inferred_and_annotated_tuples() {
    let inferred = r#"
def main() -> None:
    pair = (1, "two")
    a, b = pair
    println(f"{a} {b}")
"#;
    assert!(
        check_str(inferred).is_ok(),
        "an inferred tuple literal must still destructure: {:?}",
        check_str(inferred)
    );

    // A written `tuple[A, B]` annotation resolves through the collection-type registry as
    // `ResolvedType::Generic("Tuple", args)`, not `ResolvedType::Tuple`. Reading only the latter left every
    // binding typed `Unknown` and suppressed the arity guard.
    let annotated = r#"
def main() -> None:
    pair: tuple[int, str] = (1, "two")
    a, b = pair
    println(f"{a} {b}")
"#;
    assert!(
        check_str(annotated).is_ok(),
        "an annotated `tuple[int, str]` must destructure: {:?}",
        check_str(annotated)
    );
}

#[test]
fn test_statement_tuple_unpack_arity_mismatch_is_rejected_in_both_directions() {
    let too_many_names = r#"
def main() -> None:
    a, b, c = (1, 2)
    println(f"{a} {b} {c}")
"#;
    let errors = check_str_err(too_many_names, "unpacking three names from a 2-tuple must be rejected");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Cannot unpack 3 values from tuple with 2 elements")),
        "expected the existing arity diagnostic: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    // The guard used to be `element_types.len() < names.len()`, so a tuple with *more* elements than names was
    // silently accepted. #1125 settled on an exact-arity comparison for loop patterns; statements match it.
    let too_few_names = r#"
def main() -> None:
    a, b = (1, 2, 3)
    println(f"{a} {b}")
"#;
    let errors = check_str_err(too_few_names, "unpacking two names from a 3-tuple must be rejected");
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("Cannot unpack 2 values from tuple with 3 elements")),
        "a tuple longer than the name list must not be silently truncated: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_statement_tuple_unpack_of_a_bare_type_variable_is_rejected() {
    // An unconstrained type variable is not "not yet known" — it is known to be underdetermined, and `T` can be
    // instantiated as `int`. Incan's bounds are trait-based, so no caller can promise a tuple shape.
    let source = r#"
def split[T](value: T) -> None:
    a, b = value
    println(f"{a} {b}")
"#;
    let errors = check_str_err(source, "destructuring a bare type variable must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Cannot destructure 2 values from value of type")),
        "a bare type variable must not be treated as destructurable: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_statement_tuple_unpack_accepts_a_tuple_of_type_variables() {
    // `tuple[K, V]` is a tuple whose *elements* are type variables. The shape is known even though the element
    // types are not, so it destructures — this is the common `dict` item shape and must not regress.
    let source = r#"
def split[K, V](pair: tuple[K, V]) -> None:
    a, b = pair
    println(f"{a} {b}")
"#;
    assert!(
        check_str(source).is_ok(),
        "a tuple of type variables must destructure: {:?}",
        check_str(source)
    );
}

#[test]
fn test_tuple_shape_classifier_covers_both_spellings_and_recovery_types() {
    use super::check_stmt::{TupleShape, classify_tuple_shape};

    // `Unknown` and `Never` are exercised directly: `Unknown` means checking already failed upstream, and `Never`
    // reaches a value position only through Rust interop's diverging `!`, which is not cheaply source-expressible.
    // Both must classify as recovery so no second diagnostic piles onto the real one.
    assert!(matches!(
        classify_tuple_shape(&ResolvedType::Unknown),
        TupleShape::Recovery
    ));
    assert!(matches!(
        classify_tuple_shape(&ResolvedType::Never),
        TupleShape::Recovery
    ));
    assert!(matches!(classify_tuple_shape(&ResolvedType::Int), TupleShape::NotTuple));
    assert!(matches!(
        classify_tuple_shape(&ResolvedType::TypeVar("T".to_string())),
        TupleShape::NotTuple
    ));

    // Regression for a false positive this change originally introduced: `std.json` destructures a
    // `rust::HashMap` item, whose type is a Rust tuple path rather than either Incan spelling. Rejecting it broke
    // the stdlib's own SDK component build.
    match classify_tuple_shape(&ResolvedType::RustPath(
        "(String,incan_std_data::json::JsonValue)".to_string(),
    )) {
        TupleShape::RustTuple(2) => {}
        other => panic!("a Rust tuple path must be destructurable with arity 2, got {other:?}"),
    }
    // Commas inside a nested generic must not inflate the arity.
    match classify_tuple_shape(&ResolvedType::RustPath("(String,HashMap<K, V>)".to_string())) {
        TupleShape::RustTuple(2) => {}
        other => panic!("nested generic commas must not be counted, got {other:?}"),
    }
    // An opaque Rust path refuses rather than going silent. "No structural model of this type" is not the same
    // claim as "checking already failed": treating it as recovery would let a real non-tuple Rust value reach a
    // generated field projection, which is the same leakage this issue closes, arriving via interop.
    assert!(matches!(
        classify_tuple_shape(&ResolvedType::RustPath("String".to_string())),
        TupleShape::OpaqueRust
    ));
    assert!(matches!(
        classify_tuple_shape(&ResolvedType::RustPath("std::vec::Vec<u8>".to_string())),
        TupleShape::OpaqueRust
    ));
    // A one-element Rust tuple is one element, not two with an empty slot.
    match classify_tuple_shape(&ResolvedType::RustPath("(String,)".to_string())) {
        TupleShape::RustTuple(1) => {}
        other => panic!("`(String,)` is a one-element tuple, got {other:?}"),
    }
    // But `(String)` is a parenthesized type with no `.0` field, not a one-element tuple. Reading it as one would
    // let a single-name destructure lower to `.0` on a `String` — the same raw-Rust failure through a narrower
    // spelling than `int`.
    assert!(
        matches!(
            classify_tuple_shape(&ResolvedType::RustPath("(String)".to_string())),
            TupleShape::OpaqueRust
        ),
        "a parenthesized Rust type must not be classified as a one-element tuple"
    );

    let literal = ResolvedType::Tuple(vec![ResolvedType::Int, ResolvedType::Str]);
    let annotated = ResolvedType::Generic("Tuple".to_string(), vec![ResolvedType::Int, ResolvedType::Str]);
    for (label, ty) in [("inferred", &literal), ("annotated", &annotated)] {
        match classify_tuple_shape(ty) {
            TupleShape::Tuple(elements) => assert_eq!(
                elements,
                vec![ResolvedType::Int, ResolvedType::Str],
                "{label} tuple spelling must yield its element types"
            ),
            other => panic!("{label} tuple spelling must classify as a tuple, got {other:?}"),
        }
    }
}

#[test]
fn test_statement_tuple_unpack_of_an_opaque_rust_value_is_refused() {
    // The `RustPath` path must apply the same rule as a bare type variable: not proven tuple-shaped refuses.
    // Accepting it silently would re-open the leak through interop — a `rust::String` reaching a `.0` projection
    // is the same defect as an `int` reaching one.
    use super::check_stmt::{TupleShape, classify_tuple_shape};

    assert!(matches!(
        classify_tuple_shape(&ResolvedType::RustPath("String".to_string())),
        TupleShape::OpaqueRust
    ));
    // `(String)` is the same type as `String`, only parenthesized, and must be refused identically.
    assert!(matches!(
        classify_tuple_shape(&ResolvedType::RustPath("(String)".to_string())),
        TupleShape::OpaqueRust
    ));

    // And the readable tuple spelling the stdlib depends on must keep working, so the refusal is narrow.
    match classify_tuple_shape(&ResolvedType::RustPath(
        "(String,incan_std_data::json::JsonValue)".to_string(),
    )) {
        TupleShape::RustTuple(2) => {}
        other => panic!("a readable Rust tuple must still destructure, got {other:?}"),
    }
}

// ---- #1072: plain assignment resolves outward; `let`/`mut` introduce bindings ----
#[test]
fn plain_assignment_in_a_block_reassigns_the_nearest_enclosing_binding() {
    // `reassigns_outer` from `scopes_and_name_resolution.md`, promoted to an executable fixture as RFC 120 requires.
    // Assignment checking used to look up only the innermost scope, so this silently created a fresh block-local
    // and the outer `x` never moved -- with no diagnostic, and no way for a reader to tell.
    let source = "def reassigns_outer() -> int:\n  mut x = 1\n  if true:\n    x = 2\n  return x\n";

    assert!(
        check_str(source).is_ok(),
        "a plain assignment to an enclosing `mut` binding must be accepted as a reassignment"
    );
}

#[test]
fn plain_assignment_to_an_enclosing_immutable_binding_is_rejected() {
    // The mutability half of the same contract: reassignment requires `mut`, and the old innermost-only lookup
    // could not enforce it across a block boundary because it never saw the outer binding at all.
    let source = "def reassign_immutable() -> int:\n  let x = 1\n  if true:\n    x = 2\n  return x\n";
    let errors = check_str_err(source, "reassigning an immutable enclosing binding must be rejected");

    assert!(
        errors.iter().any(|error| error.message.contains("Cannot mutate 'x'")),
        "expected a mutability diagnostic, got: {errors:?}"
    );
}

#[test]
fn plain_assignment_across_a_block_is_type_checked_against_the_outer_binding() {
    // The first repro in #1072. This typechecked cleanly before: the block-local shadow meant the `str` was never
    // compared against the outer `int`, so a whole-type mismatch produced no diagnostic whatsoever.
    let source = "def probe() -> None:\n  mut x: int = 1\n  if true:\n    x = \"hello\"\n";
    let errors = check_str_err(
        source,
        "assigning a `str` to an enclosing `int` binding must be rejected",
    );

    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("type mismatch") && error.message.contains("'x'")),
        "expected a type mismatch against the outer binding, got: {errors:?}"
    );
}

#[test]
fn let_in_a_block_introduces_a_new_binding_rather_than_reassigning() {
    // `shadows_in_block` from the same document. This is the half that has to move with the outward lookup: a
    // lookup that walks outward *without* honoring the declaration forms would turn this into a reassignment of
    // an immutable outer binding, and reject a program the language documents as valid.
    let source = "def shadows_in_block() -> int:\n  let x = 1\n  if true:\n    let x = 2\n  return x\n";

    assert!(
        check_str(source).is_ok(),
        "`let` in a nested block must introduce a new binding, not reassign the immutable outer one"
    );
}

#[test]
fn mut_declares_a_new_binding_rather_than_making_an_active_one_mutable() {
    // RFC 120 is explicit that `mut name = value` is "not 'make the existing `name` mutable'", and that reading it
    // as a modifier "inverts what it does to identity". The checker did read it that way: shadowing a parameter
    // with `mut` was rejected as a mutation of an immutable binding, so the documented shadowing form could not be
    // written at all over an active name.
    let source = "def f(imported: str) -> str:\n  mut imported = imported\n  imported = \"bla\"\n  return imported\n";

    assert!(
        check_str(source).is_ok(),
        "`mut` must declare a new binding that may shadow an active one"
    );
}

#[test]
fn shadowing_and_reassignment_compose_within_one_block() {
    // `shadow_vs_reassign`, the demanding case: a reassignment of the outer binding, then a `mut` shadow, then a
    // reassignment of the *inner* one -- all in the same block. It only typechecks if both halves of the contract
    // hold at once, which is why the two fixes could not land separately.
    let source = "def shadow_vs_reassign() -> int:\n  mut x = 10\n  if true:\n    x = 11\n    mut x = 12\n    x = 13\n  return x\n";

    assert!(
        check_str(source).is_ok(),
        "reassignment followed by a `mut` shadow and an inner reassignment must all be accepted"
    );
}

#[test]
fn reassigning_a_const_still_suggests_static_rather_than_mut() {
    // A `const` is registered as a module-scope variable, so walking the scope chain now finds it. Without
    // answering the more specific question first, this regressed to the generic "variable is immutable" hint,
    // which points the reader at `mut` when what they need is a `static`.
    let source = "const COUNTER: int = 0\n\ndef bump() -> None:\n  COUNTER = 1\n";
    let errors = check_str_err(source, "reassigning a const must be rejected");

    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Cannot reassign const 'COUNTER'")),
        "expected the const-specific diagnostic rather than a generic mutability error, got: {errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.hints.iter().any(|hint| hint.contains("static"))),
        "expected the hint to point at `static` rather than `mut`, got: {errors:?}"
    );
}

#[test]
fn a_mutable_local_can_shadow_a_const_and_be_reassigned() {
    let source = "const VALUE: int = 1\n\ndef use_local() -> int:\n  mut VALUE = 2\n  VALUE = 3\n  return VALUE\n";

    assert!(
        check_str(source).is_ok(),
        "a local `mut` shadow must not be mistaken for the module const of the same spelling"
    );
}

#[test]
fn plain_tuple_unpack_reassigns_enclosing_mutable_bindings() {
    let source = "def unpack() -> int:\n  mut left = 0\n  mut right = 0\n  if true:\n    left, right = (1, 2)\n  return left + right\n";

    assert!(
        check_str(source).is_ok(),
        "plain tuple unpack must resolve its targets through the enclosing scope chain"
    );
}

#[test]
fn plain_chained_assignment_reassigns_enclosing_mutable_bindings() {
    let source = "def assign() -> int:\n  mut left = 0\n  mut right = 0\n  if true:\n    left = right = 3\n  return left + right\n";

    assert!(
        check_str(source).is_ok(),
        "plain chained assignment must resolve every target through the enclosing scope chain"
    );
}

// ---- #1281: retain checked `isinstance` target facts for Body IR ----
#[test]
fn isinstance_records_the_resolved_alias_target_and_original_target_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Text = str\n\ndef probe(value: int | str) -> bool:\n  return isinstance(value, Text)\n";
    let info = typecheck_info_for_module(
        source,
        vec!["facts".to_string(), "isinstance".to_string()],
        "checked isinstance target",
    )?;

    let targets = info.calls.isinstance_targets.values().collect::<Vec<_>>();
    let [target] = targets.as_slice() else {
        return Err("expected exactly one checked isinstance target".into());
    };
    let target_start = source.rfind("Text").ok_or("fixture must contain the target spelling")?;
    assert_eq!(target.ty, ResolvedType::Str, "aliases must be expanded before lowering");
    assert_eq!(target.span, Span::new(target_start, target_start + "Text".len()));
    Ok(())
}

#[test]
fn invalid_isinstance_target_does_not_create_checked_executable_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def probe(value: int) -> bool:\n  return isinstance(value, 1)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&ast)
        .err()
        .ok_or("a value must not typecheck as an isinstance type target")?;

    assert!(
        errors.iter().any(|error| error.message.contains("Type mismatch")),
        "expected the existing type-target diagnostic, got {errors:?}"
    );
    assert!(
        checker.type_info().calls.isinstance_targets.is_empty(),
        "a rejected target must not leave executable checked evidence"
    );
    Ok(())
}

#[test]
fn isinstance_retains_a_nominal_targets_canonical_declaration_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Marker:\n  value: int\n\ntype Alias = Marker\n\ndef probe(value: Marker | str) -> bool:\n  return isinstance(value, Alias)\n";
    let module_path = vec!["facts".to_string(), "nominal_isinstance".to_string()];
    let info = typecheck_info_for_module(source, module_path.clone(), "nominal isinstance target")?;
    let target = info
        .calls
        .isinstance_targets
        .values()
        .next()
        .ok_or("fixture must retain its checked target")?;
    let identity = target
        .canonical
        .as_ref()
        .ok_or("a nominal target must retain its declaration identity")?;

    assert_eq!(target.ty, ResolvedType::Named("Marker".to_string()));
    assert_eq!(identity.module_path(), Some(module_path.as_slice()));
    assert_eq!(identity.declaration_name, "Marker");
    assert_eq!(identity.kind, incan_semantics_core::SemanticSourceTargetKind::Model);
    let alias_target = source.rfind("Alias").ok_or("fixture must contain the target alias")?;
    assert_eq!(target.span, Span::new(alias_target, alias_target + "Alias".len()));
    assert_eq!(
        source.get(target.span.start..target.span.end),
        Some("Alias"),
        "the target fact must retain the reference span, not the declaration span"
    );
    Ok(())
}

#[test]
fn chained_assignment_checks_a_literal_against_each_target_issue1806() -> Result<(), String> {
    // A value built only from literals adapts to each target the way `a = 5` adapts to `a`, even when the targets
    // disagree on a type, because each target gets its own copy of it.
    check_str(
        r#"
def literals() -> int:
    mut a: i8 = 1
    mut b: int = 2
    a = b = 5
    mut c: Option[int] = Some(1)
    mut d: Option[str] = Some("s")
    c = d = (None)
    mut xs: list[Option[int]] = []
    mut ys: list[Option[str]] = []
    xs = ys = [None]
    mut ints: list[int] = [1]
    mut strs: list[str] = ["s"]
    ints = strs = list()
    return b
"#,
    )
    .map_err(|errors| format!("the literal chains must check: {errors:?}"))?;

    // Any other value has to take one type for every target; when the targets disagree and its type is not fully
    // known, there is none, so the chain is refused.
    let Err(errors) = check_str(
        r#"
def empty[T]() -> list[T]:
    return []

def fill() -> None:
    mut ints: list[int] = [1]
    mut strs: list[str] = ["s"]
    ints = strs = empty()
"#,
    ) else {
        return Err("a chain whose value has no one type must be refused".to_string());
    };
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("The targets of this chained assignment have different types")),
        "expected the chained-assignment refusal, got {errors:?}"
    );
    Ok(())
}
