//! Callable presets whose targets and preset values RFC 084 admits: a local partial of a model, class or newtype
//! constructor, and top-level presets of negative numbers and tuple and set literals. The generated Rust compiles.

use incan_frontend::{lexer, parser};

use super::mut_ownership_regressions::compile_generated_rust;
use crate::IrCodegen;
use crate::test_support::parse_program_result;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lex, parse, check and generate Rust for one program.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// A local partial of a model, class or newtype constructor builds the value with the presets as overridable
/// defaults.
#[test]
fn local_partials_of_constructors_compile() -> TestResult {
    let rust = generate(
        r#"
model Reader:
    layer: str
    format: str
    path: str

class Counter:
    pub start: int
    pub step: int

type UserId = newtype int

def main() -> None:
    layer = "bronze"
    make = partial Reader(layer=layer, format="delta")
    reader = make(path="orders")
    csv = make(path="orders", format="csv")
    counter = partial Counter(step=2)
    c = counter(start=1)
    user = partial UserId(value=7)
    u = user()
    println(f"{reader.layer} {reader.path} {csv.format} {c.start} {c.step}")
    println(f"{u:?}")
"#,
    )?;
    compile_generated_rust(&rust)
}

/// Top-level presets of a negative number and of tuple and set literals are materialized at each call, and a
/// top-level partial of a newtype constructor constructs the newtype from its `value`.
#[test]
fn top_level_presets_of_negative_numbers_tuples_and_sets_compile() -> TestResult {
    let rust = generate(
        r#"
def scale(k: int, n: int) -> int:
    return k * n

def shift(by: float, x: float) -> float:
    return x + by

def pair_sum(p: tuple[int, int], n: int) -> int:
    return p[0] + p[1] + n

def count_tags(tags: set[str], extra: int) -> int:
    return len(tags) + extra

type UserId = newtype int

negative = partial scale(k=-2)
negative_float = partial shift(by=-0.5)
default_user = partial UserId(value=7)
paired = partial pair_sum(p=(1, -2))
tagged = partial count_tags(tags={"a", "b"})

def main() -> None:
    println(negative(n=3) + paired(n=1) + tagged(extra=1))
    println(negative_float(x=1.0))
    println(f"{default_user():?} {default_user(value=8):?}")
"#,
    )?;
    compile_generated_rust(&rust)
}

/// A local partial of a generic function or of a generic model or class constructor, instantiated by its presets or
/// its written type arguments, builds and calls its target.
#[test]
fn local_partials_of_generic_callables_compile() -> TestResult {
    let rust = generate(
        r#"
def pair[T](a: T, b: T) -> list[T]:
    return [a, b]

def tagged[T with Display](value: T, prefix: str) -> str:
    return f"{prefix}{value}"

model Box[T]:
    value: T
    label: str

class Slot[T]:
    pub item: T
    pub count: int

def main() -> None:
    first = partial pair(a=1)
    items = first(b=2)
    more = first(3)
    hash_tag = partial tagged[int](prefix="no.")
    text = hash_tag(value=4)
    boxed = partial Box(value=5)
    b = boxed(label="five")
    labeled = partial Box[str](label="named")
    c = labeled(value="six")
    slot = partial Slot(item="s")
    s = slot(count=7)
    println(f"{items} {more} {text} {b.value} {b.label} {c.value} {c.label} {s.item} {s.count}")
"#,
    )?;
    compile_generated_rust(&rust)
}

/// A local partial of a function another module declares, named through its module or imported by name, and generic or
/// not, calls the function through its path with every argument.
#[test]
fn local_partials_of_another_modules_functions_pass_every_argument() -> TestResult {
    let helpers = parse_program_result(
        "pub def pair[T](a: T, b: T) -> list[T]:\n    return [a, b]\n\npub def add(a: int, b: int) -> int:\n    return a + b\n",
    )?;
    let root = parse_program_result(
        "import helpers\nfrom helpers import add\n\ndef main() -> None:\n    first = partial helpers.pair(a=1)\n    second = partial helpers.add(a=2)\n    third = partial add(b=3)\n    println(f\"{first(b=4)} {second(b=5)} {third(a=6)}\")\n",
    )?;
    let paths = vec![vec!["helpers".to_string()]];
    let mut codegen = IrCodegen::new();
    codegen.add_module_with_path_segments("helpers", &helpers, paths[0].clone());
    let (main, _) = codegen.try_generate_multi_file_nested(&root, &paths)?;
    let compact = main
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    for call in [
        ">(a.unwrap_or_else(||__incan_partial_preset_0_a.clone()),b",
        "(a.unwrap_or_else(||__incan_partial_preset_0_a.clone()),b",
        "(a,b.unwrap_or_else(||__incan_partial_preset_0_b.clone())",
    ] {
        assert!(compact.contains(call), "missing `{call}`:\n{main}");
    }
    assert!(compact.contains("crate::helpers::"), "{main}");
    Ok(())
}
