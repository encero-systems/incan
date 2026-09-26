//! A const a parameter default names is published by the module that declares it, because the default is expanded
//! at call sites outside that module.

use super::*;
use crate::decl::Visibility;

/// Return the visibility of the named const's lowered declaration.
fn const_visibility(ir: &IrProgram, name: &str) -> Result<Visibility, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Const {
                name: declared,
                visibility,
                ..
            } if declared == name => Some(*visibility),
            _ => None,
        })
        .ok_or_else(|| format!("missing const `{name}`"))
}

/// A default is evaluated as if in the module that declares its callable, so a private const it names is still a
/// valid default: `read()` from another module, or `build(3)` from another package, receives `CHUNK` or `LABEL`
/// through a path to the const. The const's generated item is published for that path; a private const no default
/// names keeps its private item.
#[test]
fn private_const_a_default_names_is_published_for_its_callers() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const CHUNK: int = 4
const LABEL: str = "flat"
const OFFSET: int = 1
const UNUSED: int = 2


pub def read(n: int = CHUNK) -> int:
    return n


pub def build(size: int, label: str = LABEL) -> str:
    return label


pub model Reader:
    pub size: int

    def shifted(self, by: int = OFFSET * 2) -> int:
        return self.size + by


pub def unused() -> int:
    return UNUSED
"#,
    )?;
    for name in ["CHUNK", "LABEL", "OFFSET"] {
        assert_eq!(
            const_visibility(&ir, name)?,
            Visibility::Public,
            "`{name}` is named by a default and published for its callers"
        );
    }
    assert_eq!(
        const_visibility(&ir, "UNUSED")?,
        Visibility::Private,
        "a const no default names keeps its private item"
    );
    Ok(())
}
