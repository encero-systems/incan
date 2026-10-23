//! Small AST constructors shared by the trait implementations the driver injects beside planned declarations.
//!
//! Each builds one node without attributes or source tokens, so an injected impl is spelled structurally and never
//! parsed from generated text.

use rustc_ast as ast;
use rustc_span::{Ident, Span, Symbol, kw};
use thin_vec::{ThinVec, thin_vec};

/// The receiver `self` as an expression.
pub(crate) fn self_value(span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Path(None, ast::Path::from_ident(Ident::new(kw::SelfLower, span))),
        span,
    )
}

/// A read of one local binding.
pub(crate) fn local(name: &str, span: Span) -> Box<ast::Expr> {
    expression(
        ast::ExprKind::Path(None, ast::Path::from_ident(ident(name, span))),
        span,
    )
}

/// A by-value binding pattern for one name.
pub(crate) fn binding(name: &str, span: Span) -> Box<ast::Pat> {
    Box::new(ast::Pat {
        id: ast::DUMMY_NODE_ID,
        kind: ast::PatKind::Ident(ast::BindingMode::NONE, ident(name, span), None),
        span,
        tokens: None,
    })
}

/// A path rooted at the crate graph (`::core::...`), so no item of the generated crate can shadow it.
pub(crate) fn global_path(segments: &[&str], span: Span) -> ast::Path {
    let mut path = ast::Path {
        span,
        segments: thin_vec![ast::PathSegment::path_root(span)],
        tokens: None,
    };
    for segment in segments {
        path.segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    path
}

/// A type named by a path.
pub(crate) fn path_ty(path: ast::Path, span: Span) -> Box<ast::Ty> {
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::Path(None, path),
        span,
        tokens: None,
    })
}

/// A function pointer with already-translated inputs and result, shared by closed and generic field layouts.
pub(crate) fn function_pointer(inputs: ThinVec<Box<ast::Ty>>, result: Box<ast::Ty>, span: Span) -> Box<ast::Ty> {
    let inputs = inputs
        .into_iter()
        .map(|ty| ast::Param {
            attrs: ThinVec::new(),
            ty,
            pat: Box::new(ast::Pat {
                id: ast::DUMMY_NODE_ID,
                kind: ast::PatKind::Wild,
                span,
                tokens: None,
            }),
            id: ast::DUMMY_NODE_ID,
            span,
            is_placeholder: false,
        })
        .collect();
    Box::new(ast::Ty {
        id: ast::DUMMY_NODE_ID,
        kind: ast::TyKind::FnPtr(Box::new(ast::FnPtrTy {
            safety: ast::Safety::Default,
            ext: ast::Extern::None,
            generic_params: ThinVec::new(),
            decl: Box::new(ast::FnDecl {
                inputs,
                output: ast::FnRetTy::Ty(result),
            }),
            decl_span: span,
        })),
        span,
        tokens: None,
    })
}

/// A trait bound on a generic parameter, without higher-ranked binders or modifiers.
pub(crate) fn trait_bound(path: ast::Path, span: Span) -> ast::GenericBound {
    ast::GenericBound::Trait(ast::PolyTraitRef::new(
        ThinVec::new(),
        path,
        ast::TraitBoundModifiers::NONE,
        span,
        ast::Parens::No,
    ))
}

/// An expression node without attributes or source tokens.
pub(crate) fn expression(kind: ast::ExprKind, span: Span) -> Box<ast::Expr> {
    Box::new(ast::Expr {
        id: ast::DUMMY_NODE_ID,
        kind,
        span,
        attrs: ThinVec::new(),
        tokens: None,
    })
}

/// A trailing expression statement, the value of its block.
pub(crate) fn expression_statement(value: Box<ast::Expr>, span: Span) -> ast::Stmt {
    ast::Stmt {
        id: ast::DUMMY_NODE_ID,
        kind: ast::StmtKind::Expr(value),
        span,
    }
}

/// A block of statements.
pub(crate) fn block(stmts: ThinVec<ast::Stmt>, span: Span) -> Box<ast::Block> {
    Box::new(ast::Block {
        stmts,
        id: ast::DUMMY_NODE_ID,
        rules: ast::BlockCheckMode::Default,
        span,
        tokens: None,
    })
}

/// The inherited visibility a trait implementation and its items must carry.
pub(crate) fn inherited(span: Span) -> ast::Visibility {
    ast::Visibility {
        kind: ast::VisibilityKind::Inherited,
        span,
        tokens: None,
    }
}

/// An identifier for a name the plan admitted.
pub(crate) fn ident(name: &str, span: Span) -> Ident {
    Ident::new(Symbol::intern(name), span)
}
