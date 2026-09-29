//! Closures whose parameter types come from their context build and run (#1561): the element type of an iterator
//! adapter or terminal, a fold's accumulator, a generic parameter's function type once the other arguments fix it,
//! and a generator's own `map` and `filter`, whose predicate closure is called in place.

use super::generated_programs::run_with_stdlib;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Each closure below is written without parameter annotations; its context gives the parameter types, so string
/// methods lower to their Rust spellings and arithmetic to the element's integer type.
#[test]
fn closures_typed_by_their_context_build_and_run_issue1561() -> TestResult {
    let stdout = run_with_stdlib(
        r#"
def apply_twice[T](f: (T) -> T, value: T) -> T:
    return f(f(value))


def numbers() -> Generator[int]:
    yield 1
    yield 2
    yield 3


def main() -> None:
    items = [1, 2, 3]
    names = ["ada", "lin"]
    println(items.iter().map((x) => x * 2).collect())
    shouted: list[str] = names.iter().map((name) => name.upper()).collect()
    println(shouted)
    println(names.iter().flat_map((name) => [name, name.upper()]).collect())
    println(items.iter().filter((x) => x % 2 == 1).collect())
    println(items.iter().take_while((x) => x * 2 < 5).collect())
    println(names.iter().any((name) => len(name.strip()) > 2))
    println(items.iter().fold(0, (acc, x) => acc + x * 10))
    println(apply_twice((x) => x + 1, 3))
    println(apply_twice((text) => text.upper(), "a"))
    println(numbers().map((x) => x * 10).collect())
    println(numbers().filter((x) => x % 2 == 1).collect())
"#,
    )?;
    assert_eq!(
        stdout,
        "[2, 4, 6]\n[\"ADA\", \"LIN\"]\n[\"ada\", \"ADA\", \"lin\", \"LIN\"]\n[1, 3]\n[1, 2]\ntrue\n60\n5\nA\n[10, 20, 30]\n[1, 3]\n"
    );
    Ok(())
}
