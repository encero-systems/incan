//! Closures that capture local values build where the checker holds them (#1561): as a new local, as an argument for
//! a function-typed parameter its function only calls (`impl Fn(A) -> R`), as the one returned value of a function
//! (`impl Fn(A) -> R`), and as the callback of a `Result` combinator, which only calls it. A local bound to one is
//! passed to such a parameter by reference, so it stays usable. A named function and a closure that captures nothing
//! keep building in a function-pointer slot.

use incan_frontend::{lexer, parser};

use super::mut_ownership_regressions::run_generated_program;
use crate::IrCodegen;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Parse, check, lower and emit one program, returning the generated Rust.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// Each accepted slot builds and runs: an annotated local, a local called and passed twice, an argument for a function
/// and for a method that only call their parameter, a closure a function and a method return, and a closure that
/// reads `self` bound to a local in its method; a named function and a closure that captures nothing in a list and a
/// field, and a returned closure that captures nothing in a list, still build.
#[test]
fn capturing_closures_build_where_their_own_type_is_kept_issue1561() -> TestResult {
    let rust = generate(
        r#"
model Op:
    step: (int) -> int

class Acc:
    pub base: int

    def run(self, f: (int) -> int) -> int:
        return f(self.base)

    def adder(self) -> (int) -> int:
        base = self.base
        return (x) => x + base

    def scaled(self) -> int:
        g: (int) -> int = (x) => x * self.base
        return g(2)

def apply(f: (int) -> int, x: int) -> int:
    return f(x)

def twice(f: (int) -> int, x: int) -> int:
    return f(f(x))

def make_adder(n: int) -> (int) -> int:
    return (x) => x + n

def double(x: int) -> int:
    return x * 2

def size_of() -> (list[int]) -> int:
    return (xs) => len(xs)

def main() -> None:
    n = 5
    g: (int) -> int = (x) => x + n
    println(g(1))
    println(apply(g, 1))
    println(twice(g, 1))
    println(g(0))
    println(apply((x) => x - n, 3))
    add = make_adder(3)
    println(add(1))
    println(apply(make_adder(10), 1))
    a = Acc(base=3)
    println(a.run((x) => x + n))
    println(a.adder()(1))
    println(a.scaled())
    ops: list[(int) -> int] = [double, (x) => x + 1]
    first = ops[0]
    println(first(4))
    op = Op(step=(x) => x - 1)
    step = op.step
    println(step(1))
    sizes: list[(list[int]) -> int] = [size_of()]
    size = sizes[0]
    println(size([1, 2]))
"#,
    )?;
    let compact = rust.split_whitespace().collect::<String>();
    assert!(compact.contains("f:implFn(i64)->i64"), "{rust}");
    assert!(compact.contains("->implFn(i64)->i64"), "{rust}");
    assert!(compact.contains("(&g)"), "{rust}");
    let output = run_generated_program(&rust)?;
    assert_eq!(output, "6\n6\n11\n5\n-2\n4\n11\n8\n4\n6\n8\n0\n2\n");
    Ok(())
}

/// A local partial of a method of a value holds its receiver as it was when the partial was built, and calls the
/// method with the presets it holds, through a method alias, on a generic type and on `self` (RFC 084).
#[test]
fn local_partials_of_methods_of_values_build_issue1561() -> TestResult {
    let rust = generate(
        r#"
model User:
    name: str

    def label(self, prefix: str, suffix: str) -> str:
        return prefix + self.name + suffix

    tag = label

model Box[T]:
    value: T

    def pair(self, other: T, sep: str) -> list[T]:
        return [self.value, other]

class Greeter:
    pub greeting: str

    def greet(self, name: str, punct: str) -> str:
        return self.greeting + " " + name + punct

    def make(self) -> str:
        hello = partial self.greet(punct="!")
        return hello("you")

def main() -> None:
    mut user = User(name="ann")
    tag = partial user.label(prefix="x")
    user = User(name="bob")
    println(tag(suffix="!"))
    println(tag("?", prefix="y"))
    aliased = partial user.tag(suffix=".")
    println(aliased("<"))
    b = Box(value=1)
    joined = partial b.pair(sep="-")
    println(joined(2))
    g = Greeter(greeting="hi")
    println(g.make())
    holder = partial g.greet(name="me")
    println(holder(punct="?"))
"#,
    )?;
    let output = run_generated_program(&rust)?;
    assert_eq!(output, "xann!\nyann?\n<bob.\n[1, 2]\nhi you!\nhi me?\n");
    Ok(())
}

/// A closure that captures local values builds and runs as the callback of each `Result` combinator, which calls it
/// before it returns (#1561).
#[test]
fn capturing_closures_build_as_result_combinator_callbacks_issue1561() -> TestResult {
    let stdout = super::generated_programs::run_with_stdlib(
        r#"
def passed() -> Result[int, str]:
    return Ok(5)


def failed() -> Result[int, str]:
    return Err("x")


def main() -> None:
    n = 2
    suffix = "!"
    println(passed().map((x) => x + n).unwrap_or(0))
    println(passed().and_then((x) => Ok(x * n)).unwrap_or(0))
    println(failed().or_else((m) => Ok(len(m) + n)).unwrap_or(0))
    passed().inspect((x) => println(x * n))
    failed().inspect_err((m) => println(len(m) + n))
    match failed().map_err((m) => m + suffix):
        Ok(_) => println("ok")
        Err(message) => println(message)
"#,
    )?;
    assert_eq!(stdout, "7\n10\n3\n10\n3\nx!\n");
    Ok(())
}
