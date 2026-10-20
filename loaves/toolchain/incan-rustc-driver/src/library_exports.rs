//! The `pub use` items a library unit publishes its re-exported names with.
//!
//! The Incan lowering resolves each name to a planned item of this crate (`crate::item`) or a Rust item (`::path`);
//! this module only spells those resolutions as items at the crate root.

use rustc_ast as ast;
use rustc_span::{Ident, Span, kw};
use thin_vec::ThinVec;

use crate::ast_build::ident;
use crate::plan::Plan;

/// One `pub use <path> as <name>;` item per plan export, in plan order; an executable plan has none.
pub(crate) fn items(plan: &Plan, span: Span) -> Vec<Box<ast::Item>> {
    plan.exports
        .iter()
        .map(|export| {
            let tree = ast::UseTree {
                prefix: export_path(&export.path, span),
                kind: ast::UseTreeKind::Simple(Some(ident(&export.name, span))),
            };
            crate::declarations::item(ast::ItemKind::Use(tree), span)
        })
        .collect()
}

/// Spell `crate::a::b` or `::a::b` as a path; the lowering emits only these two forms.
fn export_path(path: &str, span: Span) -> ast::Path {
    let mut segments = ThinVec::new();
    let rest = if let Some(rest) = path.strip_prefix("::") {
        segments.push(ast::PathSegment::path_root(span));
        rest
    } else if let Some(rest) = path.strip_prefix("crate::") {
        segments.push(ast::PathSegment::from_ident(Ident::new(kw::Crate, span)));
        rest
    } else {
        path
    };
    for segment in rest.split("::") {
        segments.push(ast::PathSegment::from_ident(ident(segment, span)));
    }
    ast::Path {
        span,
        segments,
        tokens: None,
    }
}
