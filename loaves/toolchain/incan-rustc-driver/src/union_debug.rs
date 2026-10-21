//! The `Debug` implementation legacy writes by hand for every union carrier, injected as AST beside its enum.
//!
//! A union prints the member it holds, never a variant name: `{value:?}` of an `int | str` holding `7` is `7`. Each
//! arm therefore delegates to the held member's own `Debug`, exactly as legacy's generated impl does, where
//! `#[derive(Debug)]` would print `V0(7)`.

use crate::ast_build::{binding, expression, global_path, ident, inherited, local, path_ty, self_value};
use crate::plan::EnumDeclaration;
use rustc_ast as ast;
use rustc_span::{Ident, Span, kw, respan};
use thin_vec::{ThinVec, thin_vec};

/// Build `impl ::core::fmt::Debug for Union { fn fmt(&self, formatter) { match self { Self::Vn(value) => ...  } } }`.
///
/// Validation has proved the declaration is a union: an ordinary enum whose every variant holds exactly one member.
pub fn member_debug_impl(value: &EnumDeclaration, span: Span) -> Box<ast::Item> {
    let arms = value
        .variants
        .iter()
        .map(|variant| member_arm(&variant.name, span))
        .collect();
    let body = expression(
        ast::ExprKind::Match(self_value(span), arms, ast::MatchKind::Prefix),
        span,
    );
    let method = Box::new(ast::AssocItem {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::AssocItemKind::Fn(Box::new(ast::Fn {
            defaultness: ast::Defaultness::Implicit,
            ident: ident("fmt", span),
            generics: ast::Generics::default(),
            sig: fmt_signature(span),
            contract: None,
            define_opaque: None,
            body: Some(Box::new(ast::Block {
                stmts: thin_vec![ast::Stmt {
                    id: ast::DUMMY_NODE_ID,
                    kind: ast::StmtKind::Expr(body),
                    span,
                }],
                id: ast::DUMMY_NODE_ID,
                rules: ast::BlockCheckMode::Default,
                span,
                tokens: None,
            })),
            eii_impls: ThinVec::new(),
        })),
        tokens: None,
    });
    let header = ast::TraitImplHeader {
        defaultness: ast::Defaultness::Implicit,
        safety: ast::Safety::Default,
        polarity: ast::ImplPolarity::Positive,
        trait_ref: ast::TraitRef {
            path: global_path(&["core", "fmt", "Debug"], span),
            ref_id: ast::DUMMY_NODE_ID,
        },
    };
    Box::new(ast::Item {
        attrs: ThinVec::new(),
        id: ast::DUMMY_NODE_ID,
        span,
        vis: inherited(span),
        kind: ast::ItemKind::Impl(ast::Impl {
            generics: ast::Generics::default(),
            constness: ast::Const::No,
            of_trait: Some(Box::new(header)),
            self_ty: path_ty(ast::Path::from_ident(ident(&value.name, span)), span),
            items: thin_vec![method],
        }),
        tokens: None,
    })
}

/// `fn fmt(&self, formatter: &mut ::core::fmt::Formatter) -> ::core::fmt::Result`, the trait method's signature.
pub(crate) fn fmt_signature(span: Span) -> ast::FnSig {
    let receiver = ast::Param::from_self(
        ThinVec::new(),
        respan(span, ast::SelfKind::Region(None, ast::Mutability::Not)),
        Ident::new(kw::SelfLower, span),
    );
    let formatter = ast::Param {
        attrs: ThinVec::new(),
        ty: Box::new(ast::Ty {
            id: ast::DUMMY_NODE_ID,
            kind: ast::TyKind::Ref(
                None,
                ast::MutTy {
                    ty: path_ty(global_path(&["core", "fmt", "Formatter"], span), span),
                    mutbl: ast::Mutability::Mut,
                },
            ),
            span,
            tokens: None,
        }),
        pat: binding("formatter", span),
        id: ast::DUMMY_NODE_ID,
        span,
        is_placeholder: false,
    };
    ast::FnSig {
        header: ast::FnHeader::default(),
        decl: Box::new(ast::FnDecl {
            inputs: thin_vec![receiver, formatter],
            output: ast::FnRetTy::Ty(path_ty(global_path(&["core", "fmt", "Result"], span), span)),
        }),
        span,
    }
}

/// `Self::Vn(value) => ::core::fmt::Debug::fmt(value, formatter)` for one union member.
fn member_arm(variant: &str, span: Span) -> ast::Arm {
    let mut variant_path = ast::Path::from_ident(Ident::new(kw::SelfUpper, span));
    variant_path
        .segments
        .push(ast::PathSegment::from_ident(ident(variant, span)));
    let pattern = ast::Pat {
        id: ast::DUMMY_NODE_ID,
        kind: ast::PatKind::TupleStruct(None, variant_path, thin_vec![*binding("value", span)]),
        span,
        tokens: None,
    };
    let delegate = expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(None, global_path(&["core", "fmt", "Debug", "fmt"], span)),
                span,
            ),
            thin_vec![local("value", span), local("formatter", span)],
        ),
        span,
    );
    ast::Arm {
        attrs: ThinVec::new(),
        pat: Box::new(pattern),
        guard: None,
        body: Some(delegate),
        span,
        id: ast::DUMMY_NODE_ID,
        is_placeholder: false,
    }
}
