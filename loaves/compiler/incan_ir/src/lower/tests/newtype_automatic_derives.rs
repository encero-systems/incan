//! The derives lowering gives a newtype automatically, as the typechecker's derive relation decided them, and the
//! prerequisite derives it adds from the shared implication table (#1754).

use super::*;

/// Return the derive list lowering gave the named struct or newtype.
fn struct_derives(ir: &IrProgram, name: &str) -> Result<Vec<String>, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Struct(declaration) if declaration.name == name => Some(declaration.derives.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("missing struct `{name}`"))
}

/// #1754: a newtype carries `Clone` and `Debug` when its underlying type does, `Copy` too for a `Copy` type, and
/// neither over a task handle, which implements neither.
#[test]
fn newtype_automatic_derives_follow_the_underlying_type_issue1754() -> Result<(), String> {
    let ir = lower_source(
        r#"
import std.async
from std.async.task import JoinHandle

type Email = newtype str
type UserId = newtype int
type Handle = newtype JoinHandle[int]
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    assert_eq!(struct_derives(&ir, "Email")?, vec!["Debug", "Clone"]);
    assert_eq!(struct_derives(&ir, "UserId")?, vec!["Debug", "Clone", "Copy"]);
    assert_eq!(struct_derives(&ir, "Handle")?, Vec::<String>::new());
    Ok(())
}

/// `@derive(Ord)` brings `PartialOrd`, `Eq` and `PartialEq` right after it, in the implication table's order, exactly
/// as the hand-written rules did before the table.
#[test]
fn ord_derive_adds_its_prerequisites_in_table_order() -> Result<(), String> {
    let ir = lower_source(
        r#"
@derive(Ord)
model Rank:
    value: int
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    let derives = struct_derives(&ir, "Rank")?;
    let prerequisites = derives
        .iter()
        .skip_while(|derive| derive.as_str() != "Ord")
        .take(4)
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        prerequisites,
        vec!["Ord", "PartialOrd", "Eq", "PartialEq"],
        "{derives:?}"
    );
    Ok(())
}
