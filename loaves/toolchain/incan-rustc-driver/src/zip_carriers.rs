//! Exact zip storage aliases selected by the published runtime declaration or the linked runtime ABI.

use crate::declarations;
use crate::error::PlanError;
use crate::plan::{Plan, PlanType, TupleElement};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};

/// Name a crate-local alias without losing either checked tuple element's complete representation.
pub(crate) fn alias_name(left: &TupleElement, right: &TupleElement) -> String {
    let mut name = String::from("__incan_zip_type_");
    for byte in format!("{left:?}_{right:?}").bytes() {
        name.push_str(&format!("{byte:02x}"));
    }
    name
}

/// Retain zip shapes occurring inside native callable and reference signatures without deriving new element facts.
fn retain_shapes(ty: &PlanType, shapes: &mut Vec<(TupleElement, TupleElement)>) {
    match ty {
        PlanType::ZipIterator(left, right)
        | PlanType::ZipIteratorRef(left, right)
        | PlanType::ZipIteratorMutRef(left, right) => {
            let pair = (left.clone(), right.clone());
            if !shapes.contains(&pair) {
                shapes.push(pair);
            }
        }
        PlanType::FunctionPointer(arguments)
        | PlanType::FunctionItem(_, arguments)
        | PlanType::Closure(arguments)
        | PlanType::ClosureRef(arguments)
        | PlanType::CallableArguments(arguments)
        | PlanType::Future(arguments)
        | PlanType::ScalarRef(arguments)
        | PlanType::ScalarMutRef(arguments)
        | PlanType::NativeSlice(arguments)
        | PlanType::SliceRef(arguments)
        | PlanType::SliceMutRef(arguments) => {
            for argument in arguments {
                retain_shapes(argument, shapes);
            }
        }
        _ => {}
    }
}

/// Collect every required alias, including a zip used only by a runtime frame or native callable parameter.
fn shapes(plan: &Plan) -> Vec<(TupleElement, TupleElement)> {
    let mut shapes = Vec::new();
    for function in &plan.functions {
        retain_shapes(&function.return_type, &mut shapes);
        for local in &function.locals {
            retain_shapes(&local.ty, &mut shapes);
        }
    }
    for external in &plan.externals {
        retain_shapes(&external.return_type, &mut shapes);
        for ty in external.parameters.iter().chain(&external.type_arguments) {
            retain_shapes(ty, &mut shapes);
        }
    }
    for carrier in &plan.zip_carriers {
        let pair = (carrier.left.clone(), carrier.right.clone());
        if !shapes.contains(&pair) {
            shapes.push(pair);
        }
    }
    shapes
}

/// Select the source-owned result carrier once; conflicting declarations never fall back to another layout.
fn target(plan: &Plan, left: &TupleElement, right: &TupleElement, span: Span) -> Result<Box<ast::Ty>, PlanError> {
    let mut selected = None;
    for carrier in &plan.zip_carriers {
        if &carrier.left != left || &carrier.right != right {
            continue;
        }
        if !matches!(carrier.target, PlanType::Model(_, _))
            || selected.as_ref().is_some_and(|previous| previous != &carrier.target)
        {
            return Err(PlanError::Invalid {
                function: "zip carrier".into(),
                reason: "conflicting or non-nominal governed zip carrier".into(),
            });
        }
        selected = Some(carrier.target.clone());
    }
    Ok(match selected {
        Some(target) => declarations::ty(&target, span),
        None => declarations::linked_zip_ty(left, right, span),
    })
}

/// Declare one exact storage alias per checked shape before any frame signature names it.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    shapes(plan)
        .into_iter()
        .map(|(left, right)| {
            Ok(declarations::item(
                ast::ItemKind::TyAlias(Box::new(ast::TyAlias {
                    defaultness: ast::Defaultness::Implicit,
                    ident: Ident::new(Symbol::intern(&alias_name(&left, &right)), span),
                    generics: ast::Generics::default(),
                    after_where_clause: ast::WhereClause::default(),
                    bounds: Vec::new(),
                    ty: Some(target(plan, &left, &right, span)?),
                })),
                span,
            ))
        })
        .collect()
}
