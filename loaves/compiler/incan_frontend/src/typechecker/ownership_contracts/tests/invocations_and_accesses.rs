//! Invocation projection, argument evaluation order, source accesses, copy evidence and transfer inventories.

use super::*;

/// Fixed named calls retain declaration-order evaluation and distinct written positions, slots and source loans.
#[test]
fn invocation_projection_retains_slots_and_loans() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "def choose(first: Borrowed[int], second: Borrowed[int]) -> int:\n    return first + second\ndef use(left: int, right: int) -> int:\n    return choose(second=right, first=left)\n",
    )?;
    let (owner, output) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "use")
        .ok_or("missing caller flow")?;
    let InputFlow::Checked { graph, analysis } = output else {
        return Err("caller flow unavailable".into());
    };
    assert!(analysis.conflicts.is_empty());
    let (span, invocation) = graph.invocations.iter().next().ok_or("missing invocation")?;
    assert_eq!(
        invocation
            .arguments
            .iter()
            .map(|argument| argument.slot)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    assert_eq!(
        invocation
            .arguments
            .iter()
            .map(|argument| argument.written_position)
            .collect::<Vec<_>>(),
        [1, 0]
    );
    assert_ne!(invocation.arguments[0].loan, invocation.arguments[1].loan);
    assert!(checker.type_info.checked_input_body(owner)?.is_some());
    for corruption in 0..6 {
        let mut facts = checker.type_info.clone();
        let InputFlow::Checked { graph, .. } = facts
            .protocols
            .explicit_input_flow
            .get_mut(owner)
            .ok_or("missing flow")?
        else {
            return Err("expected checked flow".into());
        };
        let invocation = graph.invocations.get_mut(span).ok_or("missing invocation")?;
        match corruption {
            0 => invocation.callee.declaration_name = "foreign".into(),
            1 => invocation.arguments[0].slot = invocation.arguments[1].slot,
            2 => invocation.arguments[0].loan = Some(flow::LoanId(usize::MAX)),
            3 => {
                invocation.arguments[0].access = incan_semantics_core::IncanInputAccess::Owned;
                invocation.arguments[0].loan = None;
            }
            4 => invocation.arguments[0].source = None,
            _ => {
                facts
                    .protocols
                    .explicit_input_calls
                    .get_mut(span)
                    .ok_or("missing call facts")?
                    .owner = None;
            }
        }
        assert!(
            facts.checked_input_body(owner).is_err(),
            "accepted corrupted invocation {corruption}"
        );
    }
    let mut facts = checker.type_info.clone();
    let InputFlow::Checked { graph, analysis } = facts
        .protocols
        .explicit_input_flow
        .get_mut(owner)
        .ok_or("missing caller flow")?
    else {
        return Err("caller flow unavailable".into());
    };
    graph
        .invocations
        .get_mut(span)
        .ok_or("missing invocation")?
        .arguments
        .swap(0, 1);
    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
    assert!(analysis.conflicts.is_empty());
    let reordered_graph = graph.clone();
    assert!(
        facts.checked_input_body(owner).is_err(),
        "accepted reordered complete invocation arguments"
    );
    for corruption in 0..5 {
        let mut graph = reordered_graph.clone();
        let invocation = graph.invocations.get_mut(span).ok_or("missing invocation")?;
        match corruption {
            0 => invocation.completion_node = usize::MAX,
            1 => invocation.final_accesses[0] = usize::MAX,
            2 => invocation.arguments[0].evaluation.start_node = usize::MAX,
            3 => invocation.arguments[0].evaluation.end_node = usize::MAX,
            _ => invocation.arguments[0].evaluation.source_node = Some(usize::MAX),
        }
        assert!(
            matches!(
                flow::analyze(&graph),
                Err(flow::InvalidGraph("invocation event link is outside callable"))
            ),
            "accepted out-of-range invocation link {corruption}"
        );
    }
    let mut borrow_only = checker.type_info.clone();
    borrow_only.protocols.explicit_input_copy.remove(owner);
    assert!(
        borrow_only.checked_input_body(owner)?.is_some(),
        "borrowed invocation arguments required an unrelated Copy decision"
    );
    Ok(())
}

/// A retained outer borrow cannot end before a later argument mutates its source, even after restored flow
/// analysis is recomputed to hide that conflict.
#[test]
fn invocation_final_use_follows_all_argument_evaluations() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def choose(first: Borrowed[int], second: bool) -> int:\n    return first\ndef mutate(mut value: BorrowedMut[int]) -> bool:\n    value = 2\n    return true\ndef use(mut value: int) -> int:\n    return choose(value, mutate(value))\n";
    let mut checker = checked(source)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
            _ => None,
        })
        .ok_or("missing use function")?;
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "use")
        .cloned()
        .ok_or("missing owner")?;
    let InputFlow::Checked { graph, analysis } = checker
        .type_info
        .protocols
        .explicit_input_flow
        .get_mut(&owner)
        .ok_or("missing flow")?
    else {
        return Err("missing checked flow".into());
    };
    let outer = graph
        .invocations
        .values()
        .find(|call| call.callee.declaration_name == "choose")
        .ok_or("missing choose")?;
    let outer_loan = outer
        .arguments
        .first()
        .and_then(|argument| argument.loan)
        .ok_or("missing outer loan")?;
    let final_read = graph.nodes.iter().position(|node| matches!(node.event, flow::Event::Access { through: Some(id), access: flow::Access::Read, .. } if id == outer_loan)).ok_or("missing outer final use")?;
    let origin = graph
        .nodes
        .iter()
        .position(|node| matches!(node.event, flow::Event::Borrow(id) if id == outer_loan))
        .ok_or("missing origin")?;
    let terminal = graph.nodes.remove(final_read);
    graph.nodes.insert(origin + 1, terminal);
    let count = graph.nodes.len();
    for (index, node) in graph.nodes.iter_mut().enumerate() {
        node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
    }
    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
    assert!(analysis.conflicts.is_empty(), "{analysis:?}");
    assert!(
        checker.type_info.direct_input_admission(&owner, function).is_err(),
        "accepted outer loan ending before later argument mutation"
    );
    Ok(())
}

/// A fresh argument's complete nested evaluation belongs inside its retained interval. A marker from the
/// nested call cannot stand in for the outer argument's start merely because their full source spans agree.
#[test]
fn invocation_intervals_cover_nested_argument_effects() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def produce(value: Owned[int]) -> int:\n    return value\ndef consume(value: Owned[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    return consume(produce(value))\n";
    let mut checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
            _ => None,
        })
        .ok_or("missing use function")?;
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "use")
        .cloned()
        .ok_or("missing owner")?;
    assert!(checker.type_info.direct_input_admission(&owner, function)?.is_some());
    let InputFlow::Checked { graph, analysis } = checker
        .type_info
        .protocols
        .explicit_input_flow
        .get_mut(&owner)
        .ok_or("missing flow")?
    else {
        return Err("missing checked flow".into());
    };
    let nested_completion = graph
        .invocations
        .values()
        .find(|call| call.callee.declaration_name == "produce")
        .ok_or("missing nested call")?
        .completion_node;
    graph
        .invocations
        .values_mut()
        .find(|call| call.callee.declaration_name == "consume")
        .ok_or("missing outer call")?
        .arguments[0]
        .evaluation
        .start_node = nested_completion;
    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
    assert!(analysis.conflicts.is_empty());
    assert!(
        checker.type_info.direct_input_admission(&owner, function).is_err(),
        "accepted argument interval omitting its nested evaluation"
    );
    let mut facts = checked(source)?.type_info;
    let InputFlow::Checked { graph, .. } = facts
        .protocols
        .explicit_input_flow
        .get_mut(&owner)
        .ok_or("missing flow")?
    else {
        return Err("missing checked flow".into());
    };
    let (nested_span, nested) = graph
        .invocations
        .iter_mut()
        .find(|(_, call)| call.callee.declaration_name == "produce")
        .ok_or("missing nested call")?;
    let nested_span = *nested_span;
    nested.arguments.clear();
    facts
        .protocols
        .explicit_input_calls
        .get_mut(&nested_span)
        .ok_or("missing selected call")?
        .parameters
        .clear();
    assert!(
        facts.checked_input_body(&owner).is_err(),
        "accepted invocation without its canonical explicit input slot"
    );
    Ok(())
}

/// Explicit Owned requests copy trivial values; a persisted event cannot silently turn that read into a move.
#[test]
fn invocation_transfer_kind_uses_checked_copy_facts() -> Result<(), Box<dyn std::error::Error>> {
    let mut checker = checked(
        "def read(value: Owned[int]) -> int:\n    return value\ndef use(value: int) -> int:\n    return read(value)\n",
    )?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "use")
        .cloned()
        .ok_or("missing owner")?;
    let InputFlow::Checked { graph, analysis } = checker
        .type_info
        .protocols
        .explicit_input_flow
        .get_mut(&owner)
        .ok_or("missing flow")?
    else {
        return Err("missing checked flow".into());
    };
    let source_node = graph
        .invocations
        .values()
        .next()
        .and_then(|call| call.arguments.first())
        .and_then(|argument| argument.evaluation.source_node)
        .ok_or("missing Copy source event")?;
    let flow::Event::Access { access, .. } = &mut graph.nodes[source_node].event else {
        return Err("missing Copy access".into());
    };
    *access = flow::Access::Move;
    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
    assert!(analysis.conflicts.is_empty());
    assert!(
        checker.type_info.checked_input_body(&owner).is_err(),
        "accepted consuming event for Copy input"
    );
    Ok(())
}

/// Owned calls transfer even when the callee only compares the value; a later caller use is a source conflict.
#[test]
fn readonly_owned_callee_still_consumes_the_argument() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(
        "def consume(value: Owned[str]) -> bool:\n    return value == \"x\"\ndef use(value: str) -> str:\n    result = consume(value)\n    return value\n",
    )?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    let (_, output) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "use")
        .ok_or("missing caller flow")?;
    let InputFlow::Checked { analysis, .. } = output else {
        return Err("caller flow unavailable".into());
    };
    assert!(
        analysis
            .conflicts
            .iter()
            .any(|conflict| matches!(conflict.kind, flow::ConflictKind::Moved(_)))
    );
    let source = "def consume(value: Owned[str]) -> bool:\n    return value == \"x\"\ndef use() -> None:\n    value = \"x\"\n    result = consume(value)\n    print(value)\n";
    for erased in [flow::Access::Move, flow::Access::Read] {
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
                crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
                _ => None,
            })
            .ok_or("missing use function")?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or("missing use owner")?;
        let InputFlow::Checked { graph, analysis } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing use flow")?
        else {
            return Err("missing checked flow".into());
        };
        let transfer = graph
            .nodes
            .iter_mut()
            .find(|node| {
                matches!(
                    node.event,
                    flow::Event::Access {
                        access,
                        ..
                    } if access == erased
                )
            })
            .ok_or("missing transfer event")?;
        transfer.event = flow::Event::Nothing;
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty(), "{analysis:?}");
        assert!(
            checker.type_info.direct_input_admission(&owner, function).is_err(),
            "accepted stored owner with erased {erased:?} event"
        );
    }
    Ok(())
}

/// A persisted replacement cannot resurrect a consumed owner when the source only initializes another local.
/// Actual reassignment remains valid and resets the moved state after its value has been evaluated.
#[test]
fn source_access_cannot_invent_reinitialization() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def consume(value: Owned[str]) -> None:\n    pass\ndef use() -> None:\n    mut value = \"old\"\n    consume(value)\n    unrelated = \"safe\"\n    print(value)\n";
    let mut checker = checked(source)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
            _ => None,
        })
        .ok_or("missing use function")?;
    let replacement_span = function
        .body
        .iter()
        .find_map(|statement| match &statement.node {
            crate::ast::Statement::Assignment(assignment) if assignment.name == "unrelated" => {
                Some(assignment.name_span)
            }
            _ => None,
        })
        .ok_or("missing unrelated declaration")?;
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "use")
        .cloned()
        .ok_or("missing owner")?;
    let InputFlow::Checked { graph, analysis } = checker
        .type_info
        .protocols
        .explicit_input_flow
        .get_mut(&owner)
        .ok_or("missing flow")?
    else {
        return Err("missing checked flow".into());
    };
    let (read, place) = graph
        .nodes
        .iter()
        .enumerate()
        .find_map(|(index, node)| match &node.event {
            flow::Event::Access {
                place,
                access: flow::Access::Read,
                through: None,
            } => Some((index, place.clone())),
            _ => None,
        })
        .ok_or("missing later source read")?;
    graph.nodes.insert(
        read,
        flow::Node {
            event: flow::Event::Access {
                place,
                access: flow::Access::Replace,
                through: None,
            },
            span: replacement_span,
            successors: Vec::new(),
        },
    );
    let count = graph.nodes.len();
    for (index, node) in graph.nodes.iter_mut().enumerate() {
        node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
    }
    *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
    assert!(analysis.conflicts.is_empty());
    assert!(
        checker.type_info.direct_input_admission(&owner, function).is_err(),
        "accepted an invented replacement of another source owner"
    );
    let valid = checked(
        "def consume(value: Owned[str]) -> None:\n    pass\ndef use() -> None:\n    mut value = \"old\"\n    consume(value)\n    value = \"new\"\n    print(value)\n",
    )?;
    assert!(valid.errors.is_empty(), "{:?}", valid.errors);
    Ok(())
}

/// Ordinary source accesses retain exact multiplicity and producer order even when neither event changes
/// liveness. Recomputing an otherwise harmless Copy-only graph cannot authorize those mutations.
#[test]
fn source_accesses_preserve_multiplicity_and_order() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def read(left: Borrowed[int], right: Borrowed[int]) -> int:\n    return left + right\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) => Some(function),
            _ => None,
        })
        .ok_or("missing function")?;
    let owner = checker
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .next()
        .ok_or("missing owner")?;
    for duplicate in [false, true] {
        let mut facts = checker.type_info.clone();
        let InputFlow::Checked { graph, analysis } = facts
            .protocols
            .explicit_input_flow
            .get_mut(owner)
            .ok_or("missing flow")?
        else {
            return Err("missing checked flow".into());
        };
        let reads = graph
            .nodes
            .iter()
            .enumerate()
            .filter_map(|(index, node)| {
                matches!(
                    node.event,
                    flow::Event::Access {
                        access: flow::Access::Read,
                        ..
                    }
                )
                .then_some(index)
            })
            .collect::<Vec<_>>();
        let [left, right] = reads.as_slice() else {
            return Err("expected exactly two source reads".into());
        };
        if duplicate {
            graph.nodes.insert(*left + 1, graph.nodes[*left].clone());
        } else {
            graph.nodes.swap(*left, *right);
        }
        let count = graph.nodes.len();
        for (index, node) in graph.nodes.iter_mut().enumerate() {
            node.successors = if index + 1 < count { vec![index + 1] } else { Vec::new() };
        }
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        assert!(
            facts.direct_input_admission(owner, function).is_err(),
            "accepted corrupted ordinary reads (duplicate: {duplicate})"
        );
    }
    Ok(())
}

/// Access coverage must preserve source Copy evidence for nominal derives instead of treating a valid read
/// as a required transfer merely because a legacy ABI projection cannot describe the nominal layout.
#[test]
fn source_access_retains_checked_copy_derives() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def derived(value: Owned[Point]) -> Point:\n    first = value\n    return value\n@derive(Copy)\nmodel Point:\n    x: int\n",
        "def optional(value: Owned[Option[int]]) -> Option[int]:\n    first = value\n    return value\n",
        "def use(value: Owned[Point]) -> int:\n    return consume(value)\ndef consume(value: Owned[Point]) -> int:\n    return value.x\n@derive(Copy)\nmodel Point:\n    x: int\n",
        "def use(value: Owned[Point]) -> Point:\n    let local: Owned[Point] = value\n    return local\n@derive(Copy)\nmodel Point:\n    x: int\n",
        "def equal(value: Borrowed[Point]) -> bool:\n    return value == value\n@derive(Copy, PartialEq)\nmodel Point:\n    x: int\n",
    ] {
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
        for owner in checker.type_info.protocols.explicit_input_flow.keys() {
            assert!(checker.type_info.checked_input_body(owner)?.is_some());
        }
    }
    Ok(())
}

/// Exact owner/type decisions survive independently of graph events. Missing legacy facts, another owner's
/// facts and a valid recomputed graph that changes a Copy read into a move all remain refused.
#[test]
fn source_copy_evidence_is_independent_of_access_events() -> Result<(), Box<dyn std::error::Error>> {
    let source =
        "def derived(value: Owned[Point]) -> Point:\n    return value\n@derive(Copy)\nmodel Point:\n    x: int\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|error| format!("{error:?}"))?)
        .map_err(|error| format!("{error:?}"))?;
    let declaration = program.declarations.first().ok_or("missing function")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("expected function".into());
    };
    let owner = checker
        .type_info
        .declarations
        .function_bindings_by_span
        .get(&declaration.span)
        .and_then(|binding| binding.identity.as_ref())
        .ok_or("missing function identity")?;
    for corruption in 0..4 {
        let mut facts = checker.type_info.clone();
        match corruption {
            0 => facts.protocols.explicit_input_copy.clear(),
            1 => {
                let decisions = facts
                    .protocols
                    .explicit_input_copy
                    .remove(owner)
                    .ok_or("missing decisions")?;
                let mut other = owner.clone();
                other.declaration_name = "other".into();
                facts.protocols.explicit_input_copy.insert(other, decisions);
            }
            2 => {
                let decisions = facts
                    .protocols
                    .explicit_input_copy
                    .get_mut(owner)
                    .ok_or("missing decisions")?;
                decisions.clear();
                decisions.insert(ResolvedType::Str, true);
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
                let access = graph
                    .nodes
                    .iter_mut()
                    .find_map(|node| match &mut node.event {
                        flow::Event::Access { access, .. } => Some(access),
                        _ => None,
                    })
                    .ok_or("missing value read")?;
                *access = flow::Access::Move;
                *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
                assert!(analysis.conflicts.is_empty());
            }
        }
        assert!(
            facts.direct_input_admission(owner, function).is_err(),
            "accepted Copy corruption {corruption}"
        );
    }
    Ok(())
}

/// Cleanup subjects follow actual authenticated non-Copy transfers, including ordinary locals and explicit
/// Owned initializers. Copy calls and shared observations cannot opt an unrelated owner into this policy.
#[test]
fn conditional_transfer_inventory_uses_actual_source_events() -> Result<(), Box<dyn std::error::Error>> {
    for (source, expected) in [
        (
            "def choose(flag: bool) -> None:\n    value = \"x\"\n    if flag:\n        consume(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            vec!["value"],
        ),
        (
            "def choose(value: Owned[str], flag: bool) -> None:\n    if flag:\n        consume(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            vec!["value"],
        ),
        (
            "def choose(flag: bool) -> None:\n    let value: Owned[str] = \"x\"\n    if flag:\n        consume(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            vec!["value"],
        ),
        (
            "def choose(flag: bool) -> None:\n    source = \"x\"\n    if flag:\n        let destination: Owned[str] = source\n",
            vec!["source"],
        ),
        (
            "def choose(flag: bool) -> None:\n    value = 1\n    if flag:\n        consume(value)\ndef consume(value: Owned[int]) -> None:\n    pass\n",
            vec![],
        ),
        (
            "def choose(flag: bool) -> None:\n    value = \"x\"\n    if flag:\n        read(value)\ndef read(value: Borrowed[str]) -> None:\n    pass\n",
            vec![],
        ),
    ] {
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{source}\n{:?}",
            checker.errors
        );
        let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|error| format!("{error:?}"))?)
            .map_err(|error| format!("{error:?}"))?;
        let declaration = program.declarations.first().ok_or("missing function")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("expected function".into());
        };
        let owner = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .get(&declaration.span)
            .and_then(|binding| binding.identity.as_ref())
            .ok_or("missing owner")?;
        let proof = checker.type_info.checked_input_body(owner)?.ok_or("missing proof")?;
        let subjects = proof.whole_transfer_subjects(function)?;
        assert_eq!(
            subjects
                .iter()
                .map(|subject| subject.declaration_name.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{source}"
        );
        assert!(
            checker.type_info.direct_input_admission(owner, function).is_err(),
            "inventory unexpectedly enabled conditional execution"
        );
    }
    Ok(())
}

/// Transfer subjects cannot come from a graph that omits a later source use or invents a Move for another
/// owner. Both mutations can produce conflict-free graph analysis and still lack complete source authority.
#[test]
fn conditional_transfer_inventory_requires_complete_accesses() -> Result<(), Box<dyn std::error::Error>> {
    for (source, erased) in [
        (
            "def choose(flag: bool) -> None:\n    value = \"x\"\n    if flag:\n        consume(value)\n    print(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            true,
        ),
        (
            "def choose(flag: bool) -> None:\n    value = \"x\"\n    other = \"y\"\n    if flag:\n        consume(value)\n    print(other)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            false,
        ),
    ] {
        let checker = checked(source)?;
        let program = crate::parser::parse(&crate::lexer::lex(source).map_err(|error| format!("{error:?}"))?)
            .map_err(|error| format!("{error:?}"))?;
        let declaration = program.declarations.first().ok_or("missing function")?;
        let crate::ast::Declaration::Function(function) = &declaration.node else {
            return Err("expected function".into());
        };
        let mut facts = checker.type_info;
        let owner = facts
            .declarations
            .function_bindings_by_span
            .get(&declaration.span)
            .and_then(|binding| binding.identity.clone())
            .ok_or("missing owner")?;
        let InputFlow::Checked { graph, analysis } = facts
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or("missing flow")?
        else {
            return Err("expected checked graph".into());
        };
        let read = graph.nodes.iter_mut().rev().find(|node| matches!(&node.event,
                flow::Event::Access { place, access: flow::Access::Read, .. }
                    if matches!(&place.root, flow::Root::Binding(identity) if identity.declaration_name == if erased { "value" } else { "other" })))
                .ok_or("missing ordinary read")?;
        if erased {
            read.event = flow::Event::Nothing;
        } else if let flow::Event::Access { access, .. } = &mut read.event {
            *access = flow::Access::Move;
        }
        *analysis = flow::analyze(graph).map_err(|error| format!("{error:?}"))?;
        assert!(analysis.conflicts.is_empty());
        let proof = facts.checked_input_body(&owner)?.ok_or("missing body view")?;
        assert!(
            proof.whole_transfer_subjects(function).is_err(),
            "accepted corrupted conditional inventory (erased: {erased})"
        );
    }
    Ok(())
}

/// Ordinary bodies need no ownership proof; source conflicts and unsupported flows cannot produce one.
#[test]
fn body_projection_distinguishes_absent_and_refused_flows() -> Result<(), Box<dyn std::error::Error>> {
    for (source, ordinary) in [
        ("def read(value: int) -> int:\n    return value\n", true),
        (
            "def read(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view\n",
            false,
        ),
        (
            "def read(value: Borrowed[int], other: Borrowed[int]) -> Borrowed[int] from value:\n    return other\n",
            false,
        ),
    ] {
        let checker = checked(source)?;
        let owner = checker
            .type_info
            .declarations
            .function_bindings_by_span
            .values()
            .find_map(|binding| binding.identity.as_ref())
            .ok_or("missing callable")?;
        let projected = checker.type_info.checked_input_body(owner);
        if ordinary {
            assert!(projected?.is_none());
        } else {
            assert!(projected.is_err());
        }
    }
    Ok(())
}
