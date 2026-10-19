//! Deferred zero-argument assertion calls retain their checked callable and error identities.

use super::{IncanType, bir, body_named, build};

/// Neither assertion spelling executes the function before the assertion's runtime unwind boundary.
#[test]
fn raises_forms_retain_function_items_instead_of_eager_calls() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        concat!(
            "from std.testing import assert_raises as check_error\n\n",
            "def explode() -> None:\n    _ = int(\"bad\")\n\n",
            "def run() -> None:\n",
            "    assert explode() raises ValueError, \"failure\"\n",
            "    check_error[ValueError](explode, \"failure\")\n",
        ),
        &["m", "deferred_raises"],
    )?;
    let run = body_named(&module, "run")?;
    let assertions = run
        .block
        .stmts
        .iter()
        .filter(|statement| {
            matches!(
                statement.kind,
                bir::StatementKind::Assert {
                    kind: bir::AssertionKind::Raises { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        assertions,
        2,
        "both forms must retain an assertion: {}",
        run.render_snapshot()
    );
    assert!(
        !run.block
            .stmts
            .iter()
            .any(|statement| matches!(statement.kind, bir::StatementKind::Call { .. })),
        "the function must run only inside the retained assertion: {}",
        run.render_snapshot()
    );
    assert_eq!(
        run.block
            .stmts
            .iter()
            .filter(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Assign { rvalue: bir::Rvalue::Closure { body, .. }, .. } if body.function_item
            ))
            .count(),
        2,
        "each assertion must use its resolver-proven function item"
    );
    for statement in &run.block.stmts {
        if let bir::StatementKind::Assign {
            place,
            rvalue: bir::Rvalue::Closure { body, .. },
        } = &statement.kind
            && body.function_item
        {
            let local = place.local_id().ok_or("a function item needs a local")?;
            assert!(
                matches!(&run.locals[local.index()].ty, IncanType::Function { params, return_type }
                if params.is_empty() && return_type.as_ref() == &IncanType::Primitive(incan_semantics_core::IncanPrimitiveType::Unit)),
                "the deferred item must carry the checked callable type: {}",
                run.render_snapshot()
            );
        }
    }
    Ok(())
}

/// An omitted SDK message uses the same empty literal as an explicit empty message and keeps the callable deferred.
#[test]
fn raises_default_message_retains_a_deferred_assertion() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.testing import assert_raises as check_error\n\ndef explode() -> None:\n    _ = int(\"bad\")\n\ndef run() -> None:\n    check_error[ValueError](explode)\n    check_error[ValueError](explode, \"\")\n",
        &["m", "default_raises"],
    )?;
    let run = body_named(&module, "run")?;
    let messages = run
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assert {
                kind: bir::AssertionKind::Raises { .. },
                message,
                ..
            } => Some(message),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 2, "{}", run.render_snapshot());
    for message in messages {
        assert!(matches!(message, Some(bir::Operand::Constant(bir::Constant::Str(text))) if text.is_empty()));
    }
    assert!(
        !run.block
            .stmts
            .iter()
            .any(|statement| matches!(statement.kind, bir::StatementKind::Call { .. })),
        "{}",
        run.render_snapshot()
    );
    Ok(())
}
