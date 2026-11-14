//! Scalar, owned, copied, nested and constructed input projections and owned local initializers.

use super::*;

/// Shared and exclusive scalar requests retain their source storage despite the carrier being Copy.
#[test]
fn scalar_inputs_borrow_existing_places() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def read(value: Borrowed[int]) -> int:\n    return value\ndef replace(mut value: BorrowedMut[int]) -> None:\n    value = 9\ndef main() -> int:\n    mut value = 3\n    read(value)\n    replace(value)\n    return value\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    let mut modes = Vec::new();
    for statement in &body.block.stmts {
        if let bir::StatementKind::Call { args, .. } = &statement.kind {
            let [bir::ArgumentElement::One(bir::Operand::Place(read))] = args.as_slice() else {
                return Err("missing place argument".into());
            };
            modes.push(read.fact);
            let local = body
                .locals
                .get(read.place.local_id().ok_or("missing source local")?.index())
                .ok_or("missing local")?;
            assert_eq!(local.name.as_deref(), Some("value"));
        }
        assert!(
            !matches!(&statement.kind, bir::StatementKind::Unsupported { .. }),
            "{statement:?}"
        );
    }
    assert_eq!(modes, [bir::OwnershipFact::Borrow, bir::OwnershipFact::MutBorrow]);
    Ok(())
}

/// An owned non-Copy input transfers even when the callee only reads; the caller cannot retain a repair clone.
#[test]
fn owned_input_moves_and_missing_proof_refuses() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def consume(value: Owned[str]) -> bool:\n    return value == \"yes\"\ndef main() -> bool:\n    value = \"yes\"\n    return consume(value)\n";
    let module = staged(source, false)?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    let call = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => Some(args),
            _ => None,
        })
        .ok_or_else(|| format!("no call: {:?}", body.block))?;
    let [bir::ArgumentElement::One(bir::Operand::Place(read))] = call.as_slice() else {
        return Err("missing source operand".into());
    };
    assert_eq!(read.fact, bir::OwnershipFact::Move);
    assert!(read.last_use);
    assert!(!body.block.stmts.iter().any(|statement| matches!(&statement.kind, bir::StatementKind::Drop { local } if Some(*local) == read.place.local_id())));
    let missing = staged(source, true)?;
    let body = missing
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no refused caller")?;
    assert!(
        body.block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { .. }))
    );
    assert!(
        !body
            .block
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Call { .. }))
    );
    Ok(())
}

/// A checked nominal Copy decision materializes as Copy even when the legacy ABI heuristic cannot describe it.
#[test]
fn checked_nominal_copy_materializes_without_legacy_abi_evidence() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def consume(value: Owned[Point]) -> int:\n    return value.x\ndef main(value: Owned[Point]) -> int:\n    return consume(value)\n@derive(Copy)\nmodel Point:\n    x: int\n",
        "def main(value: Owned[Point]) -> int:\n    let local: Owned[Point] = value\n    return local.x\n@derive(Copy)\nmodel Point:\n    x: int\n",
    ] {
        let module = admitted(source)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("missing body")?;
        assert!(
            body.first_representation_gap().is_none(),
            "checked Copy left a representation gap: {body:?}"
        );
        assert!(body.checked_input_profile.is_some());
        assert!(body.block.stmts.iter().any(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                ..
            } => read.fact == bir::OwnershipFact::Copy,
            bir::StatementKind::Call { args, .. } => args.iter().any(|argument| {
                matches!(argument, bir::ArgumentElement::One(bir::Operand::Place(read)) if read.fact == bir::OwnershipFact::Copy)
            }),
            _ => false,
        }));
    }
    let module = admitted(
        "def consume(value: Owned[&mut int]) -> None:\n    pass\ndef main(value: Owned[&mut int]) -> None:\n    consume(value)\n",
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing reference body")?;
    let transferred = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => match args.as_slice() {
                [bir::ArgumentElement::One(bir::Operand::Place(read))] => Some(read),
                _ => None,
            },
            _ => None,
        })
        .ok_or("missing reference transfer")?;
    assert_eq!(transferred.fact, bir::OwnershipFact::Move);
    Ok(())
}

/// Literal owners are retained for the invocation and named operands preserve declaration order plus written indices.
#[test]
fn named_temporary_inputs_retain_binding_provenance() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def total(first: Borrowed[int], second: Borrowed[int]) -> int:\n    return first + second\ndef main() -> int:\n    return total(second=2, first=1)\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    let (args, target) = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call {
                args,
                callee: bir::Callee::Function(bir::CallableTarget::Named(target)),
                ..
            } => Some((args, target)),
            _ => None,
        })
        .ok_or_else(|| format!("no named call: {:?}", body.block))?;
    let bir::ArgumentBinding::ResolvedDeclarationOrder {
        arguments,
        defaulted_slots,
    } = &target.binding
    else {
        return Err("wrong evaluation policy".into());
    };
    assert!(defaulted_slots.is_empty());
    assert_eq!(
        arguments
            .iter()
            .map(|argument| (argument.slot, argument.written_position))
            .collect::<Vec<_>>(),
        [(0, 1), (1, 0)]
    );
    assert_eq!(args.len(), 2);
    for argument in args {
        let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument else {
            return Err("temporary lacks storage".into());
        };
        assert_eq!(read.fact, bir::OwnershipFact::Borrow);
        let local = body
            .locals
            .get(read.place.local_id().ok_or("no temporary local")?.index())
            .ok_or("no local")?;
        assert_eq!(local.origin, bir::LocalOrigin::Temporary);
    }
    Ok(())
}

/// Nested stored fields retain canonical place identity, and a branch move executes under the conditional profile,
/// whose path-correct drops record which path retained the owner. Neither case may fall back to a synthesized copy.
#[test]
fn nested_borrow_preserves_projection_and_conditional_move_executes() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "model Point:\n    x: int\ndef replace(mut value: BorrowedMut[int]) -> None:\n    value = 7\ndef apply(mut point: BorrowedMut[Point]) -> int:\n    replace(point.x)\n    return point.x\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "apply")
        .ok_or("no nested caller")?;
    let read = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => match args.as_slice() {
                [bir::ArgumentElement::One(bir::Operand::Place(read))] => Some(read),
                _ => None,
            },
            _ => None,
        })
        .ok_or_else(|| format!("no projected call: {:?}", body.block))?;
    assert_eq!(read.fact, bir::OwnershipFact::MutBorrow);
    assert!(matches!(
        read.place.projection.as_slice(),
        [bir::PlaceElem::Field {
            canonical: Some(_),
            structural: false,
            ..
        }]
    ));
    let module = staged(
        "def consume(value: Owned[str]) -> bool:\n    return value == \"yes\"\ndef apply(flag: bool) -> None:\n    value = \"yes\"\n    if flag:\n        consume(value)\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "apply")
        .ok_or("no conditional caller")?;
    let branch = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::If { then_block, .. } => Some(then_block),
            _ => None,
        })
        .ok_or_else(|| format!("no branch: {:?}", body.block))?;
    assert!(
        branch
            .stmts
            .iter()
            .any(|statement| matches!(&statement.kind, bir::StatementKind::Call { .. })),
        "{branch:?}"
    );
    assert!(
        body.checked_input_profile
            .as_ref()
            .is_some_and(|profile| profile.version == 2)
    );
    assert!(body.first_representation_gap().is_none());
    Ok(())
}

/// A checked scalar loan names its existing field place for reads and both write forms, without a copied alias slot.
#[test]
fn local_scalar_alias_projects_existing_field_storage() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "model Settings:\n    attempts: int\ndef update(mut settings: BorrowedMut[Settings]) -> int:\n    mut alias: BorrowedMut[int] = settings.attempts\n    alias = 3\n    alias += 2\n    return alias\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "update")
        .ok_or("no caller")?;
    assert!(!body.locals.iter().any(|local| local.name.as_deref() == Some("alias")));
    let mut writes = 0;
    for statement in &body.block.stmts {
        if let bir::StatementKind::Assign { place, .. } = &statement.kind
            && !place.projection.is_empty()
        {
            let local = place
                .local_id()
                .and_then(|id| body.locals.get(id.index()))
                .ok_or("missing owner")?;
            assert_eq!(local.name.as_deref(), Some("settings"));
            assert!(matches!(
                place.projection.as_slice(),
                [bir::PlaceElem::Field { canonical: Some(_), .. }]
            ));
            writes += 1;
        }
    }
    assert_eq!(writes, 2, "{:?}", body.block);
    assert!(body.first_representation_gap().is_none());
    Ok(())
}

/// Missing body authority cannot turn a written local loan into the old copied-binding operation.
#[test]
fn missing_local_alias_proof_never_creates_owned_storage() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def main() -> int:\n    mut value = 1\n    mut view: BorrowedMut[int] = value\n    view = 2\n    return view\n",
        true,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    assert!(!body.locals.iter().any(|local| local.name.as_deref() == Some("view")));
    assert!(body.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Unsupported { description } if description.contains("no owning source body proof")
    )));
    Ok(())
}

/// Complete fresh model construction and primitive formatting preserve the nested storage passed to a direct input.
#[test]
fn constructed_nested_models_keep_direct_input_authority() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "model Address:\n    city: str\n    postcode: str\nmodel Customer:\n    address: Address\n    name: str\ndef rename(mut city: BorrowedMut[str], replacement: Borrowed[str]) -> None:\n    city = f\"{replacement}\"\ndef main() -> None:\n    mut customer = Customer(address=Address(city=\"Delft\", postcode=\"2611\"), name=\"Ada\")\n    replacement = \"Rotterdam\"\n    rename(customer.address.city, replacement)\n    assert customer.address.city == \"Rotterdam\"\n    assert customer.address.postcode == \"2611\"\n    assert replacement == \"Rotterdam\"\n",
        false,
    )?;
    for body in &module.bodies {
        assert!(
            body.first_representation_gap().is_none(),
            "{}: {:?}",
            body.name,
            body.first_representation_gap()
        );
    }
    Ok(())
}

/// A model's omitted field computation and a non-Copy source field transfer need their own access/storage proof.
#[test]
fn construction_defaults_and_noncopy_place_fields_stay_refused() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "model Page:\n    title: str = \"default\"\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef main() -> int:\n    page = Page()\n    return read(1)\n",
        "model Page:\n    title: str\ndef read(value: Borrowed[int]) -> int:\n    return value\ndef main() -> int:\n    title = \"stored\"\n    page = Page(title=title)\n    return read(1)\n",
    ] {
        let module = staged(source, false)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("no caller")?;
        assert!(body.first_representation_gap().is_some());
    }
    Ok(())
}

/// A final scalar alias use ends its checked loan, permitting a later original-field write without copied storage.
#[test]
fn local_alias_last_use_preserves_the_original_owner() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "model Settings:\n    attempts: int\n    label: str\ndef main() -> None:\n    mut settings = Settings(attempts=1, label=\"initial\")\n    mut attempts: BorrowedMut[int] = settings.attempts\n    attempts = 3\n    attempts += 2\n    assert attempts == 5\n    settings.attempts += 1\n    assert settings.attempts == 6\n    assert settings.label == \"initial\"\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    assert!(
        body.first_representation_gap().is_none(),
        "{:?}; {:?}",
        body.first_representation_gap(),
        body.block
    );
    assert!(
        !body
            .locals
            .iter()
            .any(|local| local.name.as_deref() == Some("attempts"))
    );
    Ok(())
}

/// A fresh scalar list is transferred as one owner, while a builtin length observation only borrows its storage.
#[test]
fn owned_fresh_list_retains_length_observation() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def consume(values: Owned[list[int]]) -> int:\n    return len(values)\ndef main() -> None:\n    values = [1, 2, 3]\n    assert consume(values) == 3\n",
        false,
    )?;
    for body in &module.bodies {
        assert!(
            body.first_representation_gap().is_none(),
            "{}: {:?}",
            body.name,
            body.first_representation_gap()
        );
    }
    Ok(())
}

/// An explicit owned local receives the original non-Copy owner instead of an inferred snapshot clone.
#[test]
fn owned_local_initialization_retains_transfer_authority() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def main() -> int:\n    original = \"payload\"\n    let held: Owned[str] = original\n    return len(held)\n",
        false,
    )?;
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no main body")?;
    assert!(
        body.first_representation_gap().is_none(),
        "{:?}",
        body.first_representation_gap()
    );
    let held = body
        .locals
        .iter()
        .find(|local| local.name.as_deref() == Some("held"))
        .ok_or("no owned local")?;
    let initializer = body
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
            } if place.local_id() == Some(held.id) => Some(read),
            _ => None,
        })
        .ok_or("no owned transfer initializer")?;
    assert_eq!(initializer.fact, bir::OwnershipFact::Move);
    assert!(initializer.place.projection.is_empty());
    let source = initializer
        .place
        .local_id()
        .and_then(|local| body.locals.get(local.index()))
        .ok_or("no source owner")?;
    assert_eq!(source.name.as_deref(), Some("original"));
    Ok(())
}

/// Copy initialization keeps the source available, and a fresh owned initializer does not invent a source loan.
#[test]
fn owned_local_copy_and_fresh_initializers_remain_distinct() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def main() -> int:\n    original = 3\n    mut held: Owned[int] = original\n    held = 9\n    assert original == 3\n    return held\n",
        "def main() -> int:\n    let held: Owned[list[int]] = [1, 2]\n    return len(held)\n",
        "def read(value: Borrowed[int]) -> int:\n    mut held: Owned[int] = (value)\n    held = 9\n    assert value == 3\n    return held\n",
    ] {
        let module = staged(source, false)?;
        for body in &module.bodies {
            assert!(
                body.first_representation_gap().is_none(),
                "{}: {:?}",
                body.name,
                body.first_representation_gap()
            );
        }
    }
    Ok(())
}

/// A local ownership annotation cannot authorize a missing body proof or partial non-Copy transfer.
#[test]
fn owned_locals_keep_missing_proof_and_projected_transfer_refusals() -> Result<(), Box<dyn std::error::Error>> {
    for (source, missing_proof) in [
        (
            "def main() -> int:\n    original = \"payload\"\n    let held: Owned[str] = original\n    return len(held)\n",
            true,
        ),
        (
            "model Page:\n    title: str\ndef main(page: Owned[Page]) -> int:\n    let held: Owned[str] = page.title\n    return len(held)\n",
            false,
        ),
    ] {
        let module = staged(source, missing_proof)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("no main body")?;
        assert!(body.first_representation_gap().is_some());
    }
    Ok(())
}
