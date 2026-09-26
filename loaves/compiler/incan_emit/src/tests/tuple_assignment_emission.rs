//! Tuple statements that write existing places emit Rust assignments: a tuple unpacking into bound names assigns the
//! names instead of declaring shadows (#1799), and a tuple assignment writes each field and element (#1798).

use crate::codegen::IrCodegen;
use incan_frontend::{lexer, parser};

/// Generate Rust for one checked source, with every run of whitespace collapsed to one space.
fn generate_collapsed(source: &str) -> Result<String, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    let code = IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| format!("generation failed: {error:?}"))?;
    Ok(code.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// `a, b = (b, a + b)` in a loop assigns the loop's `a` and `b` from one temporary; no `let a` or `let b` shadows
/// them inside the loop body.
#[test]
fn tuple_unpacking_in_a_loop_assigns_the_bound_names_issue1799() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def fib(n: int) -> int:
    mut a = 0
    mut b = 1
    for _ in range(n):
        a, b = (b, a + b)
    return a


def main() -> None:
    println(fib(10))
"#,
    )?;
    for expected in [
        "let __incan_tuple_unpack_a_b = (b, a + b);",
        "a = __incan_tuple_unpack_a_b.0;",
        "b = __incan_tuple_unpack_a_b.1;",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    for shadow in ["let a = __incan_tuple_unpack_a_b", "let b = __incan_tuple_unpack_a_b"] {
        assert!(
            !code.contains(shadow),
            "the loop must not shadow with `{shadow}`:\n{code}"
        );
    }
    Ok(())
}

/// A field swap through `mut self`, a list-element swap and a field beside a name write each place from one temporary
/// read before the first write.
#[test]
fn tuple_assignment_writes_each_field_and_element_issue1798() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
class Grid:
    pub width: int
    pub height: int

    def swap_dimensions(mut self) -> None:
        self.width, self.height = (self.height, self.width)


def swap_items(mut items: list[int]) -> list[int]:
    items[0], items[2] = (items[2], items[0])
    return items


def trade(mut grid: Grid, start: int) -> int:
    mut total = start
    grid.width, total = (total + 4, grid.width)
    return total


def main() -> None:
    mut grid = Grid(width=1, height=2)
    grid.swap_dimensions()
    println(swap_items([10, 20, 30])[0])
    println(trade(grid, 1))
"#,
    )?;
    for expected in [
        "let __incan_tuple_assign = (self.height, self.width);",
        "self.width = __incan_tuple_assign.0;",
        "self.height = __incan_tuple_assign.1;",
        "grid.width = __incan_tuple_assign.0;",
        "total = __incan_tuple_assign.1;",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    // An element write goes through the list accessor (`*list_get_mut(items, i) = ...`) and the formatter may break
    // the line before `.0`, so it is matched with all whitespace removed.
    let compact: String = code.split_whitespace().collect();
    assert_eq!(
        compact.matches(")=__incan_tuple_assign.").count(),
        2,
        "both list elements are written from the temporary:\n{code}"
    );
    Ok(())
}
