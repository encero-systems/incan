use incan_vocab::{DesugarError, DesugarOutput, IncanExpr, IncanStatement, VocabDesugarer, VocabSyntaxNode};

/// Minimal Rust desugarer for the pro-level `routekit` example.
///
/// The implementation intentionally stays tiny: it turns each `route ...:` block into a visible `print(...)` call.
/// The example proves that library-defined syntax can carry behavior.
pub struct RoutekitDesugarer;

impl VocabDesugarer for RoutekitDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let keyword = match node {
            VocabSyntaxNode::Declaration(decl) => &decl.keyword,
            _ => return Err(DesugarError::new("routekit desugarer expected a declaration node")),
        };

        Ok(DesugarOutput::Statements(vec![IncanStatement::Expr(IncanExpr::Call {
            callee: Box::new(IncanExpr::Name("print".to_string())),
            args: vec![IncanExpr::Str(format!("{keyword} block desugared"))],
        })]))
    }
}
