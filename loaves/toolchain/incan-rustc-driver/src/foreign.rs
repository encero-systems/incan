//! Inject target-verified foreign declarations structurally; executable bridge behavior remains in Incan.

use std::collections::BTreeMap;

use crate::ast_build::{binding, ident};
use crate::declarations::{item, ty};
use crate::error::PlanError;
use crate::plan::{
    AttributeArgument, AttributeValue, CalleeKind, ForeignFunction, NativeAttribute, Plan, TerminatorKind,
};
use rustc_ast as ast;
use rustc_hir::def_id::DefId;
use rustc_middle::ty::TyCtxt;
use rustc_span::{Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Declare each selected foreign symbol once, refusing conflicting signatures under one retained declaration key.
pub(crate) fn items(
    plan: &Plan,
    generator: &ast::attr::AttrIdGenerator,
    span: Span,
) -> Result<Vec<Box<ast::Item>>, PlanError> {
    let mut selected = BTreeMap::new();
    for function in &plan.functions {
        for block in &function.blocks {
            let TerminatorKind::Call(callee, ..) = &block.terminator.kind else {
                continue;
            };
            let CalleeKind::Foreign(declaration) = &callee.kind else {
                continue;
            };
            if let Some(previous) = selected.insert(&declaration.name, declaration)
                && previous != declaration
            {
                return Err(PlanError::Invalid {
                    function: function.name.clone(),
                    reason: "conflicting verified foreign declaration".into(),
                });
            }
        }
    }
    Ok(selected
        .values()
        .map(|declaration| foreign_module(declaration, generator, span))
        .collect())
}

/// Resolve only an injected foreign item with the exact plan declaration key, without Rust crate-path guessing.
pub(crate) fn resolve(tcx: TyCtxt<'_>, name: &str) -> Result<DefId, PlanError> {
    tcx.hir_crate_items(())
        .foreign_items()
        .map(|item| item.owner_id.to_def_id())
        .find(|def| tcx.opt_item_name(*def).is_some_and(|symbol| symbol.as_str() == name))
        .ok_or_else(|| PlanError::UnknownCallee(name.into()))
}

/// Retain the physical symbol and explicit system library as Rust link attributes on an ordinary C declaration.
fn foreign_module(declaration: &ForeignFunction, generator: &ast::attr::AttrIdGenerator, span: Span) -> Box<ast::Item> {
    let parameters = declaration
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| ast::Param {
            attrs: ThinVec::new(),
            ty: ty(parameter, span),
            pat: binding(&format!("argument_{index}"), span),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    let function = Box::new(ast::Item {
        attrs: thin_vec![ast::attr::mk_attr_name_value_str(
            generator,
            ast::AttrStyle::Outer,
            ast::Safety::Default,
            Symbol::intern("link_name"),
            Symbol::intern(&declaration.symbol),
            span
        )],
        id: ast::DUMMY_NODE_ID,
        span,
        vis: ast::Visibility {
            kind: ast::VisibilityKind::Public,
            span,
            tokens: None,
        },
        kind: ast::ForeignItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&declaration.name, span),
            generics: ast::Generics::default(),
            sig: ast::FnSig {
                header: ast::FnHeader::default(),
                decl: Box::new(ast::FnDecl {
                    inputs: parameters,
                    output: ast::FnRetTy::Ty(ty(&declaration.return_type, span)),
                }),
                span,
            },
            contract: None,
            define_opaque: None,
            body: None,
            eii_impls: ThinVec::new(),
        })),
        tokens: None,
    });
    let mut module = item(
        ast::ItemKind::ForeignMod(ast::ForeignMod {
            extern_span: span,
            safety: ast::Safety::Unsafe(span),
            abi: Some(ast::StrLit {
                symbol: Symbol::intern("C"),
                suffix: None,
                symbol_unescaped: Symbol::intern("C"),
                style: ast::StrStyle::Cooked,
                span,
            }),
            items: thin_vec![function],
        }),
        span,
    );
    module.vis.kind = ast::VisibilityKind::Inherited;
    module.attrs.push(crate::native_attributes::attribute_item(
        generator,
        &NativeAttribute {
            path: "link".into(),
            arguments: vec![AttributeArgument {
                name: "name".into(),
                value: AttributeValue::Text(declaration.library.clone()),
            }],
        },
        span,
    ));
    module
}
