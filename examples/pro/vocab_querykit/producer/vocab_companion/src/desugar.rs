use incan_vocab::{
    DesugarError, DesugarOutput, IncanExpr, IncanScopedSurfacePayload, IncanStatement, VocabBodyItem, VocabDesugarer,
    VocabSyntaxNode,
};

use crate::{QUERY_FIELD_DESCRIPTOR, QUERY_METHOD_FIELD_DESCRIPTOR, QUERY_PIPE_DESCRIPTOR};

/// Minimal Rust desugarer for the pro-level synthetic querykit scoped surface example.
///
/// The desugarer deliberately inspects scoped surface artifacts instead of re-parsing source text. It turns a
/// `query:` block into a visible `print(...)` call that reports the leading-dot fields it received.
#[derive(Default)]
pub struct QuerykitDesugarer;

impl VocabDesugarer for QuerykitDesugarer {
    fn desugar(&self, node: &VocabSyntaxNode) -> Result<DesugarOutput, DesugarError> {
        let mut fields = Vec::new();
        let mut pipes = Vec::new();
        match node {
            VocabSyntaxNode::Declaration(declaration) if declaration.keyword == "query" => {
                for item in &declaration.body {
                    collect_fields_from_body_item(item, &mut fields);
                    collect_pipes_from_body_item(item, &mut pipes);
                }
            }
            VocabSyntaxNode::Declaration(_) => {
                return Err(DesugarError::new("querykit desugarer expected a query declaration"));
            }
            _ => return Err(DesugarError::new("querykit desugarer expected a declaration node")),
        }

        fields.sort();
        fields.dedup();
        let summary = if fields.is_empty() {
            "querykit query fields: <none>".to_string()
        } else {
            format!("querykit query fields: {}", fields.join(", "))
        };
        let summary = if pipes.is_empty() {
            summary
        } else {
            format!("{summary}; pipes: {}", pipes.join(", "))
        };

        Ok(DesugarOutput::Statements(vec![IncanStatement::Expr(IncanExpr::Call {
            callee: Box::new(IncanExpr::Name("print".to_string())),
            args: vec![IncanExpr::Str(summary)],
        })]))
    }
}

fn collect_pipes_from_body_item(item: &VocabBodyItem, pipes: &mut Vec<String>) {
    match item {
        VocabBodyItem::Statement(statement) => collect_pipes_from_statement(statement, pipes),
        VocabBodyItem::Clause(clause) => {
            for expr in &clause.head {
                collect_pipes_from_expr(expr, pipes);
            }
        }
        VocabBodyItem::Declaration(declaration) => {
            for nested in &declaration.body {
                collect_pipes_from_body_item(nested, pipes);
            }
        }
        _ => {}
    }
}

fn collect_fields_from_body_item(item: &VocabBodyItem, fields: &mut Vec<String>) {
    match item {
        VocabBodyItem::Statement(statement) => collect_fields_from_statement(statement, fields),
        VocabBodyItem::Clause(clause) => {
            for expr in &clause.head {
                collect_fields_from_expr(expr, fields);
            }
        }
        VocabBodyItem::Declaration(declaration) => {
            for nested in &declaration.body {
                collect_fields_from_body_item(nested, fields);
            }
        }
        _ => {}
    }
}

fn collect_pipes_from_statement(statement: &IncanStatement, pipes: &mut Vec<String>) {
    match statement {
        IncanStatement::Expr(expr) | IncanStatement::Return(Some(expr)) => collect_pipes_from_expr(expr, pipes),
        IncanStatement::Assign { value, .. } | IncanStatement::Let { value, .. } => {
            collect_pipes_from_expr(value, pipes);
        }
        IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_pipes_from_expr(condition, pipes);
            for nested in then_body {
                collect_pipes_from_statement(nested, pipes);
            }
            for nested in else_body {
                collect_pipes_from_statement(nested, pipes);
            }
        }
        IncanStatement::While { condition, body } => {
            collect_pipes_from_expr(condition, pipes);
            for nested in body {
                collect_pipes_from_statement(nested, pipes);
            }
        }
        IncanStatement::For { iter, body, .. } => {
            collect_pipes_from_expr(iter, pipes);
            for nested in body {
                collect_pipes_from_statement(nested, pipes);
            }
        }
        IncanStatement::Pass | IncanStatement::Return(None) => {}
        _ => {}
    }
}

fn collect_fields_from_statement(statement: &IncanStatement, fields: &mut Vec<String>) {
    match statement {
        IncanStatement::Expr(expr) | IncanStatement::Return(Some(expr)) => collect_fields_from_expr(expr, fields),
        IncanStatement::Assign { value, .. } | IncanStatement::Let { value, .. } => {
            collect_fields_from_expr(value, fields);
        }
        IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => {
            collect_fields_from_expr(condition, fields);
            for nested in then_body {
                collect_fields_from_statement(nested, fields);
            }
            for nested in else_body {
                collect_fields_from_statement(nested, fields);
            }
        }
        IncanStatement::While { condition, body } => {
            collect_fields_from_expr(condition, fields);
            for nested in body {
                collect_fields_from_statement(nested, fields);
            }
        }
        IncanStatement::For { iter, body, .. } => {
            collect_fields_from_expr(iter, fields);
            for nested in body {
                collect_fields_from_statement(nested, fields);
            }
        }
        IncanStatement::Pass | IncanStatement::Return(None) => {}
        _ => {}
    }
}

fn collect_pipes_from_expr(expr: &IncanExpr, pipes: &mut Vec<String>) {
    match expr {
        IncanExpr::ScopedSurface(surface) if surface.descriptor_key == QUERY_PIPE_DESCRIPTOR => {
            if let IncanScopedSurfacePayload::ScopedGlyph { left, right, .. } = &surface.payload {
                pipes.push(format!("{} |> {}", format_expr(left), format_expr(right)));
                collect_pipes_from_expr(left, pipes);
                collect_pipes_from_expr(right, pipes);
            }
        }
        IncanExpr::Binary(left, _, right) => {
            collect_pipes_from_expr(left, pipes);
            collect_pipes_from_expr(right, pipes);
        }
        IncanExpr::Unary(_, inner) | IncanExpr::Field { object: inner, .. } => {
            collect_pipes_from_expr(inner, pipes);
        }
        IncanExpr::Call { callee, args } => {
            collect_pipes_from_expr(callee, pipes);
            for arg in args {
                collect_pipes_from_expr(arg, pipes);
            }
        }
        IncanExpr::List(items) | IncanExpr::Tuple(items) => {
            for item in items {
                collect_pipes_from_expr(item, pipes);
            }
        }
        IncanExpr::Dict(entries) => {
            for (key, value) in entries {
                collect_pipes_from_expr(key, pipes);
                collect_pipes_from_expr(value, pipes);
            }
        }
        IncanExpr::ScopedSurface(_) => {}
        _ => {}
    }
}

fn collect_fields_from_expr(expr: &IncanExpr, fields: &mut Vec<String>) {
    match expr {
        IncanExpr::ScopedSurface(surface)
            if surface.descriptor_key == QUERY_FIELD_DESCRIPTOR
                || surface.descriptor_key == QUERY_METHOD_FIELD_DESCRIPTOR =>
        {
            if let IncanScopedSurfacePayload::LeadingDotPath { segments, .. } = &surface.payload {
                fields.push(format!(".{}", segments.join(".")));
            }
        }
        IncanExpr::Binary(left, _, right) => {
            collect_fields_from_expr(left, fields);
            collect_fields_from_expr(right, fields);
        }
        IncanExpr::Unary(_, inner) | IncanExpr::Field { object: inner, .. } => {
            collect_fields_from_expr(inner, fields);
        }
        IncanExpr::Call { callee, args } => {
            collect_fields_from_expr(callee, fields);
            for arg in args {
                collect_fields_from_expr(arg, fields);
            }
        }
        IncanExpr::List(items) | IncanExpr::Tuple(items) => {
            for item in items {
                collect_fields_from_expr(item, fields);
            }
        }
        IncanExpr::Dict(entries) => {
            for (key, value) in entries {
                collect_fields_from_expr(key, fields);
                collect_fields_from_expr(value, fields);
            }
        }
        IncanExpr::ScopedSurface(_) => {}
        IncanExpr::Name(_)
        | IncanExpr::Helper(_)
        | IncanExpr::Str(_)
        | IncanExpr::Int(_)
        | IncanExpr::Bool(_)
        | IncanExpr::CurrentField(_)
        | IncanExpr::RelationField { .. } => {}
        _ => {}
    }
}

fn format_expr(expr: &IncanExpr) -> String {
    match expr {
        IncanExpr::Name(name) => name.clone(),
        IncanExpr::Str(value) => format!("{value:?}"),
        IncanExpr::CurrentField(field) => format!(".{field}"),
        IncanExpr::RelationField { relation, field } => format!("{relation}.{field}"),
        IncanExpr::Field { object, field } => format!("{}.{}", format_expr(object), field),
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
