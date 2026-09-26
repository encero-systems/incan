//! The derives lowering gives a newtype automatically, as the typechecker's derive relation decided them, and the
//! prerequisite derives it adds from the shared implication table (#1754).

use super::*;

/// Return the derive list lowering gave the named struct, newtype or enum.
fn struct_derives(ir: &IrProgram, name: &str) -> Result<Vec<String>, String> {
    ir.declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Struct(declaration) if declaration.name == name => Some(declaration.derives.clone()),
            IrDeclKind::Enum(declaration) if declaration.name == name => Some(declaration.derives.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("missing declaration `{name}`"))
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

/// #1754: a `@rust.derive` path to a derive the newtype already carries automatically is the same derive, so it is
/// emitted once; naming it twice fails the build with E0119.
#[test]
fn rust_derive_path_of_an_automatic_derive_is_emitted_once_issue1754() -> Result<(), String> {
    let ir = lower_source(
        r#"
@rust.derive("std::clone::Clone")
type Name = newtype str

@rust.derive("::core::fmt::Debug", "std::hash::Hash")
type Label = newtype str
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    assert_eq!(struct_derives(&ir, "Name")?, vec!["Debug", "Clone"]);
    assert_eq!(struct_derives(&ir, "Label")?, vec!["Debug", "Clone", "std::hash::Hash"]);
    Ok(())
}

/// #1754: a model, class or enum names each automatic or implied derive once, whatever `@rust.derive` path also names
/// it.
#[test]
fn rust_derive_paths_of_automatic_and_implied_derives_are_emitted_once_issue1754() -> Result<(), String> {
    let ir = lower_source(
        r#"
@rust.derive("std::clone::Clone", "::core::fmt::Debug")
model Person:
    name: str

@rust.derive("std::clone::Clone")
class Counter:
    count: int

@derive(Eq)
@rust.derive("std::cmp::PartialEq", "std::clone::Clone")
enum Tag:
    A
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;
    for name in ["Person", "Counter", "Tag"] {
        let derives = struct_derives(&ir, name)?;
        for leaf in ["Clone", "Debug", "PartialEq"] {
            let named = derives
                .iter()
                .filter(|derive| derive.rsplit("::").next() == Some(leaf))
                .count();
            if named > 1 {
                return Err(format!("{name} names {leaf} {named} times: {derives:?}"));
            }
        }
    }
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
