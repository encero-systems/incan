//! Native display admission follows the exact source enum's retained derive selection.

use super::build;

/// A normal enum's checked Display selection reaches its Incan formatter; raw-value and unknown selections stay gated.
#[test]
fn checked_normal_enum_display_reaches_native_admission() -> Result<(), Box<dyn std::error::Error>> {
    let source = "@derive(Display)\nenum Painted:\n    Text(str)\n    Empty\n\nenum Raw(str):\n    One = \"one\"\n";
    let tokens = super::lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = super::parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = super::TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let enum_named = |name: &str| {
        program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                super::ast::Declaration::Enum(value) if value.name == name => Some(value),
                _ => None,
            })
    };
    let painted = enum_named("Painted").ok_or("missing normal enum")?;
    let raw = enum_named("Raw").ok_or("missing value enum")?;
    assert!(super::has_direct_native_enum_derives(painted, checker.type_info()));
    assert!(super::has_direct_native_enum_derives(raw, checker.type_info()));
    let mut facts = checker.type_info().clone();
    facts
        .declarations
        .enum_derives
        .get_mut("Raw")
        .ok_or("missing value enum selection")?
        .push("Display".into());
    assert!(!super::has_direct_native_enum_derives(raw, &facts));
    facts
        .declarations
        .enum_derives
        .get_mut("Painted")
        .ok_or("missing normal enum selection")?
        .push("unretained::Display".into());
    assert!(!super::has_direct_native_enum_derives(painted, &facts));
    Ok(())
}

/// Debug and Clone alone cannot authorize the derived Display formatting path.
#[test]
fn enum_display_requires_its_checked_derive() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"@derive(Display)
enum Painted:
    Text(str)
    Empty

enum Plain:
    Text(str)
    Empty

def main() -> None:
    pass
"#,
        &["m", "enum_display"],
    )?;
    let painted = module
        .enum_declarations
        .iter()
        .find(|item| item.name == "Painted")
        .ok_or("missing Painted")?;
    let plain = module
        .enum_declarations
        .iter()
        .find(|item| item.name == "Plain")
        .ok_or("missing Plain")?;
    assert!(painted.derives.iter().any(|name| name == "Display"));
    assert!(plain.derives.iter().any(|name| name == "Debug"));
    assert!(!plain.derives.iter().any(|name| name == "Display"));
    Ok(())
}
