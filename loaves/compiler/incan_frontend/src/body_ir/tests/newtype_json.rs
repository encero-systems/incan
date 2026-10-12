//! Checked serde derives retained on raw source newtypes.

use super::build;

/// Bare and module-qualified JSON derives retain the checked serde capability on the tuple wrapper.
#[test]
fn raw_newtype_json_derives_retain_checked_capabilities() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        r#"from std.serde import json
from std.serde.json import Serialize, Deserialize

@derive(Serialize, Deserialize)
type Bare = newtype int

@derive(json.Serialize, json.Deserialize)
type Qualified = newtype str

def main() -> None:
    pass
"#,
        &["m", "newtype_json"],
    )?;
    assert_eq!(module.nominal_declarations.len(), 2);
    for declaration in &module.nominal_declarations {
        assert_eq!(declaration.fields, ["0"]);
        assert!(declaration.derives.iter().any(|name| name == "serde::Serialize"));
        assert!(declaration.derives.iter().any(|name| name == "serde::Deserialize"));
    }
    Ok(())
}
