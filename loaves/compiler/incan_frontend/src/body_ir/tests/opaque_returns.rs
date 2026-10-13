//! Concrete producers of local trait-returning functions retain the opaque carrier legacy emits.

use super::{IncanType, body_named, build};

/// A selected SDK constructor remains a template over the caller's declared binder, ready for checked specialization.
#[test]
fn sdk_generic_reader_adapter_retains_its_constructor_template() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/std_fallible_adapter_loop.incn"
        ),
        &["m", "generic_reader_adapter"],
    )?;
    let body = body_named(&module, "consume")?;
    assert!(
        body.named_type_identities.contains_key("ReaderChunks"),
        "receiver identities: {:?}",
        body.named_type_identities
    );
    assert!(
        body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, args } if base == "MapErrFallibleIterator" && args.len() == 5
        )),
        "{}",
        body.render_snapshot()
    );
    assert!(
        !body.locals.iter().any(|local| matches!(&local.ty,
            IncanType::Generic { base, .. } if base == "FallibleIterator"
        )),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

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

/// An explicit trait return remains opaque even when the receiver came from a proven concrete producer.
#[test]
fn explicit_trait_return_does_not_grant_self_carrier() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "trait Read:\n    def as_read(self) -> Read:\n        return producer()\n\nmodel Reading with Read:\n    value: int\n\ndef producer() -> Read:\n    return Reading(value=7)\n\ndef main() -> None:\n    erased = producer().as_read()\n    pass\n",
        &["m", "explicit_trait_return"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(
        body.locals
            .iter()
            .any(|local| local.ty == IncanType::Named("Reading".into()))
    );
    assert!(
        body.locals
            .iter()
            .any(|local| local.ty == IncanType::Named("Read".into()))
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

/// SDK default adapter calls retain the concrete constructors emitted by legacy, without supplying their bodies.
#[test]
fn sdk_fallible_adapters_retain_concrete_carriers() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/emit_calls_and_builtins/fallible_iterator_take.incn"
        ),
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_derives_collection_adapters.incn"
        ),
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_stdlib/std_derives_collection_adapters_with_imported_clone.incn"
        ),
    ] {
        let module = build(source, &["m", "sdk_opaque"])?;
        let body = body_named(&module, "main")?;
        assert!(
            !body
                .locals
                .iter()
                .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "FallibleIterator")),
            "{}",
            body.render_snapshot()
        );
        assert!(
            body.named_type_identities
                .values()
                .any(|identity| identity.declaration_name.ends_with("FallibleIterator")
                    && identity.kind == super::SemanticSourceTargetKind::Model),
            "the checked concrete adapter owner must accompany its carrier: {}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// An SDK trait parameter has no known concrete producer, so its adapter remains opaque.
#[test]
fn sdk_unknown_trait_parameter_remains_opaque() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.derives.collection import FallibleIterator\n\ndef adapt(stream: FallibleIterator[int, str]) -> FallibleIterator[int, str]:\n    return stream.take(1)\n",
        &["m", "sdk_unknown_opaque"],
    )?;
    let body = body_named(&module, "adapt")?;
    assert!(
        body.locals
            .iter()
            .any(|local| matches!(&local.ty, IncanType::Generic { base, .. } if base == "FallibleIterator"))
    );
    assert!(
        !body
            .named_type_identities
            .values()
            .any(|identity| identity.declaration_name == "TakeFallibleIterator")
    );
    Ok(())
}
