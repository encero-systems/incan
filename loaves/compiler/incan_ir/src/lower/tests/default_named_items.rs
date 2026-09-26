//! A const or function a parameter default names is published by the module that declares it, because the default is
//! expanded at call sites outside that module.

use super::*;
use crate::decl::Visibility;

/// Return the visibility of the named const's or function's lowered declaration.
fn item_visibility(ir: &IrProgram, name: &str) -> Result<Visibility, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Const {
                name: declared,
                visibility,
                ..
            } if declared == name => Some(*visibility),
            IrDeclKind::Function(function) if function.name == name => Some(function.visibility),
            _ => None,
        })
        .ok_or_else(|| format!("missing const or function `{name}`"))
}

/// A default is evaluated as if in the module that declares its callable, so a private const or function it names is
/// still a valid default: `read()` from another module, or `build(3)` from another package, receives `CHUNK`, `LABEL`
/// or `_suffix()` through a path to the item. A method partial's preset is a default of the method it generates, so
/// the private const it names counts too. The generated item is published for that path; a private item no default
/// names keeps its private item.
#[test]
fn private_item_a_default_names_is_published_for_its_callers() -> Result<(), String> {
    let ir = lower_checked_source(
        r#"
const CHUNK: int = 4
const LABEL: str = "flat"
const OFFSET: int = 1
const PREFIX: str = "name"
const UNUSED: int = 2


def _suffix() -> str:
    return "!"


def _unused() -> int:
    return UNUSED


pub def read(n: int = CHUNK) -> int:
    return n


pub def build(size: int, label: str = LABEL, suffix: str = _suffix()) -> str:
    return label + suffix


pub model Reader:
    pub size: int

    def shifted(self, by: int = OFFSET * 2) -> int:
        return self.size + by

    def label(self, prefix: str) -> str:
        return prefix

    short = partial label(prefix=PREFIX)


pub def unused() -> int:
    return _unused()
"#,
    )?;
    for name in ["CHUNK", "LABEL", "OFFSET", "PREFIX", "_suffix"] {
        assert_eq!(
            item_visibility(&ir, name)?,
            Visibility::Public,
            "`{name}` is named by a default or a preset and published for its callers"
        );
    }
    for name in ["UNUSED", "_unused"] {
        assert_eq!(
            item_visibility(&ir, name)?,
            Visibility::Private,
            "`{name}` is named by no default and keeps its private item"
        );
    }
    Ok(())
}
