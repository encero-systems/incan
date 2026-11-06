//! Captured callable reuse keeps explicit ownership decisions for the native environment boundary.

use super::{bir, body_named, build};

/// A captured SDK mapper clones on its first by-value call and moves on its last call.
#[test]
fn captured_callback_reuse_keeps_clone_then_move() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"from std.io import IoError
from std.traits.callable import Callable1

def render[F with (Clone, Callable1[IoError, str])](mapper: F, error: IoError) -> str:
    saved = mapper
    return saved(error)

def main() -> None:
    prefix = "owned: "
    mapper: (IoError) -> str = (error) => f"{prefix}{error.detail}"
    println(render(mapper, IoError(kind="bad", detail="first")))
    println(render(mapper, IoError(kind="bad", detail="second")))
    println(prefix)
"#;
    let module = build(source, &["m", "reader_callable_ownership"])?;
    let main = body_named(&module, "main")?;
    let mapper = main
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("mapper"))
        .ok_or("captured mapper local absent")?
        .id;
    let representation = main
        .callable_representation
        .as_ref()
        .ok_or("checked callable representation absent")?;
    assert!(representation.closure_holding_locals.contains(&mapper));
    assert!(!representation.function_pointer_locals.contains(&mapper));
    let mut facts = Vec::new();
    for statement in &main.block.stmts {
        match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(operand),
                ..
            } => retain_mapper_read(operand, mapper, &mut facts),
            bir::StatementKind::Call { args, .. } => {
                for argument in args {
                    if let Some(operand) = argument.as_one() {
                        retain_mapper_read(operand, mapper, &mut facts);
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        facts,
        [bir::OwnershipFact::Clone, bir::OwnershipFact::Move],
        "{}",
        main.render_snapshot()
    );
    Ok(())
}

/// Inspect only whole reads of this checked local; argument temporaries keep their independent facts.
fn retain_mapper_read(operand: &bir::Operand, mapper: bir::LocalId, facts: &mut Vec<bir::OwnershipFact>) {
    if let bir::Operand::Place(read) = operand
        && read.place.local_id() == Some(mapper)
        && read.place.projection.is_empty()
    {
        facts.push(read.fact);
    }
}
