//! Generated-Rust regressions for ownership across mutually exclusive `match` arms.

use incan::backend::IrCodegen;
use incan::frontend::{lexer, parser};
use incan_semantics_core::SemanticSourceTargetKind;

#[path = "support/canonical_projection.rs"]
mod canonical_projection;

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

#[test]
fn guarded_match_arms_retain_conservative_clone_planning() -> TestResult {
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
            "pubfn{guarded}<T:Clone,>(value:T,choose_first:bool,admit_first:bool)->T"
        )),
        "a guarded arm may fail before a later arm executes, so ownership planning must remain conservative:\n{rust}"
    );
    assert!(
        rust.contains("returnvalue.clone();"),
        "the guarded arm must not consume a value that the fallback arm may still need:\n{rust}"
    );
    assert!(
        rust.contains("returnvalue;"),
        "the final fallback arm may consume the value after the guarded arm is ruled out:\n{rust}"
    );
    Ok(())
}
