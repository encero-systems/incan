// The signature block's `str.encode` builtin route: exact admission and receiver replay, arguments and source methods
// kept off the route, and fact or operation tampering refused. Included by `signature_block.rs`.

/// The exact zero-argument runtime `str.encode()` builtin evaluates its receiver as one operation and never invents
/// an explicit invocation. Fresh concatenation stays source-less; an existing string local is replayed as one Read.
#[test]
fn signature_block_admits_exact_str_encode_builtin_and_replays_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def fresh(seed: Borrowed[int], left: str, right: str) -> bytes:\n    return (left + \"\\n\" + right).encode()\ndef stored(seed: Borrowed[int], value: str) -> bytes:\n    saved = value\n    return saved.encode()\n";
    let checker = checked(source)?;
    require_checked(&checker, "fresh")?;
    require_checked(&checker, "stored")?;
    let operation = incan_lang::lang::builtins::BuiltinMethodId::Str(
        incan_lang::lang::surface::string_methods::StringMethodId::Encode,
    );
    let identity = crate::symbols::canonical_builtin_member_identity(operation.owner(), operation.member());

    for (owner_name, written, receiver) in [
        ("fresh", "(left + \"\\n\" + right).encode()", "(left + \"\\n\" + right)"),
        ("stored", "saved.encode()", "saved"),
    ] {
        let (call_span, _) = checker
            .type_info
            .calls
            .resolved_builtin_method_calls
            .iter()
            .find(|(span, selected)| **selected == operation && source.get(span.start..span.end) == Some(written))
            .ok_or_else(|| format!("missing checked {written} builtin"))?;
        assert_eq!(checker.type_info.resolved_identity(*call_span), Some(&identity));
        assert_eq!(checker.type_info.expr_type(*call_span), Some(&ResolvedType::Bytes));
        let (owner, graph) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == owner_name)
            .and_then(|(owner, flow)| match flow {
                InputFlow::Checked { graph, .. } => Some((owner, graph)),
                _ => None,
            })
            .ok_or_else(|| format!("missing {owner_name} checked graph"))?;
        assert!(
            !graph.invocations.contains_key(call_span),
            "builtin encode acquired a synthetic invocation"
        );
        assert!(
            !checker.type_info.protocols.explicit_input_calls.contains_key(call_span),
            "builtin encode acquired a synthetic input contract"
        );
        assert!(checker.type_info.checked_input_body(owner)?.is_some());
        if owner_name == "fresh" {
            assert!(
                !graph.operations.contains_key(call_span),
                "fresh concatenation acquired a stored dependency"
            );
            assert!(
                checker.type_info.expressions.expr_types.iter().any(|(span, ty)| {
                    source.get(span.start..span.end) == Some(receiver) && ty == &ResolvedType::Str
                })
            );
        } else {
            assert_eq!(
                graph
                    .nodes
                    .iter()
                    .filter(|node| {
                        call_span.start <= node.span.start
                            && node.span.end <= call_span.end
                            && source.get(node.span.start..node.span.end) == Some(receiver)
                            && matches!(
                                node.event,
                                flow::Event::Access {
                                    access: flow::Access::Read,
                                    ..
                                }
                            )
                    })
                    .count(),
                1,
                "stored encode receiver was not evaluated exactly once"
            );
        }
    }
    Ok(())
}

/// Supplied codec labels remain outside the zero-argument route, while a source-owned method with the same spelling
/// continues through its selected ordinary invocation instead of borrowing builtin authority.
#[test]
fn signature_block_keeps_encode_arguments_and_source_methods_off_builtin_route()
-> Result<(), Box<dyn std::error::Error>> {
    require_unavailable(
        "def encode(seed: Borrowed[int], value: str) -> bytes:\n    return value.encode(\"utf-8\")\n",
        "encode",
        "call has no selected explicit input access facts",
        "value.encode(\"utf-8\")",
    )?;

    let source = "class Encoder:\n    pub marker: int\n    pub def encode(self) -> int:\n        return self.marker\ndef inspect(seed: Borrowed[int], encoder: Encoder) -> int:\n    return encoder.encode()\n";
    let checker = checked(source)?;
    require_checked(&checker, "inspect")?;
    let (span, call) = checker
        .type_info
        .protocols
        .explicit_input_calls
        .iter()
        .find(|(span, call)| {
            source.get(span.start..span.end) == Some("encoder.encode()")
                && call
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.declaration_name == "inspect")
        })
        .ok_or("source encode call lost its selected input facts")?;
    assert_ne!(
        checker.type_info.resolved_builtin_method_call(*span),
        Some(incan_lang::lang::builtins::BuiltinMethodId::Str(
            incan_lang::lang::surface::string_methods::StringMethodId::Encode,
        ))
    );
    let graph = checked_graph(source, "inspect")?;
    assert!(graph.invocations.contains_key(span));
    assert_eq!(call.callee.declaration_name, "encode");
    Ok(())
}

/// Operation, canonical identity and endpoint types are independent authority. Changing any one after checking,
/// including converting the receiver to `FrozenStr`, closes the builtin route before source construction.
#[test]
fn signature_block_str_encode_fact_tampering_refuses_builtin_route() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def encode(seed: Borrowed[int], value: str) -> bytes:\n    return value.encode()\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| {
            matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == "encode")
        })
        .ok_or("missing encode declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("encode is not a function".into());
    };
    let [statement] = function.body.as_slice() else {
        return Err("encode body does not contain one return".into());
    };
    let crate::ast::Statement::Return(Some(returned)) = &statement.node else {
        return Err("encode body is not one returned value".into());
    };
    let Expr::MethodCall(receiver, _, _, _) = &returned.node else {
        return Err("encode return is not a method call".into());
    };

    for corruption in 0..4 {
        let mut checker = checked(source)?;
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "encode")
            .cloned()
            .ok_or("missing encode owner")?;
        match corruption {
            0 => checker.type_info.record_resolved_builtin_method_call(
                returned.span,
                incan_lang::lang::builtins::BuiltinMethodId::FrozenStr(
                    incan_lang::lang::surface::string_methods::StringMethodId::Encode,
                ),
            ),
            1 => {
                let mut identity = checker
                    .type_info
                    .resolved_identity(returned.span)
                    .cloned()
                    .ok_or("encode has no canonical identity")?;
                identity.declaration_name = "str.decode".into();
                checker.type_info.record_resolved_identity(returned.span, identity);
            }
            2 => {
                let operation = incan_lang::lang::builtins::BuiltinMethodId::FrozenStr(
                    incan_lang::lang::surface::string_methods::StringMethodId::Encode,
                );
                checker.record_expr_type(receiver.span, ResolvedType::FrozenStr);
                checker
                    .type_info
                    .record_resolved_builtin_method_call(returned.span, operation);
                checker.type_info.record_resolved_identity(
                    returned.span,
                    crate::symbols::canonical_builtin_member_identity(operation.owner(), operation.member()),
                );
            }
            _ => checker.record_expr_type(returned.span, ResolvedType::Str),
        }
        let unavailable = source::build(&checker, CallableSource::Function(function), declaration.span, &owner)
            .expect_err("tampered encode facts retained builtin admission");
        assert_eq!(unavailable.reason, "call has no selected explicit input access facts");
        assert_eq!(
            source.get(unavailable.span.start..unavailable.span.end),
            Some("value.encode()")
        );
    }
    Ok(())
}

/// A borrowed string field creates one exact operation-duration dependency for `str.encode()`. Removing that
/// dependency or changing its completion, operand or loan cannot survive graph analysis and current-source replay.
#[test]
fn signature_block_str_encode_operation_tampering_refuses_replay() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Holder:\n    value: str\ndef encode(holder: Borrowed[Holder]) -> bytes:\n    return holder.value.encode()\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| {
            matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == "encode")
        })
        .ok_or("missing encode declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("encode is not a function".into());
    };
    let checker = checked(source)?;
    require_checked(&checker, "encode")?;
    let (owner, flow) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "encode")
        .ok_or("missing encode flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("encode did not retain a checked graph: {flow:?}").into());
    };
    let (span, operation) = graph
        .operations
        .iter()
        .find(|(span, _)| source.get(span.start..span.end) == Some("holder.value.encode()"))
        .ok_or("encode lost its operation dependency")?;
    let [dependency] = operation.operands.as_slice() else {
        return Err("encode operation did not retain one receiver dependency".into());
    };
    assert_eq!(
        source.get(dependency.operand.start..dependency.operand.end),
        Some("holder.value")
    );
    assert!(!graph.invocations.contains_key(span));
    assert!(checker.type_info.checked_input_body(owner)?.is_some());

    for corruption in 0..4 {
        let mut facts = checker.type_info.clone();
        let InputFlow::Checked { graph, analysis } = facts
            .protocols
            .explicit_input_flow
            .get_mut(owner)
            .ok_or("missing copied encode flow")?
        else {
            return Err("copied encode flow is unavailable".into());
        };
        match corruption {
            0 => {
                graph.operations.remove(span);
            }
            1 => {
                let operation = graph.operations.get_mut(span).ok_or("missing copied operation")?;
                operation.completion_node = operation.operands[0].final_access;
            }
            2 => {
                graph
                    .operations
                    .get_mut(span)
                    .ok_or("missing copied operation")?
                    .operands[0]
                    .operand = *span;
            }
            _ => {
                let loan = graph
                    .operations
                    .get(span)
                    .and_then(|operation| operation.operands.first())
                    .map(|operand| operand.loan)
                    .ok_or("missing copied dependency loan")?;
                graph.loans.get_mut(loan.0).ok_or("missing copied loan")?.kind = flow::LoanKind::Exclusive;
            }
        }
        match flow::analyze(graph) {
            Ok(recomputed) => {
                *analysis = recomputed;
                assert!(
                    facts.direct_input_admission(owner, function).is_err(),
                    "accepted source-inconsistent encode operation corruption {corruption}"
                );
            }
            Err(_) => {}
        }
    }
    Ok(())
}
