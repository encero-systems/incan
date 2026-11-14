//! Apply the generic loan relationships retained by specialization to declaration AST signatures.

use std::collections::BTreeSet;

use crate::{error::PlanError, plan::Function};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};

/// Insert one elided AST lifetime argument for every retained loan in a source-owned nominal path. The flat signature
/// contract can then name those slots exactly as it names explicit reference nodes.
pub(crate) fn prepare_model_lifetimes(
    plan: &crate::plan::Plan,
    declaration: &mut ast::FnDecl,
    span: Span,
) -> Result<(), PlanError> {
    let invalid = |reason: &'static str| PlanError::Invalid {
        function: "model lifetime signature".into(),
        reason: reason.into(),
    };
    for parameter in &mut declaration.inputs {
        insert_model_lifetimes(plan, &mut parameter.ty, span).map_err(invalid)?;
    }
    if let ast::FnRetTy::Ty(output) = &mut declaration.output {
        insert_model_lifetimes(plan, output, span).map_err(invalid)?;
    }
    Ok(())
}

/// Add hidden lifetime arguments recursively without entering independently bound function-pointer signatures.
pub(crate) fn insert_model_lifetimes(
    plan: &crate::plan::Plan,
    ty: &mut ast::Ty,
    span: Span,
) -> Result<(), &'static str> {
    match &mut ty.kind {
        ast::TyKind::Ref(_, referent) => insert_model_lifetimes(plan, &mut referent.ty, span)?,
        ast::TyKind::Tup(elements) => {
            for element in elements {
                insert_model_lifetimes(plan, element, span)?;
            }
        }
        ast::TyKind::Slice(element) | ast::TyKind::Paren(element) => insert_model_lifetimes(plan, element, span)?,
        ast::TyKind::Path(_, path) => {
            for segment in &mut path.segments {
                if let Some(arguments) = &mut segment.args
                    && let ast::GenericArgs::AngleBracketed(arguments) = arguments.as_mut()
                {
                    for argument in &mut arguments.args {
                        if let ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty)) = argument {
                            insert_model_lifetimes(plan, ty, span)?;
                        }
                    }
                }
            }
            let Some(segment) = path.segments.last_mut() else {
                return Ok(());
            };
            let model_count = plan
                .models
                .iter()
                .find(|model| model.name == segment.ident.name.as_str())
                .map_or(0, |model| model.view_lifetime_count);
            let count = plan
                .enums
                .iter()
                .find(|declaration| declaration.name == segment.ident.name.as_str())
                .map_or(model_count, |declaration| declaration.view_lifetime_count);
            if count < 0 {
                return Err("model lifetime count is negative");
            }
            if count == 0 {
                return Ok(());
            }
            let arguments = segment.args.get_or_insert_with(|| {
                Box::new(ast::GenericArgs::AngleBracketed(ast::AngleBracketedArgs {
                    span,
                    args: thin_vec::ThinVec::new(),
                }))
            });
            let ast::GenericArgs::AngleBracketed(arguments) = arguments.as_mut() else {
                return Err("model lifetime path has parenthesized arguments");
            };
            if arguments
                .args
                .iter()
                .any(|argument| matches!(argument, ast::AngleBracketedArg::Arg(ast::GenericArg::Lifetime(_))))
            {
                return Err("model lifetime path already has lifetime arguments");
            }
            for _ in 0..count {
                arguments
                    .args
                    .push(ast::AngleBracketedArg::Arg(ast::GenericArg::Lifetime(ast::Lifetime {
                        id: ast::DUMMY_NODE_ID,
                        ident: Ident::new(Symbol::intern("'_"), span),
                    })));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Name every explicit reference and nested borrowed-nominal lifetime in a source-owned declaration's field order,
/// returning the matching declaration generics. The plan count and structural field layout must agree exactly.
pub(crate) fn apply_model_field_lifetimes(
    plan: &crate::plan::Plan,
    fields: &mut [Box<ast::Ty>],
    count: i64,
    span: Span,
) -> Result<ast::Generics, &'static str> {
    if count < 0 {
        return Err("model lifetime count is negative");
    }
    for field in fields.iter_mut() {
        insert_model_lifetimes(plan, field, span)?;
    }
    let mut index = 0usize;
    for field in fields {
        visit_references(field, &mut |lifetime| {
            if lifetime
                .as_ref()
                .is_some_and(|lifetime| lifetime.ident.name.as_str() == "'static")
            {
                return;
            }
            let name = format!("'incan_field_{index}");
            *lifetime = Some(ast::Lifetime {
                id: ast::DUMMY_NODE_ID,
                ident: Ident::new(Symbol::intern(&name), span),
            });
            index += 1;
        });
    }
    if index != usize::try_from(count).map_err(|_| "model lifetime count exceeds the native index range")? {
        return Err("model lifetime count differs from its field layout");
    }
    Ok(model_lifetime_generics(index, span))
}

/// Declare the stable lifetime names used by a source-owned model and its specialized alias.
pub(crate) fn model_lifetime_generics(count: usize, span: Span) -> ast::Generics {
    ast::Generics {
        params: (0..count)
            .map(|index| ast::GenericParam {
                id: ast::DUMMY_NODE_ID,
                ident: Ident::new(Symbol::intern(&format!("'incan_field_{index}")), span),
                attrs: Default::default(),
                bounds: Vec::new(),
                is_placeholder: false,
                kind: ast::GenericParamKind::Lifetime,
                colon_span: None,
            })
            .collect(),
        ..ast::Generics::default()
    }
}

/// Bind only source-proven generic loans; synthesized outer parameter borrows keep independent elision.
pub(crate) fn apply(
    function: &Function,
    declaration: &mut ast::FnDecl,
    span: Span,
) -> Result<ast::Generics, PlanError> {
    let invalid = |reason: &str| PlanError::Invalid {
        function: function.name.clone(),
        reason: reason.into(),
    };
    let signature = &function.signature_lifetimes;
    if signature.is_empty() {
        return Ok(ast::Generics::default());
    }
    if signature.len() != declaration.inputs.len() + 1 {
        return Err(invalid("generic loan signature arity differs from its declaration"));
    }
    let mut names = BTreeSet::new();
    for (parameter, loans) in declaration.inputs.iter_mut().zip(signature) {
        apply_type(&mut parameter.ty, loans, true, span, &mut names).map_err(invalid)?;
    }
    if let ast::FnRetTy::Ty(output) = &mut declaration.output {
        if let Some(loans) = signature.last()
            && !(function.retained_result_source >= 0 && !loans.is_empty() && reference_count(output) == 0)
        {
            apply_type(output, loans, false, span, &mut names).map_err(invalid)?;
        }
    }
    Ok(ast::Generics {
        params: names
            .into_iter()
            .map(|name| ast::GenericParam {
                id: ast::DUMMY_NODE_ID,
                ident: Ident::new(Symbol::intern(&format!("'incan_{name}")), span),
                attrs: Default::default(),
                bounds: Vec::new(),
                is_placeholder: false,
                kind: ast::GenericParamKind::Lifetime,
                colon_span: None,
            })
            .collect(),
        ..ast::Generics::default()
    })
}

/// Count explicit or hidden source-owned lifetime slots visible in one injected AST type.
fn reference_count(ty: &mut ast::Ty) -> usize {
    let mut count = 0;
    visit_references(ty, &mut |_| count += 1);
    count
}

/// Match structural reference order exactly, allowing only the parameter ABI's additional outer borrow.
fn apply_type(
    ty: &mut ast::Ty,
    loans: &[String],
    parameter: bool,
    span: Span,
    names: &mut BTreeSet<String>,
) -> Result<(), &'static str> {
    if !loans.iter().any(|name| !name.is_empty()) {
        return Ok(());
    }
    let mut count = 0;
    visit_references(ty, &mut |_| count += 1);
    let mut target = ty;
    if parameter && count == loans.len() + 1 {
        let ast::TyKind::Ref(None, referent) = &mut target.kind else {
            return Err("generic loan signature has an unexpected additional reference");
        };
        target = &mut referent.ty;
        count -= 1;
    }
    if count != loans.len() {
        return Err("generic loan signature reference shape differs from its declaration");
    }
    let mut labels = loans.iter();
    visit_references(target, &mut |lifetime| {
        if let Some(name) = labels.next().filter(|name| !name.is_empty()) {
            names.insert(name.clone());
            *lifetime = Some(ast::Lifetime {
                id: ast::DUMMY_NODE_ID,
                ident: Ident::new(Symbol::intern(&format!("'incan_{name}")), span),
            });
        }
    });
    Ok(())
}

/// Traverse reference-bearing type arguments without entering independently bound function-pointer signatures.
fn visit_references(ty: &mut ast::Ty, visit: &mut impl FnMut(&mut Option<ast::Lifetime>)) {
    match &mut ty.kind {
        ast::TyKind::Ref(lifetime, referent) => {
            visit(lifetime);
            visit_references(&mut referent.ty, visit);
        }
        ast::TyKind::Tup(elements) => {
            for element in elements {
                visit_references(element, visit);
            }
        }
        ast::TyKind::Slice(element) | ast::TyKind::Paren(element) => visit_references(element, visit),
        ast::TyKind::Path(_, path) => {
            for segment in &mut path.segments {
                let Some(arguments) = &mut segment.args else {
                    continue;
                };
                if let ast::GenericArgs::AngleBracketed(arguments) = arguments.as_mut() {
                    for argument in &mut arguments.args {
                        match argument {
                            ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty)) => {
                                visit_references(ty, visit);
                            }
                            ast::AngleBracketedArg::Arg(ast::GenericArg::Lifetime(lifetime)) => {
                                let mut selected = Some(lifetime.clone());
                                visit(&mut selected);
                                if let Some(selected) = selected {
                                    *lifetime = selected;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        _ => {}
    }
}
