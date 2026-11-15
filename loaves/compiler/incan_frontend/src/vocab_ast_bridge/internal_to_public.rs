//! Conversion of internal vocab blocks, clauses, statements and expressions into the public vocab AST.

use super::*;

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
    let head = if let Some(signature) = &block.signature_head {
        incan_vocab::VocabDeclarationHead {
            name: Some(signature.name.clone()),
            header_args: Vec::new(),
            parameters: signature
                .parameters
                .iter()
                .map(|parameter| {
                    Ok(incan_vocab::VocabParameter {
                        name: parameter.node.name.clone(),
                        param_type: Some(incan_vocab::VocabTypeExpr {
                            source: parameter.node.ty.node.to_string(),
                            span: public_span(parameter.node.ty.span),
                        }),
                        default_value: parameter
                            .node
                            .default
                            .as_ref()
                            .map(|value| internal_expr_to_public(&value.node))
                            .transpose()?,
                        span: public_span(parameter.span),
                    })
                })
                .collect::<Result<Vec<_>, VocabAstBridgeError>>()?,
            return_type: signature
                .return_type
                .as_ref()
                .map(|return_type| incan_vocab::VocabTypeExpr {
                    source: return_type.node.to_string(),
                    span: public_span(return_type.span),
                }),
        }
    } else {
        let mut header_args = block
            .header_args
            .iter()
            .map(|arg| internal_expr_to_public(&arg.node))
            .collect::<Result<Vec<_>, _>>()?;
        let name = match block.header_args.first() {
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
        incan_vocab::VocabDeclarationHead {
            name,
            header_args,
            parameters: Vec::new(),
            return_type: None,
        }
    };

    Ok(incan_vocab::VocabDeclaration {
        keyword: block.keyword.clone(),
        compound_tokens: block.keyword_binding.compound_tokens.clone(),
        keyword_metadata: Some(incan_vocab::VocabKeywordMetadata {
            dependency_key: block.keyword_binding.dependency_key.clone(),
            activation_namespace: block.keyword_binding.activation_namespace.clone(),
            surface_kind: block.keyword_binding.surface_kind,
            placement: block.keyword_binding.placement.clone(),
        }),
        head,
        decorators,
        body,
        span: public_span(span),
    })
}

/// Classify one internal statement as a public vocab body item.
///
/// Nested `VocabBlock` nodes do not all mean the same thing. Blocks owned by a rich declaration's `ClauseSurface`
/// become `VocabBodyItem::Clause`, while other nested blocks remain full nested declarations. The ownership bit is
/// carried by parser metadata because low-level keyword surface kinds do not faithfully describe public AST ownership
/// for compatibility registrations; the historical context/sub-block interpretation remains as a fallback for raw
/// registrations that do not provide a rich surface. Ordinary statements are bridged through the public statement
/// model instead.
///
/// # Errors
///
/// Returns the first bridge failure from the nested clause/declaration/statement conversion.
pub(super) fn internal_statement_to_body_item(
    statement: &ast::Spanned<ast::Statement>,
) -> Result<incan_vocab::VocabBodyItem, VocabAstBridgeError> {
    match &statement.node {
        ast::Statement::VocabBlock(nested)
            if nested.keyword_binding.is_declaration_owned_clause
                || matches!(
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
/// Clauses reuse the same parsed `VocabBlockStmt` carrier as declarations on the internal side. Rich declaration
/// ownership metadata decides that this block should be interpreted as clause syntax instead of a nested declaration;
/// legacy block-context and sub-block registrations retain their existing clause interpretation.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError`] when the clause head or body cannot be represented in the public contract.
pub(super) fn internal_vocab_clause_to_public(
    block: &ast::VocabBlockStmt,
    span: ast::Span,
) -> Result<incan_vocab::VocabClause, VocabAstBridgeError> {
    let head = block
        .header_args
        .iter()
        .map(|arg| internal_expr_to_public(&arg.node))
        .collect::<Result<Vec<_>, _>>()?;
    let body = internal_clause_body_to_public(&block.body, block.keyword_binding.clause_body_kind)?;
    Ok(incan_vocab::VocabClause {
        keyword: block.keyword.clone(),
        compound_tokens: block.keyword_binding.compound_tokens.clone(),
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
pub(super) fn internal_clause_body_to_public(
    statements: &[ast::Spanned<ast::Statement>],
    declared_body_kind: Option<incan_vocab::ClauseBodyKind>,
) -> Result<incan_vocab::VocabClauseBody, VocabAstBridgeError> {
    if statements.is_empty() {
        return Ok(incan_vocab::VocabClauseBody::Empty);
    }
    if matches!(declared_body_kind, Some(incan_vocab::ClauseBodyKind::ExpressionList)) {
        return expression_list_body_to_public(statements);
    }
    if let Some(fields) = try_internal_field_set(statements)? {
        return Ok(incan_vocab::VocabClauseBody::FieldSet(fields));
    }

    let expression_only = statements
        .iter()
        .map(|statement| match &statement.node {
            ast::Statement::Expr(expr) => Ok(Some(incan_vocab::VocabExpressionItem {
                expr: internal_expr_to_public(&expr.node)?,
                alias: None,
                modifiers: Vec::new(),
                span: public_span(statement.span),
            })),
            _ => Ok(None),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if expression_only.iter().all(Option::is_some) {
        let expression_items = expression_only
            .into_iter()
            .map(|item| {
                item.ok_or(VocabAstBridgeError::UnsupportedInternalStatement(
                    "clause expression extraction expected expression statements",
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        return if expression_items.len() == 1 {
            Ok(incan_vocab::VocabClauseBody::Expression(
                expression_items
                    .into_iter()
                    .next()
                    .ok_or(VocabAstBridgeError::UnsupportedInternalStatement(
                        "single-expression clause conversion expected one expression item",
                    ))?
                    .expr,
            ))
        } else {
            Ok(incan_vocab::VocabClauseBody::ExpressionList(expression_items))
        };
    }

    let items = statements
        .iter()
        .map(internal_statement_to_body_item)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(incan_vocab::VocabClauseBody::Items(items))
}

/// Convert a clause body declared as `ClauseBodyKind::ExpressionList`.
///
/// This preserves declared trailing keyword metadata as first-class public AST rather than forcing desugarers to
/// recover DSL item structure from ordinary statements.
pub(super) fn expression_list_body_to_public(
    statements: &[ast::Spanned<ast::Statement>],
) -> Result<incan_vocab::VocabClauseBody, VocabAstBridgeError> {
    let mut items = Vec::with_capacity(statements.len());
    for statement in statements {
        match &statement.node {
            ast::Statement::Expr(expr) => items.push(incan_vocab::VocabExpressionItem {
                expr: internal_expr_to_public(&expr.node)?,
                alias: None,
                modifiers: Vec::new(),
                span: public_span(statement.span),
            }),
            ast::Statement::VocabExpressionItem(item) => items.push(incan_vocab::VocabExpressionItem {
                expr: internal_expr_to_public(&item.expr.node)?,
                alias: item.alias.clone(),
                modifiers: item
                    .modifiers
                    .iter()
                    .map(|modifier| {
                        Ok(incan_vocab::VocabExpressionItemModifier {
                            keyword: modifier.keyword.clone(),
                            value: internal_expr_to_public(&modifier.value.node)?,
                            span: public_span(modifier.span),
                        })
                    })
                    .collect::<Result<Vec<_>, VocabAstBridgeError>>()?,
                span: public_span(statement.span),
            }),
            _ => {
                return Err(VocabAstBridgeError::UnsupportedInternalStatement(
                    "expression-list clause body expected expression entries",
                ));
            }
        }
    }
    Ok(incan_vocab::VocabClauseBody::ExpressionList(items))
}

/// Detect whether a clause body is representable as a public field-set payload.
///
/// A field set is only recognized when every statement is a non-reassignment assignment. Any other statement shape
/// means the caller must fall back to a more general body representation.
///
/// # Errors
///
/// Returns an expression-level bridge failure when a default value cannot be mapped to the public AST.
pub(super) fn try_internal_field_set(
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
/// [`VocabAstBridgeError::UnsupportedInternalExpression`] when a shape cannot be represented in the current public AST.
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
        ast::Statement::Assignment(assign) => {
            let name = assign.name.clone();
            let mutable = matches!(assign.binding, ast::BindingKind::Mutable);
            let value = internal_expr_to_public(&assign.value.node)?;
            if let Some(ty) = &assign.ty {
                Ok(incan_vocab::IncanStatement::TypedLet {
                    name,
                    mutable,
                    ty: incan_vocab::VocabTypeExpr {
                        source: ty.node.to_string(),
                        span: public_span(ty.span),
                    },
                    value,
                })
            } else {
                Ok(incan_vocab::IncanStatement::Let { name, mutable, value })
            }
        }
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

/// Convert an internal vocab condition expression into the public AST form.
pub(super) fn internal_condition_expr_to_public(
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
pub(super) fn internal_statements_to_public(
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
pub(super) fn internal_expr_to_public(expr: &ast::Expr) -> Result<incan_vocab::IncanExpr, VocabAstBridgeError> {
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
pub(super) fn internal_call_args_to_public(
    args: &[ast::CallArg],
) -> Result<Vec<incan_vocab::IncanExpr>, VocabAstBridgeError> {
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
pub(super) fn internal_surface_expr_to_public(
    surface: &ast::SurfaceExpr,
) -> Result<incan_vocab::IncanExpr, VocabAstBridgeError> {
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
pub(super) fn internal_race_for_to_public(
    race: &ast::RaceForExpr,
) -> Result<incan_vocab::IncanRaceForExpr, VocabAstBridgeError> {
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
        binding: race.binding.node.clone(),
        arms,
    })
}
