//! A record pattern over a generic model or class names the fields of the instantiated type, and a variant pattern over
//! a generic enum its instantiated payloads; the generated Rust destructures the generic struct and enum.

use incan_frontend::{lexer, parser};

use super::mut_ownership_regressions::compile_generated_rust;
use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// Record patterns over a generic model and a generic class, with literal, binding and rest sub-patterns, and a literal
/// payload pattern over a generic enum, compile.
#[test]
fn record_patterns_over_a_generic_model_and_class_compile() -> TestResult {
    let rust = generate(
        r#"
model Box[T]:
    value: T
    label: str

class Pair[A, B]:
    pub first: A
    pub second: B

def unbox(b: Box[int]) -> int:
    match b:
        Box(value=0) => return -1
        Box(value=v, label="x") => return v
        Box(value=v) => return v + 1

def first(p: Pair[str, int]) -> str:
    match p:
        Pair(second=2, first=f) => return f
        Pair(first=f) => return f

enum Shape[T]:
    Filled(T)
    Empty

def area(shape: Shape[int]) -> int:
    match shape:
        Shape.Filled(0) => return -1
        Shape.Filled(n) => return n
        Shape.Empty => return 0

def main() -> None:
    println(unbox(Box(value=3, label="y")))
    println(first(Pair(first="a", second=2)))
    println(area(Shape.Filled(4)))
"#,
    )?;
    compile_generated_rust(&rust)
}
