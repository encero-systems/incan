//! Checked newtype storage layouts and derive facts for the direct route.

use super::*;

/// Checked structural derives are retained without duplicating automatic traits or admitting validation hooks.
#[test]
fn derived_newtype_retains_checked_structural_traits() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "@derive(Clone, Eq, Ord, Hash, Default)\ntype Label = newtype str\n\ndef main() -> str:\n    return Label(\"label\").0\n",
        &["m", "derived_newtype"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("expected one retained derived newtype".into());
    };
    assert_eq!(
        declaration.derives,
        [
            "Debug",
            "Clone",
            "Eq",
            "PartialEq",
            "Ord",
            "PartialOrd",
            "Hash",
            "Default"
        ]
    );
    assert!(module.is_well_formed_nominal_declaration(declaration));
    let checked = build(
        "@derive(Clone)\ntype Positive = newtype int:\n    def from_underlying(value: int) -> Result[Self, ValidationError]:\n        if value < 0:\n            return Err(ValidationError(\"negative\"))\n        return Ok(Positive(value))\n\ndef main() -> None:\n    value = Positive(1)\n",
        &["m", "derived_checked_newtype"],
    )?;
    assert!(checked.nominal_declarations.is_empty());
    Ok(())
}
