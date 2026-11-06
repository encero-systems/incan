//! Source Eq adoption remains a checked slot, independently of native hashing storage.

use super::build;

/// A source comparison adopter keeps its exact selected implementation without a fabricated structural Eq derive.
#[test]
fn source_equality_retains_its_implementation() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "from std.derives.comparison import Eq\n\n@derive(Hash)\nmodel UserKey with Eq:\n    id: int\n\n    def __eq__(self, other: UserKey) -> bool:\n        return self.id == other.id\n\ndef main() -> None:\n    keys: Set[UserKey] = Set()\n    println(len(keys))\n",
        &["m", "hashed_models"],
    )?;
    let owner = module
        .nominal_declarations
        .iter()
        .find(|item| item.name == "UserKey")
        .ok_or("missing UserKey layout")?;
    assert!(owner.derives.iter().any(|item| item == "Hash"));
    assert!(!owner.derives.iter().any(|item| item == "PartialEq"));
    let implementation = module
        .trait_implementations
        .iter()
        .find(|item| item.owner == owner.canonical && item.method.declaration_name == "__eq__")
        .ok_or("missing checked source Eq slot")?;
    assert_ne!(implementation.method, implementation.implementation);
    assert!(
        module
            .bodies
            .iter()
            .any(|body| body.canonical.as_ref() == Some(&implementation.implementation))
    );
    Ok(())
}
