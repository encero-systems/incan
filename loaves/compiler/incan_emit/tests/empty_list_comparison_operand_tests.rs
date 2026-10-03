//! An empty list literal compared against a list carries the partner's element type from the checker down (#1476).
//!
//! `values == []` accepts `[]` by compatibility, but the recorded expression type is what lowering carries onto the
//! literal and what emission spells; a bare `vec![]` beside a `Vec<String>` leaves rustc with an ambiguous
//! `PartialEq` (E0283). These tests pin each stage: the checker's recorded type, the lowered `TypedExpr::ty`, and the
//! generated spelling, for both operand orders and both equality operators.

use incan_emit::IrCodegen;
use incan_frontend::ast::{Declaration, Span, Statement};
use incan_frontend::symbols::ResolvedType;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_ir::expr::IrExprKind;
use incan_ir::lower::AstLowering;
use incan_ir::types::IrType;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Check `source` and return the type recorded for the first `[]` in it.
fn recorded_empty_list_type(source: &str) -> Result<ResolvedType, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find("[]").ok_or("empty literal missing from source")?;
    checker
        .type_info()
        .expr_type(Span { start, end: start + 2 })
        .cloned()
        .ok_or_else(|| "no type recorded for the empty literal".into())
}

/// The checker records `List[str]` for the empty operand whichever side it is on and whichever operator is used.
#[test]
fn empty_list_operand_records_the_partner_list_type() -> TestResult {
    let string_list = ResolvedType::Generic("List".into(), vec![ResolvedType::Str]);
    for body in [
        "return values == []",
        "return values != []",
        "return [] == values",
        "return [] != values",
        "return values == ([])",
    ] {
        let source = format!("def probe(values: list[str]) -> bool:\n    {body}\n");
        assert_eq!(recorded_empty_list_type(&source)?, string_list, "{body}");
    }
    Ok(())
}

/// Two empty literals have no partner type to adopt and stay unresolved rather than inventing one.
#[test]
fn two_empty_list_operands_stay_unresolved() -> TestResult {
    let unknown_list = ResolvedType::Generic("List".into(), vec![ResolvedType::Unknown]);
    assert_eq!(
        recorded_empty_list_type("def probe() -> bool:\n    return [] == []\n")?,
        unknown_list
    );
    Ok(())
}

/// Compatibility is untouched: a populated literal of another element type is still rejected, and a non-list
/// partner never lends its type.
#[test]
fn populated_and_non_list_operands_keep_existing_checks() -> TestResult {
    let source = "def probe(values: list[str]) -> bool:\n    return values == [1]\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let errors = TypeChecker::new()
        .check_program(&program)
        .err()
        .ok_or("a populated list of another element type was accepted")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("List[str]") && error.message.contains("List[int]")),
        "{errors:?}"
    );

    let source = "def probe(count: int) -> bool:\n    return count == []\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    assert!(
        TypeChecker::new().check_program(&program).is_err(),
        "an int compared against an empty list must still be rejected"
    );
    Ok(())
}

/// Lowering carries the recorded type onto the literal's `TypedExpr`, which is what emission reads.
#[test]
fn lowered_empty_list_operand_carries_the_element_type() -> TestResult {
    let source = "def probe(values: list[str]) -> bool:\n    return [] != values\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or("function missing")?;
    let comparison = function
        .body
        .iter()
        .find_map(|stmt| match &stmt.node {
            Statement::Return(Some(value)) => Some(value),
            _ => None,
        })
        .ok_or("return missing")?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    let lowered = lowering.lower_expr_spanned(comparison)?;
    let IrExprKind::BinOp { left, .. } = &lowered.kind else {
        return Err("expected a lowered comparison".into());
    };
    assert!(matches!(&left.kind, IrExprKind::List(entries) if entries.is_empty()));
    assert_eq!(left.ty, IrType::List(Box::new(IrType::String)));
    Ok(())
}

/// The generated Rust names the element type on the literal for every operand order and operator.
#[test]
fn generated_rust_names_the_element_type_on_the_empty_operand() -> TestResult {
    let source = "def is_empty(values: list[str]) -> bool:\n    return values == []\n\ndef is_not_empty(values: list[str]) -> bool:\n    return [] != values\n\ndef main() -> None:\n    assert is_empty([])\n    assert is_not_empty([\"value\"])\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let rust = IrCodegen::new().try_generate(&program)?;
    let compact: String = rust.chars().filter(|character| !character.is_whitespace()).collect();
    assert!(compact.contains("values==Vec::<String>::new()"), "{rust}");
    assert!(compact.contains("Vec::<String>::new()!=values"), "{rust}");
    assert!(
        !compact.contains("vec![]") && !compact.contains("Vec::<_>::new()"),
        "{rust}"
    );
    Ok(())
}
