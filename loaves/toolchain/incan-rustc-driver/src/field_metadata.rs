//! Immutable compiler-generated field metadata, constructed from the Incan plan's checked declaration facts.

use crate::ast_build::{
    RUST_OPTION_NONE_SYMBOL, RUST_OPTION_SOME_SYMBOL, block, expression, expression_statement, global_path, ident,
    local, path_ty,
};
use crate::error::PlanError;
use crate::plan::{CalleeKind, FieldMetadata, ModelDeclaration, Plan, PlanType, TerminatorKind};
use incan_lang::lang::types::collections::{self, CollectionTypeId};
use rustc_ast as ast;
use rustc_span::{Span, Symbol};
use std::collections::BTreeSet;
use thin_vec::{ThinVec, thin_vec};

/// Name the immutable metadata accessor of one exact model layout.
pub fn name(owner: i64) -> String {
    format!("__incan_field_metadata_{owner}")
}

/// Validate the selected source declaration and the native SDK FieldInfo output before generating any accessor.
pub fn result(plan: &Plan, owner: i64, info: i64) -> Result<PlanType, PlanError> {
    let refused = || PlanError::Invalid {
        function: name(owner),
        reason: "unsupported field metadata declaration or SDK result identity".into(),
    };
    let model = usize::try_from(owner)
        .ok()
        .and_then(|index| plan.models.get(index))
        .ok_or_else(refused)?;
    let fields = model.reflection_metadata.as_ref().ok_or_else(refused)?;
    let metadata = usize::try_from(info)
        .ok()
        .and_then(|index| plan.models.get(index))
        .ok_or_else(refused)?;
    if !model.library_item.is_empty()
        || !model.derives.iter().any(|derive| derive == "FieldInfo")
        || metadata.library_item != "incan_std_core::reflection::FieldInfo"
        || fields
            .iter()
            .any(|field| !model.fields.iter().any(|slot| slot.name == field.name))
    {
        return Err(refused());
    }
    Ok(PlanType::FrozenCollection(
        collections::as_str(CollectionTypeId::FrozenList).into(),
        vec![PlanType::Model(info, metadata.name.clone())],
    ))
}

/// Inject each selected metadata accessor and its immutable static records once, retaining native const evaluation.
pub fn items(plan: &Plan, span: Span) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for block in plan.functions.iter().flat_map(|function| &function.blocks) {
        let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
            continue;
        };
        let CalleeKind::FieldMetadata(owner, info) = &callee.kind else {
            continue;
        };
        let output = result(plan, *owner, *info)?;
        if !seen.insert(*owner) {
            continue;
        }
        let index = usize::try_from(*owner).map_err(|_| PlanError::UnknownCallee(name(*owner)))?;
        items.push(storage(*owner, &plan.models[index], span));
        items.push(accessor(*owner, &output, span));
    }
    Ok(items)
}

/// Declare a zero-input accessor; the source receiver has already been evaluated by Body IR and is never consumed.
fn accessor(owner: i64, output: &PlanType, span: Span) -> Box<ast::Item> {
    let value = call(
        &[
            "incan_std_core",
            "frozen",
            collections::as_str(CollectionTypeId::FrozenList),
            "new",
        ],
        thin_vec![local(&storage_name(owner), span)],
        span,
    );
    accessor_item(owner, output, value, span)
}

/// Name the private static slice independently from its accessor's function identity.
fn storage_name(owner: i64) -> String {
    format!("__incan_field_records_{owner}")
}

/// Construct the records in an explicit static initializer; const-function calls do not promote runtime temporaries.
fn storage(owner: i64, model: &ModelDeclaration, span: Span) -> Box<ast::Item> {
    let entries = model
        .reflection_metadata
        .as_ref()
        .into_iter()
        .flatten()
        .map(|field| entry(field, span))
        .collect();
    let array = expression(ast::ExprKind::Array(entries), span);
    let slice = expression(
        ast::ExprKind::AddrOf(ast::BorrowKind::Ref, ast::Mutability::Not, array),
        span,
    );
    let element = path_ty(global_path(&["incan_std_core", "reflection", "FieldInfo"], span), span);
    let ty = Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Ref(
            None,
            ast::MutTy {
                ty: Box::new(ast::Ty {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::TyKind::Slice(element),
                    span,
                    tokens: None,
                }),
                mutbl: ast::Mutability::Not,
            },
        ),
        span,
        tokens: None,
    });
    crate::declarations::item(
        ast::ItemKind::Static(Box::new(ast::StaticItem {
            ident: ident(&storage_name(owner), span),
            ty,
            safety: ast::Safety::Default,
            mutability: ast::Mutability::Not,
            expr: Some(slice),
            define_opaque: None,
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Declare the native accessor with its exact validated frozen-list result type.
fn accessor_item(owner: i64, output: &PlanType, value: Box<ast::Expr>, span: Span) -> Box<ast::Item> {
    crate::declarations::item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&name(owner), span),
            generics: ast::Generics::default(),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs: ThinVec::new(),
                    output: ast::FnRetTy::Ty(crate::declarations::ty(output, span)),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: Some(block(thin_vec![expression_statement(value, span)], span)),
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Reproduce the frozen legacy record exactly; extra metadata is currently an empty frozen dictionary there too.
fn entry(field: &FieldMetadata, span: Span) -> Box<ast::Expr> {
    let values = [
        ("name", frozen_text(&field.name, span)),
        ("alias", optional_text(&field.alias, span)),
        ("description", optional_text(&field.description, span)),
        (
            "wire_name",
            frozen_text(field.alias.as_deref().unwrap_or(&field.name), span),
        ),
        ("type_name", frozen_text(&field.type_name, span)),
        (
            "has_default",
            expression(
                ast::ExprKind::Lit(ast::token::Lit::new(
                    ast::token::LitKind::Bool,
                    Symbol::intern(if field.has_default { "true" } else { "false" }),
                    None,
                )),
                span,
            ),
        ),
        (
            "extra",
            call(
                &[
                    "incan_std_core",
                    "frozen",
                    collections::as_str(CollectionTypeId::FrozenDict),
                    "new",
                ],
                thin_vec![expression(
                    ast::ExprKind::AddrOf(
                        ast::BorrowKind::Ref,
                        ast::Mutability::Not,
                        expression(ast::ExprKind::Array(ThinVec::new()), span)
                    ),
                    span
                )],
                span,
            ),
        ),
    ];
    expression(
        ast::ExprKind::Struct(Box::new(ast::StructExpr {
            qself: None,
            path: global_path(&["incan_std_core", "reflection", "FieldInfo"], span),
            fields: values
                .into_iter()
                .map(|(name, value)| ast::ExprField {
                    attrs: ThinVec::new(),
                    id: ast::DUMMY_NODE_ID,
                    span,
                    ident: ident(name, span),
                    expr: value,
                    is_shorthand: false,
                    is_placeholder: false,
                })
                .collect(),
            rest: ast::StructRest::None,
        })),
        span,
    )
}

/// Construct one frozen text value from an escaped native literal, preserving all source bytes.
fn frozen_text(text: &str, span: Span) -> Box<ast::Expr> {
    let literal = expression(
        ast::ExprKind::Lit(ast::token::Lit::new(
            ast::token::LitKind::Str,
            Symbol::intern(&text.escape_default().to_string()),
            None,
        )),
        span,
    );
    call(
        &["incan_std_core", "frozen", "FrozenStr", "new"],
        thin_vec![literal],
        span,
    )
}

/// Preserve the distinction between absent metadata and an explicitly empty string.
fn optional_text(value: &Option<String>, span: Span) -> Box<ast::Expr> {
    match value {
        Some(text) => call(
            &["core", "option", "Option", RUST_OPTION_SOME_SYMBOL],
            thin_vec![frozen_text(text, span)],
            span,
        ),
        None => expression(
            ast::ExprKind::Path(
                None,
                global_path(&["core", "option", "Option", RUST_OPTION_NONE_SYMBOL], span),
            ),
            span,
        ),
    }
}

/// Call an exact native path structurally without parsing generated Rust text.
fn call(path: &[&str], arguments: ThinVec<Box<ast::Expr>>, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Call(
            expression(ast::ExprKind::Path(None, global_path(path, span)), span),
            arguments,
        ),
        span,
    )
}
