//! Slice syntax builds and runs (#1561): on a type that defines `__getslice__`, the hook `Sliceable[T]` declares, it
//! calls the hook with each part as an `Option[int]`; and a slice whose end is omitted may write its two colons
//! together (`[::2]`, `[1::2]`).

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

/// `obj[a:b:c]` calls the type's `__getslice__` with `Some` of each written part and `None` for each omitted one, on a
/// generic type too; `list` and `str` slices written with `::` keep their meaning.
#[test]
fn slice_syntax_calls_the_getslice_hook_issue1561() -> TestResult {
    let output = program_output(
        r#"
model Window:
    items: list[int]

    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[int]:
        return [start.unwrap_or(-1), end.unwrap_or(-1), step.unwrap_or(-1)]

model Shelf[T]:
    items: list[T]

    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[T]:
        return self.items[start.unwrap_or(0):]

def main() -> None:
    w = Window(items=[1, 2, 3])
    println(w[1:2])
    println(w[:])
    println(w[::2])
    println(w[0:3:2])
    shelf = Shelf(items=["a", "b", "c"])
    println(shelf[1:])
    items = [1, 2, 3, 4, 5]
    println(items[::2])
    println(items[1::2])
    println("hello"[::-1])
"#,
    )?;
    assert_eq!(
        output,
        "[1, 2, -1]\n[-1, -1, -1]\n[-1, -1, 2]\n[0, 3, 2]\n[\"b\", \"c\"]\n[1, 3, 5]\n[2, 4]\nolleh\n"
    );
    Ok(())
}
