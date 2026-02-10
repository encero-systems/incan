//! List helper feature detection (remove/count/index).

use crate::frontend::ast::{CallArg, Declaration, Expr, Program, Spanned, Statement};

/// Detect list helper usage (remove, count, index).
pub fn detect_list_helpers_usage(program: &Program) -> bool {
    for decl in &program.declarations {
        match &decl.node {
            Declaration::Function(func) => {
                if body_uses_list_helpers(&func.body) {
                    return true;
                }
            }
            Declaration::Model(model) => {
                for method in &model.methods {
                    if let Some(body) = &method.node.body
                        && body_uses_list_helpers(body)
                    {
                        return true;
                    }
                }
            }
            Declaration::Class(class) => {
                for method in &class.methods {
                    if let Some(body) = &method.node.body
                        && body_uses_list_helpers(body)
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Detect whether the body uses the list helpers.
fn body_uses_list_helpers(body: &[Spanned<Statement>]) -> bool {
    body.iter().any(|stmt| stmt_uses_list_helpers(&stmt.node))
}

/// Detect whether the statement uses the list helpers.
fn stmt_uses_list_helpers(stmt: &Statement) -> bool {
    match stmt {
        Statement::Expr(expr) => expr_uses_list_helpers(&expr.node),
        Statement::Assignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::CompoundAssignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::FieldAssignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::IndexAssignment(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::TupleUnpack(unpack) => expr_uses_list_helpers(&unpack.value.node),
        Statement::TupleAssign(assign) => expr_uses_list_helpers(&assign.value.node),
        Statement::Return(Some(expr)) => expr_uses_list_helpers(&expr.node),
        Statement::If(if_stmt) => {
            body_uses_list_helpers(&if_stmt.then_body)
                || if_stmt.else_body.as_ref().is_some_and(|b| body_uses_list_helpers(b))
        }
        Statement::While(while_stmt) => body_uses_list_helpers(&while_stmt.body),
        Statement::For(for_stmt) => body_uses_list_helpers(&for_stmt.body),
        _ => false,
    }
}

/// Detect whether the expression uses the list helpers.
fn expr_uses_list_helpers(expr: &Expr) -> bool {
    match expr {
        Expr::MethodCall(_base, method, _args) => matches!(method.as_str(), "remove" | "count" | "index"),
        Expr::Call(function, args) => {
            expr_uses_list_helpers(&function.node)
                || args.iter().any(|arg| match arg {
                    CallArg::Positional(e) | CallArg::Named(_, e) => expr_uses_list_helpers(&e.node),
                })
        }
        Expr::Binary(left, _, right) => expr_uses_list_helpers(&left.node) || expr_uses_list_helpers(&right.node),
        Expr::Unary(_, expr) => expr_uses_list_helpers(&expr.node),
        Expr::List(items) | Expr::Tuple(items) | Expr::Set(items) => {
            items.iter().any(|item| expr_uses_list_helpers(&item.node))
        }
        Expr::If(if_expr) => {
            body_uses_list_helpers(&if_expr.then_body)
                || if_expr.else_body.as_ref().is_some_and(|b| body_uses_list_helpers(b))
        }
        _ => false,
    }
}
