use incan_vocab::{
    DesugarError, DesugarOutput, IncanExpr, IncanScopedSurfacePayload, IncanStatement, VocabBodyItem, VocabDesugarer,
    VocabSyntaxNode,
};

use crate::{
    WORKFLOW_BIND_DESCRIPTOR, WORKFLOW_FALLBACK_DESCRIPTOR, WORKFLOW_PIPE_DESCRIPTOR, WORKFLOW_SHAPE_DESCRIPTOR,
};

/// Minimal desugarer stub for the richer surrogate surface.
///
/// The implementation is intentionally tiny. Its job is to show the shape of the desugarer contract once a DSL can
/// lower either declarations that become expressions or declarations that become statement lists.
#[derive(Default)]
pub struct StudioKitDesugarer;

impl VocabDesugarer for StudioKitDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        match node {
            VocabSyntaxNode::Declaration(declaration) if declaration.keyword == "query" => {
                Ok(DesugarOutput::Expression(IncanExpr::Name(
                    "__studiokit_query_result".to_string(),
                )))
            }
            VocabSyntaxNode::Declaration(declaration) if declaration.keyword == "workflow" => {
                let mut surfaces = Vec::new();
                for item in &declaration.body {
                    collect_surface_summaries_from_body_item(item, &mut surfaces);
                }
                let summary = if surfaces.is_empty() {
                    "studiokit workflow desugared".to_string()
                } else {
                    format!("studiokit workflow surfaces: {}", surfaces.join("; "))
                };
                Ok(DesugarOutput::Statements(vec![IncanStatement::Expr(IncanExpr::Call {
                    callee: Box::new(IncanExpr::Name("print".to_string())),
                    args: vec![IncanExpr::Str(summary)],
                })]))
            }
            VocabSyntaxNode::Declaration(_) => Ok(DesugarOutput::Statements(vec![IncanStatement::Pass])),
            _ => Err(DesugarError::new(
                "studiokit desugarer expects declaration nodes in this design sketch",
            )),
        }
    }
}

fn collect_surface_summaries_from_body_item(item: &VocabBodyItem, surfaces: &mut Vec<String>) {
    match item {
        VocabBodyItem::Statement(IncanStatement::Expr(expr)) => surfaces.push(format_expr(expr)),
        VocabBodyItem::Clause(clause) => {
            for expr in &clause.head {
                surfaces.push(format_expr(expr));
            }
        }
        VocabBodyItem::Declaration(declaration) => {
            for nested in &declaration.body {
                collect_surface_summaries_from_body_item(nested, surfaces);
            }
        }
        _ => {}
    }
}

fn format_expr(expr: &IncanExpr) -> String {
    match expr {
        IncanExpr::Name(name) => name.clone(),
        IncanExpr::Str(value) => format!("{value:?}"),
        IncanExpr::Field { object, field } => format!("{}.{}", format_expr(object), field),
        IncanExpr::ScopedSurface(surface)
            if matches!(
                surface.descriptor_key.as_str(),
                WORKFLOW_PIPE_DESCRIPTOR
                    | WORKFLOW_FALLBACK_DESCRIPTOR
                    | WORKFLOW_BIND_DESCRIPTOR
                    | WORKFLOW_SHAPE_DESCRIPTOR
            ) =>
        {
            if let IncanScopedSurfacePayload::ScopedGlyph { glyph, left, right, .. } = &surface.payload {
                format!("{} {glyph} {}", format_expr(left), format_expr(right))
            } else {
                "<workflow-surface>".to_string()
            }
        }
        IncanExpr::ScopedSurface(surface) => match &surface.payload {
            IncanScopedSurfacePayload::LeadingDotPath { segments, .. } => format!(".{}", segments.join(".")),
            IncanScopedSurfacePayload::ScopedGlyph { glyph, left, right, .. } => {
                format!("{} {glyph} {}", format_expr(left), format_expr(right))
            }
            _ => "<scoped-surface>".to_string(),
        },
        _ => "<expr>".to_string(),
    }
}
