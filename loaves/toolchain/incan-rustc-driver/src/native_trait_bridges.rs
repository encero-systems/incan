//! Rust trait implementations a source class fills with its own planned methods.

use crate::ast_build::{expression, global_path, inherited, local, path_ty, self_value};
use crate::error::PlanError;
use crate::plan::{ModelDeclaration, NativeTraitMethod, Plan, PlanType};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::{ThinVec, thin_vec};

/// Declare one impl header per adopted Rust trait, each method forwarding to the class's planned method. rustc checks
/// the implementation against the trait: signatures, required methods, defaults, supertraits and coherence. Nothing
/// here is trait-specific, and no vtable is built by hand.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for (index, model) in plan.models.iter().enumerate() {
        for adoption in &model.native_trait_adoptions {
            if !model.library_item.is_empty() {
                return Err(refused(model, "a Rust trait implementation requires a source class"));
            }
            let owner = i64::try_from(index).map_err(|_| refused(model, "Rust trait implementer index overflow"))?;
            let methods = adoption
                .methods
                .iter()
                .map(|method| forwarding_method(plan, model, owner, method, span))
                .collect::<Result<ThinVec<_>, _>>()?;
            let path = global_path(&adoption.path.split("::").collect::<Vec<_>>(), span);
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
                    items: methods,
                }),
                span,
            );
            item.vis = inherited(span);
            result.push(item);
        }
    }
    Ok(result)
}

/// Declare the trait method with the planned method's receiver, parameters and result, calling that method with
/// its own arguments. The receiver must borrow exactly this class; the trait decides whether that borrow fits.
fn forwarding_method(
    plan: &Plan,
    model: &ModelDeclaration,
    owner: i64,
    method: &NativeTraitMethod,
    span: Span,
) -> Result<Box<ast::AssocItem>, PlanError> {
    let mut candidates = plan
        .functions
        .iter()
        .filter(|function| function.name == method.function);
    let function = candidates
        .next()
        .ok_or_else(|| refused(model, "missing Rust trait method body"))?;
    if candidates.next().is_some() {
        return Err(refused(model, "ambiguous Rust trait method body"));
    }
    let mutable = match function.parameters.first().map(|parameter| &parameter.ty) {
        Some(PlanType::ModelRef(index, name)) if *index == owner && name == &model.name => false,
        Some(PlanType::ModelMutRef(index, name)) if *index == owner && name == &model.name => true,
        _ => return Err(refused(model, "a Rust trait method must borrow its own class")),
    };
    let mut inputs = ThinVec::new();
    let mut arguments = thin_vec![self_value(span)];
    for parameter in function.parameters.iter().skip(1) {
        inputs.push((parameter.name.as_str(), crate::declarations::ty(&parameter.ty, span)));
        arguments.push(local(&parameter.name, span));
    }
    let body = expression(ast::ExprKind::Call(local(&method.function, span), arguments), span);
    Ok(crate::fallible_bridges::method(
        &method.name,
        mutable,
        ThinVec::new(),
        inputs,
        crate::declarations::ty(&function.return_type, span),
        body,
        span,
    ))
}

/// Refuse a Rust trait implementation the plan cannot state exactly.
fn refused(model: &ModelDeclaration, reason: &str) -> PlanError {
    PlanError::Invalid {
        function: model.name.clone(),
        reason: reason.into(),
    }
}
