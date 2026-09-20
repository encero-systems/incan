//! Reads inside a `return` operand are last uses on that path, except for loop bindings (#1489).
//!
//! Lowering records `VarAccess` for every local read, and every ownership plan clones a non-consuming `Read` at an
//! owned sink. A `return` leaves the function, so the final read of an owned local in its operand can move however
//! many loops or arms surround it; a `for` pattern binding cannot, because the emitter ordinarily iterates by shared
//! reference and the binding is a borrow of the element. These tests pin the recorded access per shape.

use incan_frontend::typechecker::TypeChecker;
use incan_frontend::{lexer, parser};
use incan_ir::decl::IrDeclKind;
use incan_ir::expr::{IrExpr, IrExprKind, VarAccess, VarRefKind};
use incan_ir::lower::AstLowering;
use incan_ir::stmt::{IrStmt, IrStmtKind};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Lower one program and collect `(name, access)` for every local read inside a `return` operand of `probe`.
fn return_operand_reads(source: &str) -> Result<Vec<(String, VarAccess)>, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let ir = AstLowering::new_with_type_info(checker.type_info().clone())
        .lower_program(&program)
        .map_err(|error| format!("{error:?}"))?;
    let body = ir
        .declarations
        .into_iter()
        .find_map(|decl| match decl.kind {
            IrDeclKind::Function(function) if function.name == "probe" => Some(function.body),
            _ => None,
        })
        .ok_or("probe missing from lowered program")?;
    let mut reads = Vec::new();
    for stmt in &body {
        collect_return_reads(stmt, &mut reads);
    }
    Ok(reads)
}

/// Walk statements for `return` operands and record the local reads inside them.
fn collect_return_reads(stmt: &IrStmt, reads: &mut Vec<(String, VarAccess)>) {
    match &stmt.kind {
        IrStmtKind::Return(Some(value)) => collect_reads(value, reads),
        IrStmtKind::For { body, .. } | IrStmtKind::While { body, .. } | IrStmtKind::Loop { body, .. } => {
            for stmt in body {
                collect_return_reads(stmt, reads);
            }
        }
        IrStmtKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            for stmt in then_branch.iter().chain(else_branch.iter().flatten()) {
                collect_return_reads(stmt, reads);
            }
        }
        IrStmtKind::Match { arms, .. } => {
            for arm in arms {
                collect_expr_return_reads(&arm.body, reads);
            }
        }
        IrStmtKind::Expr(value) => collect_expr_return_reads(value, reads),
        _ => {}
    }
}

/// Walk block-shaped expressions (match arms, blocks) for nested `return` statements.
fn collect_expr_return_reads(expr: &IrExpr, reads: &mut Vec<(String, VarAccess)>) {
    match &expr.kind {
        IrExprKind::Block { stmts, .. } => {
            for stmt in stmts {
                collect_return_reads(stmt, reads);
            }
        }
        IrExprKind::Match { arms, .. } => {
            for arm in arms {
                collect_expr_return_reads(&arm.body, reads);
            }
        }
        _ => {}
    }
}

/// Record every local value read reachable inside one expression; constructor and type names are skipped.
fn collect_reads(expr: &IrExpr, reads: &mut Vec<(String, VarAccess)>) {
    match &expr.kind {
        IrExprKind::Var {
            name,
            access,
            ref_kind: VarRefKind::Value,
        } if !matches!(name.as_str(), "Ok" | "Err" | "Some") => reads.push((name.clone(), *access)),
        IrExprKind::Struct { fields, .. } => {
            for (_, value) in fields {
                collect_reads(value, reads);
            }
        }
        IrExprKind::Call { func, args, .. } => {
            collect_reads(func, reads);
            for arg in args {
                collect_reads(&arg.expr, reads);
            }
        }
        IrExprKind::Tuple(items) => {
            for item in items {
                collect_reads(item, reads);
            }
        }
        IrExprKind::ListComp { element, .. } => collect_reads(element, reads),
        IrExprKind::BinOp { left, right, .. } => {
            collect_reads(left, reads);
            collect_reads(right, reads);
        }
        IrExprKind::Match { scrutinee, arms } => {
            collect_reads(scrutinee, reads);
            for arm in arms {
                collect_reads(&arm.body, reads);
            }
        }
        IrExprKind::Block { stmts, value } => {
            for stmt in stmts {
                if let IrStmtKind::Return(Some(value)) | IrStmtKind::Expr(value) = &stmt.kind {
                    collect_reads(value, reads);
                }
            }
            if let Some(value) = value {
                collect_reads(value, reads);
            }
        }
        _ => {}
    }
}

/// A `for` binding returned from inside its loop stays a non-consuming read: the emitter iterates by reference.
#[test]
fn loop_binding_returned_inside_its_loop_stays_a_read() -> TestResult {
    let reads = return_operand_reads(
        "def probe(values: list[list[str]], name: str) -> Result[list[str], str]:\n    for row in values:\n        if row[0] == name:\n            return Ok(row)\n    return Err(\"absent\")\n",
    )?;
    assert_eq!(reads, vec![("row".to_string(), VarAccess::Read)]);
    Ok(())
}

/// An owned local returned from inside a loop is at its last use and moves, even though it is `mut`.
#[test]
fn owned_local_returned_inside_a_loop_moves() -> TestResult {
    let reads = return_operand_reads(
        "def probe(names: list[str], name: str) -> Result[list[str], str]:\n    mut found: list[str] = []\n    for candidate in names:\n        if candidate == name:\n            found.append(candidate)\n            return Ok(found)\n    return Err(\"absent\")\n",
    )?;
    assert_eq!(reads, vec![("found".to_string(), VarAccess::Move)]);
    Ok(())
}

/// A pattern binding returned from a match arm inside a loop moves too.
#[test]
fn match_binding_returned_inside_a_loop_moves() -> TestResult {
    let reads = return_operand_reads(
        "def probe(values: list[Option[str]]) -> Result[str, str]:\n    for value in values:\n        match value:\n            Some(inner) => return Ok(inner)\n            None => pass\n    return Err(\"absent\")\n",
    )?;
    assert_eq!(reads, vec![("inner".to_string(), VarAccess::Move)]);
    Ok(())
}

/// Two reads of one local inside the same operand: only the final read moves.
#[test]
fn repeated_read_inside_the_operand_moves_only_the_last() -> TestResult {
    let reads = return_operand_reads(
        "def probe(values: list[str]) -> (list[str], list[str]):\n    for value in values:\n        if value == \"stop\":\n            return (values, values)\n    return (values, values)\n",
    )?;
    assert_eq!(
        reads,
        vec![
            ("values".to_string(), VarAccess::Read),
            ("values".to_string(), VarAccess::Move),
            ("values".to_string(), VarAccess::Read),
            ("values".to_string(), VarAccess::Move),
        ]
    );
    Ok(())
}

/// A comprehension binding inside the operand is per-item and keeps the conservative read.
#[test]
fn comprehension_binding_inside_the_operand_stays_a_read() -> TestResult {
    let reads = return_operand_reads(
        "def probe(values: list[list[str]]) -> list[list[str]]:\n    while true:\n        return [row for row in values]\n",
    )?;
    assert!(
        reads
            .iter()
            .any(|(name, access)| name == "row" && *access == VarAccess::Read),
        "expected the comprehension binding to stay a non-consuming read: {reads:?}"
    );
    assert!(
        !reads
            .iter()
            .any(|(name, access)| name == "row" && *access == VarAccess::Move),
        "a comprehension binding must never move: {reads:?}"
    );
    Ok(())
}
