//! Checked callable expression invocation and its source evaluation order.

use super::*;

/// A returned callable retains its checked signature and executes through its evaluated frame value.
#[test]
fn returned_callable_retains_its_evaluated_target() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def make(base: int) -> (int) -> int:\n    return (value) => base + value\n\ndef main() -> int:\n    return make(3)(4)\n",
        &["m", "returned_callable"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(
        body.locals.iter().all(|local| local.ty != IncanType::Unknown),
        "{}",
        body.render_snapshot()
    );
    let target = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Function(bir::CallableTarget::Local(target)),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or("missing evaluated callable target")?;
    assert!(target.operand.place.projection.is_empty());
    let local = target
        .operand
        .place
        .local_id()
        .ok_or("callable target must be a frame local")?;
    assert!(matches!(body.locals[local.index()].ty, IncanType::Function { .. }));
    Ok(())
}
