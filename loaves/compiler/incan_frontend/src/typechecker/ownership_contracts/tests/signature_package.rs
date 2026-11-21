//! Integrated Package-call evidence for the bounded compiler signature block.

use super::*;

/// Require one named imported body to retain a conflict-free checked graph on both dependency routes.
fn require_package_checked(provider: &str, consumer: &str, owner_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    for (route, checker) in super::one_type::package_consumer_checkers(provider, consumer)? {
        if !checker.errors.is_empty() {
            return Err(format!("{route}: {owner_name} produced diagnostics: {:?}", checker.errors).into());
        }
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == owner_name)
            .ok_or_else(|| format!("{route}: missing {owner_name} flow"))?;
        if !matches!(flow, InputFlow::Checked { .. }) {
            return Err(format!("{route}: {owner_name} did not retain a checked graph: {flow:?}").into());
        }
        let imported = checker
            .type_info
            .protocols
            .explicit_input_calls
            .values()
            .find(|call| call.callee.declaration_name == "decode" && call.owner.as_ref() == Some(owner))
            .ok_or_else(|| format!("{route}: missing imported decode call"))?;
        assert!(matches!(
            &imported.callee.origin,
            incan_semantics_core::SymbolOrigin::Package { library, module_path }
                if library == "fixture_records" && module_path.iter().map(String::as_str).eq(["views"])
        ));
    }
    Ok(())
}

/// The exact outer invocation, receiver and capture-free callback are one current-source call-result classification;
/// deleting, substituting or transplanting any part makes full replay refuse instead of skipping closure storage.
#[test]
fn signature_block_value_use_mutations_reject_map_err_callback_substitution() -> Result<(), Box<dyn std::error::Error>>
{
    let provider = "pub def decode(value: int) -> Result[int, str]:\n    return Ok(value)\n";
    let consumer = "from views import decode\n\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    decoded = decode(value).map_err((_) => Fault.Malformed)?\n    return Ok(decoded)\n";
    let tokens = crate::lexer::lex(consumer).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
            _ => None,
        })
        .ok_or("missing use function")?;
    for (route, checker) in super::one_type::package_consumer_checkers(provider, consumer)? {
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or_else(|| format!("{route}: missing use owner"))?;
        let mut missing = checker.type_info.clone();
        let receiver = {
            let InputFlow::Checked { graph, .. } = missing
                .protocols
                .explicit_input_flow
                .get_mut(&owner)
                .ok_or_else(|| format!("{route}: missing use flow"))?
            else {
                return Err(format!("{route}: use flow is not checked").into());
            };
            let (receiver, value_use) = graph
                .value_uses
                .iter_mut()
                .find(|(_, value_use)| value_use.result_map_err.is_some())
                .ok_or_else(|| format!("{route}: missing callback classification"))?;
            value_use.result_map_err = None;
            *receiver
        };
        assert!(
            missing.executable_function_input(&owner, function).is_err(),
            "{route}: deleted callback marker replayed"
        );

        let mut substituted = checker.type_info.clone();
        let InputFlow::Checked { graph, .. } = substituted
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or_else(|| format!("{route}: missing use flow"))?
        else {
            return Err(format!("{route}: use flow is not checked").into());
        };
        graph
            .value_uses
            .values_mut()
            .find(|value_use| value_use.result_map_err.is_some())
            .ok_or_else(|| format!("{route}: missing callback classification"))?
            .result_map_err
            .as_mut()
            .ok_or_else(|| format!("{route}: missing map_err route"))?
            .callback = receiver;
        assert!(
            substituted.executable_function_input(&owner, function).is_err(),
            "{route}: substituted callback marker replayed"
        );

        for corruption in 0..7 {
            let mut corrupted = checker.type_info.clone();
            let InputFlow::Checked { graph, .. } = corrupted
                .protocols
                .explicit_input_flow
                .get_mut(&owner)
                .ok_or_else(|| format!("{route}: missing use flow"))?
            else {
                return Err(format!("{route}: use flow is not checked").into());
            };
            let marker = graph
                .value_uses
                .values()
                .find_map(|value_use| value_use.result_map_err)
                .ok_or_else(|| format!("{route}: missing map_err marker"))?;
            let outer_receiver = graph
                .invocations
                .get(&marker.invocation)
                .and_then(|invocation| invocation.receiver)
                .ok_or_else(|| format!("{route}: missing map_err receiver loan"))?;
            let receiver_source = graph
                .loans
                .get(outer_receiver.0)
                .ok_or_else(|| format!("{route}: missing map_err receiver loan"))?
                .source
                .clone();
            match corruption {
                0 => {
                    graph
                        .value_uses
                        .get_mut(&marker.receiver)
                        .ok_or("missing receiver value use")?
                        .kind = flow::ValueUseKind::Read;
                }
                1 => {
                    graph
                        .invocations
                        .get_mut(&marker.invocation)
                        .ok_or("missing outer invocation")?
                        .arguments[0]
                        .slot = 1;
                }
                2 => {
                    graph
                        .invocations
                        .get_mut(&marker.invocation)
                        .ok_or("missing outer invocation")?
                        .arguments[0]
                        .access = incan_semantics_core::IncanInputAccess::Owned;
                }
                3 => {
                    graph
                        .invocations
                        .get_mut(&marker.invocation)
                        .ok_or("missing outer invocation")?
                        .arguments[0]
                        .source = Some(receiver_source);
                }
                4 => {
                    graph
                        .invocations
                        .get_mut(&marker.invocation)
                        .ok_or("missing outer invocation")?
                        .arguments[0]
                        .loan = Some(outer_receiver);
                }
                5 => {
                    let outer_callee = graph
                        .invocations
                        .get(&marker.invocation)
                        .ok_or("missing outer invocation")?
                        .callee
                        .clone();
                    graph
                        .invocations
                        .get_mut(&marker.receiver)
                        .ok_or("missing inner invocation")?
                        .callee = outer_callee;
                }
                _ => {
                    let loan = graph.loans.get_mut(outer_receiver.0).ok_or("missing receiver loan")?;
                    let flow::Root::Temporary { expression, .. } = &mut loan.source.root else {
                        return Err("map_err receiver loan is not a temporary".into());
                    };
                    *expression = marker.callback;
                }
            }
            assert!(
                corrupted.executable_function_input(&owner, function).is_err(),
                "{route}: accepted map_err graph corruption {corruption}"
            );
        }
    }

    let paired = "from views import decode\n\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    fault = Fault.Malformed\n    first = decode(value).map_err((_) => Fault.Malformed)?\n    second = decode(value).map_err((_) => Fault.Malformed)?\n    return Ok(first + second)\n";
    let paired_tokens = crate::lexer::lex(paired).map_err(|errors| format!("{errors:?}"))?;
    let paired_program = crate::parser::parse(&paired_tokens).map_err(|errors| format!("{errors:?}"))?;
    let paired_function = paired_program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
            _ => None,
        })
        .ok_or("missing paired use function")?;
    for (route, checker) in super::one_type::package_consumer_checkers(provider, paired)? {
        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "use")
            .cloned()
            .ok_or_else(|| format!("{route}: missing paired use owner"))?;
        assert!(
            checker
                .type_info
                .executable_function_input(&owner, paired_function)
                .is_ok(),
            "{route}: paired capture-free baseline did not replay"
        );
        let mut transplanted = checker.type_info.clone();
        let InputFlow::Checked {
            graph: transplanted_graph,
            ..
        } = transplanted
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or_else(|| format!("{route}: missing paired use flow"))?
        else {
            return Err(format!("{route}: paired use flow is not checked").into());
        };
        let mut routes = transplanted_graph
            .value_uses
            .values()
            .filter_map(|value_use| value_use.result_map_err)
            .collect::<Vec<_>>();
        routes.sort_by_key(|route| route.invocation.start);
        let [first, second] = routes.as_slice() else {
            return Err(format!("{route}: expected two map_err routes, got {}", routes.len()).into());
        };
        let mut reused = checker.type_info.clone();
        let InputFlow::Checked {
            graph: reused_graph, ..
        } = reused
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or_else(|| format!("{route}: missing paired use flow"))?
        else {
            return Err(format!("{route}: paired use flow is not checked").into());
        };
        for value_use in reused_graph.value_uses.values_mut() {
            let Some(map_err) = value_use.result_map_err.as_mut() else {
                continue;
            };
            if map_err.invocation == second.invocation {
                map_err.invocation = first.invocation;
                map_err.callback = first.callback;
            }
        }
        assert!(
            reused.executable_function_input(&owner, paired_function).is_err(),
            "{route}: duplicate map_err invocation/callback replayed"
        );

        for value_use in transplanted_graph.value_uses.values_mut() {
            let Some(map_err) = value_use.result_map_err.as_mut() else {
                continue;
            };
            if map_err.invocation == first.invocation {
                map_err.callback = second.callback;
            } else if map_err.invocation == second.invocation {
                map_err.callback = first.callback;
            }
        }
        assert!(
            transplanted.executable_function_input(&owner, paired_function).is_err(),
            "{route}: cross-call callback transplant replayed"
        );
    }
    Ok(())
}

/// A direct dependency-free Package `Result` call is one current-source call-result observation. The same
/// classification feeds builtin `map_err` and the later constructor routes without turning stored or captureful
/// Result values into generally independent temporaries.
#[test]
fn signature_block_classifies_package_map_err_and_decoded_record() -> Result<(), Box<dyn std::error::Error>> {
    let scalar_provider = "pub def decode(value: int) -> Result[int, str]:\n    return Ok(value)\n";
    let captureful = "from views import decode\n\nenum Fault:\n    Malformed\n\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    fault = Fault.Malformed\n    decoded = decode(value).map_err((_) => fault)?\n    return Ok(decoded)\n";
    for (route, checker) in super::one_type::package_consumer_checkers(scalar_provider, captureful)? {
        let state = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "use")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("{route}: missing captureful map_err flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{route}: captureful callback entered Package call-result route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, "call has no selected explicit input access facts");
        assert_eq!(
            captureful.get(unavailable.span.start..unavailable.span.end),
            Some("decode(value).map_err((_) => fault)")
        );
        assert!(
            !checker.errors.is_empty()
                && checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123"))
        );
    }

    let provider = "@derive(Clone)\npub model Cell:\n    pub value: str\n@derive(Clone)\npub enum Payload:\n    Record(list[Cell])\n@derive(Clone)\npub model Row:\n    pub value: Payload\npub def decode(value: int) -> Result[list[Row], str]:\n    values = [Cell(value=str(value)), Cell(value=\"binary\"), Cell(value=\"family\"), Cell(value=\"decoded\"), Cell(value=\"native\")]\n    return Ok([Row(value=Payload.Record(values))])\n";
    let map_err = "from views import Row, decode\n\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[list[Row], Fault]:\n    rows = decode(value).map_err((_) => Fault.Malformed)?\n    return Ok(rows)\n";
    require_package_checked(provider, map_err, "use")?;
    for (route, checker) in super::one_type::package_consumer_checkers(provider, map_err)? {
        let inner = checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .find(|(_, call)| call.callee.declaration_name == "decode")
            .ok_or_else(|| format!("{route}: missing established Ordinary map_err receiver"))?;
        assert_eq!(inner.1.kind, InputCallKind::Ordinary);
        let owner = inner
            .1
            .owner
            .as_ref()
            .ok_or_else(|| format!("{route}: missing owner"))?;
        let InputFlow::Checked { graph, .. } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get(owner)
            .ok_or_else(|| format!("{route}: missing Ordinary flow"))?
        else {
            return Err(format!("{route}: Ordinary flow refused").into());
        };
        assert!(
            graph
                .invocations
                .get(inner.0)
                .is_some_and(|call| call.result_requirement.is_none())
        );
    }

    let decoded = "from views import Payload, decode\n\nenum Fault:\n    Malformed\nenum Family:\n    Local\n@derive(Clone)\nmodel OutputCell:\n    value: str\nmodel Record:\n    text: str\n    binary: bytes\n    family: Family\n    decoded: list[OutputCell]\n    native: list[OutputCell]\ndef text_value(value: str) -> Result[str, Fault]:\n    return Ok(value)\ndef binary_value(value: str) -> Result[bytes, Fault]:\n    return Ok(b\"value\")\ndef family_value(value: str) -> Result[Family, Fault]:\n    return Ok(Family.Local)\ndef decoded_fields(value: str) -> Result[list[OutputCell], Fault]:\n    return Ok([OutputCell(value=value)])\ndef native_fields(value: str) -> Result[list[OutputCell], Fault]:\n    return Ok([OutputCell(value=value)])\ndef decoded_record(value: Borrowed[int]) -> Result[Record, Fault]:\n    rows = decode(value).map_err((_) => Fault.Malformed)?\n    row = rows[0]\n    values = match row.value:\n        Payload.Record(fields) => fields\n    return Ok(Record(text=text_value(values[0].value)?, binary=binary_value(values[1].value)?, family=family_value(values[2].value)?, decoded=decoded_fields(values[3].value)?, native=native_fields(values[4].value)?))\n";
    require_package_checked(provider, decoded, "decoded_record")
}

/// An imported Explicit producer retains the exact conditional Result obligation for both direct match and the
/// capture-free builtin map_err observation. Replay refuses if the inner call's obligation is removed, replaced or
/// reanchored onto the outer builtin invocation.
#[test]
fn signature_block_retains_explicit_list_result_for_map_err_and_match() -> Result<(), Box<dyn std::error::Error>> {
    let provider = "pub def produce(seed: Borrowed[int]) -> Result[list[bytes], str]:\n    return Ok([b\"row\"])\n";
    let consumer = "from views import produce\n\ndef map_rows(seed: Borrowed[int]) -> Result[list[bytes], str]:\n    rows = produce(seed).map_err((fault) => fault)?\n    return Ok(rows)\n\ndef match_rows(seed: Borrowed[int]) -> int:\n    match produce(seed):\n        Ok(rows) => return len(rows)\n        Err(_) => return 0\n";
    for refused in [
        "from views import produce\n\ndef map_rows(seed: Borrowed[int]) -> Result[list[bytes], str]:\n    captured = \"fault\"\n    rows = produce(seed).map_err((_) => captured)?\n    return Ok(rows)\n",
        "from views import produce\n\ndef map_rows(seed: Borrowed[int]) -> Result[list[bytes], str]:\n    result = produce(seed)\n    rows = result.map_err((fault) => fault)?\n    return Ok(rows)\n",
    ] {
        for (route, checker) in super::one_type::package_consumer_checkers(provider, refused)? {
            assert!(
                checker
                    .errors
                    .iter()
                    .any(|error| error.stable_code() == Some("INCAN-T0123")),
                "{route}: unsupported Explicit map_err receiver admitted: {:?}",
                checker.errors
            );
        }
    }
    let tokens = crate::lexer::lex(consumer).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let map_function = program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "map_rows" => Some(function),
            _ => None,
        })
        .ok_or("missing map_rows function")?;
    for (route, checker) in super::one_type::package_consumer_checkers(provider, consumer)? {
        assert!(checker.errors.is_empty(), "{route}: {:?}", checker.errors);
        for name in ["map_rows", "match_rows"] {
            let (owner, flow) = checker
                .type_info
                .protocols
                .explicit_input_flow
                .iter()
                .find(|(owner, _)| owner.declaration_name == name)
                .ok_or_else(|| format!("{route}: missing {name} flow"))?;
            let InputFlow::Checked { graph, .. } = flow else {
                return Err(format!("{route}: {name} refused: {flow:?}").into());
            };
            let (span, call) = graph
                .invocations
                .iter()
                .find(|(_, call)| call.callee.declaration_name == "produce")
                .ok_or_else(|| format!("{route}: missing {name} produce call"))?;
            assert_eq!(consumer.get(span.start..span.end), Some("produce(seed)"));
            let selected = checker
                .type_info
                .protocols
                .explicit_input_calls
                .get(span)
                .ok_or_else(|| format!("{route}: missing selected produce facts"))?;
            assert_eq!(selected.kind, InputCallKind::Explicit);
            assert_eq!(selected.owner.as_ref(), Some(owner));
            assert_eq!(call.result_requirement.as_ref(), checker.type_info.expr_type(*span));
            assert!(checker.type_info.checked_input_body(owner)?.is_some());
        }

        let owner = checker
            .type_info
            .protocols
            .explicit_input_flow
            .keys()
            .find(|owner| owner.declaration_name == "map_rows")
            .cloned()
            .ok_or_else(|| format!("{route}: missing map_rows owner"))?;
        assert!(matches!(
            checker.type_info.executable_function_input(&owner, map_function),
            Ok(Some(_))
        ));
        let InputFlow::Checked { graph, .. } = checker
            .type_info
            .protocols
            .explicit_input_flow
            .get(&owner)
            .ok_or_else(|| format!("{route}: missing map_rows graph"))?
        else {
            return Err(format!("{route}: map_rows graph refused").into());
        };
        let inner = *graph
            .invocations
            .iter()
            .find(|(_, call)| call.callee.declaration_name == "produce")
            .map(|(span, _)| span)
            .ok_or_else(|| format!("{route}: missing retained inner call"))?;
        let outer = *graph
            .invocations
            .iter()
            .find(|(_, call)| call.callee == crate::symbols::canonical_builtin_member_identity("Result", "map_err"))
            .map(|(span, _)| span)
            .ok_or_else(|| format!("{route}: missing retained outer call"))?;
        let exact = graph
            .invocations
            .get(&inner)
            .and_then(|call| call.result_requirement.clone())
            .ok_or("missing exact retained requirement")?;
        for replacement in [None, Some(ResolvedType::Str)] {
            let mut corrupted = checker.type_info.clone();
            let Some(InputFlow::Checked { graph, .. }) = corrupted.protocols.explicit_input_flow.get_mut(&owner) else {
                return Err("missing copied map_rows graph".into());
            };
            graph
                .invocations
                .get_mut(&inner)
                .ok_or("missing copied inner")?
                .result_requirement = replacement;
            assert!(corrupted.executable_function_input(&owner, map_function).is_err());
        }
        let mut reanchored = checker.type_info.clone();
        let Some(InputFlow::Checked { graph, .. }) = reanchored.protocols.explicit_input_flow.get_mut(&owner) else {
            return Err("missing reanchored graph".into());
        };
        graph
            .invocations
            .get_mut(&inner)
            .ok_or("missing reanchored inner")?
            .result_requirement = None;
        graph
            .invocations
            .get_mut(&outer)
            .ok_or("missing reanchored outer")?
            .result_requirement = Some(exact);
        assert!(reanchored.executable_function_input(&owner, map_function).is_err());
    }
    Ok(())
}
