//! Frontend regressions for the protected `print` builtin binding and its `println` alias.

use incan::frontend::diagnostics::CompileError;
use incan::frontend::{lexer, parser, typechecker};

/// Type-check `source`, retaining compiler diagnostics rather than process-level rendering.
fn check_source(source: &str) -> Result<Vec<CompileError>, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lex failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parse failed: {errors:?}"))?;
    let mut checker = typechecker::TypeChecker::new();
    match checker.check_program(&program) {
        Ok(()) => Ok(Vec::new()),
        Err(errors) => Ok(errors),
    }
}

/// Assert that a source binding is rejected at an original source span for one protected spelling.
fn assert_protected_binding(source: &str, spelling: &str) -> Result<(), String> {
    let errors = check_source(source)?;
    let error = errors
        .iter()
        .find(|error| error.message.contains("protected builtin binding"))
        .ok_or_else(|| format!("expected protected-binding diagnostic for `{spelling}`, got {errors:?}"))?;
    let covers_source_spelling = source
        .match_indices(spelling)
        .any(|(start, _)| error.span.start <= start && error.span.end >= start + spelling.len());
    if !covers_source_spelling {
        return Err(format!(
            "expected diagnostic for `{spelling}` to cover its original source spelling, got {:?}",
            error.span
        ));
    }
    Ok(())
}

/// Every covered source binding form rejects both registry-owned output spellings.
#[test]
fn source_bindings_cannot_replace_print_or_println_issue1249() -> Result<(), String> {
    for (spelling, source) in [
        ("print", "def print(value: str) -> None:\n    pass\n"),
        ("println", "def println(value: str) -> None:\n    pass\n"),
        ("print", "def main(print: str) -> None:\n    pass\n"),
        ("println", "def main(println: str) -> None:\n    pass\n"),
        (
            "print",
            "def display[print](value: print) -> None:\n    print(\"hello\")\n",
        ),
        (
            "println",
            "def display[println](value: println) -> None:\n    println(\"hello\")\n",
        ),
        ("print", "def main() -> None:\n    print = \"local\"\n"),
        ("println", "def main() -> None:\n    println = \"local\"\n"),
        ("print", "def main() -> None:\n    for print in [1]:\n        pass\n"),
        (
            "println",
            "def main() -> None:\n    for println in [1]:\n        pass\n",
        ),
        (
            "print",
            "def main(value: Option[str]) -> None:\n    match value:\n        case Some(print):\n            pass\n",
        ),
        (
            "println",
            "def main(value: Option[str]) -> None:\n    match value:\n        case Some(println):\n            pass\n",
        ),
        (
            "print",
            "def main() -> int:\n    print, value = (1, 2)\n    return print + value\n",
        ),
        (
            "println",
            "def main() -> int:\n    total = 0\n    for println, value in [(1, 2)]:\n        total = println + value\n    return total\n",
        ),
        (
            "print",
            "def main() -> int:\n    operation = (print) => print\n    return operation(1)\n",
        ),
        (
            "println",
            "def main() -> list[int]:\n    return [println for println in [1, 2]]\n",
        ),
        (
            "print",
            "const print: int = 1\n\n\ndef main() -> int:\n    return print\n",
        ),
        (
            "println",
            "static println: int = 1\n\n\ndef main() -> int:\n    return println\n",
        ),
        (
            "println",
            "from std.hash import sha1 as println\n\n\ndef main() -> int:\n    return 1\n",
        ),
        (
            "print",
            "import std.async\n\n\nasync def fast() -> int:\n    return 1\n\n\nasync def slow() -> int:\n    return 2\n\n\nasync def main() -> int:\n    return race for print:\n        await fast() => print\n        await slow() => print\n",
        ),
        (
            "print",
            "def main() -> int:\n    mut print: int = 1\n    print += 1\n    return print\n",
        ),
        ("print", "import std.web as print\n"),
        ("println", "import std.web as println\n"),
    ] {
        assert_protected_binding(source, spelling)?;
    }
    Ok(())
}

/// Unprotected builtins retain their existing ordinary source-binding behavior.
#[test]
fn unrelated_builtin_names_remain_shadowable_issue1249() -> Result<(), String> {
    let errors =
        check_source("def len(value: int) -> int:\n    return value + 1\n\n\ndef main() -> int:\n    return len(1)\n")?;
    assert!(errors.is_empty(), "unprotected len must remain shadowable: {errors:?}");
    Ok(())
}

/// Member and field names occupy a distinct namespace from protected free-function bindings.
#[test]
fn fields_and_members_named_print_remain_ordinary_source_names_issue1249() -> Result<(), String> {
    let source = r#"
model Reporter:
    print: str

    def print(self) -> str:
        return self.print

def main() -> str:
    reporter = Reporter(print="field")
    return reporter.print()
"#;
    let errors = check_source(source)?;
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("field/member spelling must remain unreserved, got {errors:?}"))
    }
}
