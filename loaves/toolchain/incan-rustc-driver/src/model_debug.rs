//! Structural `Debug` implementations for models and classes declared in Incan.
//!
//! Concrete and generic layouts use this one renderer so their source names, declaration-order fields and call-form
//! punctuation cannot drift. Each field still delegates its value to that field type's own `Debug` implementation.

use crate::ast_build::{
    RUST_FMT_DEBUG_SYMBOL, block, expression, expression_statement, global_path, ident, local, self_value,
};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::{ThinVec, thin_vec};

/// Implement Incan's structural Debug text on one declared model or class layout.
///
/// `fields` must retain source declaration order. The caller supplies the actual closed or generic layout type and
/// the field-derived bounds that rustc must prove. Values delegate to their own Debug implementation, so nested Incan
/// layouts use this form recursively while Rust-defined fields keep Rust's own text.
pub(crate) fn implementation<'a>(
    name: &str,
    fields: impl IntoIterator<Item = &'a str>,
    self_ty: Box<ast::Ty>,
    generics: ast::Generics,
    span: Span,
) -> Box<ast::Item> {
    let method = crate::generic_layouts::trait_method(
        "fmt",
        crate::union_debug::fmt_signature(span),
        rendering_body(name, fields, span),
        span,
    );
    crate::generic_layouts::trait_implementation(
        self_ty,
        generics,
        &["core", "fmt", RUST_FMT_DEBUG_SYMBOL],
        thin_vec![method],
        span,
    )
}

/// Write the source call form directly to the formatter without Rust's `DebugStruct` punctuation policy.
fn rendering_body<'a>(name: &str, fields: impl IntoIterator<Item = &'a str>, span: Span) -> Box<ast::Block> {
    let mut fields = fields.into_iter().peekable();
    if fields.peek().is_none() {
        return block(
            thin_vec![expression_statement(write_text(&format!("{name}()"), span), span)],
            span,
        );
    }

    let mut statements = ThinVec::new();
    for (index, field) in fields.enumerate() {
        let prefix = if index == 0 {
            format!("{name}({field}=")
        } else {
            format!(", {field}=")
        };
        statements.push(fallible_statement(write_text(&prefix, span), span));
        statements.push(fallible_statement(write_field(field, span), span));
    }
    statements.push(expression_statement(write_text(")", span), span));
    block(statements, span)
}

/// Write one fixed structural fragment through the formatter's fallible sink.
fn write_text(value: &str, span: Span) -> Box<ast::Expr> {
    crate::generic_layouts::method_call(
        local("formatter", span),
        "write_str",
        thin_vec![crate::generic_layouts::text(value, span)],
        span,
    )
}

/// Delegate one field value to its own Debug implementation.
fn write_field(field: &str, span: Span) -> Box<ast::Expr> {
    let value = expression(ast::ExprKind::Field(self_value(span), ident(field, span)), span);
    let borrowed = expression(
        ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, value),
        span,
    );
    expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(None, global_path(&["core", "fmt", RUST_FMT_DEBUG_SYMBOL, "fmt"], span)),
                span,
            ),
            thin_vec![borrowed, local("formatter", span)],
        ),
        span,
    )
}

/// Propagate a formatter failure before the next structural fragment is written.
fn fallible_statement(value: Box<ast::Expr>, span: Span) -> ast::Stmt {
    ast::Stmt {
        id: ast::DUMMY_NODE_ID,
        kind: ast::StmtKind::Semi(expression(ast::ExprKind::Try(value), span)),
        span,
    }
}
