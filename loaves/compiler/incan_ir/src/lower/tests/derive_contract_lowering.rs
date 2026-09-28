//! What lowering derives for a declaration under the derive contract: no `Display` derive, which Rust does not have,
//! and no derived `PartialEq` beside an enum's own `__eq__` (#1872).

use super::*;

/// Return the derive list lowering gave the named struct or enum.
fn declaration_derives(ir: &IrProgram, name: &str) -> Result<Vec<String>, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Struct(declaration) if declaration.name == name => Some(declaration.derives.clone()),
            IrDeclKind::Enum(declaration) if declaration.name == name => Some(declaration.derives.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("missing declaration `{name}`"))
}

/// #1872: `@derive(Display)` provides nothing, so it is not passed on as a derive; an enum that defines `__eq__` gets
/// no automatic `PartialEq` beside it, while one that does not keeps it.
#[test]
fn display_derive_and_enum_partial_eq_beside_eq_dunder_are_not_derived_issue1872() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
@derive(Display)
model Shown:
    id: int

enum Custom:
    Red
    Blue

    def __eq__(self, other: Self) -> bool:
        return true

enum Plain:
    Red
    Blue
"#,
    )?;
    let shown = declaration_derives(&ir, "Shown")?;
    assert!(!shown.iter().any(|derive| derive == "Display"), "{shown:?}");
    let custom = declaration_derives(&ir, "Custom")?;
    assert!(!custom.iter().any(|derive| derive == "PartialEq"), "{custom:?}");
    let plain = declaration_derives(&ir, "Plain")?;
    assert!(plain.iter().any(|derive| derive == "PartialEq"), "{plain:?}");
    Ok(())
}

/// Return the `impl Default` lowering gave the named type, when it has one.
fn default_impl<'a>(ir: &'a IrProgram, name: &str) -> Option<&'a crate::decl::IrImpl> {
    ir.declarations.iter().find_map(|decl| match &decl.kind {
        IrDeclKind::Impl(implementation)
            if implementation.target_type == name && implementation.trait_name.as_deref() == Some("Default") =>
        {
            Some(implementation)
        }
        _ => None,
    })
}

/// #1879: a model or class that derives `Default` and declares a field default gets an explicit `impl Default` whose
/// `default()` constructs the value, passing a default only for the fields that declare none, instead of Rust's derive,
/// which would ignore the declared defaults; one without field defaults keeps the derive.
#[test]
fn default_derive_over_field_defaults_lowers_to_a_constructing_impl_issue1879() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
@derive(Default)
model Settings:
    theme: str = "dark"
    retries: int

@derive(Default)
class Counter:
    pub count: int = 3

@derive(Default)
model Plain:
    count: int
"#,
    )?;
    for name in ["Settings", "Counter"] {
        let derives = declaration_derives(&ir, name)?;
        assert!(!derives.iter().any(|derive| derive == "Default"), "{name}: {derives:?}");
        let implementation = default_impl(&ir, name).ok_or_else(|| format!("{name} has no impl Default"))?;
        let [method] = implementation.methods.as_slice() else {
            return Err(format!("{name}: expected one method, got {:?}", implementation.methods));
        };
        assert_eq!(method.name, "default");
        assert!(method.params.is_empty());
    }
    let settings = default_impl(&ir, "Settings").ok_or("Settings has no impl Default")?;
    let body = format!("{:?}", settings.methods.first().map(|method| &method.body));
    assert!(body.contains("\"retries\""), "{body}");
    assert!(
        !body.contains("\"theme\""),
        "the declared default is filled by construction: {body}"
    );
    let plain = declaration_derives(&ir, "Plain")?;
    assert!(plain.iter().any(|derive| derive == "Default"), "{plain:?}");
    assert!(default_impl(&ir, "Plain").is_none());
    Ok(())
}
