//! Canonical set conversions retain the checked iterable's element without changing legacy inference.

use super::{body_named, build};
use incan_semantics_core::{IncanPrimitiveType, IncanType, generic};

/// A generator result supplies the canonical set constructor's exact retained element.
#[test]
fn set_of_generator_keeps_its_element() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def numbers() -> Generator[int]:\n    yield 3\n\ndef main() -> None:\n    unique = set(numbers())\n    println(len(unique))\n",
        &["m", "set_generator"],
    )?;
    let body = body_named(&module, "main")?;
    let unique = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("unique"))
        .ok_or("missing set binding")?;
    assert_eq!(
        unique.ty,
        generic("Set".into(), vec![IncanType::Primitive(IncanPrimitiveType::Int)]),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// An immutable collection's element remains its value type when a set owns each copied item.
#[test]
fn set_of_frozen_list_keeps_its_element() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "const NAMES: FrozenList[str] = [\"a\", \"b\"]\n\ndef main() -> None:\n    unique = set(NAMES)\n    println(len(unique))\n",
        &["m", "set_frozen"],
    )?;
    let body = body_named(&module, "main")?;
    let unique = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("unique"))
        .ok_or("missing set binding")?;
    assert_eq!(
        unique.ty,
        generic("Set".into(), vec![IncanType::Primitive(IncanPrimitiveType::Str)]),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A frozen parameter retains its explicit binder before a caller closes its set conversion.
#[test]
fn set_of_frozen_parameter_keeps_its_declared_binder() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def unique[T](values: FrozenList[T]) -> Set[T]:\n    return set(values)\n",
        &["m", "set_frozen_parameter"],
    )?;
    let body = body_named(&module, "unique")?;
    let expected = generic("Set".into(), vec![IncanType::TypeVar("T".into())]);
    assert!(
        body.locals.iter().any(|local| local.ty == expected),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A same-spelled source model does not grant the builtin generator's iterable element contract.
#[test]
fn a_source_model_named_generator_keeps_set_storage_unproven() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Generator[T]:\n    value: T\n\ndef main() -> None:\n    unique = set(Generator[int](value=3))\n",
        &["m", "source_generator"],
    )?;
    let body = body_named(&module, "main")?;
    let unique = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("unique"))
        .ok_or("missing set binding")?;
    assert_eq!(
        unique.ty,
        generic("Set".into(), vec![IncanType::Unknown]),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}
