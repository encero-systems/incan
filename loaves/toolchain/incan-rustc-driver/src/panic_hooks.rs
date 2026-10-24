//! Native boxing glue for a checked function-pointer panic callback; reporting policy remains in Incan.

use crate::ast_build::{block, expression, expression_statement, global_path, inherited, local};
use crate::error::PlanError;
use crate::plan::{CalleeKind, Function, Parameter, Plan, PlanType, TerminatorKind};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::{ThinVec, thin_vec};

/// Name one native callback signature without depending on its source function's spelling.
pub(crate) fn name(signature: &[PlanType]) -> String {
    format!("__incan_panic_hook_{}", crate::shared_callables::name(signature))
}

/// Let rustc own the allocation, unsizing, auto-trait obligations and hook API signature. The callback's executable
/// body is an ordinary Incan-planned function; this bridge adds no reporting or payload semantics.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut signatures = Vec::new();
    for function in &plan.functions {
        for block in &function.blocks {
            if let TerminatorKind::Call(callee, _, _, _, _) = &block.terminator.kind
                && let CalleeKind::InstallPanicHook(signature) = &callee.kind
                && !signatures.contains(signature)
            {
                signatures.push(signature.clone());
            }
        }
    }
    signatures
        .into_iter()
        .map(|signature| declaration(plan, signature, span))
        .collect()
}

/// Inject a native `set_hook(Box::new(callback))` boundary with the exact checked function-pointer carrier.
fn declaration(plan: &Plan, signature: Vec<PlanType>, span: Span) -> Result<Box<ast::Item>, PlanError> {
    let function = Function {
        name: name(&signature),
        parameters: vec![Parameter {
            name: "callback".into(),
            ty: PlanType::FunctionPointer(signature),
            span: plan.span.clone(),
        }],
        return_type: PlanType::Unit,
        locals: Vec::new(),
        blocks: Vec::new(),
        span: plan.span.clone(),
        is_async: false,
        future_callee: None,
        signature_lifetimes: Vec::new(),
        attributes: Vec::new(),
    };
    let mut item = crate::declarations::function(&function, span)?;
    item.vis = inherited(span);
    let ast::ItemKind::Fn(declaration) = &mut item.kind else {
        return Err(PlanError::Invalid {
            function: function.name,
            reason: "missing native hook declaration".into(),
        });
    };
    let boxed = call(
        &["std", "boxed", "Box", "new"],
        thin_vec![local("callback", span)],
        span,
    );
    let install = call(&["std", "panic", "set_hook"], thin_vec![boxed], span);
    declaration.body = Some(block(thin_vec![expression_statement(install, span)], span));
    Ok(item)
}

/// Build a rooted native call so a same-spelled source declaration cannot capture the boxing or hook operation.
fn call(path: &[&str], arguments: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(
            expression(ast::ExprKind::Path(None, global_path(path, span)), span),
            arguments,
        ),
        span,
    )
}
