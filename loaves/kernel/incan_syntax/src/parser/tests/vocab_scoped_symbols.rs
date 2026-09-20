//! Imported-vocab scoped symbols: scoped leading-dot paths inside and outside vocab blocks, scoped glyphs and symbols
//! against core operators and calls, same-depth ambiguity, innermost-scope preference, descriptor diagnostics,
//! multi-token glyphs and the RFC 040 product probes.

#[test]
fn test_imported_vocab_block_accepts_scoped_leading_dot_path() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    .order.amount\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "analytics".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "analytics.query".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "query".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_declaration_body("query")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
        return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
        return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
    };
    let crate::ast::Expr::Surface(surface_expr) = &expr.node else {
        return Err(format!("expected surface expression, got {:?}", expr.node).into());
    };
    assert!(matches!(
        &surface_expr.key,
        incan_semantics_core::SurfaceFeatureKey::ScopedDslSurface {
            dependency_key,
            descriptor_key,
        } if dependency_key == "analytics" && descriptor_key == "query.field"
    ));
    assert!(matches!(
        &surface_expr.payload,
        crate::ast::SurfaceExprPayload::LeadingDotPath { segments, owner, .. }
            if segments == &["order".to_string(), "amount".to_string()]
                && owner.declaration == "query"
    ));
    Ok(())
}

#[test]
fn test_scoped_leading_dot_path_still_rejects_outside_vocab_block() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  .amount\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let keyword_map = std::collections::HashMap::new();
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_declaration_body("query")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration)
                        .with_misuse_scope(incan_vocab::ScopedSurfaceMisuseScope::ActivatingFile)
                        .with_diagnostic(
                            incan_vocab::ScopedSurfaceDiagnosticTemplate::new(
                                "query-field-outside-scope",
                                incan_vocab::ScopedSurfaceDiagnosticKind::OutsideScope,
                                "query field shorthand is only valid inside query blocks",
                            )
                            .with_help("move this expression into a `query:` block"),
                        ),
                ),
        ],
    );

    let result = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
    let errors = result.expect_err("leading-dot path should remain invalid outside the owning vocab block");
    let first_error = errors.first().expect("expected at least one leading-dot diagnostic");
    assert!(
        first_error
            .message
            .contains("query field shorthand is only valid inside query blocks"),
        "expected author-provided diagnostic, got {:?}",
        errors
    );
    assert!(
        first_error
            .hints
            .iter()
            .any(|hint| hint.contains("move this expression")),
        "expected author-provided help, got {:?}",
        errors
    );
    Ok(())
}

#[test]
fn test_imported_vocab_block_prefers_scoped_glyph_over_core_binary() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::pipeline\n\ndef configure() -> None:\n  flow:\n    extract + normalize\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "pipeline".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "pipeline.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "flow".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "pipeline".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("pipeline.dsl")
                .with_declaration(incan_vocab::DeclarationSurface::named("flow"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::operator("flow.link", "+")
                        .in_declaration_body("flow")
                        .pairwise_chain(),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
        return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
        return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
    };
    assert!(matches!(
        &expr.node,
        crate::ast::Expr::Surface(surface)
            if matches!(
                &surface.payload,
                crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                    if glyph == "+" && owner.declaration == "flow"
            )
    ));
    Ok(())
}

#[test]
fn test_imported_vocab_block_prefers_scoped_symbol_over_core_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "analytics".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "analytics.query".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "query".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                        .in_declaration_body("query"),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
        return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
        return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
    };
    assert!(matches!(
        &expr.node,
        crate::ast::Expr::Surface(surface)
            if matches!(
                &surface.payload,
                crate::ast::SurfaceExprPayload::ScopedSymbolCall { symbol, args, owner }
                    if symbol == "sum" && args.len() == 1 && owner.declaration == "query"
            )
    ));
    Ok(())
}

#[test]
fn test_scoped_symbol_descriptor_does_not_change_call_outside_vocab_block() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  sum(amount)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
    let keyword_map = std::collections::HashMap::new();
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum").in_declaration_body("query"),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    assert!(matches!(
        &function.body[0].node,
        crate::ast::Statement::Expr(expr)
            if matches!(&expr.node, crate::ast::Expr::Call(callee, _, _)
                if matches!(&callee.node, crate::ast::Expr::Ident(name) if name == "sum"))
    ));
    Ok(())
}

#[test]
fn test_same_depth_scoped_symbol_ambiguity_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\nimport pub::metrics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "analytics".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "analytics.query".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::block("query")],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum.analytics", "sum")
                        .in_declaration_body("query"),
                ),
        ],
    );
    surface_map.insert(
        "metrics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("metrics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum.metrics", "sum")
                        .in_declaration_body("query"),
                ),
        ],
    );

    let result = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
    let errors = result.expect_err("same-depth scoped symbol collision should be ambiguous");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("Ambiguous scoped symbol `sum`")),
        "expected ambiguous scoped symbol diagnostic, got {:?}",
        errors
    );
    Ok(())
}

#[test]
fn test_nested_vocab_block_prefers_innermost_scoped_symbol() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    stage:\n      sum(amount)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "analytics".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "analytics.query".to_string(),
            },
            keywords: vec![
                incan_vocab::KeywordSpec::block("query"),
                incan_vocab::KeywordSpec::sub_block("stage", "query"),
            ],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_declaration(incan_vocab::DeclarationSurface::named("stage").in_block("query"))
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum").in_declaration_body("query"),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("stage.sum", "sum").in_declaration_body("stage"),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(query_block) = &function.body[0].node else {
        return Err(format!("expected query block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::VocabBlock(stage_block) = &query_block.body[0].node else {
        return Err(format!("expected stage block, got {:?}", query_block.body[0].node).into());
    };
    let crate::ast::Statement::Expr(expr) = &stage_block.body[0].node else {
        return Err(format!("expected expression statement, got {:?}", stage_block.body[0].node).into());
    };
    assert!(matches!(
        &expr.node,
        crate::ast::Expr::Surface(surface)
            if matches!(
                &surface.payload,
                crate::ast::SurfaceExprPayload::ScopedSymbolCall { symbol, owner, .. }
                    if symbol == "sum" && owner.declaration == "stage"
            )
    ));
    Ok(())
}

#[test]
fn test_active_scoped_symbol_misuse_uses_descriptor_diagnostic() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    sum(amount)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "analytics".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "analytics.query".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::block("query")],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "analytics".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("analytics.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .in_clause_body("query", "SELECT")
                        .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                        .with_diagnostic(
                            incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                                "query-sum-outside-select",
                                incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                                "query aggregate `sum` is only valid inside SELECT clauses",
                            )
                            .with_help("move `sum(...)` into a SELECT clause"),
                        ),
                ),
        ],
    );

    let result = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
    let errors = result.expect_err("active scoped symbol misuse should use descriptor diagnostic");
    let first_error = errors.first().expect("expected at least one scoped-symbol diagnostic");
    assert!(
        first_error
            .message
            .contains("query aggregate `sum` is only valid inside SELECT clauses"),
        "expected author-provided scoped-symbol diagnostic, got {:?}",
        errors
    );
    assert!(
        first_error.hints.iter().any(|hint| hint.contains("move `sum(...)`")),
        "expected author-provided scoped-symbol help, got {:?}",
        errors
    );
    Ok(())
}

#[test]
fn test_imported_vocab_block_accepts_multitoken_pipe_glyph() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::querykit\n\ndef configure() -> None:\n  query:\n    orders |> paid_orders\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "querykit".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "querykit".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "query".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "querykit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("querykit")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                        .in_declaration_body("query")
                        .pairwise_chain(),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
        return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
        return Err(format!("expected expression statement, got {:?}", block.body[0].node).into());
    };
    assert!(matches!(
        &expr.node,
        crate::ast::Expr::Surface(surface)
            if matches!(
                &surface.payload,
                crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                    if glyph == "|>" && owner.declaration == "query"
            )
    ));
    Ok(())
}

#[test]
fn test_scoped_glyph_descriptor_does_not_change_core_binary_outside_vocab_block()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::pipeline\n\ndef configure() -> None:\n  extract + normalize\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
    let keyword_map = std::collections::HashMap::new();
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "pipeline".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("pipeline.dsl")
                .with_declaration(incan_vocab::DeclarationSurface::named("flow"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::operator("flow.link", "+")
                        .in_declaration_body("flow")
                        .pairwise_chain(),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    assert!(matches!(
        &function.body[0].node,
        crate::ast::Statement::Expr(expr)
            if matches!(&expr.node, crate::ast::Expr::Binary(_, crate::ast::BinaryOp::Add, _))
    ));
    Ok(())
}

#[test]
fn rfc040_product_probe_query_method_arguments_accept_leading_dot_fields() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::querykit\n\ndef configure(orders: Any) -> None:\n  orders.filter(.amount > 100).select(.customer_id)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let keyword_map = std::collections::HashMap::new();
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "querykit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("querykit")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .with_eligibilities([
                            incan_vocab::ScopedSurfaceEligibility::call_argument("query", "filter"),
                            incan_vocab::ScopedSurfaceEligibility::call_argument("query", "select"),
                        ])
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver")),
                ),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::Expr(expr) = &function.body[0].node else {
        return Err(format!("expected expression statement, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Expr::MethodCall(filter_call, select_name, _, select_args) = &expr.node else {
        return Err(format!("expected select method call, got {:?}", expr.node).into());
    };
    assert_eq!(select_name, "select");
    assert!(matches!(
        &select_args[0],
        crate::ast::CallArg::Positional(arg)
            if matches!(
                &arg.node,
                crate::ast::Expr::Surface(surface)
                    if matches!(
                        &surface.payload,
                        crate::ast::SurfaceExprPayload::LeadingDotPath { segments, owner, .. }
                            if segments == &["customer_id".to_string()]
                                && owner.declaration == "query"
                                && owner.call.as_deref() == Some("select")
                    )
            )
    ));
    let crate::ast::Expr::MethodCall(_, filter_name, _, filter_args) = &filter_call.node else {
        return Err(format!("expected filter method call, got {:?}", filter_call.node).into());
    };
    assert_eq!(filter_name, "filter");
    assert!(matches!(
        &filter_args[0],
        crate::ast::CallArg::Positional(arg)
            if matches!(
                &arg.node,
                crate::ast::Expr::Binary(left, crate::ast::BinaryOp::Gt, _)
                    if matches!(
                        &left.node,
                        crate::ast::Expr::Surface(surface)
                            if matches!(
                                &surface.payload,
                                crate::ast::SurfaceExprPayload::LeadingDotPath { segments, owner, .. }
                                    if segments == &["amount".to_string()]
                                        && owner.declaration == "query"
                                        && owner.call.as_deref() == Some("filter")
                            )
                    )
            )
    ));
    Ok(())
}

#[test]
fn test_call_argument_scoped_leading_dot_rejects_unregistered_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::querykit\n\ndef configure(orders: Any) -> None:\n  orders.map(.amount)\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let keyword_map = std::collections::HashMap::new();
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "querykit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("querykit")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_call_argument("query", "filter")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver")),
                ),
        ],
    );

    let result = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map));
    let errors = result.expect_err("leading-dot path should remain invalid in unregistered calls");
    let first_error = errors
        .first()
        .expect("expected at least one unregistered-call diagnostic");
    assert!(
        first_error
            .message
            .contains("Expected expression, found Punctuation(Dot)"),
        "expected ordinary leading-dot parse rejection, got {:?}",
        errors
    );
    Ok(())
}

#[test]
fn rfc040_product_probe_route_head_accepts_verb_composition_and_mapping() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "import pub::routekit\n\ndef configure() -> None:\n  route \"/users\":\n    get + post -> users.index\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "routekit".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routekit".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "route".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "routekit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("routekit")
                .with_declaration(incan_vocab::DeclarationSurface::named("route"))
                .with_scoped_surfaces([
                    incan_vocab::ScopedSurfaceDescriptor::operator("route.verb", "+")
                        .in_declaration_body("route")
                        .pairwise_chain(),
                    incan_vocab::ScopedSurfaceDescriptor::operator("route.map", "->").in_declaration_body("route"),
                ]),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
        return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::Expr(expr) = &block.body[0].node else {
        return Err(format!("expected route expression statement, got {:?}", block.body[0].node).into());
    };
    let crate::ast::Expr::Surface(surface) = &expr.node else {
        return Err(format!("expected route mapping surface expression, got {:?}", expr.node).into());
    };
    let crate::ast::SurfaceExprPayload::ScopedGlyph {
        glyph,
        left,
        right,
        owner,
    } = &surface.payload
    else {
        return Err(format!("expected route mapping scoped glyph, got {:?}", surface.payload).into());
    };
    assert_eq!(glyph, "->");
    assert_eq!(owner.declaration, "route");
    assert!(matches!(
        &left.node,
        crate::ast::Expr::Surface(left_surface)
            if matches!(
                &left_surface.payload,
                crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                    if glyph == "+" && owner.declaration == "route"
            )
    ));
    assert!(matches!(
        &right.node,
        crate::ast::Expr::Field(object, field)
            if matches!(&object.node, crate::ast::Expr::Ident(name) if name == "users")
                && field == "index"
    ));
    Ok(())
}

#[test]
fn rfc040_product_probe_workflow_accepts_pipeline_fallback_binding_and_shape_check()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::workflowkit\n\ndef configure() -> None:\n  flow:\n    extract >> normalize // fallback\n    result := check === expected\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert(
        "workflowkit".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "workflowkit".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "flow".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert(
        "workflowkit".to_string(),
        vec![
            incan_vocab::DslSurface::on_import("workflowkit")
                .with_declaration(incan_vocab::DeclarationSurface::named("flow"))
                .with_scoped_surfaces([
                    incan_vocab::ScopedSurfaceDescriptor::operator("flow.pipe", ">>")
                        .in_declaration_body("flow")
                        .pairwise_chain(),
                    incan_vocab::ScopedSurfaceDescriptor::operator("flow.fallback", "//").in_declaration_body("flow"),
                    incan_vocab::ScopedSurfaceDescriptor::binding("flow.bind", ":=").in_declaration_body("flow"),
                    incan_vocab::ScopedSurfaceDescriptor::operator("flow.shape", "===").in_declaration_body("flow"),
                ]),
        ],
    );

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(block) = &function.body[0].node else {
        return Err(format!("expected vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::Expr(pipe_expr) = &block.body[0].node else {
        return Err(format!("expected workflow pipeline expression, got {:?}", block.body[0].node).into());
    };
    let crate::ast::Statement::Expr(bind_expr) = &block.body[1].node else {
        return Err(format!("expected workflow binding expression, got {:?}", block.body[1].node).into());
    };
    assert!(matches!(
        &pipe_expr.node,
        crate::ast::Expr::Surface(surface)
            if matches!(
                &surface.payload,
                crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, right, owner, .. }
                    if glyph == ">>"
                        && owner.declaration == "flow"
                        && matches!(
                            &right.node,
                            crate::ast::Expr::Surface(right_surface)
                                if matches!(
                                    &right_surface.payload,
                                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                                        if glyph == "//" && owner.declaration == "flow"
                                )
                        )
            )
    ));
    assert!(matches!(
        &bind_expr.node,
        crate::ast::Expr::Surface(surface)
            if matches!(
                &surface.payload,
                crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, right, owner, .. }
                    if glyph == ":="
                        && owner.declaration == "flow"
                        && matches!(
                            &right.node,
                            crate::ast::Expr::Surface(right_surface)
                                if matches!(
                                    &right_surface.payload,
                                    crate::ast::SurfaceExprPayload::ScopedGlyph { glyph, owner, .. }
                                        if glyph == "===" && owner.declaration == "flow"
                                )
                        )
            )
    ));
    Ok(())
}
