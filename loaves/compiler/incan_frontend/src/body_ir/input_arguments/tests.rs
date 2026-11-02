//! Inspect direct argument facts and profile refusal independently of full native execution qualification.

use super::bir;

mod conditional;

/// Inspect the checked projection directly: admitted source must have no representation gap, while a deferred
/// profile retains its source diagnostic. Removing identity after checking deliberately tests the next boundary.
fn staged(source: &str, remove_call_proof: bool) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    let checked = checker.check_program(&program);
    if let Err(errors) = &checked {
        assert!(
            errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{errors:?}"
        );
    }
    let mut facts = checker.into_type_info();
    if remove_call_proof {
        for binding in facts.declarations.function_bindings_by_span.values_mut() {
            if binding
                .identity
                .as_ref()
                .is_some_and(|identity| identity.declaration_name == "main")
            {
                binding.identity = None;
            }
        }
    }
    let providers = super::super::ProviderOperationCatalog::new();
    let context = super::super::BodyIrContext {
        modules: &[],
        provider_operations: &providers,
        registry_package: None,
        library_package: None,
    };
    let module = super::super::build_module_seed(&program, &[], &facts, &context);
    if checked.is_ok() && !remove_call_proof {
        for body in &module.bodies {
            assert!(
                body.first_representation_gap().is_none(),
                "{}: {:?}",
                body.name,
                body.first_representation_gap()
            );
        }
    }
    Ok(module)
}

/// Positive execution profiles must pass source checking as well as materialize complete Body IR.
fn admitted(source: &str) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    crate::typechecker::TypeChecker::new()
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    staged(source, false)
}

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

/// A checked nominal Copy decision cannot become a native Move merely because the current carrier ABI lacks
/// that proof. Calls and explicit Owned initializers must refuse until their carrier can preserve the decision.
#[test]
fn checked_copy_cannot_be_reinterpreted_as_native_move() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def consume(value: Owned[Point]) -> int:\n    return value.x\ndef main(value: Owned[Point]) -> int:\n    return consume(value)\n@derive(Copy)\nmodel Point:\n    x: int\n",
        "def main(value: Owned[Point]) -> int:\n    let local: Owned[Point] = value\n    return local.x\n@derive(Copy)\nmodel Point:\n    x: int\n",
    ] {
        let module = staged(source, false)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("missing body")?;
        assert!(
            body.first_representation_gap().is_some(),
            "native carrier silently reinterpreted checked Copy: {body:?}"
        );
        assert!(body.checked_input_profile.is_none());
        for statement in &body.block.stmts {
            assert!(
                !matches!(statement.kind, bir::StatementKind::Call { .. }),
                "refused input emitted a fallback call"
            );
            if let bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Use(bir::Operand::Place(read)),
                ..
            } = &statement.kind
            {
                let local = read.place.local_id().and_then(|id| body.locals.get(id.index()));
                assert!(
                    !local.is_some_and(|local| local.name.as_deref() == Some("value"))
                        || !matches!(read.fact, bir::OwnershipFact::Copy | bir::OwnershipFact::Move),
                    "refused input emitted a fallback value transfer"
                );
            }
        }
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

/// An owned local transfer makes later source use invalid; shared inputs and local loans cannot be consumed.
#[test]
fn owned_local_source_conflicts_remain_typed() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def main() -> int:\n    original = \"payload\"\n    let held: Owned[str] = original\n    return len(original)\n",
        "def read(original: Borrowed[str]) -> int:\n    let held: Owned[str] = original\n    return len(held)\n",
        "def read(original: Owned[str]) -> str:\n    let view: Borrowed[str] = original\n    return view\n",
    ] {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("owned source conflict was accepted")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{errors:?}"
        );
    }
    Ok(())
}

/// A non-Copy alias borrows its original owner for a direct call, then permits transfer after its final use.
#[test]
fn noncopy_local_alias_retains_owner_until_last_use() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "def length(value: Borrowed[str]) -> int:\n    return len(value)\ndef main() -> int:\n    original = \"payload\"\n    let view: Borrowed[str] = original\n    assert length(view) == 7\n    let held: Owned[str] = original\n    return len(held)\n",
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
    assert!(!body.locals.iter().any(|local| local.name.as_deref() == Some("view")));
    Ok(())
}

/// An exclusive model alias projects nested fields into the original aggregate without a second owner.
#[test]
fn noncopy_model_alias_keeps_nested_storage() -> Result<(), Box<dyn std::error::Error>> {
    let module = staged(
        "model Page:\n    title: str\n    count: int\ndef main() -> None:\n    mut page = Page(title=\"old\", count=1)\n    mut view: BorrowedMut[Page] = page\n    view.title = \"new\"\n    view.count += 1\n    assert view.title == \"new\"\n    assert page.count == 2\n",
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
    assert!(!body.locals.iter().any(|local| local.name.as_deref() == Some("view")));
    Ok(())
}

/// Observers consume alias operands after evaluating the other operands, without a hidden owned snapshot.
#[test]
fn noncopy_alias_operation_retains_loan_through_later_arguments() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(view, replace(original))",
        "assert view == replace(original)",
        "print(f\"{view}{replace(original)}\")",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef main() -> None:\n    mut original = \"payload\"\n    let view: Borrowed[str] = original\n    {expression}\n"
        );
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("later mutation released alias too early")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{expression}: {errors:?}"
        );
    }
    Ok(())
}

/// Disjoint mutation is permitted, and a fresh formatted value ends its source dependency before the next argument.
#[test]
fn noncopy_alias_observers_preserve_disjoint_and_completed_reads() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(view)",
        "print(view, replace(other))",
        "print(replace(other), view)",
        "assert view != replace(other)",
        "print(f\"{view}{replace(other)}\")",
        "print(f\"{view}\", replace(original))",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef main() -> None:\n    mut original = \"payload\"\n    mut other = \"other\"\n    let view: Borrowed[str] = original\n    {expression}\n"
        );
        let module = staged(&source, false)?;
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "main")
            .ok_or("missing main")?;
        assert!(
            body.first_representation_gap().is_none(),
            "{expression}: {:?}",
            body.first_representation_gap()
        );
    }
    Ok(())
}

/// Explicit borrowed parameters retain their original storage at a primitive observer, without a repair snapshot.
#[test]
fn borrowed_text_parameter_observers_keep_source_storage() -> Result<(), Box<dyn std::error::Error>> {
    for mode in ["Borrowed", "BorrowedMut"] {
        let module = admitted(&format!(
            "def read(value: {mode}[str]) -> int:\n    return len(value)\n"
        ))?;
        let body = module.bodies.first().ok_or("missing body")?;
        let arguments = body
            .block
            .stmts
            .iter()
            .find_map(|statement| match &statement.kind {
                bir::StatementKind::Call { args, .. } => Some(args),
                _ => None,
            })
            .ok_or("missing observer")?;
        let [bir::ArgumentElement::One(bir::Operand::Place(read))] = arguments.as_slice() else {
            return Err("missing observer operand".into());
        };
        assert_eq!(read.fact, bir::OwnershipFact::Borrow, "{mode}");
        assert_eq!(
            read.place.local_id(),
            body.params.first().map(|parameter| parameter.local),
            "{mode}"
        );
        assert!(!read.last_use);
    }
    Ok(())
}

/// A direct exclusive parameter's observed text remains shared until later operands finish evaluating.
#[test]
fn borrowed_parameter_operation_refuses_later_mutation() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(value, replace(value))",
        "assert value == replace(value)",
        "print(f\"{value}{replace(value)}\")",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str]) -> None:\n    {expression}\n"
        );
        let tokens = crate::lexer::lex(&source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = crate::typechecker::TypeChecker::new();
        let errors = checker
            .check_program(&program)
            .err()
            .ok_or("borrowed operand was silently snapshotted")?;
        assert!(
            errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
            "{expression}: {errors:?}"
        );
    }
    Ok(())
}

/// An exclusive alias cannot use its parent capability to mutate a text operand still shared by an operation.
#[test]
fn exclusive_alias_operation_requires_a_shared_child() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str]) -> None:\n    mut view: BorrowedMut[str] = value\n    print(view, replace(view))\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("exclusive parent bypassed operation read")?;
    assert!(
        errors.iter().any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{errors:?}"
    );
    Ok(())
}

/// Disjoint parameter mutations and completed inner values remain usable; Copy snapshots retain ordinary behavior.
#[test]
fn borrowed_parameter_operations_keep_disjoint_and_copy_behavior() -> Result<(), Box<dyn std::error::Error>> {
    for expression in [
        "print(value, value)",
        "print(value, replace(other))",
        "print(replace(other), value)",
        "assert value != replace(other)",
        "print(f\"{value}{replace(other)}\")",
        "print(f\"{value}\", replace(value))",
    ] {
        let source = format!(
            "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str], mut other: BorrowedMut[str]) -> None:\n    {expression}\n"
        );
        let module = admitted(&source)?;
        assert!(
            module
                .bodies
                .iter()
                .all(|body| body.first_representation_gap().is_none()),
            "{expression}"
        );
    }
    let module = admitted(
        "def bump(mut value: BorrowedMut[int]) -> int:\n    value += 1\n    return value\ndef inspect(mut value: BorrowedMut[int]) -> None:\n    print(value, bump(value))\n",
    )?;
    assert!(
        module
            .bodies
            .iter()
            .all(|body| body.first_representation_gap().is_none())
    );
    Ok(())
}

/// A plain Copy helper keeps its inferred declaration ABI, while an explicit local still gives its caller a profile.
/// Native persistence of that local-only obligation is a separate representation acceptance gate.
#[test]
fn ordinary_copy_helper_retains_caller_profile_and_value_operand() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "def add(value: int) -> int:\n    return value + 1\ndef use(value: int) -> int:\n    let view: Borrowed[int] = value\n    return add(view)\n",
    )?;
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "use")
        .ok_or("missing caller")?;
    let callee = module
        .bodies
        .iter()
        .find(|body| body.name == "add")
        .ok_or("missing callee")?;
    assert!(
        caller
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(
        callee
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(
        caller
            .checked_input_profile
            .as_ref()
            .is_some_and(|profile| profile.version == 1)
    );
    assert!(callee.checked_input_profile.is_none());
    let argument = caller
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Call { args, .. } => args.first(),
            _ => None,
        })
        .ok_or("missing helper call")?;
    assert!(
        matches!(argument, bir::ArgumentElement::One(bir::Operand::Place(read)) if read.fact == bir::OwnershipFact::Copy)
    );
    Ok(())
}

/// An explicit local in an otherwise ordinary body keeps its obligation on the body: losing the checked profile
/// refuses the body and its call to an ordinary helper instead of leaving a body that no longer looks explicit.
#[test]
fn explicit_local_requires_profile_when_callee_is_ordinary() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "def add(value: int) -> int:\n    return value + 1\ndef use(value: int) -> int:\n    let view: Borrowed[int] = value\n    return add(view)\n",
    )?;
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "use")
        .ok_or("missing caller")?;
    let callee = module
        .bodies
        .iter()
        .find(|body| body.name == "add")
        .ok_or("missing callee")?;
    assert!(
        caller
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(
        callee
            .params
            .iter()
            .all(|parameter| parameter.input_access.is_inferred())
    );
    assert!(caller.checked_input_profile.is_some());
    assert!(matches!(
        caller.explicit_input_bindings.as_slice(),
        [bir::ExplicitInputBinding {
            identity: Some(_),
            access: incan_semantics_core::IncanInputAccess::Shared,
            ..
        }]
    ));
    assert!(callee.explicit_input_bindings.is_empty());
    assert!(caller.input_profile_gap().is_none());
    assert!(caller.input_call_profile_gap(callee).is_none());
    let mut stripped = caller.clone();
    stripped.checked_input_profile = None;
    assert!(
        stripped.input_profile_gap().is_some(),
        "explicit local obligation was erased with the profile"
    );
    assert!(stripped.input_call_profile_gap(callee).is_some());
    assert!(stripped.first_representation_gap().is_some());
    Ok(())
}

/// A method body is a checked frame: its receiver and explicit inputs borrow the caller's existing storage, and the
/// caller materializes the receiver from the checked invocation rather than the ordinary receiver route.
#[test]
fn method_frames_borrow_receiver_and_inputs() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "class Counter:\n    pub value: int\n\n    def bump(mut self, mut amount: BorrowedMut[int]) -> None:\n        self.value = self.value + amount\n        amount = 0\n\ndef main() -> int:\n    mut counter = Counter(value=1)\n    mut step = 2\n    counter.bump(step)\n    return counter.value + step\n",
    )?;
    let method = module
        .bodies
        .iter()
        .find(|body| body.name == "bump")
        .ok_or("no method")?;
    let profile = method
        .checked_input_profile
        .as_ref()
        .ok_or("method has no checked profile")?;
    assert_eq!(profile.parameters.len(), 2);
    assert_eq!(
        profile.parameters[1].access,
        incan_semantics_core::IncanInputAccess::Exclusive
    );
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("no caller")?;
    assert!(caller.checked_input_profile.is_some());
    let mut facts = Vec::new();
    for statement in &caller.block.stmts {
        if let bir::StatementKind::Call {
            callee: bir::Callee::Method(_),
            args,
            ..
        } = &statement.kind
        {
            for argument in args {
                let bir::ArgumentElement::One(bir::Operand::Place(read)) = argument else {
                    return Err("method input is not a place".into());
                };
                let local = caller
                    .locals
                    .get(read.place.local_id().ok_or("missing source local")?.index())
                    .ok_or("missing local")?;
                facts.push((local.name.clone(), read.fact));
            }
        }
    }
    assert_eq!(
        facts,
        [
            (Some("counter".into()), bir::OwnershipFact::MutBorrow),
            (Some("step".into()), bir::OwnershipFact::MutBorrow),
        ]
    );
    Ok(())
}

/// A view body returns a borrow of its declared place, and a caller runs the view call for its effects and aliases
/// the substituted place: the later receiver is the original field, not a copy or a reference-valued local.
#[test]
fn returned_views_alias_the_substituted_place() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "class Address:\n    pub city: str\n\n    def set_city(mut self, city: str) -> Reference[Self] from self:\n        self.city = city\n        return self\n\nclass Customer:\n    pub name: str\n    pub address: Address\n\n    def edit_address(mut self) -> Reference[Address] from self.address:\n        return self.address\n\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\"))\n    mut address = customer.edit_address()\n    customer.name = \"Samuel\"\n    address.set_city(\"Rotterdam\")\n    assert customer.address.city == \"Rotterdam\"\n",
    )?;
    let body = |name: &str| {
        module
            .bodies
            .iter()
            .find(|body| body.name == name)
            .ok_or_else(|| format!("missing {name}"))
    };
    let edit = body("edit_address")?;
    let view = edit.returned_view.as_ref().ok_or("edit_address returns no view")?;
    assert_eq!(
        (view.slot, view.access),
        (0, incan_semantics_core::IncanInputAccess::Exclusive)
    );
    let returned = edit
        .block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            bir::StatementKind::Return {
                value: Some(bir::Operand::Place(read)),
            } => Some(read),
            _ => None,
        })
        .ok_or("edit_address returns no place")?;
    assert_eq!(returned.fact, bir::OwnershipFact::MutBorrow);
    assert_eq!(returned.place.projection.len(), 1);
    // A view body returns its source's value type, here both `Address`; the returned view carries the access.
    let address = &body("set_city")?.return_type;
    assert!(
        matches!(address, incan_semantics_core::IncanType::Named(_)),
        "{address:?}"
    );
    assert_eq!(&edit.return_type, address);
    let caller = body("main")?;
    assert!(caller.checked_input_profile.is_some());
    let mut calls = Vec::new();
    for statement in &caller.block.stmts {
        if let bir::StatementKind::Call {
            destination,
            callee: bir::Callee::Method(target),
            args,
            ..
        } = &statement.kind
        {
            let Some(bir::ArgumentElement::One(bir::Operand::Place(receiver))) = args.first() else {
                return Err("method call has no place receiver".into());
            };
            let local = caller
                .locals
                .get(receiver.place.local_id().ok_or("receiver has no local")?.index())
                .ok_or("missing receiver local")?;
            calls.push((
                target.name.clone(),
                destination.is_none(),
                local.name.clone(),
                receiver.place.projection.len(),
            ));
        }
    }
    assert_eq!(
        calls,
        [
            ("edit_address".into(), true, Some("customer".into()), 0),
            ("set_city".into(), true, Some("customer".into()), 1),
        ]
    );
    Ok(())
}

/// A fluent chain continues on the inner view's substituted place: both calls receive the original nested field,
/// and a view body may return another view of exactly its declared source.
#[test]
fn chained_views_continue_on_the_original_field() -> Result<(), Box<dyn std::error::Error>> {
    let module = admitted(
        "class Address:\n    pub city: str\n    pub postcode: str\n\n    def set_city(mut self, city: str) -> Reference[Self] from self:\n        self.city = city\n        return self\n\n    def set_postcode(mut self, postcode: str) -> Reference[Self] from self:\n        self.postcode = postcode\n        return self\n\n    def relocate(mut self) -> Reference[Self] from self:\n        return self.set_city(\"Delft\")\n\nclass Customer:\n    pub name: str\n    pub address: Address\n\ndef main() -> None:\n    mut customer = Customer(name=\"Sam\", address=Address(city=\"Utrecht\", postcode=\"3511\"))\n    customer.address.set_city(\"Rotterdam\").set_postcode(\"3011\")\n    customer.address.relocate()\n    print(customer.address.city)\n",
    )?;
    let caller = module
        .bodies
        .iter()
        .find(|body| body.name == "main")
        .ok_or("missing main")?;
    let mut receivers = Vec::new();
    for statement in &caller.block.stmts {
        if let bir::StatementKind::Call {
            destination: None,
            callee: bir::Callee::Method(target),
            args,
            ..
        } = &statement.kind
            && let Some(bir::ArgumentElement::One(bir::Operand::Place(receiver))) = args.first()
        {
            receivers.push((target.name.clone(), receiver.place.projection.len(), receiver.fact));
        }
    }
    assert_eq!(
        receivers,
        [
            ("set_city".into(), 1, bir::OwnershipFact::MutBorrow),
            ("set_postcode".into(), 1, bir::OwnershipFact::MutBorrow),
            ("relocate".into(), 1, bir::OwnershipFact::MutBorrow),
        ]
    );
    // Discarded view calls run for their effects only; their substituted places are never read as values.
    assert!(!caller.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Expr { value: bir::Operand::Place(read) } if read.fact == bir::OwnershipFact::MutBorrow
    )));
    let relocate = module
        .bodies
        .iter()
        .find(|body| body.name == "relocate")
        .ok_or("missing relocate")?;
    assert!(relocate.returned_view.is_some());
    assert!(relocate.block.stmts.iter().any(|statement| matches!(
        &statement.kind,
        bir::StatementKind::Return { value: Some(bir::Operand::Place(read)) }
            if read.fact == bir::OwnershipFact::MutBorrow && read.place.projection.is_empty()
    )));
    Ok(())
}
