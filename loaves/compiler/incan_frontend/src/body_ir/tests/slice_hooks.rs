//! A checked source slice hook is a canonical call; builtin slices remain projections.

use super::{bir, body_named, build};
use incan_semantics_core::{IncanPrimitiveType, IncanType, generic};

/// A source hook receives exactly three typed Option arguments and retains its checked method identity.
#[test]
fn a_slice_hook_retains_its_method_and_option_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "model Window:\n    items: list[int]\n\n    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[int]:\n        return [start.unwrap_or(-1), end.unwrap_or(-1), step.unwrap_or(-1)]\n\ndef main() -> None:\n    window = Window(items=[1, 2])\n    selected = window[1::2]\n    println(selected)\n",
        &["m", "slice_hook"],
    )?;
    let body = body_named(&module, "main")?;
    let (target, args) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                callee: bir::Callee::Method(target),
                args,
                ..
            } if target.name == "__getslice__" => Some((target, args)),
            _ => None,
        })
        .ok_or("missing checked slice hook call")?;
    assert_eq!(
        target
            .canonical
            .as_ref()
            .map(|identity| identity.declaration_name.as_str()),
        Some("__getslice__")
    );
    assert_eq!(args.len(), 4);
    let option_int = generic("Option".into(), vec![IncanType::Primitive(IncanPrimitiveType::Int)]);
    for argument in &args[1..] {
        let Some(bir::Operand::Place(read)) = argument.as_one() else {
            return Err("slice part lacks typed Option storage".into());
        };
        let local = read.place.local_id().ok_or("slice part is not a local")?;
        let retained = body.locals.get(local.index()).ok_or("missing slice part local")?;
        assert_eq!(retained.ty, option_int);
    }
    Ok(())
}

/// A builtin list slice keeps its structural representation without granting a source method target.
#[test]
fn a_builtin_slice_keeps_its_projection() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "def take(items: list[int]) -> list[int]:\n    return items[1:]\n",
        &["m", "builtin_slice"],
    )?;
    let body = body_named(&module, "take")?;
    assert!(body.block.stmts.iter().any(|statement| {
        match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(read)),
            } => read
                .place
                .projection
                .iter()
                .any(|projection| matches!(projection, bir::PlaceElem::Slice { .. })),
            _ => false,
        }
    }));
    Ok(())
}
