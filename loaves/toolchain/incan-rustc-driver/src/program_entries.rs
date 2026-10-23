//! Native process selection for explicitly described, canonically lowered program entries.
//!
//! Selection changes neither argv nor the entry's return contract. Rustc's standard Termination implementation
//! reports the selected result, just as it reports a standalone main. Executors and integer-main handling belong
//! entirely to Incan lowering.

use crate::ast_build::{binding, block, expression, expression_statement, global_path, local, path_ty};
use crate::plan::Plan;
use rustc_ast as ast;
use rustc_span::{Span, Symbol, respan};
use thin_vec::{ThinVec, thin_vec};

/// Declare one process main that reports exactly one selected entry through the standard Termination trait.
pub(crate) fn dispatcher(plan: &Plan, span: Span) -> Box<ast::Item> {
    let mut arms = ThinVec::new();
    for (index, name) in plan.programs.iter().enumerate() {
        let pattern = ast::Pat {
            id: ast::DUMMY_NODE_ID,
            kind: ast::PatKind::TupleStruct(
                None,
                global_path(&["std", "result", "Result", "Ok"], span),
                thin_vec![*binding("selected", span)],
            ),
            span,
            tokens: None,
        };
        let guard = expression(
            ast::ExprKind::Binary(
                respan(span, ast::BinOpKind::Eq),
                local("selected", span),
                text(&index.to_string(), span),
            ),
            span,
        );
        let entry = expression(ast::ExprKind::Call(local(name, span), ThinVec::new()), span);
        arms.push(arm(
            Box::new(pattern),
            Some(guard),
            call(&["std", "process", "Termination", "report"], thin_vec![entry], span),
            span,
        ));
    }
    arms.push(arm(
        Box::new(ast::Pat {
            id: ast::DUMMY_NODE_ID,
            kind: ast::PatKind::Wild,
            span,
            tokens: None,
        }),
        None,
        expression(
            ast::ExprKind::Path(None, global_path(&["std", "process", "ExitCode", "FAILURE"], span)),
            span,
        ),
        span,
    ));
    let selector = call(
        &["std", "env", "var"],
        thin_vec![text("INCAN_CONTRACT_CASE", span)],
        span,
    );
    let value = expression(ast::ExprKind::Match(selector, arms, ast::MatchKind::Prefix), span);
    let mut item = crate::declarations::function(&plan.functions[0], span);
    if let ast::ItemKind::Fn(function) = &mut item.kind {
        function.ident = crate::ast_build::ident("main", span);
        function.sig.header = ast::FnHeader::default();
        function.sig.decl.inputs.clear();
        function.sig.decl.output = ast::FnRetTy::Ty(path_ty(global_path(&["std", "process", "ExitCode"], span), span));
        function.body = Some(block(thin_vec![expression_statement(value, span)], span));
    }
    item
}

/// Call a standard-library item using an absolute path immune to source declarations.
fn call(path: &[&str], arguments: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(
            expression(ast::ExprKind::Path(None, global_path(path, span)), span),
            arguments,
        ),
        span,
    )
}

/// Construct a selector literal without parsing generated source.
fn text(value: &str, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Lit(ast::token::Lit::new(
            ast::token::LitKind::Str,
            Symbol::intern(value),
            None,
        )),
        span,
    )
}

/// Keep selection guards explicit; a missing or unknown selector never executes a case.
fn arm(pattern: Box<ast::Pat>, guard: Option<Box<ast::Expr>>, body: Box<ast::Expr>, span: Span) -> ast::Arm {
    ast::Arm {
        attrs: ThinVec::new(),
        pat: pattern,
        guard: guard.map(|cond| {
            Box::new(ast::Guard {
                cond: *cond,
                span_with_leading_if: span,
            })
        }),
        body: Some(body),
        span,
        id: ast::DUMMY_NODE_ID,
        is_placeholder: false,
    }
}
