//! A variant pattern names its variant through the subject's enum: an unqualified variant (`Filled(n)`), a variant
//! alias and a variant nested in another pattern each match the enum's own variant, and the generated Rust compiles.

use incan_frontend::{lexer, parser};

use super::mut_ownership_regressions::compile_generated_rust;
use crate::IrCodegen;
use crate::test_support::parse_program_result;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// Unqualified variant patterns over a plain and a generic enum, at the top of an arm, in an alternation, nested in
/// `Some(...)` and in `if let`, and variant aliases spelled bare or qualified, compile.
#[test]
fn unqualified_and_aliased_variant_patterns_compile() -> TestResult {
    let rust = generate(
        r#"
enum Plain:
    Circle(float)
    Dot

enum Shape[T]:
    Filled(T)
    Pair(T, T)
    Empty
    Full = alias Filled

def radius(c: Plain) -> float:
    match c:
        Circle(r) => return r
        Plain.Dot => return 0.0

def total(s: Shape[int]) -> int:
    match s:
        Filled(0) => return -1
        Filled(n) => return n
        Pair(a, _) | Pair(_, a) => return a
        Shape.Empty => return 0

def nested(o: Option[Shape[int]]) -> int:
    match o:
        Some(Filled(n)) => return n
        Some(Pair(a, b)) => return a + b
        _ => return 0

def aliased(s: Shape[int]) -> int:
    match s:
        Full(n) => return n
        Shape.Full(n) => return n + 1
        _ => return 0

def first(s: Shape[int]) -> int:
    if let Filled(n) = s:
        return n
    return 0

def main() -> None:
    println(radius(Plain.Circle(1.5)))
    println(total(Shape.Filled(3)))
    println(nested(Some(Shape.Pair(4, 5))))
    println(aliased(Shape.Filled(6)))
    println(first(Shape.Filled(7)))
"#,
    )?;
    compile_generated_rust(&rust)
}

/// A module that matches a value of an enum another module declares names the variant through the enum's name when it
/// imports the enum, and from the crate root when it does not.
#[test]
fn variant_patterns_over_another_modules_enum_name_a_path_the_module_can_reach() -> TestResult {
    let shapes = parse_program_result(
        "pub enum Shape[T]:\n    Filled(T)\n    Empty\n    Full = alias Filled\n\npub def make(n: int) -> Shape[int]:\n    return Shape.Filled(n)\n",
    )?;
    let named = parse_program_result(
        "from shapes import Shape\n\npub def total(s: Shape[int]) -> int:\n    match s:\n        Full(n) => return n\n        _ => return 0\n",
    )?;
    let unnamed = parse_program_result(
        "from shapes import make\n\npub def first(k: int) -> int:\n    if let Filled(n) = make(k):\n        return n\n    return 0\n",
    )?;
    let root = parse_program_result(
        "from named import total\nfrom unnamed import first\nfrom shapes import make\n\ndef main() -> None:\n    println(total(make(1)) + first(2))\n",
    )?;
    let paths = [["shapes"], ["named"], ["unnamed"]].map(|path| path.map(str::to_string).to_vec());
    let mut codegen = IrCodegen::new();
    for ((name, ast), path) in [("shapes", &shapes), ("named", &named), ("unnamed", &unnamed)]
        .into_iter()
        .zip(&paths)
    {
        codegen.add_module_with_path_segments(name, ast, path.clone());
    }
    let (_, modules) = codegen.try_generate_multi_file_nested(&root, &paths)?;
    let compact = |name: &str| -> Result<String, String> {
        modules
            .get(&vec![name.to_string()])
            .map(|code| code.chars().filter(|character| !character.is_whitespace()).collect())
            .ok_or_else(|| format!("no generated module `{name}`"))
    };
    let named = compact("named")?;
    assert!(
        named.contains("Shape::Filled(n)=>") && !named.contains("crate::shapes::Shape::Filled"),
        "{named}"
    );
    let unnamed = compact("unnamed")?;
    assert!(unnamed.contains("crate::shapes::Shape::Filled(n)=>"), "{unnamed}");
    Ok(())
}
