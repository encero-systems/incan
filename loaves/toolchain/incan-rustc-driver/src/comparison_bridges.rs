//! Native trait ABI forwarding for comparison bodies selected and validated by Incan lowering.

use crate::ast_build::{
    RUST_CMP_EQ_SYMBOL, RUST_CMP_ORD_SYMBOL, RUST_CMP_PARTIAL_EQ_SYMBOL, RUST_CMP_PARTIAL_ORD_SYMBOL, expression,
    global_path, ident, inherited, local, path_ty, self_value,
};
use crate::error::PlanError;
use crate::plan::{EnumDeclaration, ModelDeclaration, Plan, PlanType};
use incan_lang::lang::derives::{self, DeriveId};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::{ThinVec, thin_vec};

/// The common source symbol view for a validated model or enum bridge.
struct ComparisonMethods<'a> {
    name: &'a str,
    comparison_methods: &'a [String],
}

/// Validate every concrete bridge signature before injecting any trait implementation.
fn validate(plan: &Plan, owner: usize, model: &ModelDeclaration) -> Result<i64, PlanError> {
    let refused = |reason: &str| PlanError::Invalid {
        function: model.name.clone(),
        reason: format!("unsupported native comparison bridge: {reason}"),
    };
    let owner = i64::try_from(owner).map_err(|_| refused("owner index is out of range"))?;
    if !matches!(model.comparison_methods.len(), 1 | 5) || !model.library_item.is_empty() {
        return Err(refused("source comparison layout or slot count"));
    }
    if model.derives.iter().any(|derive| {
        matches!(
            derives::from_str(derive),
            Some(DeriveId::PartialEq | DeriveId::Eq | DeriveId::PartialOrd | DeriveId::Ord)
        )
    }) {
        return Err(refused("conflicting native comparison derive"));
    }
    if model.comparison_methods.len() == 5
        && !model
            .derives
            .iter()
            .any(|derive| derives::from_str(derive) == Some(DeriveId::Clone))
    {
        return Err(refused("ordering requires the retained native Clone"));
    }
    let reference = PlanType::ModelRef(owner, model.name.clone());
    for (slot, symbol) in model.comparison_methods.iter().enumerate() {
        let mut candidates = plan.functions.iter().filter(|function| &function.name == symbol);
        let Some(function) = candidates.next() else {
            return Err(refused("selected body is absent"));
        };
        let other = if slot == 0 {
            reference.clone()
        } else {
            PlanType::Model(owner, model.name.clone())
        };
        if candidates.next().is_some()
            || function.parameters.len() != 2
            || function.parameters[0].ty != reference
            || function.parameters[1].ty != other
            || function.return_type != PlanType::Bool
        {
            return Err(refused("selected body has a different input or result ABI"));
        }
    }
    Ok(owner)
}

/// Grant hashed layout admission only when the exact native equality bridge and all of its referenced signatures
/// validate.
pub(crate) fn has_equality(plan: &Plan, index: i64) -> bool {
    let Ok(owner) = usize::try_from(index) else {
        return false;
    };
    let Some(model) = plan.models.get(owner) else {
        return false;
    };
    !model.comparison_methods.is_empty() && validate(plan, owner, model).is_ok()
}

/// Inject standard native comparison traits for checked source adopters, preserving each selected body and override.
pub(crate) fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut result = Vec::new();
    for (owner, model) in plan.models.iter().enumerate() {
        if model.comparison_methods.is_empty() {
            continue;
        }
        let owner = validate(plan, owner, model)?;
        let reference = crate::declarations::ty(&PlanType::ModelRef(owner, model.name.clone()), span);
        let methods = ComparisonMethods {
            name: &model.name,
            comparison_methods: &model.comparison_methods,
        };
        result.extend(bridge_items(&methods, reference, span));
    }
    for (owner, declaration) in plan.enums.iter().enumerate() {
        if declaration.comparison_methods.is_empty() {
            continue;
        }
        let owner = validate_enum(plan, owner, declaration)?;
        let reference = crate::declarations::ty(&PlanType::EnumRef(owner, declaration.name.clone()), span);
        let methods = ComparisonMethods {
            name: &declaration.name,
            comparison_methods: &declaration.comparison_methods,
        };
        result.extend(bridge_items(&methods, reference, span));
    }
    Ok(result)
}

/// Validate a source enum's exact selected signatures and Clone authority before granting native comparison traits.
fn validate_enum(plan: &Plan, owner: usize, declaration: &EnumDeclaration) -> Result<i64, PlanError> {
    let refused = |reason: &str| PlanError::Unsupported {
        function: declaration.name.clone(),
        reason: format!("native enum comparison bridge: {reason}"),
    };
    let owner = i64::try_from(owner).map_err(|_| refused("owner index is out of range"))?;
    if !matches!(declaration.comparison_methods.len(), 1 | 5)
        || !declaration.library_item.is_empty()
        || declaration.derives.iter().any(|name| {
            matches!(
                derives::from_str(name),
                Some(DeriveId::PartialEq | DeriveId::Eq | DeriveId::PartialOrd | DeriveId::Ord)
            )
        })
    {
        return Err(refused("source layout or conflicting native derive"));
    }
    if declaration.comparison_methods.len() == 5
        && !declaration
            .derives
            .iter()
            .any(|name| derives::from_str(name) == Some(DeriveId::Clone))
    {
        return Err(refused("ordering requires retained Clone"));
    }
    let reference = PlanType::EnumRef(owner, declaration.name.clone());
    for (slot, symbol) in declaration.comparison_methods.iter().enumerate() {
        let mut candidates = plan.functions.iter().filter(|function| &function.name == symbol);
        let Some(function) = candidates.next() else {
            return Err(refused("selected body is absent"));
        };
        let other = if slot == 0 {
            reference.clone()
        } else {
            PlanType::Enum(owner, declaration.name.clone())
        };
        if candidates.next().is_some()
            || function.parameters.len() != 2
            || function.parameters[0].ty != reference
            || function.parameters[1].ty != other
            || function.return_type != PlanType::Bool
        {
            return Err(refused("selected body has a different input or result ABI"));
        }
    }
    Ok(owner)
}

/// Inject shared comparison AST only after the owner's source signatures have been validated.
fn bridge_items(model: &ComparisonMethods<'_>, reference: Box<ast::Ty>, span: Span) -> Vec<Box<ast::Item>> {
    let equality = planned(model, 0, self_value(span), local("argument", span), span);
    let mut result = vec![
        implementation(
            model,
            RUST_CMP_PARTIAL_EQ_SYMBOL,
            thin_vec![crate::shared_callables::method(
                "eq",
                Some(reference.clone()),
                crate::declarations::ty(&PlanType::Bool, span),
                equality,
                span
            )],
            span,
        ),
        implementation(model, RUST_CMP_EQ_SYMBOL, ThinVec::new(), span),
    ];
    if model.comparison_methods.len() == 5 {
        result.push(partial_order(model, reference.clone(), span));
        result.push(total_order(model, reference, span));
    }
    result
}

/// Forward a native ABI method to its exact planned source function.
fn planned(
    model: &ComparisonMethods<'_>,
    slot: usize,
    left: Box<ast::Expr>,
    right: Box<ast::Expr>,
    span: Span,
) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(local(&model.comparison_methods[slot], span), thin_vec![left, right]),
        span,
    )
}

/// Clone the owned input required by a source ordering method; equality never uses this conversion.
fn cloned(value: Box<ast::Expr>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::MethodCall(Box::new(ast::MethodCall {
            seg: ast::PathSegment::from_ident(ident("clone", span)),
            receiver: value,
            args: ThinVec::new(),
            span,
        })),
        span,
    )
}

/// Borrow a comparison's bool result for the native Ordering bridge, without allocating or cloning a source value.
fn borrowed(value: Box<ast::Expr>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, value),
        span,
    )
}

/// Build a concrete standard comparison trait implementation around the already declared source layout.
fn implementation(
    model: &ComparisonMethods<'_>,
    name: &str,
    methods: ThinVec<Box<ast::AssocItem>>,
    span: Span,
) -> Box<ast::Item> {
    let mut item = crate::declarations::item(
        ast::ItemKind::Impl(ast::Impl {
            generics: ast::Generics::default(),
            constness: ast::Const::No,
            of_trait: Some(Box::new(ast::TraitImplHeader {
                defaultness: ast::Defaultness::Implicit,
                safety: ast::Safety::Default,
                polarity: ast::ImplPolarity::Positive,
                trait_ref: ast::TraitRef {
                    path: global_path(&["std", "cmp", name], span),
                    ref_id: ast::DUMMY_NODE_ID,
                },
            })),
            self_ty: path_ty(ast::Path::from_ident(ident(model.name, span)), span),
            items: methods,
        }),
        span,
    );
    item.vis = inherited(span);
    item
}

/// Forward all four partial-order operations separately so a source override remains observable.
fn partial_order(model: &ComparisonMethods<'_>, reference: Box<ast::Ty>, span: Span) -> Box<ast::Item> {
    let comparison = expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(None, global_path(&["std", "cmp", RUST_CMP_ORD_SYMBOL, "cmp"], span)),
                span,
            ),
            thin_vec![self_value(span), local("argument", span)],
        ),
        span,
    );
    let some = expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(
                    None,
                    global_path(
                        &["std", "option", "Option", crate::ast_build::RUST_OPTION_SOME_SYMBOL],
                        span,
                    ),
                ),
                span,
            ),
            thin_vec![comparison],
        ),
        span,
    );
    let mut option = global_path(&["std", "option", "Option"], span);
    crate::shared_callables::type_arguments(&mut option, thin_vec![ordering_type(span)], span);
    let mut methods = thin_vec![crate::shared_callables::method(
        "partial_cmp",
        Some(reference.clone()),
        path_ty(option, span),
        some,
        span
    )];
    for (slot, name) in [(1, "lt"), (2, "le"), (3, "gt"), (4, "ge")] {
        let body = planned(
            model,
            slot,
            self_value(span),
            cloned(local("argument", span), span),
            span,
        );
        methods.push(crate::shared_callables::method(
            name,
            Some(reference.clone()),
            crate::declarations::ty(&PlanType::Bool, span),
            body,
            span,
        ));
    }
    implementation(model, RUST_CMP_PARTIAL_ORD_SYMBOL, methods, span)
}

/// Preserve legacy's total-order bridge: evaluate reverse less-than first, forward less-than second, then compare those
/// bools.
fn total_order(model: &ComparisonMethods<'_>, reference: Box<ast::Ty>, span: Span) -> Box<ast::Item> {
    let reverse = planned(model, 1, local("argument", span), cloned(self_value(span), span), span);
    let forward = planned(model, 1, self_value(span), cloned(local("argument", span), span), span);
    let body = expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(None, global_path(&["std", "cmp", RUST_CMP_ORD_SYMBOL, "cmp"], span)),
                span,
            ),
            thin_vec![borrowed(reverse, span), borrowed(forward, span)],
        ),
        span,
    );
    implementation(
        model,
        RUST_CMP_ORD_SYMBOL,
        thin_vec![crate::shared_callables::method(
            "cmp",
            Some(reference),
            ordering_type(span),
            body,
            span
        )],
        span,
    )
}

/// The standard library's exact native comparison result type.
fn ordering_type(span: Span) -> Box<ast::Ty> {
    path_ty(global_path(&["std", "cmp", "Ordering"], span), span)
}
