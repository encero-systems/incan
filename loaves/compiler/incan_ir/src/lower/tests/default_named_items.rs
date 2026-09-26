//! An item a parameter default names -- a const, a static, a function, or a model, class, enum or newtype -- is
//! published by the module that declares it, because the default is expanded at call sites outside that module.

use super::*;
use crate::decl::Visibility;

/// Return the visibility of the named item's lowered declaration.
fn item_visibility(ir: &IrProgram, name: &str) -> Result<Visibility, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Const {
                name: declared,
                visibility,
                ..
            }
            | IrDeclKind::Static {
                name: declared,
                visibility,
                ..
            } if declared == name => Some(*visibility),
            IrDeclKind::Function(function) if function.name == name => Some(function.visibility),
            IrDeclKind::Struct(declared) if declared.name == name => Some(declared.visibility),
            IrDeclKind::Enum(declared) if declared.name == name => Some(declared.visibility),
            _ => None,
        })
        .ok_or_else(|| format!("missing item `{name}`"))
}

/// Return the visibility of every field of the named struct's lowered declaration.
fn field_visibilities(ir: &IrProgram, name: &str) -> Result<Vec<Visibility>, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Struct(declared) if declared.name == name => {
                Some(declared.fields.iter().map(|field| field.visibility).collect())
            }
            _ => None,
        })
        .ok_or_else(|| format!("missing struct `{name}`"))
}

/// A default is evaluated as if in the module that declares its callable, so a private item it names is still a valid
/// default: `read()` from another module, or `build(3)` from another package, receives `CHUNK`, `LABEL`, `_suffix()`,
/// `_Default()` or `_Mode.Fast` through a path to the item. A method partial's preset is a default of the method it
/// generates, so the private const it names counts too. The generated item is published for that path, with every
/// field a construction spells; a private item no default names keeps its private item.
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


model _Default:
    size: int = 4


enum _Mode:
    Fast
    Slow


model _Unused:
    size: int = 0


pub def make(settings: _Default = _Default()) -> int:
    return settings.size


pub def run(mode: _Mode = _Mode.Fast) -> int:
    return 1


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
    for name in ["CHUNK", "LABEL", "OFFSET", "PREFIX", "_suffix", "_Default", "_Mode"] {
        assert_eq!(
            item_visibility(&ir, name)?,
            Visibility::Public,
            "`{name}` is named by a default or a preset and published for its callers"
        );
    }
    assert!(
        field_visibilities(&ir, "_Default")?
            .iter()
            .all(|visibility| *visibility == Visibility::Public),
        "the fields a default's construction spells are published"
    );
    for name in ["UNUSED", "_unused", "_Unused"] {
        assert_eq!(
            item_visibility(&ir, name)?,
            Visibility::Private,
            "`{name}` is named by no default and keeps its private item"
        );
    }
    Ok(())
}
