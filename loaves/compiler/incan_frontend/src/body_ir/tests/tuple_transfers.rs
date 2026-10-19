//! Temporary tuple destructuring transfers owned fields without inventing Clone requirements.

use super::{body_named, build};
use incan_semantics_core::body_ir::{Operand, OwnershipFact, Rvalue, StatementKind};

/// A constructor result owns its channel ends; unpacking transfers both even though Receiver is not Clone.
#[test]
fn temporary_channel_tuple_transfers_its_fields() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.async.channel import channel\n\ndef main() -> None:\n    sender, receiver = channel[int](2)\n    println(0)\n",
        &["m", "tuple_transfers"],
    )?;
    let body = body_named(&module, "main")?;
    let fields: Vec<_> = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| {
            if let StatementKind::Assign {
                rvalue: Rvalue::Use(Operand::Place(read)),
                ..
            } = &statement.kind
                && !read.place.projection.is_empty()
            {
                Some(read)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(fields.len(), 2, "{}", body.render_snapshot());
    assert!(
        fields
            .iter()
            .all(|read| read.fact == OwnershipFact::Move && read.last_use),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}
