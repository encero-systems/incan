use super::*;

fn default_keyword_binding(surface_kind: incan_vocab::KeywordSurfaceKind) -> ast::VocabKeywordBinding {
    ast::VocabKeywordBinding {
        is_declaration_owned_clause: false,
        dependency_key: "demo".to_string(),
        activation_namespace: "demo.dsl".to_string(),
        surface_kind,
        compound_tokens: Vec::new(),
        placement: incan_vocab::KeywordPlacement::TopLevel,
        clause_body_kind: None,
    }
}

fn expression_list_clause_binding() -> ast::VocabKeywordBinding {
    ast::VocabKeywordBinding {
        is_declaration_owned_clause: true,
        surface_kind: incan_vocab::KeywordSurfaceKind::BlockContextKeyword,
        placement: incan_vocab::KeywordPlacement::in_block(["query"]),
        clause_body_kind: Some(incan_vocab::ClauseBodyKind::ExpressionList),
        ..default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockContextKeyword)
    }
}

#[test]
fn bridges_block_context_keywords_as_clauses() -> Result<(), Box<dyn std::error::Error>> {
    let clause_block = ast::VocabBlockStmt {
        keyword: "FROM".to_string(),
        keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockContextKeyword),
        decorators: Vec::new(),
        signature_head: None,
        header_args: vec![ast::Spanned::new(
            ast::Expr::Ident("orders".to_string()),
            ast::Span::default(),
        )],
        body: Vec::new(),
        body_item_trailing_commas: Vec::new(),
    };
    let declaration_block = ast::VocabBlockStmt {
        keyword: "query".to_string(),
        keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration),
        decorators: Vec::new(),
        signature_head: None,
        header_args: Vec::new(),
        body: vec![ast::Spanned::new(
            ast::Statement::VocabBlock(clause_block),
            ast::Span::default(),
        )],
        body_item_trailing_commas: vec![false],
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
fn bridges_owned_clause_even_when_compatibility_metadata_uses_block_declaration()
-> Result<(), Box<dyn std::error::Error>> {
    let clause_block = ast::VocabBlockStmt {
        keyword: "FROM".to_string(),
        keyword_binding: ast::VocabKeywordBinding {
            is_declaration_owned_clause: true,
            placement: incan_vocab::KeywordPlacement::in_block(["query"]),
            ..default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration)
        },
        decorators: Vec::new(),
        signature_head: None,
        header_args: Vec::new(),
        body: vec![ast::Spanned::new(
            ast::Statement::Expr(ast::Spanned::new(
                ast::Expr::Ident("orders".to_string()),
                ast::Span::default(),
            )),
            ast::Span::default(),
        )],
        body_item_trailing_commas: vec![false],
    };
    let window_clause_block = ast::VocabBlockStmt {
        keyword: "WINDOW".to_string(),
        keyword_binding: ast::VocabKeywordBinding {
            is_declaration_owned_clause: true,
            placement: incan_vocab::KeywordPlacement::in_block(["query"]),
            ..default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration)
        },
        decorators: Vec::new(),
        signature_head: None,
        header_args: Vec::new(),
        body: Vec::new(),
        body_item_trailing_commas: Vec::new(),
    };
    let declaration_block = ast::VocabBlockStmt {
        keyword: "query".to_string(),
        keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration),
        decorators: Vec::new(),
        signature_head: None,
        header_args: Vec::new(),
        body: vec![
            ast::Spanned::new(ast::Statement::VocabBlock(clause_block), ast::Span::default()),
            ast::Spanned::new(
                ast::Statement::Expr(ast::Spanned::new(
                    ast::Expr::Ident("retained".to_string()),
                    ast::Span::default(),
                )),
                ast::Span::default(),
            ),
            ast::Spanned::new(ast::Statement::VocabBlock(window_clause_block), ast::Span::default()),
        ],
        body_item_trailing_commas: vec![false, false, false],
    };

    let bridged = internal_vocab_block_to_public(&declaration_block, ast::Span::default())?;
    let [
        incan_vocab::VocabBodyItem::Clause(clause),
        incan_vocab::VocabBodyItem::Statement(_),
        incan_vocab::VocabBodyItem::Clause(window),
    ] = bridged.body.as_slice()
    else {
        return Err(format!("expected an owned clause, got {:?}", bridged.body).into());
    };
    assert_eq!(clause.keyword, "FROM");
    assert_eq!(
        clause.body,
        incan_vocab::VocabClauseBody::Expression(incan_vocab::IncanExpr::Name("orders".to_string()))
    );
    assert_eq!(window.keyword, "WINDOW");
    Ok(())
}

#[test]
fn bridges_compound_declaration_tokens() -> Result<(), Box<dyn std::error::Error>> {
    let mut keyword_binding = default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration);
    keyword_binding.compound_tokens = vec!["AGAINST".to_string()];
    let block = ast::VocabBlockStmt {
        keyword: "MATCH".to_string(),
        keyword_binding,
        decorators: Vec::new(),
        signature_head: None,
        header_args: Vec::new(),
        body: Vec::new(),
        body_item_trailing_commas: Vec::new(),
    };

    let bridged = internal_vocab_block_to_public(&block, ast::Span::default())?;
    assert_eq!(bridged.keyword, "MATCH");
    assert_eq!(bridged.compound_tokens, vec!["AGAINST".to_string()]);
    Ok(())
}

#[test]
fn bridges_public_field_callee_calls_back_to_method_calls() -> Result<(), Box<dyn std::error::Error>> {
    let expr = incan_vocab::IncanExpr::Call {
        callee: Box::new(incan_vocab::IncanExpr::Field {
            object: Box::new(incan_vocab::IncanExpr::Name("orders".to_string())),
            field: "select".to_string(),
        }),
        args: vec![incan_vocab::IncanExpr::Name("amount".to_string())],
    };

    let internal = public_expr_to_internal(&expr)?;
    let ast::Expr::MethodCall(receiver, method, _, args) = internal else {
        return Err(format!("expected public field-callee call to bridge as method call, got {internal:?}").into());
    };
    assert_eq!(method, "select");
    assert!(matches!(receiver.node, ast::Expr::Ident(ref name) if name == "orders"));
    assert_eq!(args.len(), 1);
    Ok(())
}

#[test]
fn public_expression_anchor_assigns_distinct_spans_to_nested_calls() -> Result<(), Box<dyn std::error::Error>> {
    let expr = incan_vocab::IncanExpr::Call {
        callee: Box::new(incan_vocab::IncanExpr::Name("outer".to_string())),
        args: vec![incan_vocab::IncanExpr::Call {
            callee: Box::new(incan_vocab::IncanExpr::Name("inner".to_string())),
            args: vec![incan_vocab::IncanExpr::Int(5)],
        }],
    };

    let internal = public_expr_to_internal_with_anchor(&expr, ast::Span::new(10, 20))?;
    let ast::Expr::Call(_, _, args) = internal else {
        return Err(format!("expected outer call, got {internal:?}").into());
    };
    let ast::CallArg::Positional(inner) = &args[0] else {
        return Err(format!("expected positional inner call, got {:?}", args[0]).into());
    };
    let ast::Expr::Call(_, _, inner_args) = &inner.node else {
        return Err(format!("expected nested call, got {:?}", inner.node).into());
    };
    let ast::CallArg::Positional(value) = &inner_args[0] else {
        return Err(format!("expected positional literal, got {:?}", inner_args[0]).into());
    };

    assert_ne!(inner.span, value.span);
    assert_ne!(inner.span, ast::Span::default());
    assert_ne!(value.span, ast::Span::default());
    Ok(())
}

#[test]
fn bridges_expression_list_clause_alias_items() -> Result<(), Box<dyn std::error::Error>> {
    let clause_block = ast::VocabBlockStmt {
        keyword: "SELECT".to_string(),
        keyword_binding: expression_list_clause_binding(),
        decorators: Vec::new(),
        signature_head: None,
        header_args: Vec::new(),
        body: vec![
            ast::Spanned::new(
                ast::Statement::VocabExpressionItem(ast::VocabExpressionItemStmt {
                    expr: ast::Spanned::new(ast::Expr::Ident("amount".to_string()), ast::Span::new(10, 16)),
                    alias: Some("total".to_string()),
                    modifiers: vec![ast::VocabExpressionItemModifierStmt {
                        keyword: "for".to_string(),
                        value: ast::Spanned::new(ast::Expr::Ident("customer".to_string()), ast::Span::new(30, 38)),
                        span: ast::Span::new(26, 38),
                    }],
                }),
                ast::Span::new(10, 38),
            ),
            ast::Spanned::new(
                ast::Statement::Expr(ast::Spanned::new(
                    ast::Expr::Ident("region".to_string()),
                    ast::Span::new(30, 36),
                )),
                ast::Span::new(30, 36),
            ),
        ],
        body_item_trailing_commas: vec![false, false],
    };

    let clause = internal_vocab_clause_to_public(&clause_block, ast::Span::default())?;
    let incan_vocab::VocabClauseBody::ExpressionList(items) = clause.body else {
        return Err(format!("expected expression-list body, got {:?}", clause.body).into());
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].alias.as_deref(), Some("total"));
    assert_eq!(items[0].modifiers.len(), 1);
    assert_eq!(items[0].modifiers[0].keyword, "for");
    assert_eq!(
        items[0].modifiers[0].value,
        incan_vocab::IncanExpr::Name("customer".to_string())
    );
    assert_eq!(items[0].expr, incan_vocab::IncanExpr::Name("amount".to_string()));
    assert_eq!(items[1].alias, None);
    assert!(items[1].modifiers.is_empty());
    assert_eq!(items[1].expr, incan_vocab::IncanExpr::Name("region".to_string()));
    Ok(())
}

#[test]
fn infers_declaration_head_name_from_first_identifier_arg() -> Result<(), Box<dyn std::error::Error>> {
    let block = ast::VocabBlockStmt {
        keyword: "workflow".to_string(),
        keyword_binding: default_keyword_binding(incan_vocab::KeywordSurfaceKind::BlockDeclaration),
        decorators: Vec::new(),
        signature_head: None,
        header_args: vec![
            ast::Spanned::new(ast::Expr::Ident("daily".to_string()), ast::Span::default()),
            ast::Spanned::new(
                ast::Expr::Literal(ast::Literal::String("reports".to_string())),
                ast::Span::default(),
            ),
        ],
        body: Vec::new(),
        body_item_trailing_commas: Vec::new(),
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

/// Operators retain public structure while generated operands receive stable, distinct provenance.
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
        let round_trip = public_expression_to_internal(&public)?;
        assert_eq!(internal_expr_to_public(&round_trip)?, public);
        assert_eq!(round_trip, public_expression_to_internal(&public)?);
        assert!(matches!(
            &round_trip,
            ast::Expr::Binary(left, _, right)
                if left.span.expansion.is_some() && right.span.expansion.is_some()
                    && left.span.expansion != right.span.expansion
        ));
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
    let round_trip = public_expression_to_internal(&public)?;
    assert_eq!(internal_expr_to_public(&round_trip)?, public);
    assert!(matches!(&round_trip, ast::Expr::Unary(_, value) if value.span.expansion.is_some()));
    Ok(())
}

/// Scoped expressions retain their public structure and generated call arguments gain stable provenance.
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
    assert_public_expression_round_trip(&public)?;

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
    assert_eq!(internal_expr_to_public(&round_trip)?, public);
    assert_eq!(round_trip, public_expression_to_internal(&public)?);
    assert!(matches!(
        &round_trip,
        ast::Expr::Surface(surface)
            if matches!(&surface.payload, ast::SurfaceExprPayload::ScopedSymbolCall { args, .. }
                if args.iter().all(|argument| matches!(argument,
                    ast::CallArg::Positional(value) if value.span.expansion.is_some())))
    ));
    Ok(())
}

/// Async surface structure survives bridging and each generated binding retains expansion provenance.
#[test]
fn bridges_race_for_expression_artifacts_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let race = ast::Expr::Surface(Box::new(ast::SurfaceExpr {
        key: incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
            dependency_key: "std.async".to_string(),
            descriptor_key: "race_for".to_string(),
        },
        payload: ast::SurfaceExprPayload::RaceFor(Box::new(ast::RaceForExpr {
            binding: ast::Spanned::new("value".to_string(), ast::Span::default()),
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
    let round_trip = public_expression_to_internal(&public)?;
    assert_eq!(internal_expr_to_public(&round_trip)?, public);
    assert_eq!(round_trip, public_expression_to_internal(&public)?);
    assert!(matches!(
        &round_trip,
        ast::Expr::Surface(surface)
            if matches!(&surface.payload, ast::SurfaceExprPayload::RaceFor(race)
                if race.binding.span.expansion.is_some()
                    && race.arms.iter().all(|arm| arm.awaitable.span.expansion.is_some()))
    ));
    Ok(())
}

/// Public DTOs omit spans; import assigns deterministic expansion identities rather than restoring empty spans.
fn assert_public_expression_round_trip(public: &incan_vocab::IncanExpr) -> Result<(), Box<dyn std::error::Error>> {
    let restored = public_expression_to_internal(public)?;
    assert_eq!(&internal_expr_to_public(&restored)?, public);
    assert_eq!(restored, public_expression_to_internal(public)?);
    Ok(())
}
