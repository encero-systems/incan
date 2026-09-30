//! Consumers that reach a `pub::` dependency's functions and trait methods, built against the dependency's generated
//! crate and run by rustc.

use std::sync::Arc;

use super::mut_ownership_regressions::run_generated_program_with_dependency;
use super::packages::{TestResult, parse, provider_plan_of, publish_package};
use crate::IrCodegen;

/// Publish `provider` as the dependency `name`, generate `consumer` against it, and run the consumer.
pub(super) fn run_consumer(name: &str, provider: &str, consumer: &str) -> Result<String, Box<dyn std::error::Error>> {
    let (manifest, provider_code) = publish_package(name, provider, &[])?;
    let program = parse(consumer)?;
    let mut codegen = IrCodegen::new();
    codegen.set_provider_plan(Arc::new(provider_plan_of(&[&manifest])?));
    let consumer_code = codegen.try_generate(&program)?;
    run_generated_program_with_dependency(name, &provider_code, &consumer_code)
}

/// #1840: a dependency function taken as a value through a module binding, aliased or not, is callable: bound to a
/// local and passed as an argument. No import brings the function into the consumer's scope, so the value reaches it
/// through the dependency's path.
#[test]
fn dependency_function_through_a_module_binding_is_a_callable_value_issue1840() -> TestResult {
    let provider = r#"
pub def show(prefix: str, label: str) -> str:
    return prefix + label


pub show_ace = partial show(label="ace")


pub def calculate(value: int) -> int:
    return value * 2
"#;
    let consumer = r#"
from pub::modulelib import show_ace
import pub::modulelib as cl
import pub::modulelib


def apply(f: (int) -> int, value: int) -> int:
    return f(value)


def main() -> None:
    println(show_ace("p:"))
    calculate = cl.calculate
    println(calculate(2))
    println(apply(cl.calculate, 3))
    whole = modulelib.calculate
    println(whole(4))
"#;
    assert_eq!(run_consumer("modulelib", provider, consumer)?, "p:ace\n4\n6\n8\n");
    Ok(())
}

/// A dependency whose model gets methods from the trait it adopts: a default, a generic default and a method the
/// model implements for the trait.
const PICKER: &str = r#"
pub trait Picker:
    def pick[K](self, items: list[K]) -> K:
        return items[0]

    def plain(self) -> int:
        return 1

    def required(self) -> int: ...


pub model Selector with Picker:
    pub id: int

    def required(self) -> int:
        return self.id
"#;

/// A consumer calls the methods a dependency's model has through its adopted trait, which the consumer never
/// imports: the calls name the trait through the dependency, since a Rust trait method is callable only with its
/// trait in scope.
#[test]
fn adopted_trait_methods_of_a_dependency_type_are_callable_without_the_trait() -> TestResult {
    let consumer = r#"
from pub::bounds import Selector


def main() -> None:
    selector = Selector(id=3)
    println(selector.plain())
    println(selector.required())
    println(selector.pick(["picked"]))
"#;
    assert_eq!(run_consumer("bounds", PICKER, consumer)?, "1\n3\npicked\n");
    Ok(())
}

/// A generic dependency trait's defaults, called through an adopter the consumer imports without the trait, run on
/// the adoption's type argument.
#[test]
fn generic_trait_default_of_a_dependency_type_is_callable_without_the_trait() -> TestResult {
    let provider = r#"
pub trait Failing[E]:
    def failure(self) -> E: ...

    def failures(self) -> list[E]:
        return [self.failure(), self.failure()]


pub model Lookup with Failing[str]:
    pub key: str

    def failure(self) -> str:
        return self.key
"#;
    let consumer = r#"
from pub::failing import Lookup


def main() -> None:
    failures = Lookup(key="bad").failures()
    println(f"{len(failures)} {failures[0]}")
"#;
    assert_eq!(run_consumer("failing", provider, consumer)?, "2 bad\n");
    Ok(())
}
