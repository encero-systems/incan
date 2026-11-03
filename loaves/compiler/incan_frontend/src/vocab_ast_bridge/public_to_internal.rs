//! Conversion of public vocab declarations, decorators, statements and expressions into the internal AST.

use super::*;

/// Convert public vocabulary declarations into internal compilation-unit declarations under one source anchor.
pub fn public_declarations_to_internal_with_anchor(
    declarations: &[incan_vocab::IncanDeclaration],
    anchor: ast::Span,
) -> Result<Vec<ast::Spanned<ast::Declaration>>, VocabAstBridgeError> {
    let mut spans = SyntheticSpanAllocator::new(anchor);
    declarations
        .iter()
        .map(|declaration| {
            let internal = public_declaration_to_internal_with_spans(declaration, &mut spans)?;
            Ok(ast::Spanned::new(internal, spans.next()))
        })
        .collect()
}

/// Convert one public vocabulary declaration into the ordinary compiler declaration AST.
pub(super) fn public_declaration_to_internal_with_spans(
    declaration: &incan_vocab::IncanDeclaration,
    spans: &mut SyntheticSpanAllocator,
) -> Result<ast::Declaration, VocabAstBridgeError> {
    match declaration {
        incan_vocab::IncanDeclaration::FromImport(import) => {
            if import.module.is_empty() || import.module.iter().any(|segment| segment.is_empty()) {
                return Err(VocabAstBridgeError::UnsupportedPublicDeclaration(
                    "public vocabulary import must name a non-empty module path",
                ));
            }
            if import.items.is_empty() || import.items.iter().any(|item| item.name.is_empty()) {
                return Err(VocabAstBridgeError::UnsupportedPublicDeclaration(
                    "public vocabulary import must name at least one non-empty item",
                ));
            }
            Ok(ast::Declaration::Import(ast::ImportDecl {
                visibility: public_visibility_to_internal(import.visibility),
                kind: ast::ImportKind::From {
                    module: ast::ImportPath::simple(import.module.clone()),
                    items: import
                        .items
                        .iter()
                        .map(|item| ast::ImportItem {
                            name: item.name.clone(),
                            alias: item.alias.clone(),
                        })
                        .collect(),
                },
                alias: None,
            }))
        }
        incan_vocab::IncanDeclaration::Class(class) => {
            if class.name.is_empty() {
                return Err(VocabAstBridgeError::UnsupportedPublicDeclaration(
                    "public vocabulary class must have a name",
                ));
            }
            Ok(ast::Declaration::Class(ast::ClassDecl {
                visibility: public_visibility_to_internal(class.visibility),
                decorators: class
                    .decorators
                    .iter()
                    .map(|decorator| public_decorator_to_internal_with_spans(decorator, spans))
                    .collect::<Result<Vec<_>, _>>()?,
                name: class.name.clone(),
                type_params: Vec::new(),
                extends: class.extends.clone(),
                traits: Vec::new(),
                docstring: None,
                fields: Vec::new(),
                method_aliases: Vec::new(),
                method_partials: Vec::new(),
                properties: Vec::new(),
                methods: Vec::new(),
                declarative_members: class.declarative_members.clone(),
            }))
        }
        _ => Err(VocabAstBridgeError::UnsupportedPublicDeclaration(
            "declaration form is not yet supported by the internal AST bridge",
        )),
    }
}

/// Convert public vocabulary visibility into the compiler's ordinary declaration visibility.
pub(super) fn public_visibility_to_internal(visibility: incan_vocab::IncanVisibility) -> ast::Visibility {
    match visibility {
        incan_vocab::IncanVisibility::Private => ast::Visibility::Private,
        incan_vocab::IncanVisibility::Public => ast::Visibility::Public,
        _ => ast::Visibility::Private,
    }
}

/// Convert a public vocabulary decorator into the ordinary compiler decorator AST.
pub(super) fn public_decorator_to_internal_with_spans(
    decorator: &incan_vocab::Decorator,
    spans: &mut SyntheticSpanAllocator,
) -> Result<ast::Spanned<ast::Decorator>, VocabAstBridgeError> {
    let name = decorator
        .path
        .last()
        .cloned()
        .ok_or(VocabAstBridgeError::UnsupportedPublicDeclaration(
            "decorator path must contain at least one segment",
        ))?;
    let args = decorator
        .args
        .iter()
        .map(|arg| {
            let value = match &arg.value {
                incan_vocab::DecoratorArgValue::Str(value) => ast::Expr::Literal(ast::Literal::String(value.clone())),
                incan_vocab::DecoratorArgValue::Int(value) => {
                    ast::Expr::Literal(ast::Literal::Int(ast::IntLiteral::synthetic(*value)))
                }
                incan_vocab::DecoratorArgValue::Bool(value) => ast::Expr::Literal(ast::Literal::Bool(*value)),
                incan_vocab::DecoratorArgValue::Expr(value) => public_expr_to_internal_with_spans(value, spans)?,
                _ => {
                    return Err(VocabAstBridgeError::UnsupportedPublicDeclaration(
                        "decorator argument form is not yet supported by the internal AST bridge",
                    ));
                }
            };
            let value = ast::Spanned::new(value, spans.next());
            Ok(match &arg.name {
                Some(name) => ast::DecoratorArg::Named(name.clone(), ast::DecoratorArgValue::Expr(value)),
                None => ast::DecoratorArg::Positional(value),
            })
        })
        .collect::<Result<Vec<_>, VocabAstBridgeError>>()?;

    Ok(ast::Spanned::new(
        ast::Decorator {
            path: ast::ImportPath::simple(decorator.path.clone()),
            name,
            type_args: Vec::new(),
            is_call: !decorator.args.is_empty(),
            args,
        },
        public_decorator_span_or_synthetic(decorator, spans),
    ))
}

/// Preserve an explicit vocabulary decorator source span while retaining unique synthetic spans for generated helpers.
///
/// A companion desugarer may intentionally attach the source declaration span to an ordinary decorator that carries
/// semantic diagnostics. When it does, downstream checked descriptors must retain that user-facing anchor instead of
/// exposing an implementation-only synthetic span. Default spans still receive a unique synthetic identity so they
/// cannot collide with other generated nodes in typechecker metadata.
pub(super) fn public_decorator_span_or_synthetic(
    decorator: &incan_vocab::Decorator,
    spans: &mut SyntheticSpanAllocator,
) -> ast::Span {
    if decorator.span == incan_vocab::Span::default() {
        return spans.next();
    }
    ast::Span::new(decorator.span.start, decorator.span.end.max(decorator.span.start))
        .with_expansion(spans.next().expansion)
}

/// Convert public statements while sharing one synthetic span allocator across the whole generated subtree.
pub(super) fn public_statements_to_internal_with_spans(
    stmts: &[incan_vocab::IncanStatement],
    spans: &mut SyntheticSpanAllocator,
) -> Result<Vec<ast::Spanned<ast::Statement>>, VocabAstBridgeError> {
    stmts
        .iter()
        .map(|stmt| {
            let internal = public_statement_to_internal_with_spans(stmt, spans)?;
            Ok(ast::Spanned::new(internal, spans.next()))
        })
        .collect()
}

/// Convert one public statement with a caller-owned synthetic span allocator.
pub(super) fn public_statement_to_internal_with_spans(
    stmt: &incan_vocab::IncanStatement,
    spans: &mut SyntheticSpanAllocator,
) -> Result<ast::Statement, VocabAstBridgeError> {
    match stmt {
        incan_vocab::IncanStatement::Pass => Ok(ast::Statement::Pass),
        incan_vocab::IncanStatement::Expr(expr) => Ok(ast::Statement::Expr(ast::Spanned::new(
            public_expr_to_internal_with_spans(expr, spans)?,
            spans.next(),
        ))),
        incan_vocab::IncanStatement::Return(value) => Ok(ast::Statement::Return(
            value
                .as_ref()
                .map(|expr| {
                    public_expr_to_internal_with_spans(expr, spans).map(|node| ast::Spanned::new(node, spans.next()))
                })
                .transpose()?,
        )),
        incan_vocab::IncanStatement::Assign { target, value } => Ok(ast::Statement::Assignment(ast::AssignmentStmt {
            binding: ast::BindingKind::Reassign,
            name: target.clone(),
            name_span: spans.next(),
            ty: None,
            value: ast::Spanned::new(public_expr_to_internal_with_spans(value, spans)?, spans.next()),
        })),
        incan_vocab::IncanStatement::Let { name, mutable, value } => {
            Ok(ast::Statement::Assignment(ast::AssignmentStmt {
                binding: if *mutable {
                    ast::BindingKind::Mutable
                } else {
                    ast::BindingKind::Let
                },
                name: name.clone(),
                name_span: spans.next(),
                ty: None,
                value: ast::Spanned::new(public_expr_to_internal_with_spans(value, spans)?, spans.next()),
            }))
        }
        incan_vocab::IncanStatement::If {
            condition,
            then_body,
            else_body,
        } => Ok(ast::Statement::If(ast::IfStmt {
            condition: ast::Condition::Expr(ast::Spanned::new(
                public_expr_to_internal_with_spans(condition, spans)?,
                spans.next(),
            )),
            then_body: public_statements_to_internal_with_spans(then_body, spans)?,
            elif_branches: Vec::new(),
            else_body: if else_body.is_empty() {
                None
            } else {
                Some(public_statements_to_internal_with_spans(else_body, spans)?)
            },
        })),
        incan_vocab::IncanStatement::While { condition, body } => Ok(ast::Statement::While(ast::WhileStmt {
            condition: ast::Condition::Expr(ast::Spanned::new(
                public_expr_to_internal_with_spans(condition, spans)?,
                spans.next(),
            )),
            body: public_statements_to_internal_with_spans(body, spans)?,
        })),
        incan_vocab::IncanStatement::For { binding, iter, body } => Ok(ast::Statement::For(ast::ForStmt {
            pattern: ast::Spanned::new(ast::Pattern::Binding(binding.clone()), spans.next()),
            iter: ast::Spanned::new(public_expr_to_internal_with_spans(iter, spans)?, spans.next()),
            body: public_statements_to_internal_with_spans(body, spans)?,
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

/// Convert one public `incan_vocab::IncanExpr` to internal compiler expression AST.
///
/// This is the inverse of [`internal_expr_to_public`] for the bridgeable subset of the public vocab expression model.
/// Special query helpers such as `CurrentField` and `RelationField` are lowered through internal field-access shapes
/// using a sentinel identifier when necessary.
///
/// # Errors
///
/// Returns [`VocabAstBridgeError::UnsupportedPublicExpression`] for unsupported public expression kinds or operators.
pub fn public_expr_to_internal(expr: &incan_vocab::IncanExpr) -> Result<ast::Expr, VocabAstBridgeError> {
    let mut spans = SyntheticSpanAllocator::new(ast::Span::default());
    public_expr_to_internal_with_spans(expr, &mut spans)
}

/// Convert one public expression into internal AST while assigning synthetic spans rooted at `anchor`.
pub fn public_expr_to_internal_with_anchor(
    expr: &incan_vocab::IncanExpr,
    anchor: ast::Span,
) -> Result<ast::Expr, VocabAstBridgeError> {
    let mut spans = SyntheticSpanAllocator::new(anchor);
    public_expr_to_internal_with_spans(expr, &mut spans)
}

/// Convert one public expression with a caller-owned synthetic span allocator.
pub(super) fn public_expr_to_internal_with_spans(
    expr: &incan_vocab::IncanExpr,
    spans: &mut SyntheticSpanAllocator,
) -> Result<ast::Expr, VocabAstBridgeError> {
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
                spans.next(),
            )),
            field.clone(),
        )),
        incan_vocab::IncanExpr::RelationField { relation, field } => Ok(ast::Expr::Field(
            Box::new(ast::Spanned::new(ast::Expr::Ident(relation.clone()), spans.next())),
            field.clone(),
        )),
        incan_vocab::IncanExpr::Tuple(values) => values
            .iter()
            .map(|value| {
                public_expr_to_internal_with_spans(value, spans).map(|node| ast::Spanned::new(node, spans.next()))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ast::Expr::Tuple),
        incan_vocab::IncanExpr::List(values) => values
            .iter()
            .map(|value| {
                public_expr_to_internal_with_spans(value, spans)
                    .map(|node| ast::ListEntry::Element(ast::Spanned::new(node, spans.next())))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(ast::Expr::List),
        incan_vocab::IncanExpr::Dict(entries) => {
            let mut mapped = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                mapped.push(ast::DictEntry::Pair(
                    ast::Spanned::new(public_expr_to_internal_with_spans(key, spans)?, spans.next()),
                    ast::Spanned::new(public_expr_to_internal_with_spans(value, spans)?, spans.next()),
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
            Box::new(ast::Spanned::new(
                public_expr_to_internal_with_spans(value, spans)?,
                spans.next(),
            )),
        )),
        incan_vocab::IncanExpr::Binary(left, op, right) => Ok(ast::Expr::Binary(
            Box::new(ast::Spanned::new(
                public_expr_to_internal_with_spans(left, spans)?,
                spans.next(),
            )),
            map_public_binary_op(*op)?,
            Box::new(ast::Spanned::new(
                public_expr_to_internal_with_spans(right, spans)?,
                spans.next(),
            )),
        )),
        incan_vocab::IncanExpr::Call { callee, args } => {
            let mapped = args
                .iter()
                .map(|arg| {
                    public_expr_to_internal_with_spans(arg, spans)
                        .map(|node| ast::CallArg::Positional(ast::Spanned::new(node, spans.next())))
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let incan_vocab::IncanExpr::Field { object, field } = callee.as_ref() {
                return Ok(ast::Expr::MethodCall(
                    Box::new(ast::Spanned::new(
                        public_expr_to_internal_with_spans(object, spans)?,
                        spans.next(),
                    )),
                    field.clone(),
                    Vec::new(),
                    mapped,
                ));
            }
            Ok(ast::Expr::Call(
                Box::new(ast::Spanned::new(
                    public_expr_to_internal_with_spans(callee, spans)?,
                    spans.next(),
                )),
                Vec::new(),
                mapped,
            ))
        }
        incan_vocab::IncanExpr::Field { object, field } => Ok(ast::Expr::Field(
            Box::new(ast::Spanned::new(
                public_expr_to_internal_with_spans(object, spans)?,
                spans.next(),
            )),
            field.clone(),
        )),
        incan_vocab::IncanExpr::RaceFor(race) => public_race_for_to_internal(race, spans),
        incan_vocab::IncanExpr::ScopedSurface(surface) => public_scoped_surface_expr_to_internal(surface, spans),
        incan_vocab::IncanExpr::ScopedSymbolCall(call) => public_scoped_symbol_call_to_internal(call, spans),
        _ => Err(VocabAstBridgeError::UnsupportedPublicExpression(
            "expression form is not yet supported by internal AST bridge",
        )),
    }
}

/// Convert one public race expression back into the compiler AST.
pub(super) fn public_race_for_to_internal(
    race: &incan_vocab::IncanRaceForExpr,
    spans: &mut SyntheticSpanAllocator,
) -> Result<ast::Expr, VocabAstBridgeError> {
    let arms = race
        .arms
        .iter()
        .map(|arm| {
            let body = match &arm.body {
                incan_vocab::IncanRaceForBody::Expr(expr) => ast::RaceForBody::Expr(ast::Spanned::new(
                    public_expr_to_internal_with_spans(expr, spans)?,
                    spans.next(),
                )),
                incan_vocab::IncanRaceForBody::Block(statements) => {
                    ast::RaceForBody::Block(public_statements_to_internal_with_spans(statements, spans)?)
                }
                _ => {
                    return Err(VocabAstBridgeError::UnsupportedPublicExpression(
                        "race arm body form is not supported by internal AST bridge",
                    ));
                }
            };
            Ok(ast::RaceForArm {
                awaitable: ast::Spanned::new(public_expr_to_internal_with_spans(&arm.awaitable, spans)?, spans.next()),
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
            binding: ast::Spanned::new(race.binding.clone(), spans.next()),
            arms,
        })),
    })))
}

/// Convert a public scoped-surface expression back into the compiler AST.
pub(super) fn public_scoped_surface_expr_to_internal(
    surface: &incan_vocab::IncanScopedSurfaceExpr,
    spans: &mut SyntheticSpanAllocator,
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
            left: Box::new(ast::Spanned::new(
                public_expr_to_internal_with_spans(left, spans)?,
                spans.next(),
            )),
            right: Box::new(ast::Spanned::new(
                public_expr_to_internal_with_spans(right, spans)?,
                spans.next(),
            )),
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
pub(super) fn public_scoped_symbol_call_to_internal(
    call: &incan_vocab::IncanScopedSymbolCall,
    spans: &mut SyntheticSpanAllocator,
) -> Result<ast::Expr, VocabAstBridgeError> {
    let key = incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
        dependency_key: call.dependency_key.clone(),
        descriptor_key: call.descriptor_key.clone(),
    };
    let args = call
        .args
        .iter()
        .map(|arg| {
            public_expr_to_internal_with_spans(arg, spans)
                .map(|node| ast::CallArg::Positional(ast::Spanned::new(node, spans.next())))
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
pub(super) fn public_decorator_from_internal(
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
pub(super) fn public_decorator_arg_value_from_internal_expr(
    expr: &ast::Expr,
) -> Result<incan_vocab::DecoratorArgValue, VocabAstBridgeError> {
    match expr {
        ast::Expr::Literal(ast::Literal::String(value)) => Ok(incan_vocab::DecoratorArgValue::Str(value.clone())),
        ast::Expr::Literal(ast::Literal::Int(il)) => Ok(incan_vocab::DecoratorArgValue::Int(il.value)),
        ast::Expr::Literal(ast::Literal::Bool(value)) => Ok(incan_vocab::DecoratorArgValue::Bool(*value)),
        _ => Ok(incan_vocab::DecoratorArgValue::Expr(internal_expr_to_public(expr)?)),
    }
}
