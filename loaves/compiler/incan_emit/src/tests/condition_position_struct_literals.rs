//! Struct-producing constructors at the exposed root of Rust condition-position expressions, compiled by rustc.

use std::error::Error;

use super::dependency_references::run_consumer;
use super::mut_ownership_regressions::compile_generated_rust;
use super::packages::parse;
use crate::IrCodegen;

/// Generate one program and compile it with rustc as a library.
fn build(case: &str, source: &str) -> Result<String, Box<dyn Error>> {
    let code = IrCodegen::new()
        .try_generate(&parse(source)?)
        .map_err(|error| format!("{case}: generation failed: {error:?}"))?;
    // The focused rustc harness has no generated `__incan_std` facade. Canonical testing assertions are already
    // expanded before Rust emission, so omit only their now-unused source import from this standalone crate.
    let compilable = code
        .lines()
        .filter(|line| !line.contains("pub use crate::__incan_std::testing::assert_eq;"))
        .collect::<Vec<_>>()
        .join("\n");
    compile_generated_rust(&compilable).map_err(|error| format!("{case} did not build: {error}\n{code}"))?;
    Ok(code)
}

/// #1561: struct literals exposed through top-level operators and receiver chains build in Rust condition positions.
#[test]
fn struct_literals_in_condition_positions_build_issue1561() -> Result<(), Box<dyn Error>> {
    build(
        "if, while, match, assert, and testing assert",
        r#"from std.testing import assert_eq

model Reading:
    pub value: int


pub def exercise() -> None:
    if Reading(value=3).value != 3:
        println("if")
    while Reading(value=3).value == 4:
        println("while")
    match Reading(value=3):
        Reading(value=v) if Reading(value=v).value == 3 => println(v)
        _ => println(0)
    assert Reading(value=3).value == 3
    assert_eq(Reading(value=3).value, 3)
    println(int(Reading(value=3).value == 3))
"#,
    )?;
    build(
        "elif, if expression, filters, for iterable, receiver chains, and constructor neighbors",
        r#"model Reading:
    pub value: int

    def is_three(self) -> bool:
        return self.value == 3


model Outer:
    pub inner: Reading
    pub values: list[int]


model Box[T]:
    pub value: T


class Flag:
    pub values: list[bool]


model Small:
    pub value: i8


@derive(Eq)
type Meters = newtype int


enum State:
    Ready(int)
    Empty


pub def exercise() -> None:
    if false:
        println("if")
    elif Outer(inner=Reading(value=3), values=[1]).inner.is_three():
        println("elif")
    chosen = if Reading(value=3).value == 3:
        1
    else:
        0
    squares = [x * x for x in [1, 2] if Reading(value=x).value > 0]
    lookup = {x: x for x in [1, 2] if not Reading(value=x).value < 0}
    generated = (x for x in [1, 2] if Reading(value=x).is_three())
    for value in Outer(inner=Reading(value=3), values=[3]).values:
        println(value)
    if Box[int](value=3).value == 3:
        println(len(squares) + len(lookup) + len(generated.collect()))
    if Flag(values=[true]).values[0]:
        println("class index")
    if Small(value=1i8).value < 2i16:
        println("numeric cast")
    if Meters(3) == Meters(3):
        println("newtype")
    match State.Ready(3):
        State.Ready(value) => println(value)
        State.Empty => println(0)
    println("done")
"#,
    )?;
    build(
        "behavior fixture",
        include_str!(
            "../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/struct_literals_in_condition_positions.incn"
        ),
    )?;
    let dependency_stdout = run_consumer(
        "conditionmodels",
        "pub model Reading:\n    pub value: int\n    pub values: list[int]\n",
        r#"from pub::conditionmodels import Reading


def main() -> None:
    if Reading(value=3, values=[1]).value == 3:
        println("dependency if")
    for value in Reading(value=3, values=[1]).values:
        println(value)
"#,
    )?;
    assert_eq!(dependency_stdout, "dependency if\n1\n");
    Ok(())
}
