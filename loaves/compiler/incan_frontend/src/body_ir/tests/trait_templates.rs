//! Generic adopter defaults retain only placeholders declared by their exact nominal owner.

use super::{IncanType, build};

/// A retained generic owner binds its default arguments; a foreign or uninferred placeholder grants no template.
#[test]
fn generic_owner_defaults_validate_only_declared_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"trait Echo[T]:
    def echo(self, value: T) -> T:
        return value

model Holder[T] with Echo[T]:
    value: T

def main() -> None:
    println(Holder(value=2).echo(3))
"#,
        &["m", "trait_templates"],
    )?;
    let record = module
        .trait_implementations
        .iter()
        .find(|record| record.method.declaration_name == "echo" && record.method == record.implementation)
        .ok_or("missing generic adopter default")?;
    assert!(module.is_well_formed_trait_implementation(record), "{record:?}");
    for invalid in [
        IncanType::TypeVar("Unrelated".into()),
        IncanType::Unknown,
        IncanType::SelfType,
    ] {
        let mut corrupted = record.clone();
        corrupted
            .type_arguments
            .first_mut()
            .ok_or("missing trait binder")?
            .argument = invalid;
        assert!(!module.is_well_formed_trait_implementation(&corrupted));
    }
    Ok(())
}
