//! The callables a module publishes: each `pub` function, partial or method, which another module, package or Rust
//! caller can name. Their parameter passing is part of the published signature, so shared-read settlement decides it
//! from declared rules only and never from the body.

use std::collections::HashSet;

use super::{CanonicalSymbolId, HirSourceSpan, ast, bir, hir_span};

/// Select the lowered bodies whose source declarations are written `pub`. A method is published only when the method
/// itself is `pub`: an unmarked method is private to its type, whatever the type's own visibility.
pub(super) fn collect(program: &ast::Program, bodies: &[bir::Body]) -> Vec<CanonicalSymbolId> {
    let mut spans = HashSet::new();
    for declaration in &program.declarations {
        match &declaration.node {
            ast::Declaration::Function(function) if function.visibility == ast::Visibility::Public => {
                spans.insert(hir_span(declaration.span));
            }
            ast::Declaration::Partial(partial) if partial.visibility == ast::Visibility::Public => {
                spans.insert(hir_span(declaration.span));
            }
            ast::Declaration::Model(model) => published_methods(&model.methods, &mut spans),
            ast::Declaration::Class(class) => published_methods(&class.methods, &mut spans),
            ast::Declaration::Trait(trait_decl) => published_methods(&trait_decl.methods, &mut spans),
            ast::Declaration::Newtype(newtype) => published_methods(&newtype.methods, &mut spans),
            ast::Declaration::Enum(enum_decl) => published_methods(&enum_decl.methods, &mut spans),
            _ => {}
        }
    }
    bodies
        .iter()
        .filter(|body| spans.contains(&body.span))
        .filter_map(|body| body.canonical.clone())
        .collect()
}

/// Record each `pub` method's declaration span, which is the span its lowered body carries.
fn published_methods(methods: &[ast::Spanned<ast::MethodDecl>], spans: &mut HashSet<HirSourceSpan>) {
    for method in methods {
        if method.node.visibility == ast::Visibility::Public {
            spans.insert(hir_span(method.span));
        }
    }
}
