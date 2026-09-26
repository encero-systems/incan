//! Assignments with several targets that write existing places emit Rust assignments: a tuple unpacking or a chained
//! assignment into bound names assigns the names instead of declaring shadows (#1799, #1806), and a tuple assignment
//! writes each field and element (#1798).

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

/// `x = y = x + 1` in a loop assigns the loop's `x` and declares the chain's new `y` from one temporary; no `let x`
/// shadows `x`.
#[test]
fn chained_assignment_in_a_loop_assigns_the_bound_name_issue1806() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def count_up(n: int) -> int:
    mut x = 0
    for _ in range(n):
        x = y = x + 1
    return x


def main() -> None:
    println(count_up(5))
"#,
    )?;
    for expected in [
        // The bound target `x` fixes the value's type.
        "let __incan_chain_value: i64 = x + 1;",
        "x = __incan_chain_value;",
        // `y` is never read, so it is declared as `_y`.
        "let _y = __incan_chain_value;",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    assert!(!code.contains("let x ="), "the loop must not shadow `x`:\n{code}");
    Ok(())
}

/// Every target of a chain takes the value in its own type, and a value that is not `Copy` is cloned for every target
/// but the last: no target is written from another target.
#[test]
fn chained_assignment_gives_every_target_the_value_issue1806() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def option_target(seed: int) -> int:
    mut maybe: Option[int] = None
    mut count = 0
    count = maybe = seed
    return count + maybe.unwrap_or(0)


def strings() -> str:
    mut a = "x"
    mut b = "y"
    a = b = "z"
    return f"{a} {b}"


def main() -> None:
    println(option_target(5))
    println(strings())
"#,
    )?;
    for expected in [
        "count = __incan_chain_value;",
        "maybe = Some(__incan_chain_value);",
        "a = __incan_chain_value.clone();",
        "b = __incan_chain_value;",
    ] {
        assert!(code.contains(expected), "missing `{expected}` in:\n{code}");
    }
    for crossed in ["count = maybe;", "a = b;"] {
        assert!(
            !code.contains(crossed),
            "no target reads another target (`{crossed}`):\n{code}"
        );
    }
    Ok(())
}

/// `a = b = None` over two `Option[int]` targets reads `None` as an `Option<i64>` rather than an `Option` of nothing,
/// and a module static before the last target takes an explicit copy of the value, which the last target then takes.
#[test]
fn chained_assignment_types_its_value_and_copies_for_a_static_issue1806() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
static names: list[str] = []


def reset() -> int:
    mut a: Option[int] = Some(1)
    mut b: Option[int] = Some(2)
    a = b = None
    return a.unwrap_or(0) + b.unwrap_or(0)


def fill() -> int:
    names = local = ["a"]
    return len(local)


def main() -> None:
    println(reset())
    println(fill())
"#,
    )?;
    let compact: String = code.split_whitespace().collect();
    assert!(
        compact.contains("let__incan_chain_value:Option<i64>=None"),
        "the chain's `None` must take the targets' type:\n{code}"
    );
    assert!(!compact.contains("None::<()>"), "no `None` of nothing:\n{code}");
    assert!(
        compact.contains("=__incan_chain_value.clone();"),
        "the static takes an explicit copy:\n{code}"
    );
    assert!(
        compact.contains("letlocal=__incan_chain_value;"),
        "the last target takes the value itself:\n{code}"
    );
    Ok(())
}

/// A value built only from literals over targets that disagree on a type is written once per target, in that target's
/// type: no `None` of nothing and no temporary for `None`, `(None)`, `[None]`, `[]`, `{}` or `list()`.
#[test]
fn chained_literal_over_disagreeing_targets_is_written_per_target_issue1806() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def reset() -> int:
    mut a: Option[int] = Some(1)
    mut b: Option[str] = Some("s")
    a = b = None
    mut ints: list[int] = [1]
    mut strs: list[str] = ["s"]
    ints = strs = []
    mut by_name: dict[str, int] = {"a": 1}
    mut by_id: dict[int, str] = {1: "a"}
    by_name = by_id = {}
    mut c: Option[int] = Some(1)
    mut d: Option[str] = Some("s")
    c = d = (None)
    mut xs: list[Option[int]] = []
    mut ys: list[Option[str]] = []
    xs = ys = [None]
    mut more_ints: list[int] = [1]
    mut more_strs: list[str] = ["s"]
    more_ints = more_strs = list()
    return a.unwrap_or(0) + c.unwrap_or(0) + len(ints) + len(strs) + len(by_name) + len(by_id) + len(xs) + len(ys) + len(more_ints) + len(more_strs)


def main() -> None:
    println(reset())
"#,
    )?;
    let compact: String = code.split_whitespace().collect();
    for target in [
        "a=None",
        "b=None",
        "ints=",
        "strs=",
        "by_name=",
        "by_id=",
        "c=None::<i64>",
        "d=None::<String>",
        "None::<i64>]",
        "None::<String>]",
        "more_ints=",
        "more_strs=",
    ] {
        assert!(compact.contains(target), "missing `{target}` in:\n{code}");
    }
    assert!(!compact.contains("None::<()>"), "no `None` of nothing:\n{code}");
    assert!(
        !compact.contains("__incan_chain_value"),
        "a literal over disagreeing targets reads no temporary:\n{code}"
    );
    Ok(())
}

/// A user function named `set` is called once, into the chain's temporary, and every target reads the temporary; only a
/// builtin constructor is written once per target.
#[test]
fn chained_call_to_a_user_function_named_set_is_emitted_once_issue1806() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def set() -> set[int]:
    println("tick")
    return {1}


def main() -> None:
    mut a: set[int] = {2}
    mut b: Option[set[int]] = None
    a = b = set()
    println(f"{len(a)}")
"#,
    )?;
    let compact: String = code.split_whitespace().collect();
    assert!(
        compact.contains("let__incan_chain_value"),
        "the call goes into one temporary:\n{code}"
    );
    assert!(
        compact.contains("a=__incan_chain_value.clone();") && compact.contains("b=Some(__incan_chain_value);"),
        "every target reads the temporary:\n{code}"
    );
    Ok(())
}

/// Targets of `list[int]` and `list[i64]` share one type, so `empty()` is read once into a `Vec<i64>` temporary that
/// both take.
#[test]
fn chained_targets_of_equivalent_types_share_one_temporary_issue1806() -> Result<(), String> {
    let code = generate_collapsed(
        r#"
def empty[T]() -> list[T]:
    return []


def main() -> None:
    mut a: list[int] = [1]
    mut b: list[i64] = [2]
    a = b = empty()
    println(f"{len(a)} {len(b)}")
"#,
    )?;
    let compact: String = code.split_whitespace().collect();
    assert!(
        compact.contains("let__incan_chain_value:Vec<i64>="),
        "one typed temporary:\n{code}"
    );
    assert!(
        compact.contains("a=__incan_chain_value.clone();") && compact.contains("b=__incan_chain_value;"),
        "both targets read the temporary:\n{code}"
    );
    Ok(())
}
