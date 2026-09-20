//! Soft keyword activation and vocab block surfaces: `async` / `await` / `property` gated by `std.*` imports, library
//! imports that activate keywords, vocab block statements, expression-list clauses and clause modifiers,
//! expression-desugaring and braced blocks, nested declaration surfaces and in-block placement.

use super::*;

#[test]
fn test_parse_import_path_with_async_segment() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.async.time import sleep
"#;
    let program = parse_str(source)?;
    let decl = match &program.declarations[0].node {
        Declaration::Import(import) => import,
        _ => panic!("Expected import declaration"),
    };
    let ImportKind::From { module, .. } = &decl.kind else {
        panic!("Expected from-import");
    };
    assert_eq!(module.segments, vec!["std", "async", "time"]);
    Ok(())
}

#[test]
fn test_parse_async_requires_std_async_import() {
    let source = r#"
async def foo() -> None:
  pass
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected async function without std.async import to fail");
    };
    assert!(
        err[0].message.contains("only available after importing `std.async`"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_parse_async_with_std_async_import_ok() -> Result<(), Vec<CompileError>> {
    let source = r#"
import std.async

async def foo() -> None:
  pass
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[1].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function declaration"),
    };
    assert!(func.is_async());
    Ok(())
}

#[test]
fn test_parse_async_fixture_with_yield_ok() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async
from std.testing import fixture

@fixture(scope="function")
async def resource() -> int:
  yield 1
"#;
    let program = parse_str(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let Declaration::Function(func) = &program.declarations[2].node else {
        return Err(std::io::Error::other("expected function declaration").into());
    };
    assert!(func.is_async());
    assert!(matches!(
        &func.body[0].node,
        Statement::Expr(expr) if matches!(expr.node, Expr::Yield(Some(_)))
    ));
    Ok(())
}

#[test]
fn test_parse_await_with_std_async_import_ok() -> Result<(), Vec<CompileError>> {
    let source = r#"
from std.async.time import sleep

async def foo() -> None:
  await sleep(1.0)
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[1].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function declaration"),
    };
    assert!(matches!(
        &func.body[0].node,
        Statement::Expr(expr)
            if matches!(
                expr.node,
                Expr::Surface(ref surface)
                    if matches!(
                        surface.payload,
                        SurfaceExprPayload::PrefixUnary(_)
                    )
            )
    ));
    Ok(())
}

#[test]
fn test_parse_async_identifier_without_import_ok() -> Result<(), Vec<CompileError>> {
    let source = r#"
def value(async: int) -> int:
  return async
"#;
    parse_str(source)?;
    Ok(())
}

#[test]
fn test_parse_property_identifier_without_member_context_ok() -> Result<(), Vec<CompileError>> {
    let source = r#"
def value(property: int) -> int:
  return property
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.params[0].node.name, "property");
    Ok(())
}

#[test]
fn test_parse_async_method_requires_std_async_import() {
    let source = r#"
class Worker:
  async def run(self) -> None:
    pass
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected async method without std.async import to fail");
    };
    assert!(
        err[0].message.contains("only available after importing `std.async`"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_parse_async_trait_method_requires_std_async_import() {
    let source = r#"
trait Worker:
  async def run(self) -> None:
    ...
"#;
    let Err(err) = parse_str(source) else {
        panic!("Expected async trait method without std.async import to fail");
    };
    assert!(
        err[0].message.contains("only available after importing `std.async`"),
        "Unexpected error: {}",
        err[0].message
    );
}

#[test]
fn test_library_import_activates_soft_keywords() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::mylib\n\nasync def my_func() -> None:\n    pass\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    // Without context, async should fail
    let result_no_context = crate::parser::parse(&tokens);
    assert!(
        result_no_context.is_err(),
        "Expected async function without soft keyword context to fail"
    );

    // With imported vocab registrations mapping mylib -> async modifier, it should succeed.
    let mut map = std::collections::HashMap::new();
    map.insert(
        "mylib".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "mylib.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "async".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::FunctionDecl,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: Vec::new(),
        }],
    );

    let result_with_context = crate::parser::parse_with_context(&tokens, None, Some(&map));
    assert!(
        result_with_context.is_ok(),
        "Expected async function to parse with soft keyword context"
    );
    Ok(())
}

#[test]
fn test_parse_imported_vocab_block_statement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::routes\n\ndef configure() -> None:\n  route \"/health\":\n    pass\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "routes".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec {
                name: "route".to_string(),
                surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                compound_tokens: Vec::new(),
                placement: incan_vocab::KeywordPlacement::TopLevel,
            }],
            valid_decorators: vec!["cached".to_string()],
        }],
    );

    let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    assert!(matches!(function.body[0].node, crate::ast::Statement::VocabBlock(_)));
    Ok(())
}

#[test]
fn test_imported_vocab_keyword_can_still_parse_assignment_statement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::routes\n\ndef configure() -> None:\n  route = \"/health\"\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "routes".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
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

    let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    assert!(matches!(function.body[0].node, crate::ast::Statement::Assignment(_)));
    Ok(())
}

#[test]
fn test_expression_list_clause_accepts_declared_item_modifiers() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  query:\n    SELECT:\n      sum(amount) as total for customer with context\n      amount\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("analytics.query").with_declaration(
                incan_vocab::DeclarationSurface::named("query")
                    .with_clause_body()
                    .with_clause(
                        incan_vocab::ClauseSurface::expr_list("SELECT")
                            .with_expression_item_modifiers([
                                incan_vocab::ExpressionItemModifierSurface::expr("for"),
                                incan_vocab::ExpressionItemModifierSurface::expr("with"),
                            ])
                            .required(),
                    ),
            ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert("analytics".to_string(), metadata.keyword_registrations);
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("analytics".to_string(), metadata.dsl_surfaces);

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(query_block) = &function.body[0].node else {
        return Err(format!("expected query vocab block, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Statement::VocabBlock(select_block) = &query_block.body[0].node else {
        return Err(format!("expected SELECT clause block, got {:?}", query_block.body[0].node).into());
    };
    assert_eq!(
        select_block.keyword_binding.clause_body_kind,
        Some(incan_vocab::ClauseBodyKind::ExpressionList)
    );
    assert!(matches!(
        &select_block.body[0].node,
        crate::ast::Statement::VocabExpressionItem(item)
            if item.alias.as_deref() == Some("total")
                && item.modifiers.len() == 2
                && item.modifiers[0].keyword == "for"
                && matches!(&item.modifiers[0].value.node, crate::ast::Expr::Ident(name) if name == "customer")
                && item.modifiers[1].keyword == "with"
                && matches!(&item.modifiers[1].value.node, crate::ast::Expr::Ident(name) if name == "context")
                && matches!(&item.expr.node, crate::ast::Expr::Call(callee, _, _)
                    if matches!(&callee.node, crate::ast::Expr::Ident(name) if name == "sum"))
    ));
    assert!(matches!(
        &select_block.body[1].node,
        crate::ast::Statement::Expr(expr)
            if matches!(&expr.node, crate::ast::Expr::Ident(name) if name == "amount")
    ));
    Ok(())
}

#[test]
fn test_expression_desugaring_vocab_block_parses_in_assignment_value() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  value = query:\n    FROM orders\n    SELECT:\n      amount as total\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("analytics.query").with_declaration(
                incan_vocab::DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clause(incan_vocab::ClauseSurface::expr("FROM").required())
                    .with_clause(incan_vocab::ClauseSurface::expr_list("SELECT").required()),
            ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert("analytics".to_string(), metadata.keyword_registrations);
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("analytics".to_string(), metadata.dsl_surfaces);

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::Assignment(assign) = &function.body[0].node else {
        return Err(format!("expected assignment, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Expr::VocabBlock(block) = &assign.value.node else {
        return Err(format!("expected vocab expression block, got {:?}", assign.value.node).into());
    };
    assert_eq!(block.keyword, "query");
    assert!(matches!(
        &block.body[0].node,
        crate::ast::Statement::VocabBlock(from)
            if from.keyword == "FROM"
                && matches!(
                    &from.body[0].node,
                    crate::ast::Statement::Expr(expr)
                        if matches!(&expr.node, crate::ast::Expr::Ident(name) if name == "orders")
                )
    ));
    assert!(matches!(
        &block.body[1].node,
        crate::ast::Statement::VocabBlock(select)
            if select.keyword == "SELECT"
                && matches!(
                    &select.body[0].node,
                    crate::ast::Statement::VocabExpressionItem(item)
                        if item.alias.as_deref() == Some("total")
                )
    ));
    Ok(())
}

#[test]
fn test_braced_expression_vocab_block_uses_clause_metadata_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::analytics\n\ndef configure() -> None:\n  value = query { FROM orders GROUP BY amount as grouped SELECT total as total }\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("analytics.query").with_declaration(
                incan_vocab::DeclarationSurface::named("query")
                    .with_clause_body()
                    .desugars_to_expression()
                    .with_clauses([
                        incan_vocab::ClauseSurface::expr("FROM").required(),
                        incan_vocab::ClauseSurface::expr_list("GROUP BY").optional(),
                        incan_vocab::ClauseSurface::expr_list("SELECT").required(),
                    ]),
            ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert("analytics".to_string(), metadata.keyword_registrations);
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("analytics".to_string(), metadata.dsl_surfaces);

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::Assignment(assign) = &function.body[0].node else {
        return Err(format!("expected assignment, got {:?}", function.body[0].node).into());
    };
    let crate::ast::Expr::VocabBlock(block) = &assign.value.node else {
        return Err(format!("expected vocab expression block, got {:?}", assign.value.node).into());
    };
    assert_eq!(block.body.len(), 3);
    assert!(matches!(
        &block.body[0].node,
        crate::ast::Statement::VocabBlock(from)
            if from.keyword == "FROM"
                && matches!(
                    &from.body[0].node,
                    crate::ast::Statement::Expr(expr)
                        if matches!(&expr.node, crate::ast::Expr::Ident(name) if name == "orders")
                )
    ));
    assert!(matches!(
        &block.body[1].node,
        crate::ast::Statement::VocabBlock(group)
            if group.keyword == "GROUP"
                && group.keyword_binding.compound_tokens == vec!["BY".to_string()]
                && matches!(
                    &group.body[0].node,
                    crate::ast::Statement::VocabExpressionItem(item)
                        if item.alias.as_deref() == Some("grouped")
                )
    ));
    assert!(matches!(
        &block.body[2].node,
        crate::ast::Statement::VocabBlock(select)
            if select.keyword == "SELECT"
                && matches!(
                    &select.body[0].node,
                    crate::ast::Statement::VocabExpressionItem(item)
                        if item.alias.as_deref() == Some("total")
                )
    ));
    Ok(())
}

#[test]
fn nested_declaration_vocab_preserves_signature_and_keyword_heads() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.interop import c\n\nbinding Fixture:\n    header = \"fixture.h\"\n    link = c.system_library(\"fixture\")\n\n    symbol version() -> c.i32:\n        native = \"fixture_version\"\n\n    enum Status:\n        OK: c.i32 = fixture_status.OK\n\n    struct Pair:\n        native = \"fixture_pair\"\n        left: c.i32 = left\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
    let metadata = incan_vocab::VocabRegistration::new()
        .with_surface(
            incan_vocab::DslSurface::on_import("std.interop").with_declaration(
                incan_vocab::DeclarationSurface::named("binding")
                    .with_statement_body()
                    .with_declaration(
                        incan_vocab::DeclarationSurface::named("symbol")
                            .with_signature_head()
                            .with_statement_body(),
                    )
                    .with_declaration(incan_vocab::DeclarationSurface::named("enum").with_statement_body())
                    .with_declaration(incan_vocab::DeclarationSurface::named("struct").with_statement_body()),
            ),
        )
        .metadata();
    let mut keyword_map = std::collections::HashMap::new();
    keyword_map.insert("std.interop".to_string(), metadata.keyword_registrations);
    let mut surface_map = std::collections::HashMap::new();
    surface_map.insert("std.interop".to_string(), metadata.dsl_surfaces);

    let program = crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keyword_map), Some(&surface_map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let crate::ast::Declaration::VocabBlock(binding) = &program.declarations[1].node else {
        return Err(format!("expected binding declaration, got {:?}", program.declarations[1].node).into());
    };
    let crate::ast::Statement::VocabBlock(symbol) = &binding.body[2].node else {
        return Err(format!("expected symbol declaration, got {:?}", binding.body[2].node).into());
    };
    assert!(matches!(
        &symbol.signature_head,
        Some(head) if head.name == "version" && head.return_type.is_some()
    ));
    assert!(matches!(
        &binding.body[3].node,
        crate::ast::Statement::VocabBlock(block) if block.keyword == "enum"
    ));
    assert!(matches!(
        &binding.body[4].node,
        crate::ast::Statement::VocabBlock(block) if block.keyword == "struct"
    ));
    Ok(())
}

#[test]
fn test_imported_vocab_keyword_can_still_parse_expression_statement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::routes\n\ndef configure() -> None:\n  route(\"/health\")\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "routes".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
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

    let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    assert!(matches!(function.body[0].node, crate::ast::Statement::Expr(_)));
    Ok(())
}

#[test]
fn test_imported_vocab_keyword_can_still_parse_typed_assignment_statement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::routes\n\ndef configure() -> None:\n  route: str = \"/health\"\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "routes".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
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

    let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    assert!(matches!(function.body[0].node, crate::ast::Statement::Assignment(_)));
    Ok(())
}

#[test]
fn test_parse_nested_vocab_block_with_in_block_placement() -> Result<(), Box<dyn std::error::Error>> {
    let source = "import pub::routes\n\ndef configure() -> None:\n  route \"/home\":\n    get:\n      pass\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "routes".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
            },
            keywords: vec![
                incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                },
                incan_vocab::KeywordSpec {
                    name: "get".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::SubBlock,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::InBlock(vec!["route".to_string()]),
                },
            ],
            valid_decorators: Vec::new(),
        }],
    );

    let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(route_block) = &function.body[0].node else {
        return Err("expected top-level vocab block in function body".into());
    };
    assert!(matches!(route_block.body[0].node, crate::ast::Statement::VocabBlock(_)));
    Ok(())
}

#[test]
fn test_parse_block_context_keyword_surface_as_vocab_block() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "import pub::routes\n\ndef configure() -> None:\n  route \"/home\":\n    middleware auth:\n      pass\n";
    let tokens = crate::lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;

    let mut map = std::collections::HashMap::new();
    map.insert(
        "routes".to_string(),
        vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "routes.dsl".to_string(),
            },
            keywords: vec![
                incan_vocab::KeywordSpec {
                    name: "route".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockDeclaration,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                },
                incan_vocab::KeywordSpec {
                    name: "middleware".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::BlockContextKeyword,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::InBlock(vec!["route".to_string()]),
                },
            ],
            valid_decorators: Vec::new(),
        }],
    );

    let program = crate::parser::parse_with_context(&tokens, None, Some(&map))
        .map_err(|errs| format!("parse errors: {errs:?}"))?;
    let function = match &program.declarations[1].node {
        crate::ast::Declaration::Function(function) => function,
        other => return Err(format!("expected function declaration, got {other:?}").into()),
    };
    let crate::ast::Statement::VocabBlock(route_block) = &function.body[0].node else {
        return Err("expected top-level vocab block in function body".into());
    };
    let crate::ast::Statement::VocabBlock(context_block) = &route_block.body[0].node else {
        return Err("expected nested context vocab block in route body".into());
    };
    assert_eq!(context_block.keyword, "middleware");
    Ok(())
}
