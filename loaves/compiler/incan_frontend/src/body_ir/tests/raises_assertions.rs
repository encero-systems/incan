//! Deferred zero-argument assertion calls retain their checked callable and error identities.

use super::*;

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
    Ok(())
}
