use incan_vocab::{DesugarError, DesugarOutput, IncanExpr, VocabDesugarer, VocabSyntaxNode};

/// Minimal desugarer stub for the richer surrogate surface.
///
/// The implementation is intentionally tiny. Its job is to show the shape of the desugarer
/// contract once a DSL can lower either declarations that become expressions or declarations that
/// become statement lists.
pub struct StudioKitDesugarer;

impl VocabDesugarer for StudioKitDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        match node {
            VocabSyntaxNode::Declaration(declaration) if declaration.keyword == "query" => {
                Ok(DesugarOutput::Expression(IncanExpr::Name(
                    "__studiokit_query_result".to_string(),
                )))
            }
            VocabSyntaxNode::Declaration(_) => Ok(DesugarOutput::Statements(Vec::new())),
            _ => Err(DesugarError::new(
                "studiokit desugarer expects declaration nodes in this design sketch",
            )),
        }
    }
}
