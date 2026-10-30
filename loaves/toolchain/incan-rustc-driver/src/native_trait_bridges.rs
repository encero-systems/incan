//! Structural foreign trait declarations forwarding all behavior to Incan-authored methods.

use crate::ast_build::{expression, global_path, inherited, local, path_ty, self_value};
use crate::error::PlanError;
use crate::plan::{Plan, PlanType};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::{ThinVec, thin_vec};

/// Declare requested native impl headers and forwarding calls; rustc checks the actual trait ABI, defaults,
/// required methods and coherence. No trait-specific behavior or unchecked vtable is supplied here.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for (index, model) in plan.models.iter().enumerate() {
        let refused = |reason: &str| PlanError::Invalid {
            function: model.name.clone(),
            reason: reason.into(),
        };
        let owner = i64::try_from(index).map_err(|_| refused("native trait owner index overflow"))?;
        for adoption in &model.native_trait_adoptions {
            if !model.library_item.is_empty() {
                return Err(refused("foreign trait implementations require a local owner"));
            }
            let mut methods = ThinVec::new();
            for method in &adoption.methods {
                let mut candidates = plan
                    .functions
                    .iter()
                    .filter(|function| function.name == method.function);
                let function = candidates
                    .next()
                    .ok_or_else(|| refused("missing native trait source function"))?;
                if candidates.next().is_some() {
                    return Err(refused("ambiguous native trait source function"));
                }
                let mutable = match function.parameters.first().map(|parameter| &parameter.ty) {
                    Some(PlanType::ModelRef(index, name)) if *index == owner && name == &model.name => false,
                    Some(PlanType::ModelMutRef(index, name)) if *index == owner && name == &model.name => true,
                    _ => return Err(refused("native trait source method must borrow its exact owner")),
                };
                let mut inputs = ThinVec::new();
                let mut arguments = thin_vec![self_value(span)];
                for parameter in function.parameters.iter().skip(1) {
                    inputs.push((parameter.name.as_str(), crate::declarations::ty(&parameter.ty, span)));
                    arguments.push(local(&parameter.name, span));
                }
                let body = expression(ast::ExprKind::Call(local(&method.function, span), arguments), span);
                methods.push(crate::fallible_bridges::method(
                    &method.name,
                    mutable,
                    ThinVec::new(),
                    inputs,
                    crate::declarations::ty(&function.return_type, span),
                    body,
                    span,
                ));
            }
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
