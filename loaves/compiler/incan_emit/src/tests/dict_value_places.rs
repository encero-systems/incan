//! A dict value is a place: a method call that changes it, an assignment through it and a loop that changes its items
//! build and change the stored value in place, through a `mut` local, a field of `self`, an element of a list, a nested
//! dict and a `mut` dict parameter. A `mut self` method called on a list element or a dict value, and a loop that
//! changes the items of a list field of one, change the stored value rather than a copy read out of the collection
//! (#1561).

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

/// A changing method, an element or field assignment and a loop reach a dict value in place, under a literal, a
/// variable and an `int` key, through a tuple in the value, a nested dict, a list of dicts and a field of `self`.
#[test]
fn dict_value_changes_in_place_through_a_mut_dict_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Registry:
    pub groups: dict[str, list[int]]

    def add(mut self, value: int) -> None:
        self.groups["a"].append(value)


def main() -> None:
    mut d: dict[str, list[int]] = {"a": [1]}
    d["a"].append(2)
    key = "a"
    d[key].append(3)
    d["a"][0] = 10
    d["a"][0] += 1
    mut by_id: dict[int, list[int]] = {1: []}
    id = 1
    by_id[id].append(4)
    by_id[1].append(5)
    mut paired: dict[str, tuple[list[int], int]] = {"a": ([], 0)}
    paired["a"][0].append(6)
    mut nested: dict[str, dict[str, list[int]]] = {"a": {"b": []}}
    nested["a"]["b"].append(7)
    mut rows: list[dict[str, list[int]]] = [{"a": []}]
    rows[0]["a"].append(8)
    mut registry = Registry(groups={"a": []})
    registry.add(9)
    registry.groups["a"].append(10)
    mut grid: dict[str, list[list[int]]] = {"a": [[], []]}
    for row in grid["a"]:
        row.append(11)
    println(d["a"])
    println(by_id[1])
    println(paired["a"][0])
    println(nested["a"]["b"])
    println(rows[0]["a"])
    println(registry.groups["a"])
    println(grid["a"])
"#,
    )?;
    assert_eq!(output, "[11, 2, 3]\n[4, 5]\n[6]\n[7]\n[8]\n[9, 10]\n[[11], [11]]\n");
    Ok(())
}

/// A `mut self` method, a field assignment and a compound field assignment through a dict value change the stored
/// model, through a `mut` local and through a `mut` dict parameter, whose `str` key is read as `str` too.
#[test]
fn dict_value_of_a_model_changes_in_place_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1


def bump_all(mut counters: dict[str, Counter]) -> None:
    counters["a"].bump()
    counters["a"].count += 10


def main() -> None:
    mut counters: dict[str, Counter] = {"a": Counter(count=0)}
    counters["a"].bump()
    counters["a"].count = counters["a"].count + 5
    bump_all(counters)
    mut held: dict[str, tuple[Counter, int]] = {"a": (Counter(count=0), 1)}
    held["a"][0].bump()
    println(counters["a"].count)
    println(held["a"][0].count)
"#,
    )?;
    assert_eq!(output, "17\n1\n");
    Ok(())
}

/// A `mut self` method called on a list element, on a list element of a field and on a field of a list element changes
/// the element in the list.
#[test]
fn mut_self_call_on_a_list_element_changes_it_in_place_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Counter:
    pub count: int

    def bump(mut self) -> None:
        self.count += 1


class Holder:
    pub counters: list[Counter]


class Wrap:
    pub inner: Counter


def main() -> None:
    mut counters = [Counter(count=0)]
    counters[0].bump()
    mut holder = Holder(counters=[Counter(count=0)])
    holder.counters[0].bump()
    holder.counters[-1].bump()
    mut wraps = [Wrap(inner=Counter(count=0))]
    wraps[0].inner.bump()
    println(counters[0].count)
    println(holder.counters[0].count)
    println(wraps[0].inner.count)
"#,
    )?;
    assert_eq!(output, "1\n2\n1\n");
    Ok(())
}

/// A loop that changes the items of a list field of a list element, or of a dict value, changes them in the collection
/// rather than in a copy read out of it.
#[test]
fn loop_over_a_list_field_of_an_element_changes_it_in_place_issue1561() -> TestResult {
    let output = program_output(
        r#"
class Holder:
    pub rows: list[list[int]]


def main() -> None:
    mut holders = [Holder(rows=[[1], [2]])]
    for row in holders[0].rows:
        row.append(3)
    mut by_name: dict[str, Holder] = {"a": Holder(rows=[[4]])}
    for row in by_name["a"].rows:
        row.append(5)
    println(holders[0].rows)
    println(by_name["a"].rows)
"#,
    )?;
    assert_eq!(output, "[[1, 3], [2, 3]]\n[[4, 5]]\n");
    Ok(())
}
