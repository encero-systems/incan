use incan_vocab::{
    DesugarError, DesugarOutput, IncanExpr, IncanScopedSurfacePayload, IncanStatement, VocabBodyItem, VocabDesugarer,
    VocabSyntaxNode,
};

use crate::{ROUTE_MAP_DESCRIPTOR, ROUTE_VERB_DESCRIPTOR};

/// Minimal Rust desugarer for the pro-level `routekit` example.
///
/// The implementation intentionally stays tiny: it turns each `route ...:` block into a visible `print(...)` call.
/// The example proves that library-defined syntax can carry behavior.
#[derive(Default)]
pub struct RoutekitDesugarer;

impl VocabDesugarer for RoutekitDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let keyword = match node {
            VocabSyntaxNode::Declaration(decl) => &decl.keyword,
            _ => return Err(DesugarError::new("routekit desugarer expected a declaration node")),
        };

        let mut verbs = Vec::new();
        let mut target = None;
        if let VocabSyntaxNode::Declaration(declaration) = node {
            for item in &declaration.body {
                collect_verbs_from_body_item(item, &mut verbs);
                collect_route_target_from_body_item(item, &mut target);
            }
        }
        verbs.sort();
        verbs.dedup();
        let summary = if verbs.is_empty() {
            format!("{keyword} block desugared")
        } else if let Some(target) = target {
            format!("{keyword} block desugared for {} -> {target}", verbs.join(" + "))
        } else {
            format!("{keyword} block desugared for {}", verbs.join(" + "))
        };

        Ok(DesugarOutput::Statements(vec![IncanStatement::Expr(IncanExpr::Call {
            callee: Box::new(IncanExpr::Name("print".to_string())),
            args: vec![IncanExpr::Str(summary)],
        })]))
    }
}

fn collect_route_target_from_body_item(item: &VocabBodyItem, target: &mut Option<String>) {
    match item {
        VocabBodyItem::Statement(IncanStatement::Expr(expr)) => collect_route_target_from_expr(expr, target),
        VocabBodyItem::Clause(clause) => {
            for expr in &clause.head {
                collect_route_target_from_expr(expr, target);
            }
        }
        VocabBodyItem::Declaration(declaration) => {
            for nested in &declaration.body {
                collect_route_target_from_body_item(nested, target);
            }
        }
        _ => {}
    }
}

fn collect_verbs_from_body_item(item: &VocabBodyItem, verbs: &mut Vec<String>) {
    match item {
        VocabBodyItem::Statement(IncanStatement::Expr(expr)) => collect_verbs_from_expr(expr, verbs),
        VocabBodyItem::Clause(clause) => {
            for expr in &clause.head {
                collect_verbs_from_expr(expr, verbs);
            }
        }
        VocabBodyItem::Declaration(declaration) => {
            for nested in &declaration.body {
                collect_verbs_from_body_item(nested, verbs);
            }
        }
        _ => {}
    }
}

fn collect_verbs_from_expr(expr: &IncanExpr, verbs: &mut Vec<String>) {
    match expr {
        IncanExpr::ScopedSurface(surface) if surface.descriptor_key == ROUTE_VERB_DESCRIPTOR => {
            if let IncanScopedSurfacePayload::ScopedGlyph { left, right, .. } = &surface.payload {
                collect_verbs_from_expr(left, verbs);
                collect_verbs_from_expr(right, verbs);
            }
        }
        IncanExpr::ScopedSurface(surface) if surface.descriptor_key == ROUTE_MAP_DESCRIPTOR => {
            if let IncanScopedSurfacePayload::ScopedGlyph { left, .. } = &surface.payload {
                collect_verbs_from_expr(left, verbs);
            }
        }
        IncanExpr::Name(name) => verbs.push(name.clone()),
        IncanExpr::ScopedSurface(_) => {}
        _ => {}
    }
}

fn collect_route_target_from_expr(expr: &IncanExpr, target: &mut Option<String>) {
    match expr {
        IncanExpr::ScopedSurface(surface) if surface.descriptor_key == ROUTE_MAP_DESCRIPTOR => {
            if let IncanScopedSurfacePayload::ScopedGlyph { right, .. } = &surface.payload {
                *target = Some(format_expr(right));
            }
        }
        IncanExpr::ScopedSurface(surface) => {
            if let IncanScopedSurfacePayload::ScopedGlyph { left, right, .. } = &surface.payload {
                collect_route_target_from_expr(left, target);
                collect_route_target_from_expr(right, target);
            }
        }
        _ => {}
    }
}

fn format_expr(expr: &IncanExpr) -> String {
    match expr {
        IncanExpr::Name(name) => name.clone(),
        IncanExpr::Field { object, field } => format!("{}.{}", format_expr(object), field),
        IncanExpr::RelationField { relation, field } => format!("{relation}.{field}"),
        IncanExpr::Str(value) => format!("{value:?}"),
        IncanExpr::ScopedSurface(surface) => match &surface.payload {
            IncanScopedSurfacePayload::ScopedGlyph { glyph, left, right, .. } => {
                format!("{} {glyph} {}", format_expr(left), format_expr(right))
            }
            IncanScopedSurfacePayload::LeadingDotPath { segments, .. } => format!(".{}", segments.join(".")),
            _ => "<scoped-surface>".to_string(),
        },
        _ => "<expr>".to_string(),
    }
}
