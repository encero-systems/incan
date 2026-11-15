//! Body projection evidence, alias restoration, operation consumption, conditional paths and assertions.

use super::*;

/// The body owner is a canonical input, not a span-table lookup. A dependency module with an equal source range
/// cannot turn the current callable's borrowed parameter into an ordinary transferable one.
#[test]
fn flow_owner_is_independent_of_shared_span_tables() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def read(value: Borrowed[int]) -> int:\n    return value\n";
    let mut checker = checked(source)?;
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program.declarations.first().ok_or("missing declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("expected function".into());
    };
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .next()
        .cloned()
        .ok_or("missing owner")?;
    let binding = checker
        .type_info
        .declarations
        .function_bindings_by_span
        .get_mut(&declaration.span)
        .ok_or("missing binding")?;
    let mut other = owner.clone();
    other.origin = incan_semantics_core::SymbolOrigin::Module(vec!["other".into()]);
    binding.identity = Some(other);
    let graph = super::source::build(&checker, CallableSource::Function(function), declaration.span, &owner)
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(graph.permissions.len(), 1);
    assert!(
        graph
            .permissions
            .values()
            .all(|permission| !permission.transferable && !permission.exclusive)
    );
    Ok(())
}

/// A retained map entry alone cannot authenticate an annotation, destination, graph owner or analysis result.
#[test]
fn body_projection_requires_complete_canonical_local_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let checker =
        checked("def read(value: Borrowed[int]) -> int:\n    let view: Borrowed[int] = value\n    return view\n")?;
    let (local, mode) = checker
        .type_info
        .protocols
        .explicit_local_modes
        .iter()
        .next()
        .ok_or("missing local")?;
    let owner = &mode.owner;
    let proof = checker.type_info.checked_input_body(owner)?.ok_or("missing proof")?;
    assert_eq!(proof.local_mode(local), Some(OwnershipMode::Borrowed));
    let mut other = local.clone();
    other.declaration_name = "foreign".into();
    assert_eq!(proof.local_mode(&other), None);
    for corruption in 0..8 {
        let mut facts = checker.type_info.clone();
        match corruption {
            0 => {
                facts.protocols.explicit_input_flow.remove(owner);
            }
            1 => {
                facts.protocols.explicit_local_modes.remove(local);
            }
            2 => {
                facts
                    .protocols
                    .explicit_local_modes
                    .get_mut(local)
                    .ok_or("missing mode")?
                    .owner
                    .declaration_name = "foreign".into();
            }
            3 => {
                facts
                    .protocols
                    .explicit_local_modes
                    .get_mut(local)
                    .ok_or("missing mode")?
                    .value_type = ResolvedType::Str;
            }
            4 => {
                facts
                    .references
                    .resolved_write_identities
                    .remove(&(local.declaration_span, local.declaration_name.clone()));
            }
            _ => {
                let InputFlow::Checked { graph, analysis } = facts
                    .protocols
                    .explicit_input_flow
                    .get_mut(owner)
                    .ok_or("missing flow")?
                else {
                    return Err("expected checked flow".into());
                };
                match corruption {
                    5 => graph.local_loans.clear(),
                    6 => analysis.live_before.clear(),
                    _ => graph.owner.declaration_name = "foreign".into(),
                }
            }
        }
        assert!(
            facts.checked_input_body(owner).is_err(),
            "accepted corrupted evidence {corruption}"
        );
    }
    Ok(())
}

/// Restored graphs cannot omit an operand's consumption dependency and recompute themselves into acceptance.
#[test]
fn restored_alias_flow_requires_operation_consumption() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef main() -> None:\n    mut original = \"payload\"\n    let view: Borrowed[str] = original\n    print(view, replace(original))\n";
    let mut checker = checked(source)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126"))
    );
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "main" => Some(function),
            _ => None,
        })
        .ok_or("missing main")?;
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "main")
        .cloned()
        .ok_or("missing owner")?;
    let InputFlow::Checked { graph, analysis } = checker
        .type_info
        .protocols
        .explicit_input_flow
        .get_mut(&owner)
        .ok_or("missing flow")?
    else {
        return Err("missing checked graph".into());
    };
    let loan = graph
        .operations
        .values()
        .flat_map(|operation| &operation.operands)
        .next()
        .ok_or("missing operation loan")?
        .loan;
    let consumption = graph
        .nodes
        .iter_mut()
        .rev()
        .find(|node| {
            matches!(node.event,
                flow::Event::Access { access: flow::Access::Read, through: Some(id), .. } if id == loan
            )
        })
        .ok_or("missing consumption read")?;
    consumption.event = flow::Event::Nothing;
    graph.operations.clear();
    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
    assert!(analysis.conflicts.is_empty());
    assert!(
        checker.type_info.direct_input_admission(&owner, function).is_err(),
        "stripped operation dependency admitted an invalid alias read"
    );
    Ok(())
}

/// Existing operation metadata cannot authorize missing, reused or changed consumption events.
#[test]
fn operation_consumption_rejects_altered_relationships() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def read(value: Borrowed[str]) -> int:\n    let view: Borrowed[str] = value\n    return len(view)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let crate::ast::Declaration::Function(function) = &program.declarations.first().ok_or("missing declaration")?.node
    else {
        return Err("expected function".into());
    };
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .next()
        .cloned()
        .ok_or("missing owner")?;
    for corruption in 0..10 {
        let mut facts = checker.type_info.clone();
        let InputFlow::Checked { graph, analysis } = facts
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked graph".into());
        };
        let inputs = graph.operations.values_mut().next().ok_or("missing operation")?;
        let dependency = inputs.operands.first_mut().ok_or("missing operand")?;
        match corruption {
            0 => graph.operations.clear(),
            1 => graph.nodes[dependency.final_access].event = flow::Event::Nothing,
            2 => dependency.final_access = inputs.completion_node,
            3 => inputs.completion_node = 0,
            4 => dependency.operand.start += 1,
            5 => dependency.loan = flow::LoanId(graph.loans.len()),
            6 => {
                let flow::Root::Binding(root) = &mut dependency.source.root else {
                    return Err("expected declared source".into());
                };
                root.declaration_name = "foreign".into();
            }
            7 => {
                let parent = graph.loans[dependency.loan.0].parent.ok_or("missing parent")?;
                dependency.loan = parent;
                let flow::Event::Access { through, .. } = &mut graph.nodes[dependency.final_access].event else {
                    return Err("missing final read".into());
                };
                *through = Some(parent);
            }
            8 => graph.loans[dependency.loan.0].parent = None,
            _ => graph.loans[dependency.loan.0].kind = flow::LoanKind::Exclusive,
        }
        // Recompute when structurally possible; retained analysis alone must not be the refusal mechanism.
        if let Ok(recomputed) = flow::analyze(graph) {
            *analysis = recomputed;
        }
        assert!(
            facts.direct_input_admission(&owner, function).is_err(),
            "accepted corruption {corruption}"
        );
    }
    Ok(())
}

/// A restored graph cannot move an earlier text read past a later mutation to erase the source-order conflict.
#[test]
fn operation_reads_cannot_move_after_later_operands() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str]) -> None:\n    print(value, replace(value))\n",
        "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut value: BorrowedMut[str], flag: bool) -> None:\n    assert flag, f\"{value}{replace(value)}\"\n    print(value)\n",
    ] {
        let mut checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126"))
        );
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let function = program
            .declarations
            .iter()
            .find_map(|declaration| match &declaration.node {
                crate::ast::Declaration::Function(function) if function.name == "inspect" => Some(function),
                _ => None,
            })
            .ok_or("missing inspect")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "inspect")
            .cloned()
            .ok_or("missing owner")?;
        let original = checker.type_info.clone();
        for corruption in 0..5 {
            checker.type_info = original.clone();
            let InputFlow::Checked { graph, analysis } = checker
                .type_info
                .protocols
                .explicit_input_flow
                .get_mut(&owner)
                .ok_or("missing flow")?
            else {
                return Err("missing checked flow".into());
            };
            let dependency = graph
                .operations
                .values()
                .flat_map(|operation| &operation.operands)
                .next()
                .ok_or("missing operand")?;
            let origin = graph
                .nodes
                .iter()
                .position(|node| matches!(node.event, flow::Event::Borrow(id) if id == dependency.loan))
                .ok_or("missing creation")?;
            let read = origin.checked_sub(1).ok_or("missing evaluated read")?;
            let invocation = graph.invocations.values().next().ok_or("missing nested call")?;
            let call_start = invocation
                .arguments
                .first()
                .ok_or("missing input")?
                .evaluation
                .start_node;
            let call_end = *invocation.final_accesses.last().ok_or("missing final input use")?;
            // Move the complete nested invocation, including its evaluation markers, around the first operand.
            if corruption == 0 {
                let before = read.checked_sub(1).ok_or("missing preceding node")?;
                graph.nodes[before].successors = vec![call_start];
                graph.nodes[call_end].successors = vec![read];
                graph.nodes[origin].successors = vec![call_end + 1];
            } else if corruption == 1 {
                graph.nodes[read..=call_end].rotate_left(call_start - read);
                let count = graph.nodes.len();
                for (index, node) in graph.nodes.iter_mut().enumerate() {
                    node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
                }
            } else if corruption == 2 {
                graph.nodes[graph.entry].successors.clear();
            } else if corruption == 3 {
                let operation = graph.operations.values().next().ok_or("missing operation")?;
                let last = operation.operands.last().ok_or("missing final operand")?.final_access;
                let continuation = graph.nodes[last].successors.clone();
                graph.nodes[origin].successors = vec![operation.completion_node];
                graph.nodes[last].successors = vec![call_start];
                graph.nodes[call_end].successors = continuation;
            } else {
                graph.nodes[graph.entry].successors = vec![graph.entry];
            }
            *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
            assert!(analysis.conflicts.is_empty(), "{analysis:?}");
            assert!(
                checker.type_info.direct_input_admission(&owner, function).is_err(),
                "accepted invalid operation execution (corruption {corruption})"
            );
        }
    }
    Ok(())
}

/// Conditional path evidence is checked independently, and a body whose conditional paths project completely
/// executes under the conditional profile.
#[test]
fn conditional_paths_require_their_source_edges() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def inspect(value: Borrowed[str], flag: bool) -> None:\n    if flag:\n        pass\n    print(value)\n",
        "def inspect(value: Borrowed[str]) -> None:\n    if true:\n        pass\n    else:\n        pass\n    print(value)\n",
        "def inspect(value: Borrowed[str], flag: bool, other: bool) -> None:\n    if flag:\n        print(value)\n    elif other:\n        assert other, f\"failed: {value}\"\n        return\n    else:\n        pass\n    print(value)\n",
        "def inspect(value: Borrowed[str], flag: bool) -> None:\n    if flag:\n        print(value)\n        return\n    else:\n        print(value)\n        return\n    print(value)\n",
        "def inspect(value: Borrowed[str], flag: bool, other: bool) -> None:\n    if flag:\n        if other:\n            print(value)\n        else:\n            pass\n    else:\n        assert true, \"literal\"\n    print(value)\n",
    ] {
        let checker = checked(source)?;
        assert!(
            !checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "{source}: {:?}",
            checker.errors
        );
        assert!(
            !checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0126"))
        );
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let declaration = program.declarations.first().ok_or("missing declaration")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("missing function".into());
        };
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .next()
            .ok_or("missing owner")?;
        checker
            .type_info
            .checked_input_body(owner)?
            .ok_or("missing proof")?
            .validate_operation_coverage(CallableSource::Function(function))?;
        let InputFlow::Checked { graph, .. } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get(owner)
            .ok_or("missing graph")?
        else {
            return Err("missing checked graph".into());
        };
        for branch in graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| (node.successors.len() >= 2).then_some(index))
        {
            for reorder in [false, true] {
                if reorder && graph.nodes[branch].successors[0] == graph.nodes[branch].successors[1] {
                    continue;
                }
                let mut facts = checker.type_info.clone();
                let InputFlow::Checked { graph, analysis } = facts
                    .protocols
                    .explicit_input_flow
                    .get_mut(owner)
                    .ok_or("missing flow")?
                else {
                    return Err("missing checked flow".into());
                };
                if reorder {
                    graph.nodes[branch].successors.swap(0, 1);
                } else {
                    graph.nodes[branch].successors.remove(0);
                }
                *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
                assert!(analysis.conflicts.is_empty());
                assert!(
                    facts
                        .checked_input_body(owner)?
                        .ok_or("missing proof")?
                        .validate_operation_coverage(CallableSource::Function(function))
                        .is_err(),
                    "accepted altered branch {branch}"
                );
            }
        }
    }
    Ok(())
}

/// Failure-only message evaluation cannot become unconditional, skip its dependency, or join the success path.
#[test]
fn assertion_execution_retains_failure_edges() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def inspect(value: Borrowed[str], flag: bool) -> None:\n    assert flag, f\"failed: {value}\"\n    print(value)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program.declarations.first().ok_or("missing declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("missing function".into());
    };
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .next()
        .ok_or("missing owner")?;
    for corruption in 0..4 {
        let mut facts = checker.type_info.clone();
        let InputFlow::Checked { graph, analysis } = facts
            .protocols
            .explicit_input_flow
            .get_mut(owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let condition = graph
            .nodes
            .iter()
            .position(|node| node.successors.len() == 2)
            .ok_or("missing assertion split")?;
        let failure = graph.nodes[condition].successors[0];
        let success = graph.nodes[condition].successors[1];
        let message = graph.operations.values().next().ok_or("missing message operation")?;
        let tail = message
            .operands
            .last()
            .ok_or("missing message dependency")?
            .final_access;
        match corruption {
            0 => graph.nodes[tail].successors = vec![success],
            1 => {
                graph.nodes[condition].successors = vec![failure];
                graph.nodes[tail].successors = vec![success];
            }
            2 => graph.nodes[condition].successors = vec![success],
            _ => graph.nodes[condition].successors.swap(0, 1),
        }
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty(), "{analysis:?}");
        assert!(
            facts.direct_input_admission(owner, function).is_err(),
            "accepted assertion edge corruption {corruption}"
        );
    }
    Ok(())
}
