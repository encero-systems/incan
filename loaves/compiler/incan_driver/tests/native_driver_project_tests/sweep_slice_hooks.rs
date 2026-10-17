//! Live legacy parity for checked source slice hooks and their evaluation order.

use super::{corpus, driver_fixture};

/// The complete original preserves explicit and omitted slice parts, generic hooks, and builtin list/text slices.
#[test]
fn source_slice_hook_original_matches_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-slice-hooks-original")?,
        &graph.sysroot,
        &graph.formatting,
        include_str!(
            "../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/slice_syntax_calls_the_getslice_hook.incn"
        ),
    )
}

/// A hook preserves receiver-first evaluation; text slices retain Unicode bounds and caller-owned values.
#[test]
fn slice_hook_receiver_and_parts_match_legacy() -> Result<(), Box<dyn std::error::Error>> {
    let graph = driver_fixture()?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-slice-hooks-order")?,
        &graph.sysroot,
        &graph.formatting,
        r#"model Window:
    items: list[int]

    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[int]:
        return [start.unwrap_or(-1), end.unwrap_or(-1), step.unwrap_or(-1)]

def source() -> Window:
    println("receiver")
    return Window(items=[1, 2])

def part() -> int:
    println("part")
    return 1

def main() -> None:
    println(source()[part():])
"#,
    )?;
    corpus::check_builtin_source(
        &graph.driver_binary("debug"),
        &graph.scratch("sweep-text-slice-bounds")?,
        &graph.sysroot,
        &graph.formatting,
        r#"model Text:
    value: str

def reverse(mut value: str) -> str:
    return value[::-1]

def main() -> None:
    mut value = "héllo"
    println(value[:])
    println(value[1:4])
    println(value[::-1])
    println(value[::2])
    println(value[-2:])
    println(reverse(value))
    println(value)
    wrapped = Text(value="abcd")
    println(wrapped.value[1:3])
"#,
    )
}
