//! Conditional owner bookkeeping is source-tested while native profile admission remains closed.

use super::{bir, staged};

/// Count actual whole-owner call transfers recursively; unsupported operands are not successful transfers.
fn transfers(block: &bir::Block, owner: bir::LocalId) -> usize {
    block
        .stmts
        .iter()
        .map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => args
                .iter()
                .filter(|argument| {
                    matches!(argument,
            bir::ArgumentElement::One(bir::Operand::Place(read))
            if read.place.local_id() == Some(owner) && read.fact == bir::OwnershipFact::Move)
                })
                .count(),
            bir::StatementKind::If {
                then_block, else_block, ..
            } => transfers(then_block, owner) + else_block.as_ref().map_or(0, |block| transfers(block, owner)),
            _ => 0,
        })
        .sum()
}

/// One consuming branch cannot suppress the outer lexical drop on the other continuing path. Both source orderings,
/// nested branches, early returns and a prior move followed by partial reinitialization retain the same obligation.
#[test]
fn conditional_transfers_keep_path_correct_outer_drops() -> Result<(), Box<dyn std::error::Error>> {
    let cases = [
        ("    if flag:\n        consume(value)\n", true, 1),
        (
            "    if flag:\n        pass\n    else:\n        consume(value)\n",
            true,
            1,
        ),
        (
            "    if flag:\n        consume(value)\n    else:\n        consume(value)\n",
            false,
            2,
        ),
        (
            "    if flag:\n        if other:\n            consume(value)\n    else:\n        pass\n",
            true,
            1,
        ),
        ("    if flag:\n        consume(value)\n        return\n", true, 1),
        ("    consume(value)\n    if flag:\n        value = \"fresh\"\n", true, 1),
        (
            "    consume(value)\n    if flag:\n        pass\n    else:\n        value = \"fresh\"\n",
            true,
            1,
        ),
        (
            "    consume(value)\n    if flag:\n        value = \"fresh\"\n    else:\n        value = \"other\"\n",
            true,
            1,
        ),
    ];
    for (branches, drop, moves) in cases {
        let source = format!(
            "def consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool, other: bool) -> None:\n    mut value = \"original\"\n{branches}"
        );
        let module = staged(&source, false)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("missing caller")?;
        let owner = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("value"))
            .ok_or("missing owner")?
            .id;
        assert_eq!(transfers(&body.block, owner), moves, "{branches}\n{:?}", body.block);
        assert_eq!(
            body.block.stmts.iter().any(|statement| matches!(statement.kind,
            bir::StatementKind::Drop { local } if local == owner)),
            drop,
            "{branches}\n{:?}",
            body.block
        );
        assert!(
            body.checked_input_profile.is_none(),
            "conditional runtime admission remains closed"
        );
        assert!(body.first_representation_gap().is_some());
    }
    Ok(())
}

/// A reinitialized branch cannot authorize a read on an uninitialized continuing path. Replacing on every path
/// removes that conflict, but still does not grant the staged conditional execution profile.
#[test]
fn conditional_reinitialization_requires_every_continuing_path() -> Result<(), Box<dyn std::error::Error>> {
    for (branches, conflict) in [
        ("    if flag:\n        value = \"fresh\"\n", true),
        (
            "    if flag:\n        value = \"fresh\"\n    else:\n        value = \"other\"\n",
            false,
        ),
        (
            "    if flag:\n        return\n    else:\n        value = \"fresh\"\n",
            false,
        ),
    ] {
        let source = format!(
            "def consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    mut value = \"original\"\n    consume(value)\n{branches}    print(value)\n"
        );
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let errors = crate::typechecker::TypeChecker::new()
            .check_program(&program)
            .err()
            .ok_or("expected staged refusal")?;
        assert_eq!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            conflict,
            "{branches}\n{errors:?}"
        );
        assert!(errors.iter().any(|error| error.stable_code() == Some("INCAN-T0123")));
    }
    Ok(())
}

/// Parameters and explicit locals share cleanup by canonical storage identity. A branch-local Owned initializer
/// keeps its own lexical drop, while the source owner remains eligible on the non-consuming branch.
#[test]
fn conditional_subjects_distinguish_outer_and_branch_storage() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def consume(value: Owned[str]) -> None:\n    pass\ndef main(value: Owned[str], flag: bool) -> None:\n    if flag:\n        consume(value)\n",
        "def consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    original = \"initial\"\n    let value: Owned[str] = original\n    if flag:\n        consume(value)\n",
        "def main(flag: bool) -> None:\n    value = \"initial\"\n    if flag:\n        let held: Owned[str] = value\n        pass\n",
    ] {
        let module = staged(source, false)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("missing caller")?;
        let owner = body
            .locals
            .iter()
            .find(|local| local.name.as_deref() == Some("value"))
            .ok_or("missing owner")?
            .id;
        assert!(
            body.block
                .stmts
                .iter()
                .any(|statement| matches!(statement.kind, bir::StatementKind::Drop { local } if local == owner)),
            "{source}\n{:?}",
            body.block
        );
        let branch = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::If { then_block, .. } => Some(then_block),
                _ => None,
            })
            .ok_or("missing branch")?;
        assert!(
            !branch
                .stmts
                .iter()
                .any(|statement| matches!(statement.kind, bir::StatementKind::Unsupported { .. })),
            "{branch:?}"
        );
        if let Some(held) = body.locals.iter().find(|local| local.name.as_deref() == Some("held")) {
            assert_eq!(held.scope, branch.scope);
            assert!(
                branch
                    .stmts
                    .iter()
                    .any(|statement| matches!(statement.kind, bir::StatementKind::Drop { local } if local == held.id))
            );
        }
        assert!(body.checked_input_profile.is_none());
    }
    Ok(())
}

/// An elif condition executes only after prior arms fail. Its move affects the later false path but cannot remove
/// the lexical drop from the earlier non-consuming arm; empty arms do not change that distinction.
#[test]
fn conditional_elif_keeps_pending_false_path_state() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def consume(value: Owned[str]) -> bool:\n    return True\ndef main(flag: bool) -> None:\n    value = \"initial\"\n    if flag:\n        pass\n    elif consume(value):\n        pass\n    else:\n        pass\n";
    let module = staged(source, false)?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing caller")?;
    let owner = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("value"))
        .ok_or("missing owner")?
        .id;
    assert_eq!(transfers(&body.block, owner), 1);
    assert!(
        body.block
            .stmts
            .iter()
            .any(|statement| matches!(statement.kind, bir::StatementKind::Drop { local } if local == owner))
    );
    assert!(body.checked_input_profile.is_none());
    Ok(())
}
