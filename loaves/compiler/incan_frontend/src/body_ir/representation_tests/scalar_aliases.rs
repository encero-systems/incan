//! A writable scalar alias declared `BorrowedMut[int]` writes through to its source field: `more += 1` reads the
//! field, adds one and stores the integer sum back (RFC 164).

use super::*;

/// The compound assignment through the alias combines integers and stores an integer into the aliased field, so the
/// value written to `order.quantity` is held in an integer temporary rather than one typed as the field's owner.
#[test]
fn compound_assignment_through_a_scalar_alias_stays_an_integer() -> Result<(), Box<dyn std::error::Error>> {
    let source = "class Order:\n    pub quantity: int\n\ndef main() -> None:\n    mut order = Order(quantity=1)\n    mut more: BorrowedMut[int] = order.quantity\n    more += 1\n    println(order.quantity)\n";
    let (_, _, module) = checked_seed(source)?;
    let main = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing main")?;
    assert!(
        main.first_representation_gap().is_none(),
        "{:?}",
        main.first_representation_gap()
    );
    let stored = main
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(operand)),
            } if !place.projection.is_empty() => operand.place.local_id(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(stored.len(), 1, "{:#?}", main.block.stmts);
    let local = main.locals.get(stored[0].index()).ok_or("missing stored local")?;
    assert_eq!(local.ty, super::IncanType::Primitive(super::IncanPrimitiveType::Int));
    Ok(())
}
