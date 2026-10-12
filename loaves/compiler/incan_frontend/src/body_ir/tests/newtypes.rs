//! Declaration-bound newtype layouts, generic method frames, and selected trait implementations.

use super::{IncanType, build};

/// A generic tuple wrapper retains the declaring binder as a placeholder authorized by its canonical layout.
#[test]
fn generic_newtype_layout_retains_its_owner_parameter() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Box[T] = newtype T:
    @staticmethod
    def wrap(value: T) -> Self:
        return Box(value)

    def duplicate(self) -> Tuple[T, T]:
        return (self.0, self.0)

def main() -> None:
    println(Box.wrap(42).duplicate()[0])
"#,
        &["m", "newtype_parameters"],
    )?;
    let [declaration] = module.nominal_declarations.as_slice() else {
        return Err("expected one retained newtype layout".into());
    };
    assert_eq!(declaration.type_parameters, ["T"]);
    assert_eq!(declaration.type_parameter_count, 1);
    assert_eq!(declaration.fields, ["0"]);
    assert_eq!(declaration.field_types, [IncanType::TypeVar("T".into())]);
    assert!(module.is_well_formed_nominal_declaration(declaration));
    Ok(())
}

/// Same-named methods targeting distinct traits keep both selected implementations on a raw newtype.
#[test]
fn newtype_trait_targets_retain_distinct_implementations() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        include_str!(
            "../../../../incan_test_support/fixtures/behavior/snapshots_traits_and_generics/newtype_trait_targets.incn"
        ),
        &["m", "newtype_trait_targets"],
    )?;
    assert_eq!(module.nominal_declarations.len(), 1);
    assert_eq!(module.trait_implementations.len(), 2);
    assert_ne!(
        module.trait_implementations[0].implementation,
        module.trait_implementations[1].implementation
    );
    Ok(())
}

/// A validation hook or primitive constraint cannot be admitted as a raw tuple wrapper without its checks.
#[test]
fn checked_newtypes_remain_outside_raw_layout_admission() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"type Positive = newtype int[gt=0]

type Checked = newtype int:
    def from_underlying(value: int) -> Result[Self, ValidationError]:
        return Ok(Checked(value))

def main() -> None:
    pass
"#,
        &["m", "checked_newtype_boundary"],
    )?;
    assert!(module.nominal_declarations.is_empty());
    Ok(())
}
