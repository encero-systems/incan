//! Frozen collection constants retain their checked type and literal elements at the Body IR boundary.

use super::{IncanType, bir, body_named, build};

/// Literal tuple constants become checked data and local match scrutinees without replacing shadowing parameters.
#[test]
fn literal_tuple_constants_materialize_before_matching() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "const PAIR = (\"x\", 42)\n\ndef main() -> None:\n    match PAIR:\n        case (label, number):\n            println(f\"{label} {number}\")\n\ndef shadow(PAIR: (str, int)) -> None:\n    match PAIR:\n        case (label, number):\n            println(f\"{label} {number}\")\n",
        &["m", "literal_tuple_constants"],
    )?;
    let pair = module
        .constant_declarations
        .iter()
        .find(|constant| constant.canonical.declaration_name == "PAIR")
        .ok_or("missing retained tuple constant")?;
    assert!(
        matches!(&pair.value, bir::Constant::FrozenAggregate(IncanType::Tuple(types), values)
        if types.len() == 2 && values.len() == 2)
    );
    let main = body_named(&module, "main")?;
    let scrutinee = main
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue:
                    bir::Rvalue::Match {
                        scrutinee: bir::Operand::Place(read),
                        ..
                    },
                ..
            } => Some(&read.place),
            _ => None,
        })
        .ok_or("missing tuple match")?;
    assert!(scrutinee.local_id().is_some());
    assert!(scrutinee.global().is_none());
    let shadow = body_named(&module, "shadow")?.render_snapshot();
    assert!(!shadow.contains("\"x\""), "{shadow}");
    Ok(())
}

/// A frozen constant's static data must survive without turning a shadowing parameter into that constant.
#[test]
fn frozen_list_constants_retain_static_elements() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "const NAMES: FrozenList[str] = [\"alpha\", \"beta\"]\n\ndef main() -> int:\n    return len(NAMES)\n\ndef shadow(NAMES: FrozenList[str]) -> int:\n    return len(NAMES)\n",
        &["m", "frozen_lists"],
    )?;
    let main = body_named(&module, "main")?.render_snapshot();
    assert!(main.contains("alpha") && main.contains("beta"), "{main}");
    let shadow = body_named(&module, "shadow")?.render_snapshot();
    assert!(!shadow.contains("alpha") && !shadow.contains("beta"), "{shadow}");
    Ok(())
}
