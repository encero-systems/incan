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

const CURRENT_FIELD_SENTINEL_IDENT: &str = "__incan_vocab_current_row";

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
    let body = block
        .body
        .iter()
        .map(internal_statement_to_body_item)
        .collect::<Result<Vec<_>, _>>()?;

    let decorators = block
        .decorators
        .iter()
        .map(public_decorator_from_internal)
        .collect::<Result<Vec<_>, _>>()?;
    let mut header_args = block
        .header_args
        .iter()
        .map(|arg| internal_expr_to_public(&arg.node))
        .collect::<Result<Vec<_>, _>>()?;
    let head_name = match block.header_args.first() {
        Some(first_arg) => match &first_arg.node {
            ast::Expr::Ident(name) => {
                if !header_args.is_empty() {
                    header_args.remove(0);
                }
                Some(name.clone())
            }
            _ => None,
        },
        None => None,
    };

    Ok(incan_vocab::VocabDeclaration {
        keyword: block.keyword.clone(),
        keyword_metadata: Some(incan_vocab::VocabKeywordMetadata {
            dependency_key: block.keyword_binding.dependency_key.clone(),
            activation_namespace: block.keyword_binding.activation_namespace.clone(),
            surface_kind: block.keyword_binding.surface_kind,
            placement: block.keyword_binding.placement.clone(),
        }),
        head: incan_vocab::VocabDeclarationHead {
            name: head_name,
            header_args,
            parameters: Vec::new(),
            return_type: None,
        },
        decorators,
        body,
        span: public_span(span),
    })
}

/// Classify one internal statement as a public vocab body item.
///
/// Nested `VocabBlock` nodes do not all mean the same thing. Block-context keywords and sub-block keywords become
/// `VocabBodyItem::Clause`, while declaration-shaped nested blocks remain full nested declarations. Ordinary
/// statements are bridged through the public statement model instead.
///
/// # Errors
///
/// Returns the first bridge failure from the nested clause/declaration/statement conversion.
fn internal_statement_to_body_item(
    statement: &ast::Spanned<ast::Statement>,
) -> Result<incan_vocab::VocabBodyItem, VocabAstBridgeError> {
    match &statement.node {
        ast::Statement::VocabBlock(nested)
            if matches!(
                nested.keyword_binding.surface_kind,
                incan_vocab::KeywordSurfaceKind::BlockContextKeyword | incan_vocab::KeywordSurfaceKind::SubBlock
            ) =>
        {
            Ok(incan_vocab::VocabBodyItem::Clause(internal_vocab_clause_to_public(
                nested,
                statement.span,
            )?))
        }
        ast::Statement::VocabBlock(nested) => Ok(incan_vocab::VocabBodyItem::Declaration(
            internal_vocab_block_to_public(nested, statement.span)?,
        )),
        other => Ok(incan_vocab::VocabBodyItem::Statement(internal_statement_to_public(
            other,
        )?)),
    }
}

/// Convert one nested internal vocab block into a public clause.
///
/// Clauses reuse the same parsed `VocabBlockStmt` carrier as declarations on the internal side. The surface kind on
/// the keyword binding decides that this block should be interpreted as clause syntax instead of a nested declaration.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError`] when the clause head or body cannot be represented in the public contract.
fn internal_vocab_clause_to_public(
    block: &ast::VocabBlockStmt,
    span: ast::Span,
) -> Result<incan_vocab::VocabClause, VocabAstBridgeError> {
    let head = block
        .header_args
        .iter()
        .map(|arg| internal_expr_to_public(&arg.node))
        .collect::<Result<Vec<_>, _>>()?;
    let body = internal_clause_body_to_public(&block.body)?;
    Ok(incan_vocab::VocabClause {
        keyword: block.keyword.clone(),
        compound_tokens: Vec::new(),
        head,
        body,
        span: public_span(span),
    })
}

/// Choose the narrowest public clause-body representation for a clause body.
///
/// The bridge prefers specialized shapes in this order:
/// - `Empty` for no body items
/// - `FieldSet` for all-assignment bodies
/// - `Expression` / `ExpressionList` for expression-only bodies
/// - `Items` as the fully general fallback when the body mixes nested clauses/declarations/statements
///
/// # Errors
///
/// Returns the first bridge failure while probing or converting the contained statements.
fn internal_clause_body_to_public(
    statements: &[ast::Spanned<ast::Statement>],
) -> Result<incan_vocab::VocabClauseBody, VocabAstBridgeError> {
    if statements.is_empty() {
        return Ok(incan_vocab::VocabClauseBody::Empty);
    }
    if let Some(fields) = try_internal_field_set(statements)? {
        return Ok(incan_vocab::VocabClauseBody::FieldSet(fields));
    }

    let expression_only = statements
        .iter()
        .map(|statement| match &statement.node {
            ast::Statement::Expr(expr) => internal_expr_to_public(&expr.node).map(Some),
            _ => Ok(None),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if expression_only.iter().all(Option::is_some) {
        let expressions = expression_only
            .into_iter()
            .map(|expr| {
                expr.ok_or(VocabAstBridgeError::UnsupportedInternalStatement(
                    "clause expression extraction expected expression statements",
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        return if expressions.len() == 1 {
            Ok(incan_vocab::VocabClauseBody::Expression(
                expressions
                    .into_iter()
                    .next()
                    .ok_or(VocabAstBridgeError::UnsupportedInternalStatement(
                        "single-expression clause conversion expected one expression item",
                    ))?,
            ))
        } else {
            Ok(incan_vocab::VocabClauseBody::ExpressionList(expressions))
        };
    }

    let items = statements
        .iter()
        .map(internal_statement_to_body_item)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(incan_vocab::VocabClauseBody::Items(items))
}

/// Detect whether a clause body is representable as a public field-set payload.
///
/// A field set is only recognized when every statement is a non-reassignment assignment. Any other statement shape
/// means the caller must fall back to a more general body representation.
///
/// # Errors
///
/// Returns an expression-level bridge failure when a default value cannot be mapped to the public AST.
fn try_internal_field_set(
    statements: &[ast::Spanned<ast::Statement>],
) -> Result<Option<Vec<incan_vocab::VocabFieldSpec>>, VocabAstBridgeError> {
    let mut fields = Vec::with_capacity(statements.len());
    for statement in statements {
        let ast::Statement::Assignment(assignment) = &statement.node else {
            return Ok(None);
        };
        if matches!(assignment.binding, ast::BindingKind::Reassign) {
            return Ok(None);
        }
        fields.push(incan_vocab::VocabFieldSpec {
            name: assignment.name.clone(),
            field_type: assignment.ty.as_ref().map(|ty| incan_vocab::VocabTypeExpr {
                source: ty.node.to_string(),
                span: public_span(ty.span),
            }),
            default_value: Some(internal_expr_to_public(&assignment.value.node)?),
            span: public_span(statement.span),
        });
    }
    Ok(Some(fields))
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
            condition: internal_condition_expr_to_public(&if_stmt.condition)?,
            then_body: internal_statements_to_public(&if_stmt.then_body)?,
            else_body: if_stmt
                .else_body
                .as_ref()
                .map(|body| internal_statements_to_public(body))
                .transpose()?
                .unwrap_or_default(),
        }),
        ast::Statement::While(while_stmt) => Ok(incan_vocab::IncanStatement::While {
            condition: internal_condition_expr_to_public(&while_stmt.condition)?,
            body: internal_statements_to_public(&while_stmt.body)?,
        }),
        ast::Statement::For(for_stmt) => {
            let ast::Pattern::Binding(binding) = &for_stmt.pattern.node else {
                return Err(VocabAstBridgeError::UnsupportedInternalStatement(
                    "tuple-pattern for statements are not yet supported by public vocab AST bridge",
                ));
            };
            Ok(incan_vocab::IncanStatement::For {
                binding: binding.clone(),
                iter: internal_expr_to_public(&for_stmt.iter.node)?,
                body: internal_statements_to_public(&for_stmt.body)?,
            })
        }
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
            condition: ast::Condition::Expr(ast::Spanned::new(
                public_expr_to_internal(condition)?,
                ast::Span::default(),
            )),
            then_body: public_statements_to_internal(then_body)?,
            elif_branches: Vec::new(),
            else_body: if else_body.is_empty() {
                None
            } else {
                Some(public_statements_to_internal(else_body)?)
            },
        })),
        incan_vocab::IncanStatement::While { condition, body } => Ok(ast::Statement::While(ast::WhileStmt {
            condition: ast::Condition::Expr(ast::Spanned::new(
                public_expr_to_internal(condition)?,
                ast::Span::default(),
            )),
            body: public_statements_to_internal(body)?,
        })),
        incan_vocab::IncanStatement::For { binding, iter, body } => Ok(ast::Statement::For(ast::ForStmt {
            pattern: ast::Spanned::new(ast::Pattern::Binding(binding.clone()), ast::Span::default()),
            iter: ast::Spanned::new(public_expr_to_internal(iter)?, ast::Span::default()),
            body: public_statements_to_internal(body)?,
        })),
        _ => Err(VocabAstBridgeError::UnsupportedPublicStatement(
            "statement form is not yet supported by internal AST bridge",
        )),
    }
}

/// Convert one public `incan_vocab::IncanExpr` into internal compiler expression AST.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedPublicExpression`] when the expression shape or operators are not
/// currently represented in the internal bridge mapping.
pub fn public_expression_to_internal(expr: &incan_vocab::IncanExpr) -> Result<ast::Expr, VocabAstBridgeError> {
    public_expr_to_internal(expr)
}

fn internal_condition_expr_to_public(
    condition: &ast::Condition,
) -> Result<incan_vocab::IncanExpr, VocabAstBridgeError> {
    match condition {
        ast::Condition::Expr(expr) => internal_expr_to_public(&expr.node),
        ast::Condition::Let { .. } => Err(VocabAstBridgeError::UnsupportedInternalStatement(
            "`if let` / `while let` conditions are not yet supported by the public vocab AST bridge",
        )),
    }
}

/// Convert a list of internal spanned statements to public statements.
///
/// This helper intentionally drops span provenance and preserves only statement structure, because the public
/// statement DTOs do not carry per-statement source spans today.
///
/// # Errors
///
/// Returns the first failure from [`internal_statement_to_public`].
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
        ast::Expr::Literal(ast::Literal::Int(il)) => Ok(incan_vocab::IncanExpr::Int(il.value)),
        ast::Expr::Literal(ast::Literal::Bool(value)) => Ok(incan_vocab::IncanExpr::Bool(*value)),
        ast::Expr::Tuple(values) => values
            .iter()
            .map(|value| internal_expr_to_public(&value.node))
            .collect::<Result<Vec<_>, _>>()
            .map(incan_vocab::IncanExpr::Tuple),
        ast::Expr::List(values) => values
            .iter()
            .map(|entry| match entry {
                ast::ListEntry::Element(value) => internal_expr_to_public(&value.node),
                ast::ListEntry::Spread(_) => Err(VocabAstBridgeError::UnsupportedInternalExpression(
                    "list spread entries are not supported by public vocab AST bridge",
                )),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(incan_vocab::IncanExpr::List),
        ast::Expr::Dict(entries) => {
            let mut mapped = Vec::with_capacity(entries.len());
            for entry in entries {
                match entry {
                    ast::DictEntry::Pair(key, value) => {
                        mapped.push((
                            internal_expr_to_public(&key.node)?,
                            internal_expr_to_public(&value.node)?,
                        ));
                    }
                    ast::DictEntry::Spread(_) => {
                        return Err(VocabAstBridgeError::UnsupportedInternalExpression(
                            "dict spread entries are not supported by public vocab AST bridge",
                        ));
                    }
                }
            }
            Ok(incan_vocab::IncanExpr::Dict(mapped))
        }
        ast::Expr::Unary(op, value) => Ok(incan_vocab::IncanExpr::Unary(
            match op {
                ast::UnaryOp::Neg => incan_vocab::IncanUnaryOp::Neg,
                ast::UnaryOp::Not => incan_vocab::IncanUnaryOp::Not,
                ast::UnaryOp::Invert => incan_vocab::IncanUnaryOp::Invert,
            },
            Box::new(internal_expr_to_public(&value.node)?),
        )),
        ast::Expr::Binary(left, op, right) => Ok(incan_vocab::IncanExpr::Binary(
            Box::new(internal_expr_to_public(&left.node)?),
            map_internal_binary_op(*op)?,
            Box::new(internal_expr_to_public(&right.node)?),
        )),
        ast::Expr::Call(callee, _type_args, args) => Ok(incan_vocab::IncanExpr::Call {
            callee: Box::new(internal_expr_to_public(&callee.node)?),
            args: internal_call_args_to_public(args)?,
        }),
        ast::Expr::MethodCall(base, method, _type_args, args) => {
            let callee = incan_vocab::IncanExpr::Field {
                object: Box::new(internal_expr_to_public(&base.node)?),
                field: method.clone(),
            };
            Ok(incan_vocab::IncanExpr::Call {
                callee: Box::new(callee),
                args: internal_call_args_to_public(args)?,
            })
        }
        ast::Expr::Field(object, field) => match &object.node {
            ast::Expr::Ident(name) if name == CURRENT_FIELD_SENTINEL_IDENT => {
                Ok(incan_vocab::IncanExpr::CurrentField(field.clone()))
            }
            ast::Expr::Ident(name) => Ok(incan_vocab::IncanExpr::RelationField {
                relation: name.clone(),
                field: field.clone(),
            }),
            _ => Ok(incan_vocab::IncanExpr::Field {
                object: Box::new(internal_expr_to_public(&object.node)?),
                field: field.clone(),
            }),
        },
        ast::Expr::Surface(surface) => internal_surface_expr_to_public(surface),
        _ => Err(VocabAstBridgeError::UnsupportedInternalExpression(
            "expression form is not yet supported by public vocab AST bridge",
        )),
    }
}

/// Convert internal call arguments to public positional argument payloads.
fn internal_call_args_to_public(args: &[ast::CallArg]) -> Result<Vec<incan_vocab::IncanExpr>, VocabAstBridgeError> {
    let mut mapped_args = Vec::with_capacity(args.len());
    for arg in args {
        let value = match arg {
            ast::CallArg::Positional(expr)
            | ast::CallArg::Named(_, expr)
            | ast::CallArg::PositionalUnpack(expr)
            | ast::CallArg::KeywordUnpack(expr) => expr,
        };
        mapped_args.push(internal_expr_to_public(&value.node)?);
    }
    Ok(mapped_args)
}

/// Convert a compiler surface expression artifact into the public vocab AST.
fn internal_surface_expr_to_public(surface: &ast::SurfaceExpr) -> Result<incan_vocab::IncanExpr, VocabAstBridgeError> {
    if let ast::SurfaceExprPayload::RaceFor(race) = &surface.payload {
        return Ok(incan_vocab::IncanExpr::RaceFor(internal_race_for_to_public(race)?));
    }

    let incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
        dependency_key,
        descriptor_key,
    } = &surface.key
    else {
        return Err(VocabAstBridgeError::UnsupportedInternalExpression(
            "surface expression is not yet supported by public vocab AST bridge",
        ));
    };

    let payload = match &surface.payload {
        ast::SurfaceExprPayload::LeadingDotPath {
            segments,
            receiver,
            owner,
        } => incan_vocab::IncanScopedSurfacePayload::LeadingDotPath {
            segments: segments.clone(),
            receiver: receiver.clone(),
            owner: incan_vocab::IncanScopedSurfaceOwner {
                declaration: owner.declaration.clone(),
                clause: owner.clause.clone(),
                call: owner.call.clone(),
            },
        },
        ast::SurfaceExprPayload::ScopedGlyph {
            glyph,
            left,
            right,
            owner,
        } => incan_vocab::IncanScopedSurfacePayload::ScopedGlyph {
            glyph: glyph.clone(),
            left: Box::new(internal_expr_to_public(&left.node)?),
            right: Box::new(internal_expr_to_public(&right.node)?),
            owner: incan_vocab::IncanScopedSurfaceOwner {
                declaration: owner.declaration.clone(),
                clause: owner.clause.clone(),
                call: owner.call.clone(),
            },
        },
        ast::SurfaceExprPayload::ScopedSymbolCall { symbol, args, owner } => {
            return Ok(incan_vocab::IncanExpr::ScopedSymbolCall(
                incan_vocab::IncanScopedSymbolCall {
                    dependency_key: dependency_key.clone(),
                    descriptor_key: descriptor_key.clone(),
                    symbol: symbol.clone(),
                    args: internal_call_args_to_public(args)?,
                    owner: incan_vocab::IncanScopedSurfaceOwner {
                        declaration: owner.declaration.clone(),
                        clause: owner.clause.clone(),
                        call: owner.call.clone(),
                    },
                },
            ));
        }
        ast::SurfaceExprPayload::PrefixUnary(_) => {
            return Err(VocabAstBridgeError::UnsupportedInternalExpression(
                "soft-keyword surface expression is not yet supported by public vocab AST bridge",
            ));
        }
        ast::SurfaceExprPayload::RaceFor(_) => unreachable!("race surface expressions return before scoped conversion"),
    };

    Ok(incan_vocab::IncanExpr::ScopedSurface(
        incan_vocab::IncanScopedSurfaceExpr {
            dependency_key: dependency_key.clone(),
            descriptor_key: descriptor_key.clone(),
            payload,
        },
    ))
}

/// Convert one internal race expression into the public vocab AST.
fn internal_race_for_to_public(race: &ast::RaceForExpr) -> Result<incan_vocab::IncanRaceForExpr, VocabAstBridgeError> {
    let arms = race
        .arms
        .iter()
        .map(|arm| {
            let body = match &arm.body {
                ast::RaceForBody::Expr(expr) => {
                    incan_vocab::IncanRaceForBody::Expr(Box::new(internal_expr_to_public(&expr.node)?))
                }
                ast::RaceForBody::Block(statements) => {
                    incan_vocab::IncanRaceForBody::Block(internal_statements_to_public(statements)?)
                }
            };
            Ok(incan_vocab::IncanRaceForArm {
                awaitable: internal_expr_to_public(&arm.awaitable.node)?,
                body,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(incan_vocab::IncanRaceForExpr {
        binding: race.binding.clone(),
        arms,
    })
}

/// Convert one public `incan_vocab::IncanExpr` to internal compiler expression AST.
///
/// This is the inverse of [`internal_expr_to_public`] for the bridgeable subset of the public vocab expression model.
/// Special query helpers such as `CurrentField` and `RelationField` are lowered through internal field-access shapes
/// using a sentinel identifier when necessary.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedPublicExpression`] for unsupported public expression kinds or operators.
fn public_expr_to_internal(expr: &incan_vocab::IncanExpr) -> Result<ast::Expr, VocabAstBridgeError> {
    match expr {
        incan_vocab::IncanExpr::Name(name) => Ok(ast::Expr::Ident(name.clone())),
        incan_vocab::IncanExpr::Str(value) => Ok(ast::Expr::Literal(ast::Literal::String(value.clone()))),
        incan_vocab::IncanExpr::Int(value) => Ok(ast::Expr::Literal(ast::Literal::Int(ast::IntLiteral::synthetic(
            *value,
        )))),
        incan_vocab::IncanExpr::Bool(value) => Ok(ast::Expr::Literal(ast::Literal::Bool(*value))),
        incan_vocab::IncanExpr::CurrentField(field) => Ok(ast::Expr::Field(
            Box::new(ast::Spanned::new(
                ast::Expr::Ident(CURRENT_FIELD_SENTINEL_IDENT.to_string()),
                ast::Span::default(),
            )),
            field.clone(),
        )),
        incan_vocab::IncanExpr::RelationField { relation, field } => Ok(ast::Expr::Field(
            Box::new(ast::Spanned::new(
                ast::Expr::Ident(relation.clone()),
                ast::Span::default(),
            )),
            field.clone(),
        )),
        incan_vocab::IncanExpr::Tuple(values) => values
            .iter()
            .map(|value| public_expr_to_internal(value).map(|node| ast::Spanned::new(node, ast::Span::default())))
            .collect::<Result<Vec<_>, _>>()
            .map(ast::Expr::Tuple),
        incan_vocab::IncanExpr::List(values) => values
            .iter()
            .map(|value| {
                public_expr_to_internal(value)
                    .map(|node| ast::ListEntry::Element(ast::Spanned::new(node, ast::Span::default())))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ast::Expr::List),
        incan_vocab::IncanExpr::Dict(entries) => {
            let mut mapped = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                mapped.push(ast::DictEntry::Pair(
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
                incan_vocab::IncanUnaryOp::Invert => ast::UnaryOp::Invert,
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
                Vec::new(),
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
        incan_vocab::IncanExpr::RaceFor(race) => public_race_for_to_internal(race),
        incan_vocab::IncanExpr::ScopedSurface(surface) => public_scoped_surface_expr_to_internal(surface),
        incan_vocab::IncanExpr::ScopedSymbolCall(call) => public_scoped_symbol_call_to_internal(call),
        _ => Err(VocabAstBridgeError::UnsupportedPublicExpression(
            "expression form is not yet supported by internal AST bridge",
        )),
    }
}

/// Convert one public race expression back into the compiler AST.
fn public_race_for_to_internal(race: &incan_vocab::IncanRaceForExpr) -> Result<ast::Expr, VocabAstBridgeError> {
    let arms = race
        .arms
        .iter()
        .map(|arm| {
            let body = match &arm.body {
                incan_vocab::IncanRaceForBody::Expr(expr) => {
                    ast::RaceForBody::Expr(ast::Spanned::new(public_expr_to_internal(expr)?, ast::Span::default()))
                }
                incan_vocab::IncanRaceForBody::Block(statements) => {
                    ast::RaceForBody::Block(public_statements_to_internal(statements)?)
                }
                _ => {
                    return Err(VocabAstBridgeError::UnsupportedPublicExpression(
                        "race arm body form is not supported by internal AST bridge",
                    ));
                }
            };
            Ok(ast::RaceForArm {
                awaitable: ast::Spanned::new(public_expr_to_internal(&arm.awaitable)?, ast::Span::default()),
                body,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ast::Expr::Surface(Box::new(ast::SurfaceExpr {
        key: incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
            dependency_key: "std.async".to_string(),
            descriptor_key: "race_for".to_string(),
        },
        payload: ast::SurfaceExprPayload::RaceFor(Box::new(ast::RaceForExpr {
            binding: race.binding.clone(),
            arms,
        })),
    })))
}

/// Convert a public scoped-surface expression back into the compiler AST.
fn public_scoped_surface_expr_to_internal(
    surface: &incan_vocab::IncanScopedSurfaceExpr,
) -> Result<ast::Expr, VocabAstBridgeError> {
    let key = incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
        dependency_key: surface.dependency_key.clone(),
        descriptor_key: surface.descriptor_key.clone(),
    };
    let payload = match &surface.payload {
        incan_vocab::IncanScopedSurfacePayload::LeadingDotPath {
            segments,
            receiver,
            owner,
        } => ast::SurfaceExprPayload::LeadingDotPath {
            segments: segments.clone(),
            receiver: receiver.clone(),
            owner: ast::ScopedSurfaceOwner {
                declaration: owner.declaration.clone(),
                clause: owner.clause.clone(),
                call: owner.call.clone(),
            },
        },
        incan_vocab::IncanScopedSurfacePayload::ScopedGlyph {
            glyph,
            left,
            right,
            owner,
        } => ast::SurfaceExprPayload::ScopedGlyph {
            glyph: glyph.clone(),
            left: Box::new(ast::Spanned::new(public_expr_to_internal(left)?, ast::Span::default())),
            right: Box::new(ast::Spanned::new(public_expr_to_internal(right)?, ast::Span::default())),
            owner: ast::ScopedSurfaceOwner {
                declaration: owner.declaration.clone(),
                clause: owner.clause.clone(),
                call: owner.call.clone(),
            },
        },
        _ => {
            return Err(VocabAstBridgeError::UnsupportedPublicExpression(
                "scoped surface payload is not yet supported by internal AST bridge",
            ));
        }
    };
    Ok(ast::Expr::Surface(Box::new(ast::SurfaceExpr { key, payload })))
}

/// Convert a public scoped-symbol call back into the compiler AST.
fn public_scoped_symbol_call_to_internal(
    call: &incan_vocab::IncanScopedSymbolCall,
) -> Result<ast::Expr, VocabAstBridgeError> {
    let key = incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
        dependency_key: call.dependency_key.clone(),
        descriptor_key: call.descriptor_key.clone(),
    };
    let args = call
        .args
        .iter()
        .map(|arg| {
            public_expr_to_internal(arg)
                .map(|node| ast::CallArg::Positional(ast::Spanned::new(node, ast::Span::default())))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let payload = ast::SurfaceExprPayload::ScopedSymbolCall {
        symbol: call.symbol.clone(),
        args,
        owner: ast::ScopedSurfaceOwner {
            declaration: call.owner.declaration.clone(),
            clause: call.owner.clause.clone(),
            call: call.owner.call.clone(),
        },
    };
    Ok(ast::Expr::Surface(Box::new(ast::SurfaceExpr { key, payload })))
}

/// Convert one internal decorator to the public decorator DTO.
///
/// Decorators stay mostly structural: path segments and named/positional arguments are preserved, while unsupported
/// typed decorator arguments fail explicitly so the public bridge never invents a lossy encoding.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError`] for decorator argument forms that are intentionally unsupported by the public bridge
/// (for example typed decorator args).
fn public_decorator_from_internal(
    decorator: &ast::Spanned<ast::Decorator>,
) -> Result<incan_vocab::Decorator, VocabAstBridgeError> {
    if !decorator.node.type_args.is_empty() {
        return Err(VocabAstBridgeError::UnsupportedInternalExpression(
            "typed decorator call-site arguments are not currently bridgeable",
        ));
    }

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
/// Literal primitives map to scalar public variants; non-literals fall back to `DecoratorArgValue::Expr` so public
/// consumers can still inspect the original expression structure.
fn public_decorator_arg_value_from_internal_expr(
    expr: &ast::Expr,
) -> Result<incan_vocab::DecoratorArgValue, VocabAstBridgeError> {
    match expr {
        ast::Expr::Literal(ast::Literal::String(value)) => Ok(incan_vocab::DecoratorArgValue::Str(value.clone())),
        ast::Expr::Literal(ast::Literal::Int(il)) => Ok(incan_vocab::DecoratorArgValue::Int(il.value)),
        ast::Expr::Literal(ast::Literal::Bool(value)) => Ok(incan_vocab::DecoratorArgValue::Bool(*value)),
        _ => Ok(incan_vocab::DecoratorArgValue::Expr(internal_expr_to_public(expr)?)),
    }
}

/// Map internal binary operators to public binary operators.
///
/// This is intentionally whitelist-based so the public bridge contract expands only when both sides agree on exact
/// operator semantics.
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
        ast::BinaryOp::FloorDiv => Ok(incan_vocab::IncanBinaryOp::FloorDiv),
        ast::BinaryOp::Mod => Ok(incan_vocab::IncanBinaryOp::Mod),
        ast::BinaryOp::Pow => Ok(incan_vocab::IncanBinaryOp::Pow),
        ast::BinaryOp::MatMul => Ok(incan_vocab::IncanBinaryOp::MatMul),
        ast::BinaryOp::PipeForward => Ok(incan_vocab::IncanBinaryOp::PipeForward),
        ast::BinaryOp::PipeBackward => Ok(incan_vocab::IncanBinaryOp::PipeBackward),
        ast::BinaryOp::BitAnd => Ok(incan_vocab::IncanBinaryOp::BitAnd),
        ast::BinaryOp::BitOr => Ok(incan_vocab::IncanBinaryOp::BitOr),
        ast::BinaryOp::BitXor => Ok(incan_vocab::IncanBinaryOp::BitXor),
        ast::BinaryOp::Shl => Ok(incan_vocab::IncanBinaryOp::Shl),
        ast::BinaryOp::Shr => Ok(incan_vocab::IncanBinaryOp::Shr),
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
/// This mirrors [`map_internal_binary_op`] and keeps the round-trip surface explicit instead of assuming every public
/// enum variant already has an internal lowering.
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
        incan_vocab::IncanBinaryOp::FloorDiv => Ok(ast::BinaryOp::FloorDiv),
        incan_vocab::IncanBinaryOp::Mod => Ok(ast::BinaryOp::Mod),
        incan_vocab::IncanBinaryOp::Pow => Ok(ast::BinaryOp::Pow),
        incan_vocab::IncanBinaryOp::MatMul => Ok(ast::BinaryOp::MatMul),
        incan_vocab::IncanBinaryOp::PipeForward => Ok(ast::BinaryOp::PipeForward),
        incan_vocab::IncanBinaryOp::PipeBackward => Ok(ast::BinaryOp::PipeBackward),
        incan_vocab::IncanBinaryOp::BitAnd => Ok(ast::BinaryOp::BitAnd),
        incan_vocab::IncanBinaryOp::BitOr => Ok(ast::BinaryOp::BitOr),
        incan_vocab::IncanBinaryOp::BitXor => Ok(ast::BinaryOp::BitXor),
        incan_vocab::IncanBinaryOp::Shl => Ok(ast::BinaryOp::Shl),
        incan_vocab::IncanBinaryOp::Shr => Ok(ast::BinaryOp::Shr),
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
///
/// This is a pure shape conversion; source offsets are preserved exactly.
fn public_span(span: ast::Span) -> incan_vocab::Span {
    incan_vocab::Span {
        start: span.start,
        end: span.end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_keyword_binding(surface_kind: incan_vocab::KeywordSurfaceKind) -> ast::VocabKeywordBinding {
        ast::VocabKeywordBinding {
            dependency_key: "demo".to_string(),
            activation_namespace: "demo.dsl".to_string(),
            surface_kind,
            placement: incan_vocab::KeywordPlacement::TopLevel,
        }
    }

    #[test]
    fn bridges_block_context_keywords_as_clauses() -> Result<(), Box<dyn std::error::Error>> {
        let clause_block = ast::VocabBlockStmt {
            keyword: "FROM".to_string(),
            keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockContextKeyword),
            decorators: Vec::new(),
            header_args: vec![ast::Spanned::new(
                ast::Expr::Ident("orders".to_string()),
                ast::Span::default(),
            )],
            body: Vec::new(),
        };
        let declaration_block = ast::VocabBlockStmt {
            keyword: "query".to_string(),
            keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration),
            decorators: Vec::new(),
            header_args: Vec::new(),
            body: vec![ast::Spanned::new(
                ast::Statement::VocabBlock(clause_block),
                ast::Span::default(),
            )],
        };

        let bridged = internal_vocab_block_to_public(&declaration_block, ast::Span::default())?;
        assert_eq!(bridged.body.len(), 1);
        match &bridged.body[0] {
            incan_vocab::VocabBodyItem::Clause(clause) => {
                assert_eq!(clause.keyword, "FROM");
                assert_eq!(clause.head, vec![incan_vocab::IncanExpr::Name("orders".to_string())]);
            }
            other => {
                return Err(format!("expected clause body item, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn infers_declaration_head_name_from_first_identifier_arg() -> Result<(), Box<dyn std::error::Error>> {
        let block = ast::VocabBlockStmt {
            keyword: "workflow".to_string(),
            keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration),
            decorators: Vec::new(),
            header_args: vec![
                ast::Spanned::new(ast::Expr::Ident("daily".to_string()), ast::Span::default()),
                ast::Spanned::new(
                    ast::Expr::Literal(ast::Literal::String("reports".to_string())),
                    ast::Span::default(),
                ),
            ],
            body: Vec::new(),
        };

        let bridged = internal_vocab_block_to_public(&block, ast::Span::default())?;
        assert_eq!(bridged.head.name.as_deref(), Some("daily"));
        assert_eq!(
            bridged.head.header_args,
            vec![incan_vocab::IncanExpr::Str("reports".to_string())]
        );
        Ok(())
    }

    #[test]
    fn bridges_public_field_reference_variants_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let current = incan_vocab::IncanExpr::CurrentField("amount".to_string());
        let current_internal = public_expression_to_internal(&current)?;
        let current_roundtrip = internal_expr_to_public(&current_internal)?;
        assert_eq!(current_roundtrip, current);

        let relation = incan_vocab::IncanExpr::RelationField {
            relation: "orders".to_string(),
            field: "amount".to_string(),
        };
        let relation_internal = public_expression_to_internal(&relation)?;
        let relation_roundtrip = internal_expr_to_public(&relation_internal)?;
        assert_eq!(relation_roundtrip, relation);
        Ok(())
    }

    #[test]
    fn bridges_method_calls_as_public_field_callee_calls() -> Result<(), Box<dyn std::error::Error>> {
        let call = ast::Expr::MethodCall(
            Box::new(ast::Spanned::new(
                ast::Expr::Ident("orders".to_string()),
                ast::Span::default(),
            )),
            "filter".to_string(),
            Vec::new(),
            vec![ast::CallArg::Positional(ast::Spanned::new(
                ast::Expr::Ident("predicate".to_string()),
                ast::Span::default(),
            ))],
        );

        let public = internal_expr_to_public(&call)?;
        assert!(matches!(
            &public,
            incan_vocab::IncanExpr::Call { callee, args }
                if matches!(
                    callee.as_ref(),
                    incan_vocab::IncanExpr::Field { object, field }
                        if matches!(object.as_ref(), incan_vocab::IncanExpr::Name(name) if name == "orders")
                            && field == "filter"
                ) && args == &vec![incan_vocab::IncanExpr::Name("predicate".to_string())]
        ));
        Ok(())
    }

    #[test]
    fn bridges_rfc028_operator_enums_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let left = ast::Spanned::new(ast::Expr::Ident("left".to_string()), ast::Span::default());
        let right = ast::Spanned::new(ast::Expr::Ident("right".to_string()), ast::Span::default());
        let cases = [
            (ast::BinaryOp::FloorDiv, incan_vocab::IncanBinaryOp::FloorDiv),
            (ast::BinaryOp::Mod, incan_vocab::IncanBinaryOp::Mod),
            (ast::BinaryOp::Pow, incan_vocab::IncanBinaryOp::Pow),
            (ast::BinaryOp::MatMul, incan_vocab::IncanBinaryOp::MatMul),
            (ast::BinaryOp::PipeForward, incan_vocab::IncanBinaryOp::PipeForward),
            (ast::BinaryOp::PipeBackward, incan_vocab::IncanBinaryOp::PipeBackward),
            (ast::BinaryOp::BitAnd, incan_vocab::IncanBinaryOp::BitAnd),
            (ast::BinaryOp::BitOr, incan_vocab::IncanBinaryOp::BitOr),
            (ast::BinaryOp::BitXor, incan_vocab::IncanBinaryOp::BitXor),
            (ast::BinaryOp::Shl, incan_vocab::IncanBinaryOp::Shl),
            (ast::BinaryOp::Shr, incan_vocab::IncanBinaryOp::Shr),
        ];

        for (internal_op, public_op) in cases {
            let internal = ast::Expr::Binary(Box::new(left.clone()), internal_op, Box::new(right.clone()));
            let public = internal_expr_to_public(&internal)?;
            assert!(
                matches!(&public, incan_vocab::IncanExpr::Binary(_, op, _) if *op == public_op),
                "expected public operator {public_op:?}, got {public:?}"
            );
            assert_eq!(public_expression_to_internal(&public)?, internal);
        }

        let internal = ast::Expr::Unary(
            ast::UnaryOp::Invert,
            Box::new(ast::Spanned::new(
                ast::Expr::Ident("value".to_string()),
                ast::Span::default(),
            )),
        );
        let public = internal_expr_to_public(&internal)?;
        assert!(matches!(
            &public,
            incan_vocab::IncanExpr::Unary(incan_vocab::IncanUnaryOp::Invert, _)
        ));
        assert_eq!(public_expression_to_internal(&public)?, internal);
        Ok(())
    }

    #[test]
    fn bridges_scoped_surface_expression_artifacts_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let scoped = ast::Expr::Surface(Box::new(ast::SurfaceExpr {
            key: incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                dependency_key: "querykit".to_string(),
                descriptor_key: "query.field".to_string(),
            },
            payload: ast::SurfaceExprPayload::LeadingDotPath {
                segments: vec!["order".to_string(), "amount".to_string()],
                receiver: incan_vocab::ScopedSurfaceReceiver::OwningDeclaration,
                owner: ast::ScopedSurfaceOwner {
                    declaration: "query".to_string(),
                    clause: None,
                    call: None,
                },
            },
        }));

        let public = internal_expr_to_public(&scoped)?;
        assert!(matches!(
            &public,
            incan_vocab::IncanExpr::ScopedSurface(surface)
                if surface.dependency_key == "querykit"
                    && surface.descriptor_key == "query.field"
                    && matches!(
                        &surface.payload,
                        incan_vocab::IncanScopedSurfacePayload::LeadingDotPath { segments, owner, .. }
                            if segments == &["order".to_string(), "amount".to_string()]
                                && owner.declaration == "query"
                    )
        ));
        let round_trip = public_expression_to_internal(&public)?;
        assert_eq!(round_trip, scoped);

        let scoped_symbol = ast::Expr::Surface(Box::new(ast::SurfaceExpr {
            key: incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                dependency_key: "querykit".to_string(),
                descriptor_key: "query.sum".to_string(),
            },
            payload: ast::SurfaceExprPayload::ScopedSymbolCall {
                symbol: "sum".to_string(),
                args: vec![ast::CallArg::Positional(ast::Spanned::new(
                    ast::Expr::Ident("amount".to_string()),
                    ast::Span::default(),
                ))],
                owner: ast::ScopedSurfaceOwner {
                    declaration: "query".to_string(),
                    clause: Some("SELECT".to_string()),
                    call: None,
                },
            },
        }));

        let public = internal_expr_to_public(&scoped_symbol)?;
        assert!(matches!(
            &public,
            incan_vocab::IncanExpr::ScopedSymbolCall(call)
                if call.dependency_key == "querykit"
                    && call.descriptor_key == "query.sum"
                    && call.symbol == "sum"
                    && call.args.len() == 1
                    && call.owner.declaration == "query"
                    && call.owner.clause.as_deref() == Some("SELECT")
        ));
        let round_trip = public_expression_to_internal(&public)?;
        assert_eq!(round_trip, scoped_symbol);
        Ok(())
    }

    #[test]
    fn bridges_race_for_expression_artifacts_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let race = ast::Expr::Surface(Box::new(ast::SurfaceExpr {
            key: incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
                dependency_key: "std.async".to_string(),
                descriptor_key: "race_for".to_string(),
            },
            payload: ast::SurfaceExprPayload::RaceFor(Box::new(ast::RaceForExpr {
                binding: "value".to_string(),
                arms: vec![
                    ast::RaceForArm {
                        awaitable: ast::Spanned::new(ast::Expr::Ident("fast".to_string()), ast::Span::default()),
                        body: ast::RaceForBody::Expr(ast::Spanned::new(
                            ast::Expr::Ident("value".to_string()),
                            ast::Span::default(),
                        )),
                    },
                    ast::RaceForArm {
                        awaitable: ast::Spanned::new(ast::Expr::Ident("slow".to_string()), ast::Span::default()),
                        body: ast::RaceForBody::Block(vec![ast::Spanned::new(
                            ast::Statement::Return(Some(ast::Spanned::new(
                                ast::Expr::Ident("value".to_string()),
                                ast::Span::default(),
                            ))),
                            ast::Span::default(),
                        )]),
                    },
                ],
            })),
        }));

        let public = internal_expr_to_public(&race)?;
        assert!(matches!(
            &public,
            incan_vocab::IncanExpr::RaceFor(race)
                if race.binding == "value"
                    && race.arms.len() == 2
                    && matches!(&race.arms[0].body, incan_vocab::IncanRaceForBody::Expr(_))
                    && matches!(&race.arms[1].body, incan_vocab::IncanRaceForBody::Block(_))
        ));
        assert_eq!(public_expression_to_internal(&public)?, race);
        Ok(())
    }
}
