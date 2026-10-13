//! Native display admission follows the exact source enum's retained derive selection.

use super::build;

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
