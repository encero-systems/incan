//! RFC 000 `@derive(Display)`: a type that derives `Display` displays as its `{value:?}` structure, the form the
//! display rule gives a model, class or enum value inside a collection, in every display position and through a
//! `Display` bound. The generated program compiles and prints it.

use incan_frontend::{lexer, parser};

use super::mut_ownership_regressions::run_generated_program;
use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// A model, class, enum, newtype and generic model that derive `Display` print their structure through `println`,
/// `str(...)`, an f-string and a `Display` bound.
#[test]
fn derived_display_prints_the_value_structure() -> TestResult {
    let rust = generate(
        r#"
@derive(Display)
model Point:
    x: int
    y: int

@derive(Display)
class Account:
    pub owner: str
    pub balance: int

@derive(Display)
enum Shape:
    Circle(float)
    Empty

@derive(Display)
type UserId = newtype int

@derive(Display)
model Box[T]:
    value: T

def show[T with Display](value: T) -> str:
    return f"<{value}>"

def main() -> None:
    p = Point(x=1, y=2)
    println(p)
    println(str(p))
    println(f"{p} and {Shape.Empty}")
    println(Account(owner="Ada", balance=3))
    println(Shape.Circle(1.5))
    println(UserId(5))
    println(Box(value="x"))
    println(show(p))
    println(show(UserId(6)))
"#,
    )?;
    let printed = run_generated_program(&rust)?;
    assert_eq!(
        printed.lines().collect::<Vec<_>>(),
        vec![
            "Point { x: 1, y: 2 }",
            "Point { x: 1, y: 2 }",
            "Point { x: 1, y: 2 } and Empty",
            "Account { owner: \"Ada\", balance: 3 }",
            "Circle(1.5)",
            "UserId(5)",
            "Box { value: \"x\" }",
            "<Point { x: 1, y: 2 }>",
            "<UserId(6)>",
        ]
    );
    Ok(())
}

/// A generic type that derives `Display` displays inside a generic function whose own type parameter is its type
/// argument: the function takes the `Debug` bound the type's display needs of it.
#[test]
fn derived_display_of_a_generic_type_displays_inside_a_generic_function() -> TestResult {
    let rust = generate(
        r#"
@derive(Display)
model Box[T]:
    value: T

def describe[U](b: Box[U]) -> str:
    println(b)
    return f"<{b}> {str(b)}"

def main() -> None:
    println(describe(Box(value=1)))
"#,
    )?;
    let printed = run_generated_program(&rust)?;
    assert_eq!(
        printed.lines().collect::<Vec<_>>(),
        vec!["Box { value: 1 }", "<Box { value: 1 }> Box { value: 1 }"]
    );
    Ok(())
}
