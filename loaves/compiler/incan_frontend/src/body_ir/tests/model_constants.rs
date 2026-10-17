//! Model constant reads retain their checked aggregate without mutable global storage.

use super::{bir, body_named, build};

/// Each read constructs the checked constant value; field access projects a local value rather than a global cell.
#[test]
fn model_constant_reads_are_values() -> Result<(), Box<dyn std::error::Error>> {
    let module = build(
        "@derive(Clone, Eq, Descriptor)\nmodel Version:\n    major: int\n    minor: int\n\nconst INITIAL: Version = Version(major=0, minor=1)\n\ndef main() -> None:\n    first = INITIAL\n    println(INITIAL.major)\n    println(first.minor)\n",
        &["m", "model_constants"],
    )?;
    let body = body_named(&module, "main")?;
    assert!(body.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Aggregate(..),
            ..
        }
    )));
    assert!(!body.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Assign {
            rvalue: bir::Rvalue::Use(bir::Operand::Place(read)), ..
        } if read.place.global().is_some()
    )));
    assert!(module.static_declarations.is_empty());
    Ok(())
}
