//! Programs whose methods reach overrides and defaults through inheritance, built and run by rustc.

use incan_frontend::{lexer, parser};

use super::generated_programs::run_modules_with_stdlib;
use super::mut_ownership_regressions::run_generated_program;
use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse, check, lower and emit one module, returning the generated Rust.
fn generated_rust(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// #1841: a method a subclass inherits calls `self`'s override, and the override is kept in the program. The
/// inherited copy's `self` is the subclass, at any depth, while the declaring class keeps its own method.
#[test]
fn inherited_method_self_call_reaches_the_kept_override_issue1841() -> TestResult {
    let rust = generated_rust(
        r#"
class Animal:
    id: int

    def grow(self) -> int:
        return 1

    def feed(self) -> int:
        return self.grow()

    def twice(self) -> int:
        return self.feed() + self.feed()


class Dog extends Animal:
    def grow(self) -> int:
        return 2


class Puppy extends Dog:
    def label(self) -> int:
        return self.twice()


def main() -> None:
    println(Dog(id=1).feed())
    println(Puppy(id=1).label())
    println(Animal(id=1).feed())
"#,
    )?;
    assert_eq!(run_generated_program(&rust)?, "2\n4\n1\n");
    Ok(())
}

/// #1825: a subtrait's default fills its supertrait's slot, for a direct call and through a supertrait bound.
#[test]
fn subtrait_default_fills_the_supertrait_slot_issue1825() -> TestResult {
    let rust = generated_rust(
        r#"
trait Root:
    def label(self) -> str: ...


trait Child with Root:
    def label(self) -> str:
        return "child"


model Item with Child:
    value: int


def describe[T with Root](item: T) -> str:
    return item.label()


def main() -> None:
    println(Item(value=1).label())
    println(describe(Item(value=1)))
"#,
    )?;
    assert_eq!(run_generated_program(&rust)?, "child\nchild\n");
    Ok(())
}

/// A parameter typed by an imported subtrait can call a method declared by its unimported generic supertrait.
#[test]
fn subtrait_bound_exposes_the_unimported_supertrait_method() -> TestResult {
    let readers = r#"
pub trait Reader[T]:
    def read(self) -> T: ...


pub trait TaggedReader[T] with Reader[T]:
    def tag(self) -> str: ...


pub model Packet with TaggedReader[int]:
    pub value: int

    def read(self) -> int:
        return self.value

    def tag(self) -> str:
        return "packet"
"#;
    let main = r#"
from readers import Packet, TaggedReader


def read_tagged[T](value: TaggedReader[T]) -> T:
    return value.read()


def main() -> None:
    println(read_tagged(Packet(value=42)))
"#;
    assert_eq!(run_modules_with_stdlib(&[("readers", readers)], main)?, "42\n");
    Ok(())
}
