//! Integrated Package-call evidence for the bounded compiler signature block.

use super::*;

/// Require one named imported body to retain a conflict-free checked graph on both dependency routes.
fn require_package_checked(
    provider: &str,
    consumer: &str,
    owner_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
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
fn signature_block_value_use_mutations_reject_map_err_callback_substitution()
-> Result<(), Box<dyn std::error::Error>> {
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
    }

    let paired = "from views import decode\n\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    fault = Fault.Malformed\n    first = decode(value).map_err((_) => Fault.Malformed)?\n    second = decode(value).map_err((_) => Fault.Malformed)?\n    return Ok(first + second)\n";
    let paired_captureful = "from views import decode\n\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[int, Fault]:\n    fault = Fault.Malformed\n    first = decode(value).map_err((_) => Fault.Malformed)?\n    second = decode(value).map_err((_) => fault          )?\n    return Ok(first + second)\n";
    assert_eq!(paired.len(), paired_captureful.len());
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
    let captureful_tokens = crate::lexer::lex(paired_captureful).map_err(|errors| format!("{errors:?}"))?;
    let captureful_program = crate::parser::parse(&captureful_tokens).map_err(|errors| format!("{errors:?}"))?;
    let captureful_function = captureful_program
        .declarations
        .iter()
        .find_map(|declaration| match &declaration.node {
            crate::ast::Declaration::Function(function) if function.name == "use" => Some(function),
            _ => None,
        })
        .ok_or("missing captureful use function")?;
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
        let InputFlow::Checked { graph, .. } = transplanted
            .protocols
            .explicit_input_flow
            .get_mut(&owner)
            .ok_or_else(|| format!("{route}: missing paired use flow"))?
        else {
            return Err(format!("{route}: paired use flow is not checked").into());
        };
        let mut routes = graph
            .value_uses
            .values()
            .filter_map(|value_use| value_use.result_map_err)
            .collect::<Vec<_>>();
        routes.sort_by_key(|route| route.invocation.start);
        let [first, second] = routes.as_slice() else {
            return Err(format!("{route}: expected two map_err routes, got {}", routes.len()).into());
        };
        for value_use in graph.value_uses.values_mut() {
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
            transplanted
                .executable_function_input(&owner, captureful_function)
                .is_err(),
            "{route}: cross-call callback transplant replayed"
        );
    }
    Ok(())
}

/// A direct dependency-free Package `Result` call is one current-source call-result observation. The same
/// classification feeds builtin `map_err` and the later constructor routes without turning stored or captureful
/// Result values into generally independent temporaries.
#[test]
fn signature_block_classifies_package_map_err_and_decoded_record()
-> Result<(), Box<dyn std::error::Error>> {
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
                && checker.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123"))
        );
    }

    let provider = "@derive(Clone)\npub model Cell:\n    pub value: str\n@derive(Clone)\npub enum Payload:\n    Record(list[Cell])\n@derive(Clone)\npub model Row:\n    pub value: Payload\npub def decode(value: int) -> Result[list[Row], str]:\n    values = [Cell(value=str(value)), Cell(value=\"binary\"), Cell(value=\"family\"), Cell(value=\"decoded\"), Cell(value=\"native\")]\n    return Ok([Row(value=Payload.Record(values))])\n";
    let map_err = "from views import Row, decode\n\nenum Fault:\n    Malformed\ndef use(value: Borrowed[int]) -> Result[list[Row], Fault]:\n    rows = decode(value).map_err((_) => Fault.Malformed)?\n    return Ok(rows)\n";
    require_package_checked(provider, map_err, "use")?;

    let decoded = "from views import Payload, decode\n\nenum Fault:\n    Malformed\nenum Family:\n    Local\n@derive(Clone)\nmodel OutputCell:\n    value: str\nmodel Record:\n    text: str\n    binary: bytes\n    family: Family\n    decoded: list[OutputCell]\n    native: list[OutputCell]\ndef text_value(value: str) -> Result[str, Fault]:\n    return Ok(value)\ndef binary_value(value: str) -> Result[bytes, Fault]:\n    return Ok(b\"value\")\ndef family_value(value: str) -> Result[Family, Fault]:\n    return Ok(Family.Local)\ndef decoded_fields(value: str) -> Result[list[OutputCell], Fault]:\n    return Ok([OutputCell(value=value)])\ndef native_fields(value: str) -> Result[list[OutputCell], Fault]:\n    return Ok([OutputCell(value=value)])\ndef decoded_record(value: Borrowed[int]) -> Result[Record, Fault]:\n    rows = decode(value).map_err((_) => Fault.Malformed)?\n    row = rows[0]\n    values = match row.value:\n        Payload.Record(fields) => fields\n    return Ok(Record(text=text_value(values[0].value)?, binary=binary_value(values[1].value)?, family=family_value(values[2].value)?, decoded=decoded_fields(values[3].value)?, native=native_fields(values[4].value)?))\n";
    require_package_checked(provider, decoded, "decoded_record")
}
