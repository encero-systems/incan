//! Checked truthiness, length and membership expressions retain their receiver and selected method.

use super::{IncanType, SemanticSourceTargetKind, bir, body_named, build};

/// The original protocol fixture retains concrete results and canonical source targets for all three hooks.
#[test]
fn protocol_expressions_retain_checked_method_targets() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/protocol_hooks.incn"
        ),
        &["m", "protocol_hooks"],
    )?;
    let main = body_named(&module, "main")?;
    assert!(
        main.locals
            .iter()
            .all(|local| !matches!(local.ty, IncanType::Unknown | IncanType::Infer)),
        "{}",
        module.render_snapshot()
    );
    for method in ["__bool__", "__len__", "__contains__"] {
        assert!(
            main.block.stmts.iter().any(|statement| matches!(
                &statement.kind,
                bir::StatementKind::Call { callee: bir::Callee::Method(target), .. }
                    if target.name == method && target.canonical.as_ref().is_some_and(|identity|
                        identity.kind == SemanticSourceTargetKind::Method)
            )),
            "{}",
            module.render_snapshot()
        );
    }
    Ok(())
}
