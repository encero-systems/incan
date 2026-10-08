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

// ---- `@rust.extern` delegation (#2023) ----

#[test]
fn a_rust_extern_body_records_its_delegation_and_no_placeholder_statements_issue2023()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "rust.module(\"incan_std_testing\")\n\n@rust.extern\ndef fail_t(msg: str) -> None:\n    ...\n\ndef main() -> None:\n    fail_t(\"boom\")\n";
    let module = build(source, &["m", "extern_delegation"])?;
    let extern_body = body_named(&module, "fail_t")?;
    let delegation = extern_body
        .extern_delegation
        .as_ref()
        .ok_or("an `@rust.extern` body must record its delegation")?;
    assert_eq!(delegation.rust_path(), "incan_std_testing::fail_t");
    assert!(
        extern_body.block.stmts.is_empty(),
        "an extern's `...` placeholder must not become statements: {}",
        extern_body.render_snapshot()
    );
    assert!(
        extern_body
            .render_snapshot()
            .contains("extern rust incan_std_testing::fail_t"),
        "the snapshot must show the delegation: {}",
        extern_body.render_snapshot()
    );
    let caller = body_named(&module, "main")?;
    assert!(
        caller.extern_delegation.is_none(),
        "an ordinary body records no delegation"
    );
    Ok(())
}

// ---- User-defined decorator rebinding (RFC 036) ----

#[test]
fn a_declaration_rebound_by_a_user_defined_decorator_is_a_representation_gap() -> Result<(), Box<dyn std::error::Error>>
{
    // `run` calls the binding `as_int` returns, but its call target still names `label`'s own body, so a lowered
    // `label` would run undecorated. The decorator and its replacement stay ordinary bodies.
    let source = "def parse(value: int) -> int:\n    return value\n\ndef as_int(func: (int) -> str) -> (int) -> int:\n    println(\"decorating label\")\n    return parse\n\n@as_int\ndef label(value: int) -> str:\n    return \"value\"\n\ndef run(value: int) -> int:\n    return label(value)\n";
    let module = build(source, &["m", "decorator_rebinding"])?;
    let label = body_named(&module, "label")?;
    assert_eq!(
        label.first_representation_gap(),
        Some("declaration `label` rebound by a user-defined decorator"),
        "{}",
        label.render_snapshot()
    );
    for name in ["parse", "as_int", "run"] {
        let body = body_named(&module, name)?;
        assert_eq!(body.first_representation_gap(), None, "{}", body.render_snapshot());
    }
    Ok(())
}

#[test]
fn a_method_rebound_by_a_user_defined_decorator_is_a_representation_gap() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Counter:\n    value: int\n\n    @keep\n    def bump(mut self, by: int) -> int:\n        self.value += by\n        return self.value\n\n    def peek(self) -> int:\n        return self.value\n\ndef keep(func: (mut Counter, int) -> int) -> (mut Counter, int) -> int:\n    return func\n";
    let module = build(source, &["m", "method_decorator_rebinding"])?;
    let bump = body_named(&module, "bump")?;
    assert_eq!(
        bump.first_representation_gap(),
        Some("declaration `bump` rebound by a user-defined decorator"),
        "{}",
        bump.render_snapshot()
    );
    let peek = body_named(&module, "peek")?;
    assert_eq!(peek.first_representation_gap(), None, "{}", peek.render_snapshot());
    Ok(())
}

// ---- Representation gaps and identity well-formedness ----

#[test]
fn a_body_reports_a_representation_gap_nested_anywhere_in_it() -> Result<(), Box<dyn std::error::Error>> {
    // The refusal sits inside an `if` inside a `match` arm; the walk must still find it.
    let source = format!(
        "def run(n: int) -> int:\n  match n:\n    1 => return 1\n    _ =>\n      if n > 2:\n{}      return 0\n\ndef clean(n: int) -> int:\n  return n + 1\n",
        stand_in_refusal_stmt("        ")
    );
    let module = build(&source, &["m", "representation_gaps"])?;
    let run = body_named(&module, "run")?;
    assert!(
        run.first_representation_gap().is_some(),
        "a nested refusal is a gap: {}",
        run.render_snapshot()
    );
    let clean = body_named(&module, "clean")?;
    assert_eq!(clean.first_representation_gap(), None, "{}", clean.render_snapshot());
    Ok(())
}

#[test]
fn a_lowered_body_is_well_formed_and_a_tampered_one_is_not() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def helper(n: int = 1, other: int = 0) -> int:\n  return n + other\n\nchosen = partial helper(n=-2)\n\ndef main() -> int:\n  return chosen()\n",
        &["m", "identity"],
    )?;
    for body in &module.bodies {
        assert!(
            module.body_has_canonical_direct_call_id(body),
            "{}",
            body.render_snapshot()
        );
        assert!(module.is_own_span_declaration_id(&body.direct_call_id));
    }
    let mut tampered = body_named(&module, "helper")?.clone();
    tampered.direct_call_id = body_named(&module, "main")?.direct_call_id.clone();
    assert!(
        !module.body_has_canonical_direct_call_id(&tampered),
        "a body carrying another declaration's identity must not pass"
    );
    let mut partial = body_named(&module, "chosen")?.clone();
    assert!(
        partial
            .params
            .iter()
            .enumerate()
            .all(|(index, parameter)| parameter.local.index() == index),
        "default temporaries must follow every parameter slot"
    );
    partial.direct_call_id = body_named(&module, "helper")?.direct_call_id.clone();
    assert!(
        !module.body_has_canonical_direct_call_id(&partial),
        "a Partial must retain its own declaration identity rather than its target's"
    );
    Ok(())
}

#[test]
fn retained_declarations_are_well_formed_and_tampered_ones_are_not() -> Result<(), Box<dyn std::error::Error>> {
    // Two models with a same-named field, and two enums of each kind with a same-named member, so a member identity
    // copied from the other declaration is coherent in every respect but its owner.
    let source = "model A:\n  value: int\n  other: int\n\nmodel B:\n  value: int\n\n\
                  enum E:\n  V\n  W\n\nenum F:\n  V\n\n\
                  enum P(int):\n  V = 1\n  W = 2\n\nenum Q(int):\n  V = 3\n\n\
                  def main() -> int:\n  a = A(value=1, other=2)\n  b = B(value=3)\n  e = E.V\n  f = F.V\n\
                  \n  return a.value + b.value + P.V.value() + Q.V.value()\n";
    let module = build(source, &["m", "declaration_identity"])?;
    let nominal = |name: &str| {
        module
            .nominal_declarations
            .iter()
            .find(|declaration| declaration.name == name)
            .ok_or(format!("missing model {name}"))
    };
    let fieldless = |name: &str| {
        module
            .fieldless_enum_declarations
            .iter()
            .find(|declaration| declaration.name == name)
            .ok_or(format!("missing enum {name}"))
    };
    let valued = |name: &str| {
        module
            .value_enum_declarations
            .iter()
            .find(|declaration| declaration.name == name)
            .ok_or(format!("missing value enum {name}"))
    };

    // ---- Lowered records pass ----
    for name in ["A", "B"] {
        assert!(
            module.is_well_formed_nominal_declaration(nominal(name)?),
            "model {name}"
        );
    }
    for name in ["E", "F"] {
        assert!(
            module.is_well_formed_fieldless_enum_declaration(fieldless(name)?),
            "enum {name}"
        );
    }
    for name in ["P", "Q"] {
        assert!(
            module.is_well_formed_value_enum_declaration(valued(name)?),
            "value enum {name}"
        );
    }

    // ---- A member identity transplanted from a same-named member of another declaration fails ----
    let mut foreign_field = nominal("A")?.clone();
    foreign_field.field_identities[0] = nominal("B")?.field_identities[0].clone();
    assert!(
        !module.is_well_formed_nominal_declaration(&foreign_field),
        "B.value in A"
    );
    let mut foreign_variant = fieldless("E")?.clone();
    foreign_variant.variants[0] = fieldless("F")?.variants[0].clone();
    assert!(
        !module.is_well_formed_fieldless_enum_declaration(&foreign_variant),
        "F.V in E"
    );
    let mut foreign_value = valued("P")?.clone();
    foreign_value.variants[0] = valued("Q")?.variants[0].clone();
    assert!(
        !module.is_well_formed_value_enum_declaration(&foreign_value),
        "Q.V in P"
    );

    // ---- A duplicated member fails ----
    let mut duplicate_field = nominal("A")?.clone();
    duplicate_field.field_identities[1] = duplicate_field.field_identities[0].clone();
    assert!(
        !module.is_well_formed_nominal_declaration(&duplicate_field),
        "duplicate field"
    );
    let mut duplicate_variant = fieldless("E")?.clone();
    duplicate_variant.variants[1] = duplicate_variant.variants[0].clone();
    assert!(
        !module.is_well_formed_fieldless_enum_declaration(&duplicate_variant),
        "duplicate variant"
    );

    // ---- A member whose identity names another member fails ----
    let mut mismatched_field = nominal("A")?.clone();
    mismatched_field.fields.swap(0, 1);
    assert!(
        !module.is_well_formed_nominal_declaration(&mismatched_field),
        "swapped field names"
    );
    let mut mismatched_variant = valued("P")?.clone();
    mismatched_variant.variants[0].name = "W2".to_string();
    assert!(
        !module.is_well_formed_value_enum_declaration(&mismatched_variant),
        "renamed variant"
    );
    Ok(())
}
