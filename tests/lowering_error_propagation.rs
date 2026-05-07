use incan::backend::{AstLowering, LoweringError};
use incan::frontend::ast;

fn span() -> ast::Span {
    ast::Span { start: 0, end: 0 }
}

#[test]
fn lowering_tuple_assign_in_if_block_returns_error() {
    // Build an expression-level if with a TupleAssign statement in the then branch.
    let then_stmt = ast::Spanned::new(
        ast::Statement::TupleAssign(ast::TupleAssignStmt {
            targets: vec![ast::Spanned::new(ast::Expr::Ident("a".into()), span())],
            value: ast::Spanned::new(ast::Expr::Ident("b".into()), span()),
        }),
        span(),
    );
    let if_expr = ast::Expr::If(Box::new(ast::IfExpr {
        condition: ast::Spanned::new(ast::Expr::Literal(ast::Literal::Bool(true)), span()),
        then_body: vec![then_stmt],
        else_body: None,
    }));

    let mut lowering = AstLowering::new();
    let res = lowering.lower_expr(&if_expr, span());
    match res {
        Ok(_) => panic!("expected LoweringError, got Ok"),
        Err(LoweringError { message, .. }) => {
            assert!(message.contains("TupleAssign not yet implemented"));
        }
    }
}
