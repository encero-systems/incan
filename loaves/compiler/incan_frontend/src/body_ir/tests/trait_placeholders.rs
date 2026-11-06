//! Trait requirements carry interface identities without executing their documentation or placeholder statements.

use super::build;

/// A documented value-returning placeholder has no executable default for a consumer to specialize.
#[test]
fn documented_value_placeholder_does_not_publish_a_body() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"pub trait Stream[T, E]:
    def __next__(mut self) -> Result[Option[T], E]:
        """Describe the required step without supplying an implementation."""
        ...
"#,
        &["m", "documented_requirement"],
    )?;
    assert_eq!(module.trait_declarations.len(), 1);
    assert!(
        module.bodies.is_empty(),
        "placeholder became executable: {:?}",
        module.bodies
    );
    Ok(())
}

/// Real value defaults and valid unit no-ops retain their executable bodies beside their declaring trait.
#[test]
fn documented_real_defaults_and_unit_noops_keep_their_bodies() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"pub trait Label:
    def label(self) -> str:
        """Return the real label."""
        return "value"

    def touch(self) -> None:
        """Provide a valid no-op default."""
        pass
"#,
        &["m", "documented_defaults"],
    )?;
    assert_eq!(module.bodies.len(), 2);
    assert!(
        module
            .bodies
            .iter()
            .all(|body| module.body_has_canonical_direct_call_id(body))
    );
    Ok(())
}
