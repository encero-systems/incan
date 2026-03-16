//! Adapter layer between compiler-internal AST and public `incan_vocab` AST.
//!
//! This module is the single boundary where compiler-internal AST types are translated to/from the
//! stable public AST contract exposed by `incan_vocab`.
//!
//! Design goals:
//! - keep `incan_vocab` types from leaking throughout frontend/typechecker/lowering internals
//! - provide explicit, typed failures for shapes that are currently unsupported
//! - keep mapping rules centralized so parser/desugarer/runtime evolution stays coherent

use crate::frontend::ast;

/// Mapping failures produced by the AST bridge.
///
/// Each variant indicates:
/// - which direction failed (internal -> public or public -> internal), and
/// - whether the mismatch happened at statement or expression level.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum VocabAstBridgeError {
    /// Internal statement shape cannot be represented in current public AST.
    #[error("unsupported internal statement for vocab bridge: {0}")]
    UnsupportedInternalStatement(&'static str),
    /// Internal expression shape cannot be represented in current public AST.
    #[error("unsupported internal expression for vocab bridge: {0}")]
    UnsupportedInternalExpression(&'static str),
    /// Public statement shape cannot be represented in current internal AST bridge mapping.
    #[error("unsupported public statement for vocab bridge: {0}")]
    UnsupportedPublicStatement(&'static str),
    /// Public expression shape cannot be represented in current internal AST bridge mapping.
    #[error("unsupported public expression for vocab bridge: {0}")]
    UnsupportedPublicExpression(&'static str),
}

/// Convert one internal raw vocab block to the public `incan_vocab::VocabDeclaration` model.
///
/// This conversion:
/// - preserves keyword metadata and decorators
/// - recursively maps nested internal vocab blocks into `VocabBodyItem::Declaration`
/// - maps non-block body items through [`internal_statement_to_public`]
///
/// `span` is passed explicitly because callers decide which source span should represent the exported block boundary.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError`] when any statement/expression/decorator payload inside the block cannot be
/// represented in the public contract.
pub fn internal_vocab_block_to_public(
    block: &ast::VocabBlockStmt,
    span: ast::Span,
) -> Result<incan_vocab::VocabDeclaration, VocabAstBridgeError> {
    let mut body = Vec::new();
    for stmt in &block.body {
        match &stmt.node {
            ast::Statement::VocabBlock(nested) => body.push(incan_vocab::VocabBodyItem::Declaration(
                internal_vocab_block_to_public(nested, stmt.span)?,
            )),
            _ => body.push(incan_vocab::VocabBodyItem::Statement(internal_statement_to_public(
                &stmt.node,
            )?)),
        }
    }

    let decorators = block
        .decorators
        .iter()
        .map(public_decorator_from_internal)
        .collect::<Result<Vec<_>, _>>()?;
    let header_args = block
        .header_args
        .iter()
        .map(|arg| internal_expr_to_public(&arg.node))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(incan_vocab::VocabDeclaration {
        keyword: block.keyword.clone(),
        keyword_metadata: Some(incan_vocab::VocabKeywordMetadata {
            dependency_key: block.keyword_binding.dependency_key.clone(),
            activation_namespace: block.keyword_binding.activation_namespace.clone(),
            surface_kind: block.keyword_binding.surface_kind,
            placement: block.keyword_binding.placement.clone(),
        }),
        head: incan_vocab::VocabDeclarationHead {
            name: None,
            header_args,
            parameters: Vec::new(),
            return_type: None,
        },
        decorators,
        body,
        span: public_span(span),
    })
}

/// Convert one internal compiler statement to public `incan_vocab::IncanStatement`.
///
/// This is intentionally conservative: unsupported compiler statement forms return a typed error rather than being
/// silently dropped or lossy-transformed.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedInternalStatement`] or
/// [`VocabAstBridgeError::UnsupportedInternalExpression`] when a shape cannot be represented in the
/// current public AST.
pub fn internal_statement_to_public(stmt: &ast::Statement) -> Result<incan_vocab::IncanStatement, VocabAstBridgeError> {
    match stmt {
        ast::Statement::Pass => Ok(incan_vocab::IncanStatement::Pass),
        ast::Statement::Expr(expr) => Ok(incan_vocab::IncanStatement::Expr(internal_expr_to_public(&expr.node)?)),
        ast::Statement::Return(value) => Ok(incan_vocab::IncanStatement::Return(
            value
                .as_ref()
                .map(|expr| internal_expr_to_public(&expr.node))
                .transpose()?,
        )),
        ast::Statement::Assignment(assign) => Ok(incan_vocab::IncanStatement::Let {
            name: assign.name.clone(),
            mutable: matches!(assign.binding, ast::BindingKind::Mutable),
            value: internal_expr_to_public(&assign.value.node)?,
        }),
        ast::Statement::CompoundAssignment(assign) => Ok(incan_vocab::IncanStatement::Assign {
            target: assign.name.clone(),
            value: internal_expr_to_public(&assign.value.node)?,
        }),
        ast::Statement::If(if_stmt) => Ok(incan_vocab::IncanStatement::If {
            condition: internal_expr_to_public(&if_stmt.condition.node)?,
            then_body: internal_statements_to_public(&if_stmt.then_body)?,
            else_body: if_stmt
                .else_body
                .as_ref()
                .map(|body| internal_statements_to_public(body))
                .transpose()?
                .unwrap_or_default(),
        }),
        ast::Statement::While(while_stmt) => Ok(incan_vocab::IncanStatement::While {
            condition: internal_expr_to_public(&while_stmt.condition.node)?,
            body: internal_statements_to_public(&while_stmt.body)?,
        }),
        ast::Statement::For(for_stmt) => Ok(incan_vocab::IncanStatement::For {
            binding: for_stmt.var.clone(),
            iter: internal_expr_to_public(&for_stmt.iter.node)?,
            body: internal_statements_to_public(&for_stmt.body)?,
        }),
        ast::Statement::VocabBlock(_) => Err(VocabAstBridgeError::UnsupportedInternalStatement(
            "nested vocab blocks must be bridged through VocabBodyItem::Declaration",
        )),
        _ => Err(VocabAstBridgeError::UnsupportedInternalStatement(
            "statement form is not yet supported by public vocab AST bridge",
        )),
    }
}

/// Convert a slice of public statements into internal spanned statements.
///
/// Spans are synthesized as defaults here; the bridge preserves structure, not source provenance.
///
/// # Errors
///
/// Returns the first conversion error from [`public_statement_to_internal`].
pub fn public_statements_to_internal(
    stmts: &[incan_vocab::IncanStatement],
) -> Result<Vec<ast::Spanned<ast::Statement>>, VocabAstBridgeError> {
    stmts
        .iter()
        .map(|stmt| {
            let internal = public_statement_to_internal(stmt)?;
            Ok(ast::Spanned::new(internal, ast::Span::default()))
        })
        .collect()
}

/// Convert one public `incan_vocab::IncanStatement` into internal compiler AST.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError`] when the public statement (or any contained expression) does not
/// currently have a supported internal mapping.
pub fn public_statement_to_internal(stmt: &incan_vocab::IncanStatement) -> Result<ast::Statement, VocabAstBridgeError> {
    match stmt {
        incan_vocab::IncanStatement::Pass => Ok(ast::Statement::Pass),
        incan_vocab::IncanStatement::Expr(expr) => Ok(ast::Statement::Expr(ast::Spanned::new(
            public_expr_to_internal(expr)?,
            ast::Span::default(),
        ))),
        incan_vocab::IncanStatement::Return(value) => Ok(ast::Statement::Return(
            value
                .as_ref()
                .map(|expr| public_expr_to_internal(expr).map(|node| ast::Spanned::new(node, ast::Span::default())))
                .transpose()?,
        )),
        incan_vocab::IncanStatement::Assign { target, value } => Ok(ast::Statement::Assignment(ast::AssignmentStmt {
            binding: ast::BindingKind::Reassign,
            name: target.clone(),
            ty: None,
            value: ast::Spanned::new(public_expr_to_internal(value)?, ast::Span::default()),
        })),
        incan_vocab::IncanStatement::Let { name, mutable, value } => {
            Ok(ast::Statement::Assignment(ast::AssignmentStmt {
                binding: if *mutable {
                    ast::BindingKind::Mutable
                } else {
                    ast::BindingKind::Let
                },
                name: name.clone(),
                ty: None,
                value: ast::Spanned::new(public_expr_to_internal(value)?, ast::Span::default()),
            }))
        }
        incan_vocab::IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => Ok(ast::Statement::If(ast::IfStmt {
            condition: ast::Spanned::new(public_expr_to_internal(condition)?, ast::Span::default()),
            then_body: public_statements_to_internal(then_body)?,
            elif_branches: Vec::new(),
            else_body: if else_body.is_empty() {
                None
            } else {
                Some(public_statements_to_internal(else_body)?)
            },
        })),
        incan_vocab::IncanStatement::While { condition, body } => Ok(ast::Statement::While(ast::WhileStmt {
            condition: ast::Spanned::new(public_expr_to_internal(condition)?, ast::Span::default()),
            body: public_statements_to_internal(body)?,
        })),
        incan_vocab::IncanStatement::For { binding, iter, body } => Ok(ast::Statement::For(ast::ForStmt {
            var: binding.clone(),
            iter: ast::Spanned::new(public_expr_to_internal(iter)?, ast::Span::default()),
            body: public_statements_to_internal(body)?,
        })),
        _ => Err(VocabAstBridgeError::UnsupportedPublicStatement(
            "statement form is not yet supported by internal AST bridge",
        )),
    }
}

/// Convert a list of internal spanned statements to public statements.
///
/// This is the internal utility used for block body conversion in the internal -> public direction.
fn internal_statements_to_public(
    stmts: &[ast::Spanned<ast::Statement>],
) -> Result<Vec<incan_vocab::IncanStatement>, VocabAstBridgeError> {
    stmts
        .iter()
        .map(|stmt| internal_statement_to_public(&stmt.node))
        .collect()
}

/// Convert one internal expression to public `incan_vocab::IncanExpr`.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedInternalExpression`] for unsupported expression kinds.
fn internal_expr_to_public(expr: &ast::Expr) -> Result<incan_vocab::IncanExpr, VocabAstBridgeError> {
    match expr {
        ast::Expr::Ident(name) => Ok(incan_vocab::IncanExpr::Name(name.clone())),
        ast::Expr::Literal(ast::Literal::String(value)) => Ok(incan_vocab::IncanExpr::Str(value.clone())),
        ast::Expr::Literal(ast::Literal::Int(value)) => Ok(incan_vocab::IncanExpr::Int(*value)),
        ast::Expr::Literal(ast::Literal::Bool(value)) => Ok(incan_vocab::IncanExpr::Bool(*value)),
        ast::Expr::Tuple(values) => values
            .iter()
            .map(|value| internal_expr_to_public(&value.node))
            .collect::<Result<Vec<_>, _>>()
            .map(incan_vocab::IncanExpr::Tuple),
        ast::Expr::List(values) => values
            .iter()
            .map(|value| internal_expr_to_public(&value.node))
            .collect::<Result<Vec<_>, _>>()
            .map(incan_vocab::IncanExpr::List),
        ast::Expr::Dict(entries) => {
            let mut mapped = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                mapped.push((
                    internal_expr_to_public(&key.node)?,
                    internal_expr_to_public(&value.node)?,
                ));
            }
            Ok(incan_vocab::IncanExpr::Dict(mapped))
        }
        ast::Expr::Unary(op, value) => Ok(incan_vocab::IncanExpr::Unary(
            match op {
                ast::UnaryOp::Neg => incan_vocab::IncanUnaryOp::Neg,
                ast::UnaryOp::Not => incan_vocab::IncanUnaryOp::Not,
            },
            Box::new(internal_expr_to_public(&value.node)?),
        )),
        ast::Expr::Binary(left, op, right) => Ok(incan_vocab::IncanExpr::Binary(
            Box::new(internal_expr_to_public(&left.node)?),
            map_internal_binary_op(*op)?,
            Box::new(internal_expr_to_public(&right.node)?),
        )),
        ast::Expr::Call(callee, args) => {
            let mut mapped_args = Vec::new();
            for arg in args {
                let value = match arg {
                    ast::CallArg::Positional(expr) | ast::CallArg::Named(_, expr) => expr,
                };
                mapped_args.push(internal_expr_to_public(&value.node)?);
            }
            Ok(incan_vocab::IncanExpr::Call {
                callee: Box::new(internal_expr_to_public(&callee.node)?),
                args: mapped_args,
            })
        }
        ast::Expr::Field(object, field) => Ok(incan_vocab::IncanExpr::Field {
            object: Box::new(internal_expr_to_public(&object.node)?),
            field: field.clone(),
        }),
        _ => Err(VocabAstBridgeError::UnsupportedInternalExpression(
            "expression form is not yet supported by public vocab AST bridge",
        )),
    }
}

/// Convert one public `incan_vocab::IncanExpr` to internal compiler expression AST.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedPublicExpression`] for unsupported public expression kinds or operators.
fn public_expr_to_internal(expr: &incan_vocab::IncanExpr) -> Result<ast::Expr, VocabAstBridgeError> {
    match expr {
        incan_vocab::IncanExpr::Name(name) => Ok(ast::Expr::Ident(name.clone())),
        incan_vocab::IncanExpr::Str(value) => Ok(ast::Expr::Literal(ast::Literal::String(value.clone()))),
        incan_vocab::IncanExpr::Int(value) => Ok(ast::Expr::Literal(ast::Literal::Int(*value))),
        incan_vocab::IncanExpr::Bool(value) => Ok(ast::Expr::Literal(ast::Literal::Bool(*value))),
        incan_vocab::IncanExpr::Tuple(values) => values
            .iter()
            .map(|value| public_expr_to_internal(value).map(|node| ast::Spanned::new(node, ast::Span::default())))
            .collect::<Result<Vec<_>, _>>()
            .map(ast::Expr::Tuple),
        incan_vocab::IncanExpr::List(values) => values
            .iter()
            .map(|value| public_expr_to_internal(value).map(|node| ast::Spanned::new(node, ast::Span::default())))
            .collect::<Result<Vec<_>, _>>()
            .map(ast::Expr::List),
        incan_vocab::IncanExpr::Dict(entries) => {
            let mut mapped = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                mapped.push((
                    ast::Spanned::new(public_expr_to_internal(key)?, ast::Span::default()),
                    ast::Spanned::new(public_expr_to_internal(value)?, ast::Span::default()),
                ));
            }
            Ok(ast::Expr::Dict(mapped))
        }
        incan_vocab::IncanExpr::Unary(op, value) => Ok(ast::Expr::Unary(
            match op {
                incan_vocab::IncanUnaryOp::Neg => ast::UnaryOp::Neg,
                incan_vocab::IncanUnaryOp::Not => ast::UnaryOp::Not,
                _ => {
                    return Err(VocabAstBridgeError::UnsupportedPublicExpression(
                        "unary operator is not currently bridgeable",
                    ));
                }
            },
            Box::new(ast::Spanned::new(public_expr_to_internal(value)?, ast::Span::default())),
        )),
        incan_vocab::IncanExpr::Binary(left, op, right) => Ok(ast::Expr::Binary(
            Box::new(ast::Spanned::new(public_expr_to_internal(left)?, ast::Span::default())),
            map_public_binary_op(*op)?,
            Box::new(ast::Spanned::new(public_expr_to_internal(right)?, ast::Span::default())),
        )),
        incan_vocab::IncanExpr::Call { callee, args } => {
            let mapped = args
                .iter()
                .map(|arg| {
                    public_expr_to_internal(arg)
                        .map(|node| ast::CallArg::Positional(ast::Spanned::new(node, ast::Span::default())))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ast::Expr::Call(
                Box::new(ast::Spanned::new(
                    public_expr_to_internal(callee)?,
                    ast::Span::default(),
                )),
                mapped,
            ))
        }
        incan_vocab::IncanExpr::Field { object, field } => Ok(ast::Expr::Field(
            Box::new(ast::Spanned::new(
                public_expr_to_internal(object)?,
                ast::Span::default(),
            )),
            field.clone(),
        )),
        _ => Err(VocabAstBridgeError::UnsupportedPublicExpression(
            "expression form is not yet supported by internal AST bridge",
        )),
    }
}

/// Convert one internal decorator to the public decorator DTO.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError`] for decorator argument forms that are intentionally unsupported by the public bridge
/// (for example typed decorator args).
fn public_decorator_from_internal(
    decorator: &ast::Spanned<ast::Decorator>,
) -> Result<incan_vocab::Decorator, VocabAstBridgeError> {
    let mut args = Vec::new();
    for arg in &decorator.node.args {
        match arg {
            ast::DecoratorArg::Positional(expr) => args.push(incan_vocab::DecoratorArg {
                name: None,
                value: public_decorator_arg_value_from_internal_expr(&expr.node)?,
            }),
            ast::DecoratorArg::Named(name, value) => args.push(incan_vocab::DecoratorArg {
                name: Some(name.clone()),
                value: match value {
                    ast::DecoratorArgValue::Type(_) => {
                        return Err(VocabAstBridgeError::UnsupportedInternalExpression(
                            "typed decorator arguments are not currently bridgeable",
                        ));
                    }
                    ast::DecoratorArgValue::Expr(expr) => public_decorator_arg_value_from_internal_expr(&expr.node)?,
                },
            }),
        }
    }
    Ok(incan_vocab::Decorator {
        path: decorator.node.path.segments.clone(),
        args,
        span: public_span(decorator.span),
    })
}

/// Convert an internal decorator argument expression into a public decorator arg value.
///
/// Literal primitives map to scalar public variants; non-literals fall back to `DecoratorArgValue::Expr`.
fn public_decorator_arg_value_from_internal_expr(
    expr: &ast::Expr,
) -> Result<incan_vocab::DecoratorArgValue, VocabAstBridgeError> {
    match expr {
        ast::Expr::Literal(ast::Literal::String(value)) => Ok(incan_vocab::DecoratorArgValue::Str(value.clone())),
        ast::Expr::Literal(ast::Literal::Int(value)) => Ok(incan_vocab::DecoratorArgValue::Int(*value)),
        ast::Expr::Literal(ast::Literal::Bool(value)) => Ok(incan_vocab::DecoratorArgValue::Bool(*value)),
        _ => Ok(incan_vocab::DecoratorArgValue::Expr(internal_expr_to_public(expr)?)),
    }
}

/// Map internal binary operators to public binary operators.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedInternalExpression`] when an internal operator is not represented in the
/// public bridge contract.
fn map_internal_binary_op(op: ast::BinaryOp) -> Result<incan_vocab::IncanBinaryOp, VocabAstBridgeError> {
    match op {
        ast::BinaryOp::Add => Ok(incan_vocab::IncanBinaryOp::Add),
        ast::BinaryOp::Sub => Ok(incan_vocab::IncanBinaryOp::Sub),
        ast::BinaryOp::Mul => Ok(incan_vocab::IncanBinaryOp::Mul),
        ast::BinaryOp::Div => Ok(incan_vocab::IncanBinaryOp::Div),
        ast::BinaryOp::Eq => Ok(incan_vocab::IncanBinaryOp::Eq),
        ast::BinaryOp::NotEq => Ok(incan_vocab::IncanBinaryOp::NotEq),
        ast::BinaryOp::Lt => Ok(incan_vocab::IncanBinaryOp::Lt),
        ast::BinaryOp::Gt => Ok(incan_vocab::IncanBinaryOp::Gt),
        ast::BinaryOp::LtEq => Ok(incan_vocab::IncanBinaryOp::LtEq),
        ast::BinaryOp::GtEq => Ok(incan_vocab::IncanBinaryOp::GtEq),
        ast::BinaryOp::And => Ok(incan_vocab::IncanBinaryOp::And),
        ast::BinaryOp::Or => Ok(incan_vocab::IncanBinaryOp::Or),
        _ => Err(VocabAstBridgeError::UnsupportedInternalExpression(
            "binary operator is not currently bridgeable",
        )),
    }
}

/// Map public binary operators to internal binary operators.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedPublicExpression`] when a public operator is not represented in the
/// internal bridge mapping.
fn map_public_binary_op(op: incan_vocab::IncanBinaryOp) -> Result<ast::BinaryOp, VocabAstBridgeError> {
    match op {
        incan_vocab::IncanBinaryOp::Add => Ok(ast::BinaryOp::Add),
        incan_vocab::IncanBinaryOp::Sub => Ok(ast::BinaryOp::Sub),
        incan_vocab::IncanBinaryOp::Mul => Ok(ast::BinaryOp::Mul),
        incan_vocab::IncanBinaryOp::Div => Ok(ast::BinaryOp::Div),
        incan_vocab::IncanBinaryOp::Eq => Ok(ast::BinaryOp::Eq),
        incan_vocab::IncanBinaryOp::NotEq => Ok(ast::BinaryOp::NotEq),
        incan_vocab::IncanBinaryOp::Lt => Ok(ast::BinaryOp::Lt),
        incan_vocab::IncanBinaryOp::Gt => Ok(ast::BinaryOp::Gt),
        incan_vocab::IncanBinaryOp::LtEq => Ok(ast::BinaryOp::LtEq),
        incan_vocab::IncanBinaryOp::GtEq => Ok(ast::BinaryOp::GtEq),
        incan_vocab::IncanBinaryOp::And => Ok(ast::BinaryOp::And),
        incan_vocab::IncanBinaryOp::Or => Ok(ast::BinaryOp::Or),
        _ => Err(VocabAstBridgeError::UnsupportedPublicExpression(
            "binary operator is not currently bridgeable",
        )),
    }
}

/// Convert an internal compiler span to public `incan_vocab::Span`.
fn public_span(span: ast::Span) -> incan_vocab::Span {
    incan_vocab::Span {
        start: span.start,
        end: span.end,
    }
}
