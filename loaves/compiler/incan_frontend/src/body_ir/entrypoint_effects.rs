//! Retain compiler-owned entrypoint setup without changing the frozen legacy import scanner.

use incan_lang::lang::stdlib::STDLIB_THIS;

use super::{ast, bir, hir_span};

/// The same source asset used by legacy entrypoint emission; native source publication retains its include geometry.
const ZEN_TEXT: &str = include_str!("../../../../stdlib/zen.txt");

/// Record one output effect for the exact module import legacy recognizes, including an aliased import.
///
/// The module retains its own effect. Project assembly keeps only the entry module's effects, so a dependency's
/// import cannot create output in another module's entrypoint. Duplicate imports retain one effect, as legacy's
/// import flag does; ordinary from-imports and longer module paths do not qualify.
pub(super) fn collect(program: &ast::Program) -> Vec<bir::EntrypointEffect> {
    program
        .declarations
        .iter()
        .find_map(|declaration| {
            let ast::Declaration::Import(import) = &declaration.node else {
                return None;
            };
            let ast::ImportKind::Module(path) = &import.kind else {
                return None;
            };
            (path.segments.as_slice() == [STDLIB_THIS]).then(|| bir::EntrypointEffect::PrintText {
                text: ZEN_TEXT.to_string(),
                span: hir_span(declaration.span),
            })
        })
        .into_iter()
        .collect()
}
