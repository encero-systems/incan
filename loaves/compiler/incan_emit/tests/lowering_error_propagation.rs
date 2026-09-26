use incan_frontend::ast;
use incan_ir::{AstLowering, LoweringError};

fn span() -> ast::Span {
    ast::Span { start: 0, end: 0 }
}

#[test]
fn lowering_tuple_assign_in_if_block_returns_error() -> Result<(), String> {
    // An expression-level `if` whose then branch assigns a tuple to something that is not a place: the statement's
    // lowering error must reach the caller of `lower_expr`.
    let then_stmt = ast::Spanned::new(
        ast::Statement::TupleAssign(ast::TupleAssignStmt {
            targets: vec![ast::Spanned::new(ast::Expr::Literal(ast::Literal::Bool(false)), span())],
            value: ast::Spanned::new(ast::Expr::Literal(ast::Literal::Bool(true)), span()),
        }),
        span(),
    );
    let if_expr = ast::Expr::If(Box::new(ast::IfExpr {
        condition: ast::Spanned::new(ast::Expr::Literal(ast::Literal::Bool(true)), span()),
        then_body: vec![then_stmt],
        else_body: None,
    }));

    let mut lowering = AstLowering::new();
    match lowering.lower_expr(&if_expr, span()) {
        Ok(_) => Err("expected the tuple-assignment target's LoweringError, got Ok".to_string()),
        Err(LoweringError { message, .. }) if message.contains("tuple assignment target") => Ok(()),
        Err(LoweringError { message, .. }) => Err(format!("unexpected lowering error: {message}")),
    }
}
