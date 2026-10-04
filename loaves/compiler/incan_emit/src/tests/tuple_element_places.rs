//! A constant tuple index names a tuple element as a place: a method that changes the element, an assignment through
//! it, and a read of it build and change the tuple in place, whether the tuple is a local, a field of `self`, an
//! element of a list, the variable of a `for` loop, a nested tuple or a `mut` parameter (#1561).

use incan_frontend::{lexer, parser};

use super::mut_ownership_regressions::run_generated_program;
use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse, check, lower and emit one program, then build and run it and return its standard output.
fn program_output(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let rust = IrCodegen::new().try_generate(&program)?;
    run_generated_program(&rust)
}

/// A method that changes a tuple element changes the tuple in place: through a local tuple, a nested tuple, a list of
/// tuples, the variable of a `for` loop over that list, and an index counted from the end.
#[test]
fn tuple_element_method_changes_the_tuple_in_place_issue1561() -> TestResult {
    let output = program_output(
        r#"
def main() -> None:
    mut pair: tuple[list[int], int] = ([], 0)
    pair[0].append(1)
    pair[0].append(2)
    mut outer: tuple[int, tuple[list[int], int]] = (0, ([], 0))
    outer[1][0].append(3)
    mut rows: list[tuple[list[int], int]] = [([], 0)]
    rows[0][0].append(4)
    for row in rows:
        row[0].append(5)
    mut tail: tuple[int, list[int]] = (0, [])
    tail[-1].append(6)
    println(len(pair[0]))
    println(outer[1][0][0])
    println(rows[0][0][0])
    println(rows[0][0][1])
    println(tail[1][0])
"#,
    )?;
    assert_eq!(output, "2\n3\n4\n5\n6\n");
    Ok(())
}

/// A tuple field of `self` changes in place through a `mut self` method and through a field of a `mut` binding.
#[test]
fn tuple_element_of_a_field_changes_in_place_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Holder:
    pub pair: tuple[list[int], int]

    def add(mut self, value: int) -> None:
        self.pair[0].append(value)


def main() -> None:
    mut holder = Holder(pair=([], 0))
    holder.add(7)
    holder.pair[0].append(8)
    println(len(holder.pair[0]))
    println(holder.pair[0][1])
"#,
    )?;
    assert_eq!(output, "2\n8\n");
    Ok(())
}

/// An assignment through a tuple element, single or in a tuple assignment, writes the element's own element or field,
/// and a `mut self` method called on a tuple element changes it.
#[test]
fn writes_through_a_tuple_element_change_the_tuple_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1


def main() -> None:
    mut pair: tuple[list[int], int] = ([0], 0)
    pair[0][0] = 5
    pair[0][0] += 1
    mut counted: tuple[Counter, int] = (Counter(count=1), 0)
    counted[0].count = 10
    counted[0].bump()
    mut spare = 0
    counted[0].count, spare = (counted[0].count + 1, 7)
    println(pair[0][0])
    println(counted[0].count + spare)
"#,
    )?;
    assert_eq!(output, "6\n19\n");
    Ok(())
}

/// A `mut` tuple parameter reads and changes its elements, and the change reaches the caller's field.
#[test]
fn mut_tuple_parameter_elements_are_places_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Holder:
    pub pair: tuple[list[int], int]


def extend(mut pair: tuple[list[int], int], value: int) -> int:
    pair[0].append(value)
    return len(pair[0]) + pair[1]


def main() -> None:
    println(extend(([1], 10), 2))
    mut holder = Holder(pair=([], 0))
    println(extend(holder.pair, 3))
    println(holder.pair[0][0])
"#,
    )?;
    assert_eq!(output, "12\n1\n3\n");
    Ok(())
}
