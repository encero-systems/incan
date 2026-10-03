//! The Body IR input contract (#1166) and explicit refusals: unsupported constructs lower to a placeholder rather than
//! panicking, undesugared vocab items and scoped-DSL symbols are caller contract violations, the `unsafe:` boundary
//! refuses by name, soft-keyword surface statements stay unmodeled constructs, and feature-gated bodies lower only when
//! their feature is active.

use super::*;

#[test]
fn unsupported_constructs_lower_to_an_explicit_placeholder_instead_of_panicking()
-> Result<(), Box<dyn std::error::Error>> {
    // This case was the refusal's own pin until #1161: a destructuring generator clause used to refuse the whole
    // expression rather than bind its pattern. It now lowers, so the test asserts the positive behavior instead of
    // freezing a hole -- the clause's names become real bindings projected out of the polled item, exactly as the
    // equivalent statement `for` produces them.
    let source = "def pick(x: int) -> int:\n  gen = (left + right for left, right in [(1, 2)])\n  return x\n";
    let module = build(source, &["m", "unsupported"])?;
    let snapshot = module.render_snapshot();

    assert!(
        !snapshot.contains("unsupported("),
        "a destructuring generator clause must lower rather than refuse: {snapshot}"
    );
    for binding in ["left", "right"] {
        assert!(
            snapshot.contains(&format!(" {binding} : int [binding]")),
            "the clause must bind `{binding}` with the tuple element's resolved type: {snapshot}"
        );
    }
    Ok(())
}

// ---- Body IR input contract (#1166) ----

/// Parse `source`, project it through `active_features`, then typecheck and lower **the projection**.
///
/// This is the shape [`build_body_ir_module_v0`]'s input contract requires of every caller: the checker and the
/// lowering both see the feature-projected program, never the full parse tree. [`build`] deliberately skips the
/// projection step, so the two helpers together show what projection is worth rather than only asserting it ran.
fn build_projected(
    source: &str,
    module_path: &[&str],
    active_features: &[&str],
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let parsed = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let active = active_features
        .iter()
        .map(|feature| (*feature).to_string())
        .collect::<std::collections::BTreeSet<String>>();
    let program = parsed.projected_for_features(&active);
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Lower `source` after appending `injected` to its first top-level function body, **after** typechecking.
///
/// Vocab and scoped-DSL nodes cannot arrive through [`parser::parse`]: they need a library vocabulary that an
/// import activates, and every pipeline that has a desugar pass removes them before lowering. Splicing the node in
/// after the checker has run is therefore the only way to reach Body IR's input-contract safety net, and the state
/// it produces is exactly the one a caller that skipped the desugar pass would hand over.
fn build_with_statement_injected_after_typecheck(
    source: &str,
    module_path: &[&str],
    injected: ast::Statement,
) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let mut program = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;
    let module_path: Vec<String> = module_path.iter().map(|s| s.to_string()).collect();
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(module_path.clone()));
    checker
        .check_program(&program)
        .map_err(|errs| std::io::Error::other(format!("{errs:?}")))?;

    let function = program
        .declarations
        .iter_mut()
        .find_map(|decl| match &mut decl.node {
            ast::Declaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or("expected a top-level function to inject the undesugared node into")?;
    function.body.push(ast::Spanned::new(injected, ast::Span::new(0, 1)));

    Ok(build_body_ir_module_v0(&program, &module_path, checker.type_info()))
}

/// Every `Unsupported` refusal description in `body_name`'s top-level statement list.
fn unsupported_descriptions(module: &bir::BodyIrModule, body_name: &str) -> Vec<String> {
    module
        .bodies
        .iter()
        .filter(|body| body.name == body_name)
        .flat_map(|body| &body.block.stmts)
        .filter_map(|stmt| match &stmt.kind {
            bir::StatementKind::Unsupported { description } => Some(description.clone()),
            _ => None,
        })
        .collect()
}

/// A scoped-DSL surface owner naming a library that this compilation never loaded.
fn fixture_scoped_surface_owner() -> ast::ScopedSurfaceOwner {
    ast::ScopedSurfaceOwner {
        declaration: "query".to_string(),
        clause: None,
        call: None,
    }
}

#[test]
fn an_undesugared_vocab_expression_item_is_refused_as_a_caller_contract_violation()
-> Result<(), Box<dyn std::error::Error>> {
    // A vocab expression item is body content of a raw `vocab:` block. The desugar pass owns what it means, so one
    // reaching lowering is a broken caller rather than a construct Body IR has yet to model, and the refusal has to
    // say which of the two it is.
    let module = build_with_statement_injected_after_typecheck(
        "def run() -> int:\n    return 1\n",
        &["m"],
        ast::Statement::VocabExpressionItem(ast::VocabExpressionItemStmt {
            expr: ast::Spanned::new(ast::Expr::Literal(ast::Literal::Bool(true)), ast::Span::new(0, 1)),
            alias: None,
            modifiers: Vec::new(),
        }),
    )?;

    let descriptions = unsupported_descriptions(&module, "run");
    let [description] = descriptions.as_slice() else {
        return Err(Box::from(format!("expected exactly one refusal, got {descriptions:?}")));
    };
    assert!(
        description.contains("vocab expression item"),
        "the refusal must still name the node a program actually hit: {description}"
    );
    assert!(
        description.contains("Body IR input-contract violation") && description.contains("desugar pass"),
        "a vocab node must read as a caller contract violation, not an unmodeled construct: {description}"
    );
    Ok(())
}

#[test]
fn an_undesugared_scoped_dsl_symbol_call_is_refused_as_a_caller_contract_violation()
-> Result<(), Box<dyn std::error::Error>> {
    // `sum(value)` is one of `ScopedDslSurfaces`' canonical forms. Reaching lowering as surface syntax means no
    // desugarer ever gave it a meaning, so the backend must not invent one -- and must not report the absence as a
    // language gap either.
    let module = build_with_statement_injected_after_typecheck(
        "def run() -> int:\n    return 1\n",
        &["m"],
        ast::Statement::Expr(ast::Spanned::new(
            ast::Expr::Surface(Box::new(ast::SurfaceExpr {
                key: SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key: "demo.query".to_string(),
                    descriptor_key: "aggregate".to_string(),
                },
                payload: ast::SurfaceExprPayload::ScopedSymbolCall {
                    symbol: "sum".to_string(),
                    args: Vec::new(),
                    owner: fixture_scoped_surface_owner(),
                },
            })),
            ast::Span::new(0, 1),
        )),
    )?;

    let descriptions = unsupported_descriptions(&module, "run");
    let [description] = descriptions.as_slice() else {
        return Err(Box::from(format!("expected exactly one refusal, got {descriptions:?}")));
    };
    assert!(
        description.contains("scoped DSL symbol call"),
        "the refusal must name the scoped-DSL form rather than the bare label `expression`: {description}"
    );
    assert!(
        description.contains("Body IR input-contract violation"),
        "a scoped-DSL node must read as a caller contract violation: {description}"
    );
    Ok(())
}

#[test]
fn an_unsafe_region_refuses_under_a_named_permanent_boundary() -> Result<(), Box<dyn std::error::Error>> {
    // #1162's second half. The refusal is a decided disposition, not a missing dispatch arm: an `unsafe:` region
    // introduces no Incan scope, so inlining its statements would be trivial -- and would erase the
    // acknowledgment the region exists to record.
    let source = concat!(
        "def probe(x: int) -> int:\n",
        "  return x\n",
        "\n",
        "def touch(value: int) -> int:\n",
        "  mut total = 0\n",
        "  unsafe:\n",
        "    total = probe(value)\n",
        "  return total\n",
    );
    let module = build(source, &["m", "unsafe_region"])?;
    let snapshot = body_named(&module, "touch")?.render_snapshot();

    assert!(
        snapshot.contains("unsupported(`unsafe:` acknowledgment region:"),
        "the refusal must name the construct rather than reading as a generic placeholder: {snapshot}"
    );
    assert!(
        snapshot.contains("refused by design") && snapshot.contains("#1162"),
        "the refusal must state that it is a decided boundary and name its owner: {snapshot}"
    );
    // The region's own statements must not quietly become statements of the enclosing block, which is the
    // silent-execution outcome the refusal exists to prevent.
    assert!(
        !snapshot.contains("probe"),
        "an acknowledged region's statements must not be inlined into the enclosing block: {snapshot}"
    );
    Ok(())
}

#[test]
fn a_soft_keyword_surface_statement_stays_an_unmodeled_construct_rather_than_a_contract_violation()
-> Result<(), Box<dyn std::error::Error>> {
    // `SurfaceStmtPayload::KeywordArgs` carries both a library's scoped-DSL statement and the stdlib-registered
    // soft keywords (`assert` today). Only the first is a pipeline fault; blaming the caller for the second would
    // send its author looking for a desugar pass that was never supposed to run.
    let module = build_with_statement_injected_after_typecheck(
        "def run() -> int:\n    return 1\n",
        &["m"],
        ast::Statement::Surface(ast::SurfaceStmt {
            key: SurfaceFeatureKey::SoftKeyword(KeywordId::Assert),
            payload: ast::SurfaceStmtPayload::KeywordArgs(vec![ast::Spanned::new(
                ast::Expr::Literal(ast::Literal::Bool(true)),
                ast::Span::new(0, 1),
            )]),
        }),
    )?;

    let descriptions = unsupported_descriptions(&module, "run");
    let [description] = descriptions.as_slice() else {
        return Err(Box::from(format!("expected exactly one refusal, got {descriptions:?}")));
    };
    assert!(
        description.contains("assert"),
        "a soft-keyword surface statement must name its keyword: {description}"
    );
    assert!(
        !description.contains("input-contract violation"),
        "real language surface must not be reported as a caller contract violation: {description}"
    );
    Ok(())
}

#[test]
fn a_body_behind_an_inactive_feature_is_not_lowered() -> Result<(), Box<dyn std::error::Error>> {
    // Feature projection is part of the input contract, not an optimization. A body the compilation does not
    // contain must produce no `bir::Body` at all -- not an empty one, and not one an executor could be asked to run.
    let source =
        "when feature(\"beta\"):\n    def gated() -> int:\n        return 7\n\ndef always() -> int:\n    return 1\n";

    let projected = build_projected(source, &["m"], &[])?;
    let lowered: Vec<&str> = projected.bodies.iter().map(|body| body.name.as_str()).collect();
    assert_eq!(
        lowered,
        ["always"],
        "a declaration behind an inactive feature must not reach lowering"
    );

    // The same program without the projection step does lower it, which is what makes the step load-bearing rather
    // than incidentally satisfied by this fixture.
    let unprojected = build(source, &["m"])?;
    assert!(
        unprojected.bodies.iter().any(|body| body.name == "gated"),
        "the fixture must actually carry a gated body, or the assertion above proves nothing"
    );
    Ok(())
}

#[test]
fn projection_through_an_active_feature_lowers_exactly_as_an_ungated_program_does()
-> Result<(), Box<dyn std::error::Error>> {
    // The other half of the contract: projection removes inactive declarations and changes nothing else. With the
    // feature active the projected program and the raw parse tree must lower identically, spans included, so a
    // caller that applies the contract cannot silently perturb a body that was always part of the compilation.
    let source =
        "when feature(\"beta\"):\n    def gated() -> int:\n        return 7\n\ndef always() -> int:\n    return 1\n";

    let active = build_projected(source, &["m"], &["beta"])?;
    let unprojected = build(source, &["m"])?;
    assert_eq!(
        active.render_snapshot(),
        unprojected.render_snapshot(),
        "an active feature must make projection a no-op"
    );
    Ok(())
}
