//! Checked constrained-primitive construction facts on the direct route.

use super::{body_named, build};

/// Only the owning implementation bypasses explicit predicates; a different newtype implementation still checks.
#[test]
fn constrained_newtype_raw_bypass_is_limited_to_its_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Positive = newtype int[gt=0]:
    @staticmethod
    def raw(value: int) -> Self:
        return Positive(value)

type Other = newtype int:
    @staticmethod
    def checked(value: int) -> Positive:
        return Positive(value)

def main() -> None:
    println(Positive.raw(0).0)
"#,
        &["m", "newtype_owner_bypass"],
    )?;
    let raw = body_named(&module, "raw")?.render_snapshot();
    assert!(raw.contains("constructor(Positive)"), "{raw}");
    assert!(!raw.contains("helper:newtype_constraint_validate"), "{raw}");
    assert!(!raw.contains("unsupported("), "{raw}");
    let checked = body_named(&module, "checked")?.render_snapshot();
    assert!(checked.contains("constructor(Positive)"), "{checked}");
    assert_eq!(
        checked.matches("helper:newtype_constraint_validate").count(),
        1,
        "{checked}"
    );
    assert!(!checked.contains("unsupported("), "{checked}");
    Ok(())
}

/// An implicitly constrained model field refuses aggregate validation before selecting the fail-fast helper.
#[test]
fn constrained_newtype_aggregate_field_validation_remains_refused() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Positive = newtype int[gt=0]\n\nmodel Holder:\n    value: Positive\n\ndef f(value: int) -> Holder:\n    return Holder(value=value)\n",
        &["m", "newtype_aggregate_validation"],
    )?;
    let snapshot = body_named(&module, "f")?.render_snapshot();
    assert!(
        snapshot.contains("constrained-newtype aggregate-field validation"),
        "{snapshot}"
    );
    assert!(!snapshot.contains("helper:newtype_constraint_validate"), "{snapshot}");
    Ok(())
}

/// Every checked deserialization selection refuses a constrained carrier until its decoder retains validation.
#[test]
fn constrained_newtype_deserialize_selections_remain_outside_raw_layout_admission()
-> Result<(), Box<dyn std::error::Error>> {
    for (name, declaration) in [
        ("bare", "@derive(Deserialize)\ntype Positive = newtype int[gt=0]"),
        (
            "qualified",
            "@derive(json.Deserialize)\ntype Positive = newtype int[gt=0]",
        ),
        ("bundle", "@derive(json)\ntype Positive = newtype int[gt=0]"),
        (
            "adopted",
            "type Positive = newtype int[gt=0] with Deserialize:\n    @staticmethod\n    def from_json(text: str) -> Result[Self, str]:\n        return Ok(Positive(1))",
        ),
    ] {
        let source = format!(
            "from std.serde import json\nfrom std.serde.json import Deserialize\n\n{declaration}\n\ndef main() -> None:\n    pass\n"
        );
        let module = build(&source, &["m", name])?;
        assert!(
            module.nominal_declarations.is_empty(),
            "{name}: {:?}",
            module.nominal_declarations
        );
    }
    Ok(())
}

/// Serialization does not reconstruct a carrier and keeps checked integer construction admitted.
#[test]
fn constrained_newtype_serialize_only_layout_remains_admitted() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.serde.json import Serialize\n\n@derive(Serialize)\ntype Positive = newtype int[gt=0]\n\ndef main() -> None:\n    println(Positive(1).0)\n",
        &["m", "constrained_serialize_only"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("expected one admitted Serialize-only constrained newtype".into());
    };
    assert_eq!(declaration.name, "Positive");
    assert!(
        declaration
            .derives
            .iter()
            .any(|selection| selection == "serde::Serialize")
    );
    assert!(
        !declaration
            .derives
            .iter()
            .any(|selection| selection == "serde::Deserialize")
    );
    Ok(())
}

/// Explicit construction and implicit conversion retain the checker predicates before raw tuple wrapping.
#[test]
fn constrained_newtype_predicates_precede_each_construction() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "type Positive = newtype int[gt=0]\n\ndef f() -> None:\n    implicit: Positive = 1\n    explicit = Positive(2)\n    println(implicit.0)\n    println(explicit.0)\n",
        &["m", "newtype_constraints"],
    )?;
    assert_eq!(module.nominal_declarations.len(), 1);
    let snapshot = body_named(&module, "f")?.render_snapshot();
    assert_eq!(
        snapshot.matches("helper:newtype_constraint_validate").count(),
        2,
        "{snapshot}"
    );
    assert!(snapshot.contains("gt=0"), "{snapshot}");
    Ok(())
}
