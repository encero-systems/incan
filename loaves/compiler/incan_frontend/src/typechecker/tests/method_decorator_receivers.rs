//! Method-decorator receivers (#1790): a decorator's shapes and the functions it returns in the method's place spell
//! the receiver the way the method does, the `mut` marker on callable-type parameters, the `INCAN-T0110` refusal of the
//! `&Owner` spellings, and the `INCAN-T0116` rules for the chains whose receiver the compiler plans.

use super::*;

/// A checked program and the checker that holds the facts it recorded for lowering.
type CheckedProgram = (crate::ast::Program, TypeChecker);

/// Check `source`, returning the checker so a test can read the facts it recorded for lowering.
fn checked(source: &str) -> Result<CheckedProgram, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    Ok((ast, checker))
}

/// Return the receiver slot the checker recorded for the function declaration named `name`.
fn receiver_slot_of(
    ast: &crate::ast::Program,
    checker: &TypeChecker,
    name: &str,
) -> Option<MethodDecoratorReceiverSlot> {
    let decl = ast
        .declarations
        .iter()
        .find(|decl| matches!(&decl.node, crate::ast::Declaration::Function(function) if function.name == name))?;
    checker
        .type_info()
        .declarations
        .method_decorator_receiver_slots
        .get(&(decl.span.start, decl.span.end))
        .copied()
}

/// A shared-receiver slot in `role`.
fn shared(role: MethodDecoratorReceiverRole) -> Option<MethodDecoratorReceiverSlot> {
    Some(MethodDecoratorReceiverSlot { mutable: false, role })
}

/// Assert that `source` is refused with `code` and a message containing `expected`.
fn assert_refused(source: &str, code: &str, expected: &str) {
    let errors = check_str_err(source, "the program must be refused");
    assert!(
        errors
            .iter()
            .any(|err| err.stable_code() == Some(code) && err.message.contains(expected)),
        "expected {code} containing `{expected}`, got {errors:?}"
    );
}

/// The `Box` class with one `self` method decorated by `@as_int`, followed by `rest`.
fn boxed_label_program(rest: &str) -> String {
    format!(
        r#"
class Box:
  pub value: int

  @as_int
  def label(self, value: int) -> str:
    return "value"
{rest}"#
    )
}

/// Issue #1790: a `self` method's decorator and the function it returns in the method's place take the owner type.
/// The recorded binding keeps the form the method's wrapper passes the receiver in, and the decorator and its
/// replacement are planned for lowering.
#[test]
fn self_method_decorator_and_replacement_take_the_owner_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = boxed_label_program(
        r#"
def parse(box: Box, value: int) -> int:
  return box.value + value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return parse

def main(box: Box) -> int:
  return box.label(1) + parse(box, 2)
"#,
    );
    let (ast, checker) = checked(&source)?;
    let binding = checker
        .type_info()
        .declarations
        .decorated_method_bindings
        .get(&("Box".to_string(), "label".to_string()))
        .ok_or("expected a decorated binding for Box.label")?;
    assert_eq!(binding.unbound_ty.to_string(), "(&Box, int) -> int");
    assert_eq!(binding.original_unbound_ty.to_string(), "(&Box, int) -> str");
    assert_eq!(
        receiver_slot_of(&ast, &checker, "as_int"),
        shared(MethodDecoratorReceiverRole::Decorator)
    );
    assert_eq!(
        receiver_slot_of(&ast, &checker, "parse"),
        shared(MethodDecoratorReceiverRole::Replacement)
    );
    assert_eq!(receiver_slot_of(&ast, &checker, "main"), None);
    Ok(())
}

/// Issue #1790: a factory whose shapes name the receiver is planned with the decorator it returns and that
/// decorator's replacement.
#[test]
fn factory_is_planned_with_the_decorator_it_returns() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
class Box:
  pub value: int

  @labeled("x")
  def label(self, value: int) -> str:
    return "value"

def parse(box: Box, value: int) -> int:
  return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return parse

def labeled(name: str) -> ((Box, int) -> str) -> (Box, int) -> int:
  return as_int
"#;
    let (ast, checker) = checked(source)?;
    assert_eq!(
        receiver_slot_of(&ast, &checker, "labeled"),
        shared(MethodDecoratorReceiverRole::Factory)
    );
    assert_eq!(
        receiver_slot_of(&ast, &checker, "as_int"),
        shared(MethodDecoratorReceiverRole::Decorator)
    );
    assert_eq!(
        receiver_slot_of(&ast, &checker, "parse"),
        shared(MethodDecoratorReceiverRole::Replacement)
    );
    Ok(())
}

/// Issue #1790: every `return` of a decorator is followed, including those inside `if` branches and `match` arms,
/// and each function returned by name is planned.
#[test]
fn returns_in_branches_and_match_arms_are_planned() -> Result<(), Box<dyn std::error::Error>> {
    let source = boxed_label_program(
        r#"
const LEVEL: int = 1

def strict(box: Box, value: int) -> int:
  return value

def lenient(box: Box, value: int) -> int:
  return 0

def fallback(box: Box, value: int) -> int:
  return -1

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  if LEVEL > 5:
    return fallback
  match LEVEL:
    1 => return strict
    _ => return lenient
"#,
    );
    let (ast, checker) = checked(&source)?;
    for name in ["strict", "lenient", "fallback"] {
        assert_eq!(
            receiver_slot_of(&ast, &checker, name),
            shared(MethodDecoratorReceiverRole::Replacement),
            "`{name}` is returned in the method's place"
        );
    }
    Ok(())
}

/// Issue #1790: a `mut self` method's chain marks the receiver `mut`, and a `def` whose first parameter is `mut`
/// takes the method's place. The marker already says how the receiver is passed, so nothing is planned.
#[test]
fn mut_self_method_decorator_marks_the_receiver_mut() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
class Counter:
  pub value: int

  @keep
  def bump(mut self) -> int:
    self.value += 1
    return self.value

  @doubled
  def add(mut self, by: int) -> int:
    self.value += by
    return self.value

def keep(func: (mut Counter) -> int) -> (mut Counter) -> int:
  return func

def grow(mut counter: Counter, by: int) -> int:
  counter.value += by * 2
  return counter.value

def doubled(func: (mut Counter, int) -> int) -> (mut Counter, int) -> int:
  return grow
"#;
    let (ast, checker) = checked(source)?;
    let binding = checker
        .type_info()
        .declarations
        .decorated_method_bindings
        .get(&("Counter".to_string(), "add".to_string()))
        .ok_or("expected a decorated binding for Counter.add")?;
    assert_eq!(binding.unbound_ty.to_string(), "(&mut Counter, int) -> int");
    assert_eq!(receiver_slot_of(&ast, &checker, "grow"), None);
    assert_eq!(receiver_slot_of(&ast, &checker, "doubled"), None);
    Ok(())
}

/// Issue #1790: the `&Owner` and `&mut Owner` receiver spellings are refused with `INCAN-T0110`, whose remedy names
/// the spelling to write, in the shape a decorator accepts, the shape it returns, and on the function it returns.
#[test]
fn receiver_written_with_ampersand_is_refused() {
    let cases = [
        (
            boxed_label_program(
                r#"
def parse(box: Box, value: int) -> int:
  return value

def as_int(func: (&Box, int) -> str) -> (Box, int) -> int:
  return parse
"#,
            ),
            "&Box",
            "(Box, int) -> str",
        ),
        (
            boxed_label_program(
                r#"
def as_int(func: (Box, int) -> str) -> (&Box, int) -> str:
  return func
"#,
            ),
            "&Box",
            "(Box, int) -> str",
        ),
        (
            r#"
class Counter:
  pub value: int

  @keep
  def bump(mut self, by: int) -> int:
    self.value += by
    return self.value

def keep(func: (&mut Counter, int) -> int) -> (&mut Counter, int) -> int:
  return func
"#
            .to_string(),
            "&mut Counter",
            "(mut Counter, int) -> int",
        ),
        (
            boxed_label_program(
                r#"
def parse(box: &Box, value: int) -> int:
  return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return parse
"#,
            ),
            "&Box",
            "box: Box",
        ),
    ];
    for (source, written, replacement) in cases {
        let errors = check_str_err(&source, "a receiver written with `&` must be refused");
        assert!(
            errors.iter().any(|err| err.stable_code() == Some("INCAN-T0110")
                && err.message.contains(&format!("as `{written}`"))
                && err.hints.iter().any(|hint| hint.contains(&format!("`{replacement}`"))
                    && hint.contains("the compiler decides how the receiver is passed"))),
            "expected INCAN-T0110 for `{written}` naming `{replacement}`, got {errors:?}"
        );
    }
}

/// Issue #1790: the receiver's `mut` marker follows the method, in the decorator's shapes and on the function it
/// returns in the method's place.
#[test]
fn receiver_mut_marker_must_match_the_method() {
    let counter = |rest: &str| {
        format!(
            r#"
class Counter:
  pub value: int

  @keep
  def bump(mut self, by: int) -> int:
    self.value += by
    return self.value
{rest}"#
        )
    };
    let cases = [
        (
            counter(
                r#"
def keep(func: (Counter, int) -> int) -> (Counter, int) -> int:
  return func
"#,
            ),
            "Method decorator '@keep' takes the receiver of 'bump' without `mut`",
        ),
        (
            boxed_label_program(
                r#"
def as_int(func: (mut Box, int) -> str) -> (mut Box, int) -> str:
  return func
"#,
            ),
            "Method decorator '@as_int' marks the receiver of 'label' `mut`",
        ),
        (
            counter(
                r#"
def keep(func: (mut Counter, int) -> int) -> (Counter, int) -> int:
  return func
"#,
            ),
            "Method decorator '@keep' returns a shape that takes the receiver of 'bump' without `mut`",
        ),
        (
            counter(
                r#"
def peek(counter: Counter, by: int) -> int:
  return counter.value + by

def keep(func: (mut Counter, int) -> int) -> (mut Counter, int) -> int:
  return peek
"#,
            ),
            "Return type mismatch: expected '(mut Counter, int) -> int', found '(Counter, int) -> int'",
        ),
        (
            boxed_label_program(
                r#"
def parse(mut box: Box, value: int) -> int:
  return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return parse
"#,
            ),
            "Function 'parse', returned by '@as_int', marks the receiver of 'label' `mut`",
        ),
    ];
    for (source, expected) in cases {
        let errors = check_str_err(
            &source,
            "a receiver `mut` marker that differs from the method must be refused",
        );
        assert!(
            errors.iter().any(|err| err.message.contains(expected)),
            "expected `{expected}`, got {errors:?}"
        );
    }
}

/// Issue #1790: the `mut` marker decides how an argument is passed, so function types agree on it exactly, and a
/// `def` parameter declared `mut` carries it unless its argument is handed over whole (`int`, `float`, `bool`).
#[test]
fn mut_marker_is_part_of_the_function_type() {
    assert_check_ok(
        r#"
class Counter:
  pub value: int

def grow(mut counter: Counter) -> int:
  counter.value += 1
  return counter.value

def step(mut n: int) -> int:
  return n + 1

def pick_grow() -> (mut Counter) -> int:
  return grow

def pick_step() -> (int) -> int:
  return step
"#,
    );
    let cases = [
        (
            r#"
class Counter:
  pub value: int

def widen(func: (Counter) -> int) -> (mut Counter) -> int:
  return func
"#,
            "found '(Counter) -> int'",
        ),
        (
            r#"
class Counter:
  pub value: int

def narrow(func: (mut Counter) -> int) -> (Counter) -> int:
  return func
"#,
            "found '(mut Counter) -> int'",
        ),
        (
            r#"
class Counter:
  pub value: int

def grow(mut counter: Counter) -> int:
  counter.value += 1
  return counter.value

def pick() -> (Counter) -> int:
  return grow
"#,
            "found '(mut Counter) -> int'",
        ),
    ];
    for (source, expected) in cases {
        let errors = check_str_err(source, "callable types that disagree on `mut` must be refused");
        assert!(
            errors.iter().any(|err| err.message.contains(expected)),
            "expected `{expected}`, got {errors:?}"
        );
    }
}

/// Issue #1790: a direct call of a returned function in its module is passed the receiver the planned way, so it is
/// accepted; every other use of a planned declaration is refused with `INCAN-T0116`.
#[test]
fn planned_declarations_are_used_only_in_the_chain() {
    let chain = r#"
def parse(box: Box, value: int) -> int:
  return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return parse
"#;
    assert_check_ok(&boxed_label_program(&format!(
        "{chain}\ndef direct(box: Box) -> int:\n  return parse(box, 3)\n"
    )));
    let cases = [
        (
            "\ndef apply(f: (Box, int) -> int, box: Box) -> int:\n  return f(box, 1)\n\ndef main(box: Box) -> int:\n  return apply(parse, box)\n",
            "'parse' takes the receiver in the decorator chain of '@as_int', so it is only returned in the method's place or called directly",
        ),
        (
            "\n@as_int\ndef free(box: Box, value: int) -> str:\n  return \"free\"\n",
            "'as_int' takes the receiver in its shapes, so it is only applied as a decorator to `self` methods",
        ),
        (
            "\ndef other(box: Box, value: int) -> str:\n  return \"other\"\n\ndef main() -> (Box, int) -> int:\n  return as_int(other)\n",
            "'as_int' takes the receiver in its shapes, so it is only applied as a decorator to `self` methods",
        ),
    ];
    for (rest, expected) in cases {
        assert_refused(&boxed_label_program(&format!("{chain}{rest}")), "INCAN-T0116", expected);
    }
    assert_refused(
        &boxed_label_program(
            r#"
pub def parse(box: Box, value: int) -> int:
  return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return parse
"#,
        ),
        "INCAN-T0116",
        "'parse' is declared `pub`",
    );
}

/// Issue #1790: a decorator whose shapes name the receiver returns the decorated callable or a function of this
/// module by name; any other returned value is refused with `INCAN-T0116`.
#[test]
fn unplannable_returns_are_refused() {
    assert_refused(
        &boxed_label_program(
            r#"
def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  return (box, value) => value
"#,
        ),
        "INCAN-T0116",
        "it returns an expression",
    );
    assert_refused(
        &boxed_label_program(
            r#"
def parse(box: Box, value: int) -> int:
  return value

def as_int(func: (Box, int) -> str) -> (Box, int) -> int:
  chosen = parse
  return chosen
"#,
        ),
        "INCAN-T0116",
        "it returns 'chosen'",
    );
}

/// Issue #1790: a decorator whose shapes name a `self` method's receiver is a function of the method's module with
/// its shapes written as callable types; one imported from another module, one reached through a value, and one
/// whose shapes go through a type alias are refused with `INCAN-T0116`.
#[test]
fn decorators_the_receiver_cannot_be_planned_for_are_refused() -> Result<(), String> {
    let provider = parse_program(
        r#"
pub def log[S](label: str) -> ((S, int) -> str) -> (S, int) -> str:
  return (func) => func
"#,
        "issue1790 decorator provider",
    );
    let consumer = parse_program(
        r#"
from helpers import log

class Box:
  pub value: int

  @log("label")
  def label(self, value: int) -> str:
    return "value"
"#,
        "issue1790 decorator consumer",
    );
    let mut checker = TypeChecker::new();
    let Err(errors) = checker.check_with_imports(&consumer, &[("helpers", &provider)]) else {
        return Err("an imported decorator that names the receiver must be refused".to_string());
    };
    assert!(
        errors
            .iter()
            .any(|err| err.stable_code() == Some("INCAN-T0116")
                && err.message.contains("it is declared in another module")),
        "expected INCAN-T0116 for the imported decorator, got {errors:?}"
    );

    assert_refused(
        r#"
class Box:
  pub value: int

  @REGISTRY.add("label")
  def label(self, value: int) -> str:
    return "value"

def keep(func: (Box, int) -> str) -> (Box, int) -> str:
  return func

@derive(Clone)
class Registry:
  @staticmethod
  def new() -> Self:
    return Registry()

  def add(self, name: str) -> ((Box, int) -> str) -> (Box, int) -> str:
    return keep

static REGISTRY: Registry = Registry.new()
"#,
        "INCAN-T0116",
        "it is reached through a value or a method",
    );
    assert_refused(
        &boxed_label_program(
            r#"
type LabelShape = (Box, int) -> str

def as_int(func: LabelShape) -> LabelShape:
  return func
"#,
        ),
        "INCAN-T0116",
        "written through a type alias",
    );
    Ok(())
}

/// Issue #1790: a decorator factory generic over the whole callable names no receiver position, so it needs no plan.
#[test]
fn generic_decorators_need_no_plan() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
class Box:
  pub value: int

  @preserve()
  def label(self, value: int) -> str:
    return "value"

def preserve[F]() -> ((F) -> F):
  return (func) => func
"#;
    let (ast, checker) = checked(source)?;
    assert_eq!(receiver_slot_of(&ast, &checker, "preserve"), None);
    Ok(())
}
