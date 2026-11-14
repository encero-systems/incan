//! A module's lookup tables read the declarations its context modules publish where those modules keep them.

use super::*;
use std::borrow::Borrow;

/// A dependency module that publishes one of each declaration a module's lookup tables index.
const DEPENDENCY: &str = "model Point:\n    x: int\nenum Plain:\n    One\n    Two\nenum Policy(str):\n    Kept = \"kept\"\nenum Choice:\n    Number(int)\n    Empty\nconst LIMIT: int = 3\n";

/// The declaration a table entry stands for, whether the table keeps it or reads it in place.
fn entry<'table, T, V: Borrow<T> + 'table>(read: Option<&'table V>, kind: &str) -> Result<&'table T, String> {
    read.map(|value| Borrow::<T>::borrow(value))
        .ok_or_else(|| format!("the context's {kind} is missing from its table"))
}

/// Every module of a compilation builds these tables from the same dependency Loaves, so each must read a context
/// declaration where its module keeps it rather than copy it once per module.
#[test]
fn a_module_reads_its_context_declarations_where_they_are_kept() -> Result<(), Box<dyn std::error::Error>> {
    let (_, _, dependency) = checked_seed(DEPENDENCY)?;
    let kept = [dependency];
    let context = crate::body_ir::ContextModules::from(&kept);
    let module = &kept[0];
    let mut nominals = LocalNominalDeclarations::new();
    let mut fieldless_enums = LocalFieldlessEnumDeclarations::new();
    let mut value_enums = LocalValueEnumDeclarations::new();
    extend_published_nominal_declarations(context, &mut nominals, &mut fieldless_enums, &mut value_enums);
    let enums = collect_visible_enum_declarations(&[], context);
    let constants = collect_published_constants(context, &[]);

    assert!(
        !module.nominal_declarations.is_empty(),
        "the dependency publishes a model"
    );
    assert!(
        !module.fieldless_enum_declarations.is_empty(),
        "the dependency publishes a fieldless enum"
    );
    assert!(
        !module.value_enum_declarations.is_empty(),
        "the dependency publishes a value enum"
    );
    assert!(
        !module.enum_declarations.is_empty(),
        "the dependency publishes an enum layout"
    );
    assert!(
        !module.constant_declarations.is_empty(),
        "the dependency publishes a constant"
    );
    for declaration in &module.nominal_declarations {
        let read = entry(nominals.get(&declaration.canonical.render_compact()), "model")?;
        assert!(
            std::ptr::eq(read, declaration),
            "a module must read a context model in place, not a copy of it"
        );
    }
    for declaration in &module.fieldless_enum_declarations {
        let read = entry(
            fieldless_enums.get(&declaration.canonical.render_compact()),
            "fieldless enum",
        )?;
        assert!(
            std::ptr::eq(read, declaration),
            "a module must read a context fieldless enum in place"
        );
    }
    for declaration in &module.value_enum_declarations {
        let read = entry(value_enums.get(&declaration.canonical.render_compact()), "value enum")?;
        assert!(
            std::ptr::eq(read, declaration),
            "a module must read a context value enum in place"
        );
    }
    for declaration in &module.enum_declarations {
        let read = entry(enums.get(&declaration.canonical), "enum layout")?;
        assert!(
            std::ptr::eq(read, declaration),
            "a module must read a context enum layout in place"
        );
    }
    for declaration in &module.constant_declarations {
        let read = entry(constants.get(&declaration.canonical), "constant")?;
        assert!(
            std::ptr::eq(read, &declaration.value),
            "a module must read a context constant in place"
        );
    }
    Ok(())
}
