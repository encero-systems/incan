//! Apply the generic loan relationships retained by specialization to declaration AST signatures.

use std::collections::BTreeSet;

use crate::{error::PlanError, plan::Function};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};

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
        if let Some(loans) = signature.last() {
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
                        if let ast::AngleBracketedArg::Arg(ast::GenericArg::Type(ty)) = argument {
                            visit_references(ty, visit);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}
