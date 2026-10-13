//! Closed nominal value facts supplement legacy inference without rewriting its snapshots.

use super::{body_named, build, local_types_at, named_targets};

/// Payload inference fixes the initial generic newtype carrier at the constructor's source span.
#[test]
fn generic_newtype_payload_retains_closed_constructor() -> Result<(), Box<dyn std::error::Error>> {
    let source = "type Box[T] = newtype T\n\ndef main() -> None:\n    b = Box(42)\n    println(b.0)\n";
    let module = build(source, &["m", "closed_newtype_payload"])?;
    let main = body_named(&module, "main")?;
    assert_eq!(local_types_at(main, source, "Box(42)"), ["Box[int]"]);
    Ok(())
}

/// A destination annotation closes the generated default constructor's local result.
#[test]
fn generic_default_retains_annotated_destination() -> Result<(), Box<dyn std::error::Error>> {
    let source = "@derive(Default)\nmodel Holder[T]:\n    items: list[T]\n    size: int = 3\n\ndef main() -> None:\n    holder: Holder[int] = Holder.default()\n    println(holder.size)\n";
    let module = build(source, &["m", "closed_default_destination"])?;
    let main = body_named(&module, "main")?;
    assert_eq!(local_types_at(main, source, "Holder.default()"), ["Holder[int]"]);
    Ok(())
}

/// Generic newtype static and instance results preserve the payload-selected owner arguments.
#[test]
fn complete_generic_newtype_retain_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_newtypes_and_serde/newtype_generic.incn"
    );
    let module = build(source, &["m", "closed_newtype_fixture"])?;
    let main = body_named(&module, "main")?;
    let locals = serde_json::to_string(&main.locals)?;
    assert!(
        !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// A checked model adoption fixes a generic trait call's arguments at the call site.
#[test]
fn adopted_trait_argument_retains_closed_instantiation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "trait Reader[T]:\n    def read(self) -> T: ...\n\nmodel Packet with Reader[int]:\n    value: int\n\n    def read(self) -> int:\n        return self.value\n\ndef read[T](value: Reader[T]) -> T:\n    return value.read()\n\ndef main() -> None:\n    println(read(Packet(value=42)))\n";
    let module = build(source, &["m", "closed_trait_argument"])?;
    let main = body_named(&module, "main")?;
    assert_eq!(local_types_at(main, source, "read(Packet(value=42))"), ["int", "str"]);
    Ok(())
}

/// A standard testing helper keeps the value-selected parameter in its executable call target.
#[test]
fn testing_helper_retains_closed_call_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = "from std.testing import assert_eq\n\ndef main() -> None:\n    assert_eq(1, 1)\n";
    let module = build(source, &["m", "closed_testing_argument"])?;
    let targets = named_targets(&module, "main");
    let target = targets
        .iter()
        .find(|target| target.name == "assert_eq")
        .ok_or("missing testing helper")?;
    assert_eq!(
        target.type_args.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["int"]
    );
    Ok(())
}

/// The generic trait-returning original preserves the adopter's concrete result carrier at every call.
#[test]
fn complete_generic_trait_return_retain_closed_locals() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/generic_bounds_return_type.incn"
    );
    let module = build(source, &["m", "closed_trait_return_fixture"])?;
    let main = body_named(&module, "main")?;
    let locals = serde_json::to_string(&main.locals)?;
    assert!(
        !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
        "{}",
        main.render_snapshot()
    );
    Ok(())
}
