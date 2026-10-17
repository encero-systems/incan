//! Checked iteration hooks keep their own call facts and normalize polling without backend name lookup.

use super::{IncanType, bir, body_named, build, named_targets};
use crate::ast::Span;
use crate::symbols::ResolvedType;
use crate::typechecker::{TypeCheckInfo, TypeChecker};
use crate::{lexer, parser};

const STRUCTURAL: &str = include_str!("../../../../incan_test_support/fixtures/native_iteration/structural.incn");
const FALLIBLE: &str = include_str!("../../../../incan_test_support/fixtures/native_iteration/fallible.incn");

/// SDK generic iterator defaults and the adopter's next body retain the same exact checked trait instantiation.
#[test]
fn sdk_fallible_defaults_retain_adoption_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_iterator_loop.incn"
        ),
        &["m", "sdk_fallible_defaults"],
    )?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|owner| owner.name == "NumberStream")
        .ok_or("missing stream")?;
    for name in ["__iter__", "__next__", "collect"] {
        let record = module
            .trait_implementations
            .iter()
            .find(|record| record.owner == owner.canonical && record.method.declaration_name == name)
            .ok_or_else(|| format!("missing {name} implementation"))?;
        assert_eq!(
            record
                .type_arguments
                .iter()
                .map(|argument| argument.parameter.as_str())
                .collect::<Vec<_>>(),
            ["T", "E"]
        );
        assert_eq!(
            record.type_arguments[0].argument,
            IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Int)
        );
        assert_eq!(
            record.type_arguments[1].argument,
            IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Str)
        );
        if name != "__next__" {
            assert_eq!(record.method, record.implementation);
        }
    }
    Ok(())
}

/// Visit calls inside structured loop and match control flow without mistaking a poll spelling for a target.
fn method_calls<'body>(statements: &'body [bir::Statement], calls: &mut Vec<&'body bir::MethodTarget>) {
    for statement in statements {
        match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                ..
            } => calls.push(target),
            bir::StatementKind::Loop { body } => method_calls(&body.stmts, calls),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => {
                method_calls(&then_block.stmts, calls);
                if let Some(block) = else_block {
                    method_calls(&block.stmts, calls);
                }
            }
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Match { arms, .. },
                ..
            } => {
                for arm in arms {
                    method_calls(&arm.guard_stmts, calls);
                    method_calls(&arm.body_stmts, calls);
                }
            }
            _ => {}
        }
    }
}

/// Repeated statement loops call each checker-selected owner rather than synthesized hook names.
#[test]
fn structural_loops_keep_canonical_hook_targets() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(STRUCTURAL, &["m", "structural_iteration"])?;
    let main = body_named(&module, "main")?;
    let mut calls = Vec::new();
    method_calls(&main.block.stmts, &mut calls);
    for name in ["__iter__", "__next__"] {
        let selected: Vec<_> = calls.iter().filter(|call| call.name == name).collect();
        assert_eq!(selected.len(), 3, "{}", main.render_snapshot());
        for call in selected {
            let canonical = call.canonical.as_ref().ok_or("protocol hook has no canonical target")?;
            assert_eq!(canonical.declaration_name, name);
            assert!(
                call.type_args.is_empty(),
                "a hook inherited its iterable call's arguments"
            );
            assert_eq!(call.binding, bir::ArgumentBinding::resolved_positional(0));
        }
    }
    Ok(())
}

/// Fallible polling retains the checked call and same-error propagation before intrinsic option dispatch.
#[test]
fn fallible_polling_uses_checked_calls_and_error_routing() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(FALLIBLE, &["m", "fallible_iteration"])?;
    let drain = body_named(&module, "drain")?;
    let mut calls = Vec::new();
    method_calls(&drain.block.stmts, &mut calls);
    assert!(
        calls
            .iter()
            .any(|call| call.name == "__next__" && call.canonical.is_some()),
        "{}",
        drain.render_snapshot()
    );
    let loop_body = drain
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Loop { body } => Some(body),
            _ => None,
        })
        .ok_or("missing fallible loop")?;
    assert!(
        loop_body.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::TryPropagate {
                error_routing: bir::TryErrorRouting::SameType { .. },
                ..
            }
        )),
        "{}",
        drain.render_snapshot()
    );
    assert!(!loop_body.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::IterNext {
            protocol: bir::IterProtocol::UserDefined { .. },
            ..
        }
    )));
    Ok(())
}

/// The generic written call keeps its concrete arguments while both protocol hooks have their own empty frame.
#[test]
fn iteration_preserves_the_written_generic_call() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(STRUCTURAL, &["m", "written_iteration_call"])?;
    let chosen: Vec<_> = named_targets(&module, "main")
        .into_iter()
        .filter(|target| target.name == "choose")
        .collect();
    assert_eq!(chosen.len(), 2);
    for target in chosen {
        assert_eq!(target.type_args, [IncanType::Named("Source".into())]);
    }
    let main = body_named(&module, "main")?;
    let mut calls = Vec::new();
    method_calls(&main.block.stmts, &mut calls);
    assert!(
        calls
            .iter()
            .filter(|call| matches!(call.name.as_str(), "__iter__" | "__next__"))
            .all(|call| call.type_args.is_empty())
    );
    Ok(())
}

/// Capture real checked declarations so a transient-call snapshot regression uses accepted source types.
fn checked_source() -> Result<TypeCheckInfo, Box<dyn std::error::Error>> {
    let tokens = lexer::lex(STRUCTURAL).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".into(), "call_snapshot".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    Ok(checker.type_info().clone())
}

/// Taking and restoring temporary hook facts must also protect the separate executable constructor/call maps.
#[test]
fn transient_call_snapshot_preserves_body_ir_only_facts() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Checked source call ----
    let mut info = checked_source()?;
    let text = "choose(source(3))";
    let start = STRUCTURAL.find(text).ok_or("missing written call")?;
    let span = Span::new(start, start + text.len());
    let key = (span.start, span.end);
    let source = info.expr_type(span).ok_or("missing checked call result")?.clone();
    let method = info
        .protocols
        .body_ir_iterations
        .values()
        .find_map(|hooks| hooks.iter.canonical.clone())
        .ok_or("missing checked iterator method")?;

    // ---- Executable facts kept apart from legacy ----
    info.calls.body_ir_constructor_types.insert(key, source.clone());
    info.calls.body_ir_call_type_arguments.insert(key, vec![source.clone()]);
    info.calls.body_ir_call_result_types.insert(key, source.clone());
    info.calls.body_ir_self_result_methods.insert(key, method.clone());

    // ---- Temporary hook resolution ----
    let original = info.take_call_site_facts(span);
    assert!(!info.calls.body_ir_constructor_types.contains_key(&key));
    assert!(!info.calls.body_ir_call_type_arguments.contains_key(&key));
    assert!(!info.calls.body_ir_call_result_types.contains_key(&key));
    assert!(!info.calls.body_ir_self_result_methods.contains_key(&key));
    info.calls.body_ir_constructor_types.insert(key, ResolvedType::Unknown);
    info.calls.body_ir_call_type_arguments.insert(key, Vec::new());
    info.calls.body_ir_call_result_types.insert(key, ResolvedType::Unknown);

    // ---- Restored written call ----
    info.restore_call_site_facts(span, original);
    assert_eq!(info.calls.body_ir_constructor_types.get(&key), Some(&source));
    assert_eq!(
        info.calls.body_ir_call_type_arguments.get(&key).map(Vec::as_slice),
        Some(std::slice::from_ref(&source))
    );
    assert_eq!(info.calls.body_ir_call_result_types.get(&key), Some(&source));
    assert_eq!(info.calls.body_ir_self_result_methods.get(&key), Some(&method));
    info.clear_body_ir_call_site_facts(span);
    assert!(!info.calls.body_ir_self_result_methods.contains_key(&key));
    Ok(())
}

/// Opaque SDK iterator results retain canonical polling hooks without changing legacy's builtin iteration view.
#[test]
fn opaque_iterator_call_retains_private_polling_hooks() -> Result<(), Box<dyn std::error::Error>> {
    let original = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/loops_over_calls_returning_iterator_adopters/main.incn"
    );
    let source = original
        .split("\n    clock = clock_at")
        .next()
        .ok_or("missing local iterator section")?
        .replace("from clocks import clock_at\n", "");
    let tokens = lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["main".into()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    let start = source.find("make(30).take(1)").ok_or("missing original adapter loop")?;
    let span = Span {
        start,
        end: start + "make(30).take(1)".len(),
    };
    assert!(
        checker.type_info().protocol_iteration(span).is_none(),
        "legacy iteration facts changed"
    );
    let hooks = checker
        .type_info()
        .protocols
        .body_ir_iterations
        .get(&(span.start, span.end))
        .ok_or("missing private SDK iterator hooks")?;
    for (hook, name) in [(&hooks.iter, "iter"), (&hooks.next, "__next__")] {
        let identity = hook.canonical.as_ref().ok_or("missing checked hook identity")?;
        assert_eq!(identity.declaration_name, name);
    }
    let module = super::super::build_body_ir_module_v0(&program, &["main".into()], checker.type_info());
    let main = body_named(&module, "main")?;
    let mut calls = Vec::new();
    method_calls(&main.block.stmts, &mut calls);
    assert!(
        calls
            .iter()
            .any(|target| target.canonical.as_ref() == hooks.next.canonical.as_ref()),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}
