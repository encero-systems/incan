//! A `mut` scalar parameter of a trait method (#1827): the bodiless trait slot declares the parameter without a `mut`
//! binding, and each implementing body keeps `mut`.

use std::collections::HashMap;

use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};

/// Return the emitted Rust for one checked source module.
fn emitted_rust(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let ast = parser::parse(&lexer::lex(source).map_err(|errors| format!("{errors:?}"))?)
        .map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast).map_err(|errors| format!("{errors:?}"))?;
    let mut codegen = crate::IrCodegen::new();
    codegen.set_prechecked_type_info(checker.type_info().clone(), HashMap::new());
    let (rust, _) = codegen.try_generate_with_metadata(&ast, &["main".to_string()])?;
    Ok(rust)
}

/// Return the text of the first item that starts with `header`, up to its closing brace at column zero.
fn item<'a>(rust: &'a str, header: &str) -> Result<&'a str, String> {
    let start = rust
        .find(header)
        .ok_or_else(|| format!("`{header}` absent from:\n{rust}"))?;
    let end = rust[start..]
        .find("\n}")
        .map(|offset| start + offset)
        .ok_or_else(|| format!("`{header}` has no closing brace in:\n{rust}"))?;
    Ok(&rust[start..end])
}

/// Issue #1827: an abstract and a default trait method with `mut` parameters of type `int`, an `int` alias and
/// `float` declare those parameters without `mut` in the trait, and the implementations keep `mut`.
#[test]
fn trait_slots_declare_mut_scalar_parameters_without_a_binding() -> Result<(), Box<dyn std::error::Error>> {
    let rust = emitted_rust(
        r#"
type Count = int

trait Stepper:
    def step(self, mut n: Count) -> int

    def plain(self, mut n: int) -> int:
        return n + 1

    def bumped(self, mut x: float) -> float:
        return x + 1.0

class Walker with Stepper:
    def step(self, mut n: Count) -> int:
        return n + 1

def main() -> None:
    walker = Walker()
    println(walker.step(1) + walker.plain(1))
    println(walker.bumped(1.5))
"#,
    )?;
    let slots = item(&rust, "trait Stepper")?;
    for declared in ["n: Count", "n: i64", "x: f64"] {
        assert!(
            slots.contains(declared),
            "the trait slot must declare `{declared}`, got:\n{slots}"
        );
    }
    assert!(
        !slots.contains("mut n") && !slots.contains("mut x"),
        "a bodiless trait slot must not bind a parameter `mut`, got:\n{slots}"
    );
    let implementation = item(&rust, "impl Stepper for Walker")?;
    for bound in ["mut n: Count", "mut n: i64", "mut x: f64"] {
        assert!(
            implementation.contains(bound),
            "the implementation must declare `{bound}`, got:\n{implementation}"
        );
    }
    Ok(())
}
