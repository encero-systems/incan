//! Declarations enter as AST items; only MIR supplies executable bodies.

use crate::plan::{Function, PlanType};
use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol};
use thin_vec::{ThinVec, thin_vec};

/// Create an identifier from a name that preflight validation has already admitted.
fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}

/// Construct a scalar AST type without generating or parsing Rust source.
fn ty(kind: &PlanType, span: Span) -> Box<ast::Ty> {
    let kind = match kind {
        PlanType::Unit => ast::TyKind::Tup(ThinVec::new()),
        PlanType::CheckedInt => ast::TyKind::Tup(thin_vec![ty(&PlanType::Int, span), ty(&PlanType::Bool, span)]),
        other => {
            let name = match other {
                PlanType::Int => "i64",
                PlanType::Float => "f64",
                _ => "bool",
            };
            ast::TyKind::Path(None, ast::Path::from_ident(ident(name, span)))
        }
    };
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        tokens: None,
    })
}

/// A diverging placeholder satisfies every scalar signature; `mir_built` replaces its body.
fn placeholder(span: Span) -> Box<ast::Block> {
    let empty = Box::new(ast::Block {
        stmts: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    });
    let diverge = Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind: ast::ExprKind::Loop(empty, None, span),
        span,
        attrs: ThinVec::new(),
        tokens: None,
    });
    Box::new(ast::Block {
        stmts: thin_vec![ast::Stmt {
            id: ast::DUMMY_NODE_ID,
            kind: ast::StmtKind::Expr(diverge),
            span
        }],
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    })
}

/// Inject one source-named function's declaration with no authored body in the AST.
pub fn function(function: &Function, span: Span) -> Box<ast::Item> {
    let inputs = function
        .parameters
        .iter()
        .map(|parameter| ast::Param {
            attrs: ThinVec::new(),
            ty: ty(&parameter.ty, span),
            pat: Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(&parameter.name, span), None),
                span,
                tokens: None,
            }),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    let output = ast::FnRetTy::Ty(ty(&function.return_type, span));
    let signature = ast::FnSig {
        header: ast::FnHeader::default(),
        decl: Box::new(ast::FnDecl { inputs, output }),
        span,
    };
    item(
        ast::ItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident(&function.name, span),
            generics: ast::Generics::default(),
            sig: signature,
            contract: None,
            define_opaque: None,
            body: Some(placeholder(span)),
            eii_impls: ThinVec::new(),
        })),
        span,
    )
}

/// Build an item without source tokens; rustc assigns node identities after injection.
fn item(kind: ast::ItemKind, span: Span) -> Box<ast::Item> {
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: ast::Visibility {
            kind: ast::VisibilityKind::Public,
            span,
            tokens: None,
        },
        kind,
        tokens: None,
    })
}

/// Load each admitted external crate so canonical path resolution sees its module tree.
pub fn external_crate(name: &str, span: Span) -> Box<ast::Item> {
    item(ast::ItemKind::ExternCrate(None, ident(name, span)), span)
}
