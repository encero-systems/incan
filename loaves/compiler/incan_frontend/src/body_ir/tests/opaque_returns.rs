//! Concrete producers of local trait-returning functions retain the opaque carrier legacy emits.

use super::{IncanType, body_named, build};

/// A single checked adopter returned as a trait must keep its concrete carrier through the call result.
#[test]
fn local_trait_return_retains_its_concrete_producer() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Read[T]:\n    def value(self) -> T: ...\n\nmodel Reading with Read[int]:\n    number: int\n\n    def value(self) -> int:\n        return self.number\n\ndef reading() -> Read[int]:\n    return Reading(number=7)\n\ndef main() -> int:\n    return reading().value()\n",
        &["m", "opaque_returns"],
    )?;
    let producer = body_named(&module, "reading")?;
    assert_eq!(
        producer.return_type,
        IncanType::Named("Reading".to_owned()),
        "{}",
        producer.render_snapshot()
    );
    let main = body_named(&module, "main")?;
    assert!(
        main.locals
            .iter()
            .any(|local| local.ty == IncanType::Named("Reading".to_owned())),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}
