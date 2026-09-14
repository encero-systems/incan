//! Checked inference and native-emission regressions for empty-first nested lists (#1471).

use incan::backend::IrCodegen;
use incan::backend::ir::lower::AstLowering;
use incan::backend::ir::types::IrType;
use incan::frontend::ast::{Declaration, Span, Statement};
use incan::frontend::symbols::ResolvedType;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Check the complete string-list type and its propagation into typed lowering.
fn assert_literal_type(literal: &str, expected: &str) -> TestResult {
    let source = format!("def run() -> None:\n    for values in {literal}:\n        pass\n");
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find(literal).ok_or("literal source span missing")?;
    let span = Span {
        start,
        end: start + literal.len(),
    };
    let actual = checker
        .type_info()
        .expr_type(span)
        .ok_or("literal checked type missing")?;
    assert_eq!(actual.to_string(), expected);
    let function = program
        .declarations
        .iter()
        .find_map(|decl| match &decl.node {
            Declaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or("function missing")?;
    let loop_stmt = function
        .body
        .iter()
        .find_map(|stmt| match &stmt.node {
            Statement::For(loop_stmt) => Some(loop_stmt),
            _ => None,
        })
        .ok_or("loop missing")?;
    let mut lowering = AstLowering::new_with_type_info(checker.type_info().clone());
    let lowered = lowering.lower_expr_spanned(&loop_stmt.iter)?;
    let mut leaf = &lowered.ty;
    while let IrType::List(inner) = leaf {
        leaf = inner;
    }
    assert_eq!(leaf, &IrType::String, "checked list leaf lost in typed IR");
    Ok(())
}

/// String-list siblings refine empty literal holes, including an explicitly nested empty seed.
#[test]
fn nested_list_empty_holes_retain_checked_string_types() -> TestResult {
    assert_literal_type("[[], [\"x\"]]", "List[List[str]]")?;
    assert_literal_type("[[\"x\"], []]", "List[List[str]]")?;
    assert_literal_type("[[[]], [[\"x\"]]]", "List[List[List[str]]]")?;
    Ok(())
}

/// A concrete sibling must constrain later siblings instead of preserving an unknown acceptance hole.
#[test]
fn nested_list_empty_holes_do_not_accept_incompatible_later_members() -> TestResult {
    let source = "def run() -> None:\n    values = [[], [\"x\"], [1]]\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("mixed nested list was accepted")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("str") && error.message.contains("int"))
    );
    Ok(())
}

/// All-empty lists retain unresolved leaf types rather than inventing a peer constraint.
#[test]
fn nested_list_all_empty_leaves_remain_unknown() -> TestResult {
    let source = "def run() -> None:\n    values = [[], []]\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find("[[], []]").ok_or("literal missing")?;
    let actual = checker
        .type_info()
        .expr_type(Span { start, end: start + 8 })
        .ok_or("type missing")?;
    let expected = ResolvedType::Generic(
        "List".into(),
        vec![ResolvedType::Generic("List".into(), vec![ResolvedType::Unknown])],
    );
    assert_eq!(actual, &expected);
    Ok(())
}

/// Existing checked lowering must carry the inferred leaf into owned-string collection emission.
///
/// The empty leaf is asserted as `Vec::<String>::new()` rather than as a bare `vec![]`. #1493 gave an empty list
/// literal its element type, because as a comparison operand nothing else supplies one and `rustc` reported an
/// ambiguous `PartialEq`. That strengthened exactly the property this test is here for -- the first empty list must
/// not erase the later string element type -- so the assertion follows it rather than pinning the older spelling.
#[test]
fn nested_list_loop_emits_owned_strings_without_caller_annotation() -> TestResult {
    let source = include_str!("fixtures/nested_list_loop_1471.incn");
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let rust = IrCodegen::new().try_generate(&program)?;
    let compact: String = rust.chars().filter(|character| !character.is_whitespace()).collect();
    assert!(
        compact.contains("vec![Vec::<String>::new(),vec![\"x\".to_string()]]"),
        "{rust}"
    );
    Ok(())
}

/// Explicit destinations and the existing numeric compatibility policy remain authoritative.
#[test]
fn nested_list_contextual_and_numeric_controls_typecheck() -> TestResult {
    for source in [
        "def run() -> None:\n    cases: list[list[str]] = [[], [\"x\"]]\n",
        "def run() -> None:\n    cases: list[list[int | str]] = [[], [1], [\"x\"]]\n",
    ] {
        let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        TypeChecker::new()
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
    }
    let numeric = "def run() -> None:\n    cases = [[1.0], [2]]\n";
    let tokens = lexer::lex(numeric).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let errors = TypeChecker::new()
        .check_program(&program)
        .err()
        .ok_or("invariant nested numeric lists were widened")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("List[float]") && error.message.contains("List[int]"))
    );
    Ok(())
}
