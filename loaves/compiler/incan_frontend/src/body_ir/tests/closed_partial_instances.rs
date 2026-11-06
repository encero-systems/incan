//! Local partials consume the checker's instantiated target signatures and ordered function arguments.

use super::identities_and_imports::build_with_published_imports;
use super::{IncanType, TypeChecker, bir, body_named, build, build_body_ir_module_v0, lexer, parser};

/// Explicit and inferred function partials forward the retained instance rather than reopening declaration binders.
#[test]
fn function_partials_retain_checked_type_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def couple[T](a: T, b: T) -> list[T]:\n    return [a, b]\n\ndef tagged[T with Display](value: T, prefix: str) -> str:\n    return f\"{prefix}{value}\"\n\ndef main() -> None:\n    first = partial couple(a=1)\n    second = partial tagged[int](prefix=\"no.\")\n    println(first(2))\n    println(second(3))\n";
    let module = build(source, &["m", "checked_function_partials"])?;
    let main = body_named(&module, "main")?;
    let calls = main
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { body, .. },
                ..
            } => Some(body),
            _ => None,
        })
        .flat_map(|body| &body.stmts)
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } => Some(target),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2, "{}", main.render_snapshot());
    for call in calls {
        assert_eq!(
            call.type_args.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["int"],
            "{call:?}"
        );
        assert!(call.canonical.is_some());
    }
    assert!(main.locals.iter().all(|local| local.ty != IncanType::Unknown));
    Ok(())
}

/// Generic model and class presets retain the constructor's checked payload and complete field order.
#[test]
fn generic_constructor_partials_retain_closed_carriers() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Box[T]:\n    value: T\n    label: str\n\nclass Slot[T]:\n    pub item: T\n    pub count: int\n\ndef main() -> None:\n    boxed = partial Box(value=5)\n    first = boxed(label=\"five\")\n    labeled = partial Box[str](label=\"named\")\n    second = labeled(value=\"six\")\n    slot = partial Slot(item=\"s\")\n    third = slot(count=7)\n    println(first.value)\n    println(second.value)\n    println(third.item)\n";
    let module = build(source, &["m", "checked_constructor_partials"])?;
    let main = body_named(&module, "main")?;
    let locals = serde_json::to_string(&main.locals)?;
    assert!(
        !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// The complete original's local and module-qualified partials retain every checked instance in their native locals.
#[test]
fn complete_partial_fixture_retains_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/local_partials_of_generic_and_imported_callables/main.incn"
    );
    let helper = include_str!(
        "../../../../incan_test_support/fixtures/behavior/cli_values_and_calls/local_partials_of_generic_and_imported_callables/helpers.incn"
    );
    let provider = build(helper, &["helpers"])?;
    let module = build_with_published_imports(source, &["main"], &[("helpers", &["helpers"], helper)], &[provider])?;
    let main = body_named(&module, "main")?;
    let locals = serde_json::to_string(&main.locals)?;
    assert!(
        !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// Written arguments alone cannot authorize a function partial when the checked instantiation fact is absent.
#[test]
fn missing_checked_instance_keeps_explicit_partial_refused() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def echo[T](value: T, offset: int) -> T:\n    return value\n\ndef main() -> None:\n    held = partial echo[int](offset=0)\n    println(held(42))\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["m".to_owned(), "unproven_partial_instance".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut facts = checker.type_info().clone();
    assert!(!facts.calls.call_site_monomorph_type_args.is_empty());
    facts.calls.call_site_monomorph_type_args.clear();
    let module = build_body_ir_module_v0(&program, &path, &facts);
    let main = body_named(&module, "main")?;
    assert!(
        !main.block.stmts.iter().any(|statement| matches!(
            &statement.kind,
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { .. },
                ..
            }
        )),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// A checked nongeneric function reference keeps its declaration signature when a testing intrinsic omits its
/// expression snapshot.
#[test]
fn intrinsic_function_references_retain_checked_declaration_signature() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.testing import assert_raises\n\ndef failing() -> None:\n    int(\"bad\")\n\ndef main() -> None:\n    assert_raises[ValueError](failing, \"expected failure\")\n";
    let module = build(source, &["m", "intrinsic_function_reference"])?;
    let main = body_named(&module, "main")?;
    assert!(
        main.locals.iter().all(|local| local.ty != IncanType::Unknown),
        "{}",
        main.render_snapshot()
    );
    assert!(
        !main
            .block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. })),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// A generic declaration cannot supply a missing instance snapshot even when its parameter does not occur in the
/// signature.
#[test]
fn generic_function_without_snapshot_keeps_explicit_refusal() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def ignored[T](value: int) -> int:\n    return value\n\ndef main() -> None:\n    held = partial ignored[int](value=1)\n    println(held())\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["m".to_owned(), "unproven_generic_reference".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut facts = checker.type_info().clone();
    let read = source.rfind("ignored").ok_or("missing generic reference")?;
    facts.expressions.expr_types.remove(&(read, read + "ignored".len()));
    facts
        .expressions
        .body_ir_expr_types
        .remove(&(read, read + "ignored".len()));
    let module = build_body_ir_module_v0(&program, &path, &facts);
    let main = body_named(&module, "main")?;
    assert!(
        main.block.stmts.iter().any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { description } if description.contains("partial function without a checked target signature"))),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}
