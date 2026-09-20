//! Named constructor arguments written out of declaration order evaluate in written order (#1462).
//!
//! The emitter assembles a construction in declared field order and Rust evaluates a struct literal's fields as
//! spelled, so lowering sequences the written arguments into temporaries first. These tests pin the lowered shape:
//! where the temporaries appear, in which order, and when nothing needs sequencing at all.

use incan_emit::IrCodegen;
use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_ir::decl::IrDeclKind;
use incan_ir::expr::IrExprKind;
use incan_ir::lower::AstLowering;
use incan_ir::stmt::IrStmtKind;
use incan_ir::{IrStmt, TypedExpr};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MODELS: &str = "model Evidence:\n    source: str\n\nmodel Document:\n    evidence: Evidence\n    intent: str\n\ndef inspect(source: str) -> str:\n    return source\n\n";

/// Lower one program and return the statements of `main`.
fn lowered_main_body(source: &str) -> Result<Vec<IrStmt>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let ir = AstLowering::new_with_type_info(checker.type_info().clone())
        .lower_program(&program)
        .map_err(|error| format!("{error:?}"))?;
    ir.declarations
        .into_iter()
        .find_map(|decl| match decl.kind {
            IrDeclKind::Function(function) if function.name == "main" => Some(function.body),
            _ => None,
        })
        .ok_or_else(|| "main missing from lowered program".into())
}

/// Return the initializer of the `result` binding in `main`.
fn result_initializer(body: &[IrStmt]) -> Result<&TypedExpr, Box<dyn std::error::Error>> {
    body.iter()
        .find_map(|stmt| match &stmt.kind {
            IrStmtKind::Let { name, value, .. } if name == "result" => Some(value),
            _ => None,
        })
        .ok_or_else(|| "`result` binding missing from main".into())
}

/// Return the names bound by the `let` statements of a sequencing block, in order.
fn let_names(stmts: &[IrStmt]) -> Vec<&str> {
    stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Let { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// Arguments written out of declaration order are bound to temporaries in written order, and the construction reads
/// those temporaries.
#[test]
fn reordered_named_arguments_are_sequenced_in_written_order() -> TestResult {
    let source = format!(
        "{MODELS}def main() -> None:\n    source = \"manifest\"\n    result = Document(intent=inspect(source), evidence=Evidence(source=source))\n    println(result.intent)\n"
    );
    let body = lowered_main_body(&source)?;
    let IrExprKind::Block { stmts, value } = &result_initializer(&body)?.kind else {
        return Err("a reordered construction must lower to a sequencing block".into());
    };
    assert_eq!(let_names(stmts), ["__incan_ctor_arg_0", "__incan_ctor_arg_1"]);
    let temporaries: Vec<&str> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            IrStmtKind::Let { value, .. } => Some(match &value.kind {
                IrExprKind::Call { .. } => "call",
                IrExprKind::Struct { name, .. } => name.as_str(),
                _ => "other",
            }),
            _ => None,
        })
        .collect();
    assert_eq!(
        temporaries,
        ["call", "Evidence"],
        "`intent=inspect(source)` was written first and must be evaluated first"
    );
    let Some(value) = value else {
        return Err("the sequencing block must produce the construction".into());
    };
    let IrExprKind::Struct { name, fields, .. } = &value.kind else {
        return Err("the sequencing block must end in the struct construction".into());
    };
    assert_eq!(name, "Document");
    let reads: Vec<(&str, &str)> = fields
        .iter()
        .map(|(field, value)| match &value.kind {
            IrExprKind::Var { name, .. } => (field.as_str(), name.as_str()),
            _ => (field.as_str(), "not a temporary read"),
        })
        .collect();
    assert_eq!(
        reads,
        [("intent", "__incan_ctor_arg_0"), ("evidence", "__incan_ctor_arg_1")]
    );
    Ok(())
}

/// A construction written in declaration order needs no sequencing and lowers to the plain struct.
#[test]
fn declaration_order_arguments_stay_a_plain_construction() -> TestResult {
    let source = format!(
        "{MODELS}def main() -> None:\n    source = \"manifest\"\n    result = Document(evidence=Evidence(source=source), intent=inspect(source))\n    println(result.intent)\n"
    );
    let body = lowered_main_body(&source)?;
    assert!(
        matches!(result_initializer(&body)?.kind, IrExprKind::Struct { .. }),
        "declaration-order arguments have nothing to sequence"
    );
    Ok(())
}

/// Reordered arguments that are all literals stay inline: evaluating a literal has no effect and no owner.
#[test]
fn reordered_literal_arguments_are_not_sequenced() -> TestResult {
    let source = "model Point:\n    x: int\n    y: int\n\ndef main() -> None:\n    result = Point(y=2, x=1)\n    println(result.x)\n";
    let body = lowered_main_body(source)?;
    assert!(
        matches!(result_initializer(&body)?.kind, IrExprKind::Struct { .. }),
        "literal arguments cannot observe their evaluation order"
    );
    Ok(())
}

/// End to end: the generated Rust reads `source` for `intent` before `evidence` moves it.
#[test]
fn generated_rust_evaluates_reordered_arguments_in_written_order() -> TestResult {
    let source = format!(
        "{MODELS}def main() -> None:\n    source = \"manifest\"\n    result = Document(intent=inspect(source), evidence=Evidence(source=source))\n    println(result.intent)\n"
    );
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let rust: String = IrCodegen::new()
        .try_generate(&program)?
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    let intent = rust
        .find("source.to_string()")
        .ok_or_else(|| format!("the `intent` argument must read `source` without consuming it:\n{rust}"))?;
    let evidence = rust
        .find("Evidence{source:source}")
        .ok_or_else(|| format!("the `evidence` argument must take `source` by move:\n{rust}"))?;
    assert!(
        intent < evidence,
        "`intent` was written first, so its read of `source` must precede the move into `evidence`:\n{rust}"
    );
    Ok(())
}
