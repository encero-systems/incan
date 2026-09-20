//! Generated-Rust regressions for ownership across mutually exclusive `match` arms.

use incan_emit::IrCodegen;
use incan_frontend::{lexer, parser};
use incan_semantics_core::SemanticSourceTargetKind;

use incan_test_support::canonical_projection;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lower one source fixture through the ordinary native pipeline.
fn generate_rust(source: &str) -> Result<String, std::io::Error> {
    let tokens =
        lexer::lex(source).map_err(|errors| std::io::Error::other(format!("fixture did not lex: {errors:?}")))?;
    let program =
        parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("fixture did not parse: {errors:?}")))?;
    IrCodegen::new()
        .try_generate(&program)
        .map_err(|error| std::io::Error::other(format!("fixture did not codegen: {error:?}")))
}

/// Return generated Rust plus a whitespace-free copy for stable ownership assertions.
fn generated_and_compact_rust(source: &str) -> Result<(String, String), std::io::Error> {
    let generated = generate_rust(source)?;
    let compact = generated
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    Ok((generated, compact))
}

#[test]
fn generic_value_moves_in_every_terminal_match_arm_without_clone_bound() -> TestResult {
    let (generated, rust) = generated_and_compact_rust(
        r#"
pub def select[T](value: T, choose_first: bool) -> T:
    match choose_first:
        true => return value
        false => return value
"#,
    )?;
    let select = canonical_projection::projected_name(&generated, "select", SemanticSourceTargetKind::Function);

    assert!(
        rust.contains(&format!("pubfn{select}<T,>(value:T,choose_first:bool)->T")),
        "terminal match arms must not narrow the generic signature with Clone:\n{rust}"
    );
    assert_eq!(
        rust.matches("returnvalue;").count(),
        2,
        "each mutually exclusive terminal arm must move the same value exactly once:\n{rust}"
    );
    assert!(
        !rust.contains("value.clone()"),
        "terminal match arms must not clone a value that no path uses afterwards:\n{rust}"
    );
    Ok(())
}

#[test]
fn generic_value_clones_in_match_arms_when_used_after_match() -> TestResult {
    let (generated, rust) = generated_and_compact_rust(
        r#"
pub def preserve[T](value: T, choose_first: bool) -> T:
    match choose_first:
        true =>
            first = value
        false =>
            second = value
    return value
"#,
    )?;
    let preserve = canonical_projection::projected_name(&generated, "preserve", SemanticSourceTargetKind::Function);

    assert!(
        rust.contains(&format!("pubfn{preserve}<T:Clone,>(value:T,choose_first:bool)->T")),
        "a value used after the match must retain the required Clone bound:\n{rust}"
    );
    assert_eq!(
        rust.matches("=value.clone();").count(),
        2,
        "each arm must preserve the value for its post-match use:\n{rust}"
    );
    assert!(
        rust.contains("returnvalue;"),
        "the post-match last use must still move the original value:\n{rust}"
    );
    Ok(())
}

/// A guard decides whether its arm body runs; once the body runs, a `return` inside it is that path's last use of
/// every local it reads (#1489). The guard expression itself keeps the conservative counter, because a guard can read
/// a value, fail, and let a later arm execute.
#[test]
fn guarded_match_arm_returns_move_once_the_guard_admits_the_arm() -> TestResult {
    let (generated, rust) = generated_and_compact_rust(
        r#"
pub def guarded[T](value: T, choose_first: bool, admit_first: bool) -> T:
    match choose_first:
        case true if admit_first: return value
        case _: return value
"#,
    )?;
    let guarded = canonical_projection::projected_name(&generated, "guarded", SemanticSourceTargetKind::Function);

    assert!(
        rust.contains(&format!(
            "pubfn{guarded}<T,>(value:T,choose_first:bool,admit_first:bool)->T"
        )),
        "returning from each arm consumes the value on that path alone, so no Clone bound is needed:\n{rust}"
    );
    assert_eq!(
        rust.matches("returnvalue;").count(),
        2,
        "both the guarded arm and the fallback arm return the value by move:\n{rust}"
    );
    assert!(
        !rust.contains("value.clone()"),
        "a return inside an admitted guarded arm must not clone:\n{rust}"
    );
    Ok(())
}

/// A guard that reads a non-Copy value keeps the conservative read: the guard can fail and a later arm still needs
/// the value, so the read inside the guard clones while the admitted arm's `return` moves.
#[test]
fn guard_expression_reads_stay_conservative_while_the_admitted_return_moves() -> TestResult {
    let (_, rust) = generated_and_compact_rust(
        r#"
def admits(value: list[str]) -> bool:
    return len(value) > 0

pub def guarded(value: list[str], choose_first: bool) -> list[str]:
    match choose_first:
        case true if admits(value): return value
        case _: return value
"#,
    )?;

    assert!(
        rust.contains("(value.clone(),)=>{returnvalue;}"),
        "the guard may fail, so its read must not consume the value:\n{rust}"
    );
    assert_eq!(
        rust.matches("returnvalue;").count(),
        2,
        "each arm body returns the value by move once it runs:\n{rust}"
    );
    Ok(())
}
