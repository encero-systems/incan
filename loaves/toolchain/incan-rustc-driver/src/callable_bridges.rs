//! Native ABI forwarding for source models with a complete checked standard Callable1 adoption.

use crate::ast_build::{expression, global_path, inherited, local, path_ty, self_value};
use crate::error::PlanError;
use crate::plan::{ModelDeclaration, Plan, PlanType};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::thin_vec;

/// Validate the selected source function independently of the native implementation declaration.
fn validate(plan: &Plan, owner: usize, model: &ModelDeclaration) -> Result<(), PlanError> {
    let refused = || PlanError::Invalid {
        function: model.name.clone(),
        reason: "unsupported native Callable1 bridge ABI".into(),
    };
    let owner = i64::try_from(owner).map_err(|_| refused())?;
    if !model.library_item.is_empty() || model.callable_signature.len() != 2 {
        return Err(refused());
    }
    let mut candidates = plan
        .functions
        .iter()
        .filter(|function| function.name == model.callable_method);
    let Some(function) = candidates.next() else {
        return Err(refused());
    };
    if candidates.next().is_some()
        || function.parameters.len() != 2
        || function.parameters[0].ty != PlanType::ModelRef(owner, model.name.clone())
        || function.parameters[1].ty != model.callable_signature[0]
        || function.return_type != model.callable_signature[1]
    {
        return Err(refused());
    }
    Ok(())
}

/// Declare the exact native trait for the source layout; its behavior remains in the selected Incan function.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for (owner, model) in plan.models.iter().enumerate() {
        if model.callable_method.is_empty() && model.callable_signature.is_empty() {
            continue;
        }
        validate(plan, owner, model)?;
        let mut path = global_path(&["incan_stdlib_core", "traits", "callable", "Callable1"], span);
        crate::shared_callables::type_arguments(
            &mut path,
            model
                .callable_signature
                .iter()
                .map(|ty| crate::declarations::ty(ty, span))
                .collect(),
            span,
        );
        let body = expression(
            ast::ExprKind::Call(
                local(&model.callable_method, span),
                thin_vec![self_value(span), local("argument", span)],
            ),
            span,
        );
        let method = crate::shared_callables::method(
            "__call__",
            Some(crate::declarations::ty(&model.callable_signature[0], span)),
            crate::declarations::ty(&model.callable_signature[1], span),
            body,
            span,
        );
        let mut item = crate::declarations::item(
            ast::ItemKind::Impl(ast::Impl {
                generics: ast::Generics::default(),
                constness: ast::Const::No,
                of_trait: Some(Box::new(ast::TraitImplHeader {
                    defaultness: ast::Defaultness::Implicit,
                    safety: ast::Safety::Default,
                    polarity: ast::ImplPolarity::Positive,
                    trait_ref: ast::TraitRef {
                        path,
                        ref_id: ast::DUMMY_NODE_ID,
                    },
                })),
                self_ty: path_ty(ast::Path::from_ident(crate::ast_build::ident(&model.name, span)), span),
                items: thin_vec![method],
            }),
            span,
        );
        item.vis = inherited(span);
        result.push(item);
    }
    Ok(result)
}
