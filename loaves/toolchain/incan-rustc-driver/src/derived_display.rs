//! Publish derived Display beside its owning layout, delegating to that layout's Debug implementation.

use crate::ast_build::{
    RUST_FMT_DEBUG_SYMBOL, RUST_FMT_DISPLAY_SYMBOL, block, expression, expression_statement, global_path, inherited,
    local, self_value,
};
use rustc_ast as ast;
use rustc_span::Span;
use thin_vec::thin_vec;

/// Implement the checked structural Display contract on the actual ADT, so its rlib also serves borrowed consumers.
/// Generic declarations supply the same field-dependent Debug bounds as their Debug implementation.
pub(crate) fn implementation(self_ty: Box<ast::Ty>, generics: ast::Generics, span: Span) -> Box<ast::Item> {
    let delegate = expression(
        ast::ExprKind::Call(
            expression(
                ast::ExprKind::Path(None, global_path(&["core", "fmt", RUST_FMT_DEBUG_SYMBOL, "fmt"], span)),
                span,
            ),
            thin_vec![self_value(span), local("formatter", span)],
        ),
        span,
    );
    let method = crate::generic_layouts::trait_method(
        "fmt",
        crate::union_debug::fmt_signature(span),
        block(thin_vec![expression_statement(delegate, span)], span),
        span,
    );
    let mut item = crate::declarations::item(
        ast::ItemKind::Impl(ast::Impl {
            generics,
            constness: ast::Const::No,
            of_trait: Some(Box::new(ast::TraitImplHeader {
                defaultness: ast::Defaultness::Implicit,
                safety: ast::Safety::Default,
                polarity: ast::ImplPolarity::Positive,
                trait_ref: ast::TraitRef {
                    path: global_path(&["core", "fmt", RUST_FMT_DISPLAY_SYMBOL], span),
                    ref_id: ast::DUMMY_NODE_ID,
                },
            })),
            self_ty,
            items: thin_vec![method],
        }),
        span,
    );
    item.vis = inherited(span);
    item
}
