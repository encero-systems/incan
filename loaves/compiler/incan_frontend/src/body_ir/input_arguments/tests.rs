//! Inspect staged argument facts without enabling source or native execution of ownership declarations.

use super::bir;

/// Retain the closed execution diagnostic, then inspect the compiler's staged Body IR projection directly.
fn staged(source: &str, remove_call_proof: bool) -> Result<bir::BodyIrModule, Box<dyn std::error::Error>> {
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = crate::typechecker::TypeChecker::new();
    let errors = checker.check_program(&program).err().ok_or("execution gate opened")?;
    assert!(
        errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
        "{errors:?}"
    );
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
    Ok(super::super::build_module_seed(&program, &[], &facts, &context))
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

/// Nested stored fields retain canonical place identity; a branch move is still refused until drop obligations
/// represent which path retained the owner. Neither case may fall back to a synthesized owner copy.
#[test]
fn nested_borrow_preserves_projection_and_conditional_move_refuses() -> Result<(), Box<dyn std::error::Error>> {
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
    assert!(branch.stmts.iter().any(|statement| matches!(&statement.kind, bir::StatementKind::Unsupported { description } if description.contains("drop obligations"))), "{branch:?}");
    Ok(())
}
