//! Generated-Rust retention of a method whose only caller is a method first found used late in the scan (#1765).

use incan_emit::IrCodegen;
use incan_frontend::{lexer, parser};
use incan_semantics_core::SemanticSourceTargetKind;
use incan_test_support::canonical_projection::projected_name;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Generate the Rust for one source module.
fn generate(source: &str) -> Result<String, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
    Ok(IrCodegen::new().try_generate(&program)?)
}

/// #1765: `User` becomes reachable through `main`'s constructor before `use_it`'s body records that it calls
/// `User.first`, so the impl was scanned while `first` was still unused. `first` was then retained without its body
/// being scanned, and `second`, which only `first` calls, was dropped (E0599). The same happened to the target method
/// of a method partial, whose generated method is such a caller.
#[test]
fn method_called_only_from_a_later_scanned_function_keeps_its_callees_issue1765() -> TestResult {
    for (label, source, callee) in [
        (
            "method chain",
            r#"
model User:
    name: str

    def first(self) -> str:
        return self.second()

    def second(self) -> str:
        return self.name


pub def use_it(user: User) -> str:
    return user.first()


def main() -> None:
    println(use_it(User(name="Ada")))
"#,
            "second",
        ),
        (
            "method partial",
            r#"
model User:
    name: str

    def label(self, prefix: str) -> str:
        return prefix

    short = partial label(prefix="name")


pub def use_it(user: User) -> str:
    return user.short()


def main() -> None:
    println(use_it(User(name="Ada")))
"#,
            "label",
        ),
    ] {
        let code = generate(source)?;
        let callee_projection = projected_name(&code, callee, SemanticSourceTargetKind::Method);
        assert!(
            code.contains(&format!("fn {callee_projection}(")),
            "{label}: the method `{callee}` its caller reaches must be emitted:\n{code}"
        );
    }
    Ok(())
}
