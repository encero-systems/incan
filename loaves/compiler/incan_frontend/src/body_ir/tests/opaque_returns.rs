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

/// The checked Self-returning iteration contract keeps the same opaque carrier without inventing method dispatch.
#[test]
fn local_fallible_iterator_retains_self_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/fallible_iteration_protocol.incn"
        ),
        &["m", "opaque_stream"],
    )?;
    let body = body_named(&module, "drain_trait")?;
    assert!(
        !body
            .locals
            .iter()
            .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "FallibleStream")),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A trait parameter has no concrete producer proof, even when the current module declares only one adopter.
#[test]
fn unknown_trait_parameter_remains_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Read[T]:\n    def value(self) -> T: ...\n\nmodel Reading with Read[int]:\n    number: int\n    def value(self) -> int:\n        return self.number\n\ndef identity(value: Read[int]) -> Read[int]:\n    return value\n",
        &["m", "unknown_opaque"],
    )?;
    let body = body_named(&module, "identity")?;
    assert!(matches!(&body.return_type, IncanType::Generic { base, .. } if base == "Read"));
    Ok(())
}

/// Observable producer side effects and discarded opaque results must keep the same concrete return ABI.
#[test]
fn discarded_local_trait_producer_retains_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Read[T]:\n    def value(self) -> T: ...\n\nmodel Reading with Read[int]:\n    number: int\n    def value(self) -> int:\n        return self.number\n\ndef reading(number: int) -> Read[int]:\n    println(number)\n    return Reading(number=number)\n\ndef main() -> None:\n    reading(7)\n    reading(9)\n",
        &["m", "discarded_opaque"],
    )?;
    for name in ["reading", "main"] {
        let body = body_named(&module, name)?;
        assert!(
            !body
                .locals
                .iter()
                .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "Read")),
            "{}",
            body.render_snapshot()
        );
        if name == "reading" {
            assert_eq!(
                body.return_type,
                IncanType::Named("Reading".to_owned()),
                "{}",
                body.render_snapshot()
            );
        }
    }
    Ok(())
}
