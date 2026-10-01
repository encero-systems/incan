//! A list or dict comprehension that changes its items reads them in place (#1561): through `enumerate`, `zip`,
//! `values()` and an element as a `for` loop does, and a dict comprehension over a list place too, so the change
//! reaches the source. The items of a temporary, and those a generator expression draws from one, are the
//! comprehension's own.

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

/// A change a comprehension makes through its variable reaches a `mut` list read through `enumerate` (in the element
/// and in the filter), `zip`, a dict's `values()`, an element and a field of an element, and a dict comprehension's
/// change reaches the list it reads directly or through `enumerate`.
#[test]
fn comprehensions_change_their_items_in_place_issue1561() -> TestResult {
    let output = program_output(
        r#"
model Box:
    items: list[int]

def main() -> None:
    mut rows: list[list[int]] = [[1, 2], [3, 4]]
    popped = [row.pop() for i, row in enumerate(rows)]
    println(popped)
    println(rows)
    mut extra: list[int] = [7, 8]
    zipped = [row.pop() + n for row, n in zip(rows, extra)]
    println(zipped)
    println(rows)
    mut table: dict[str, list[int]] = {"a": [1, 2]}
    lasts = [v.pop() for v in table.values()]
    println(lasts)
    println(table)
    mut groups: list[list[list[int]]] = [[[1, 2]], [[3]]]
    got = [row.pop() for row in groups[0]]
    println(got)
    println(groups)
    mut rows2: list[list[int]] = [[5, 6], [7]]
    kept = [i for i, row in enumerate(rows2) if row.pop() > 5]
    println(kept)
    println(rows2)
    mut boxes: list[Box] = [Box(items=[1, 2])]
    counts = [b.items.pop() for i, b in enumerate(boxes)]
    println(counts)
    println(boxes[0].items)
    mut rows3: list[list[int]] = [[1, 2], [3, 4]]
    by_index = {i: row.pop() for i, row in enumerate(rows3)}
    println(f"{by_index[0]} {by_index[1]}")
    println(rows3)
    mut rows4: list[list[int]] = [[1, 2], [3, 4, 5]]
    by_length = {len(row): row.pop() for row in rows4}
    println(f"{by_length[2]} {by_length[3]}")
    println(rows4)
"#,
    )?;
    assert_eq!(
        output,
        "[2, 4]\n[[1], [3]]\n[8, 11]\n[[], []]\n[2]\n{\"a\": [1]}\n[2]\n[[[1]], [[3]]]\n[0, 1]\n[[5], []]\n[2]\n[1]\n2 4\n[[1], [3]]\n2 5\n[[1], [3, 4]]\n"
    );
    Ok(())
}

/// A comprehension or a generator expression that changes the items of a temporary changes its own copies, and builds.
#[test]
fn comprehensions_change_the_items_of_a_temporary_issue1561() -> TestResult {
    let output = program_output(
        r#"
def fresh() -> list[list[int]]:
    return [[1, 2], [3]]

def main() -> None:
    println([row.pop() for row in fresh()])
    println([row.pop() for row in [[4, 5], [6]]])
    println([row.pop() for i, row in enumerate(fresh())])
    for x in (row.pop() for row in fresh()):
        println(x)
"#,
    )?;
    assert_eq!(output, "[2, 3]\n[5, 6]\n[2, 3]\n2\n3\n");
    Ok(())
}

/// A last-use generic list parameter is consumed by its comprehension, so the generated Rust needs no `T: Clone`
/// bound and the unbounded function builds and runs (#1983).
#[test]
fn generic_list_comprehension_consumes_its_parameter_issue1983() -> TestResult {
    let output = program_output(
        r#"
def copy_all[T](items: list[T]) -> list[T]:
    return [item for item in items]

def main() -> None:
    println(len(copy_all([1, 2, 3])))
"#,
    )?;
    assert_eq!(output, "3\n");
    Ok(())
}
