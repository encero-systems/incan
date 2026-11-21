//! Current-source value-use classification for the bounded compiler signature block.

use super::*;

/// Require one named body to retain a conflict-free checked graph.
fn require_checked(checker: &TypeChecker, owner: &str) -> Result<(), Box<dyn std::error::Error>> {
    if !checker.errors.is_empty() {
        return Err(format!("{owner} produced diagnostics: {:?}", checker.errors).into());
    }
    let flow = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(identity, _)| identity.declaration_name == owner)
        .map(|(_, flow)| flow)
        .ok_or_else(|| format!("missing {owner} flow"))?;
    if !matches!(flow, InputFlow::Checked { .. }) {
        return Err(format!("{owner} did not retain a checked graph: {flow:?}").into());
    }
    Ok(())
}

/// Require one body to refuse at the current T0123 boundary before a positive route is admitted.
fn require_unavailable(source: &str, owner: &str, reason: &str, stop: &str) -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked(source)?;
    let flow = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(identity, _)| identity.declaration_name == owner)
        .map(|(_, flow)| flow)
        .ok_or_else(|| format!("missing {owner} refusal"))?;
    let InputFlow::Unavailable(unavailable) = flow else {
        return Err(format!("{owner} unexpectedly admitted: {flow:?}").into());
    };
    assert_eq!(unavailable.reason, reason, "{:?}", checker.errors);
    assert_eq!(
        source
            .get(unavailable.span.start..unavailable.span.end)
            .map(str::trim_end),
        Some(stop)
    );
    assert!(
        !checker.errors.is_empty()
            && checker
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
        "unrelated diagnostics: {:?}",
        checker.errors
    );
    Ok(())
}

/// Return one named checked graph for adversarial current-source validation.
fn checked_graph(source: &str, owner: &str) -> Result<flow::Graph, Box<dyn std::error::Error>> {
    let checker = checked(source)?;
    let flow = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(identity, _)| identity.declaration_name == owner)
        .map(|(_, flow)| flow)
        .ok_or_else(|| format!("missing {owner} flow"))?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("{owner} did not retain a checked graph: {flow:?}").into());
    };
    Ok(graph.clone())
}

/// Exact classified places, loan paths and match roles cannot be replaced while retaining an access tag or span.
#[test]
fn signature_block_value_use_mutations_reject_access_substitution() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = checked_graph(
        "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Owned[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
        "collect",
    )?;
    let retained = snapshot
        .value_uses
        .values()
        .filter_map(|value_use| value_use.accesses.first())
        .cloned()
        .collect::<Vec<_>>();
    let [first, second, ..] = retained.as_slice() else {
        return Err("snapshot fixture retained fewer than two classified accesses".into());
    };

    let mut substituted = snapshot.clone();
    let flow::Event::Access { place, .. } = &mut substituted
        .nodes
        .get_mut(first.node)
        .ok_or("missing classified access")?
        .event
    else {
        return Err("classified node is not an access".into());
    };
    *place = second.place.clone();
    assert!(flow::analyze(&substituted).is_err());

    let mut changed_loan = snapshot.clone();
    let flow::Event::Access { through, .. } = &mut changed_loan
        .nodes
        .get_mut(first.node)
        .ok_or("missing classified access")?
        .event
    else {
        return Err("classified node is not an access".into());
    };
    *through = Some(flow::LoanId(usize::MAX));
    assert!(flow::analyze(&changed_loan).is_err());

    let mut deleted = snapshot.clone();
    let deleted_span = *deleted
        .value_uses
        .iter()
        .find(|(_, value_use)| !value_use.accesses.is_empty())
        .map(|(span, _)| span)
        .ok_or("missing stored classification")?;
    deleted
        .value_uses
        .get_mut(&deleted_span)
        .ok_or("missing stored classification")?
        .accesses
        .clear();
    assert!(flow::analyze(&deleted).is_err());

    let mut repointed = snapshot;
    let first_span = *repointed
        .value_uses
        .iter()
        .find(|(_, value_use)| {
            value_use
                .accesses
                .first()
                .is_some_and(|access| access.node == first.node)
        })
        .map(|(span, _)| span)
        .ok_or("missing first classification")?;
    repointed
        .value_uses
        .get_mut(&first_span)
        .ok_or("missing first classification")?
        .accesses[0]
        .node = second.node;
    assert!(flow::analyze(&repointed).is_err());

    let mut matched = checked_graph(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Owned[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
        "decode",
    )?;
    let match_use = matched
        .value_uses
        .values_mut()
        .find(|value_use| {
            value_use
                .accesses
                .iter()
                .any(|access| access.role == flow::ValueUseRole::MatchSubject)
        })
        .ok_or("missing match classification")?;
    let [subject, payload] = match_use.accesses.as_mut_slice() else {
        return Err("match classification does not retain two roles".into());
    };
    std::mem::swap(&mut subject.node, &mut payload.node);
    assert!(flow::analyze(&matched).is_err());
    Ok(())
}

/// Immutable ordinary arguments may read only authenticated independent storage projections. Explicit ownership,
/// mutable slots and borrowed storage remain outside the read classification.
#[test]
fn signature_block_classifies_snapshot_and_element_field_reads() -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = checked(
        "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Owned[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
    )?;
    let element_field = checked(
        "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(seed: Owned[int]) -> str:\n    values = [Cell(value=\"a\")]\n    return keep(values[0].value)\n",
    )?;

    require_unavailable(
        "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Owned[int], rows: Owned[list[list[str]]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
        "collect",
        "ordinary argument requires inferred access or mutable binding facts",
        "header[3]",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(values: Borrowed[list[Cell]], proof: Owned[int] = 0) -> str:\n    return keep(values[0].value)\n",
        "decode",
        "ordinary argument requires inferred access or mutable binding facts",
        "values[0].value",
    )?;

    require_checked(&snapshot, "collect")?;
    require_checked(&element_field, "decode")
}

/// Fresh bytes literals and checked fieldless enum variants cross immutable ordinary slots without acquiring a
/// source place, loan, source node or stored-value classification.
#[test]
fn signature_block_admits_exact_fresh_ordinary_scalars_without_authority() -> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Projection:\n    Canonical\n    Named(str)\ndef observe_bytes(value: bytes) -> int:\n    return len(value)\ndef observe_projection(value: Projection) -> int:\n    match value:\n        Projection.Canonical => return 1\n        Projection.Named(name) => return len(name)\ndef inspect(seed: Owned[int]) -> int:\n    return observe_bytes(b\"family\") + observe_projection(Projection.Canonical) + observe_projection(Projection.Named(\"value\"))\n";
    let graph = checked_graph(accepted_source, "inspect")?;
    for (callee, source) in [
        ("observe_bytes", "b\"family\""),
        ("observe_projection", "Projection.Canonical"),
        ("observe_projection", "Projection.Named(\"value\")"),
    ] {
        let argument = graph
            .invocations
            .values()
            .find(|invocation| {
                invocation.callee.declaration_name == callee
                    && invocation.arguments.first().is_some_and(|argument| {
                        accepted_source.get(argument.span.start..argument.span.end) == Some(source)
                    })
            })
            .and_then(|invocation| invocation.arguments.first())
            .ok_or_else(|| format!("fresh ordinary argument lost {callee} invocation"))?;
        assert_eq!(argument.access, incan_semantics_core::IncanInputAccess::Inferred);
        assert!(
            argument.source.is_none(),
            "fresh ordinary argument acquired a source place"
        );
        assert!(argument.loan.is_none(), "fresh ordinary argument acquired a loan");
        assert!(
            argument.evaluation.source_node.is_none(),
            "fresh ordinary argument acquired a source node"
        );
        assert!(
            !graph
                .value_uses
                .keys()
                .any(|span| { accepted_source.get(span.start..span.end) == Some(source) }),
            "fresh ordinary value acquired stored ValueUse authority: {source}"
        );
    }
    Ok(())
}

/// Source-free ordinary literals and builtin indexing of immutable bytes remain usable in a body with explicit
/// input access. Repeated observation of the same element must retain an actual indexed read each time (#1605).
#[test]
fn ordinary_scalar_inputs_preserve_fresh_literals_and_indexed_reads() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"def labels(text: str, empty: Option[str]) -> int:
    return len(text)
def count(value: bytes) -> int:
    return len(value)
def inspect(seed: Owned[int], at: int) -> int:
    rows = [b"one", b"two"]
    first = count(rows[at])
    return first + count(rows[at]) + labels("utf-8", None)
"#;
    let graph = checked_graph(source, "inspect")?;
    let reads = graph
        .value_uses
        .iter()
        .filter(|(span, record)| {
            source.get(span.start..span.end) == Some("rows[at]") && record.kind == flow::ValueUseKind::Read
        })
        .count();
    assert_eq!(reads, 2, "each indexed observation must preserve its own read");
    Ok(())
}

/// An ordinary function returning the same enum stays on its authenticated CallResult route; it does not acquire the
/// source-less payload-Variant authority.
#[test]
fn signature_block_keeps_ordinary_enum_functions_on_call_result_route() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Projection:\n    Named(str)\ndef make() -> Projection:\n    return Projection.Named(\"value\")\ndef observe(value: Projection) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(make())\n";
    let graph = checked_graph(source, "inspect")?;
    let value_use = graph
        .value_uses
        .iter()
        .find(|(span, _)| source.get(span.start..span.end) == Some("make()"))
        .map(|(_, value_use)| value_use)
        .ok_or("ordinary enum function lost its CallResult authority")?;
    assert_eq!(value_use.kind, flow::ValueUseKind::CallResult);
    Ok(())
}

/// One canonical field may be taken from a fresh dependency-free direct call for an ordinary immutable argument.
/// The outer value retains source-less CallResult authority while source and replay execute the inner call once.
#[test]
fn signature_block_admits_exact_direct_call_field_result_and_replays_it() -> Result<(), Box<dyn std::error::Error>> {
    let source = "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\ndef make() -> Description:\n    return Description(metadata=Metadata(value=\"value\"))\ndef observe(value: Metadata) -> int:\n    return len(value.value)\ndef inspect(seed: Owned[int]) -> int:\n    return observe(make().metadata)\n";
    let checker = checked(source)?;
    let (make_span, make_call) = checker
        .type_info
        .protocols
        .explicit_input_calls
        .iter()
        .find(|(span, call)| {
            source.get(span.start..span.end) == Some("make()")
                && call
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.declaration_name == "inspect")
        })
        .ok_or("projected base lost its selected call facts")?;
    assert!(matches!(
        make_call.kind,
        InputCallKind::Ordinary | InputCallKind::Explicit
    ));
    let make_contract = checker.type_info.selected_input_contract(make_call)?;
    assert!(crate::ownership_metadata::returned_view(make_contract)?.is_none());
    assert!(crate::ownership_metadata::owned_result(make_contract)?.is_none());
    assert!(make_contract.is_none_or(|contract| contract.result_source.is_none()));
    assert!(checker.native_result_dependencies(*make_span)?.is_none());
    assert!(!checker.native_result_is_independent(*make_span));
    require_checked(&checker, "inspect")?;
    let (owner, graph) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .and_then(|(owner, flow)| match flow {
            InputFlow::Checked { graph, .. } => Some((owner, graph)),
            _ => None,
        })
        .ok_or("missing inspect graph")?;
    let (span, value_use) = graph
        .value_uses
        .iter()
        .find(|(span, _)| source.get(span.start..span.end) == Some("make().metadata"))
        .ok_or("fresh field projection lost its outer classification")?;
    assert_eq!(value_use.kind, flow::ValueUseKind::CallResult);
    assert!(value_use.accesses.is_empty());
    assert_eq!(source.get(span.start..span.end), Some("make().metadata"));
    let invocation = graph
        .invocations
        .values()
        .find(|invocation| invocation.callee.declaration_name == "observe")
        .ok_or("missing observe invocation")?;
    let argument = invocation.arguments.first().ok_or("missing observe argument")?;
    assert_eq!(argument.span, *span);
    assert_eq!(argument.access, incan_semantics_core::IncanInputAccess::Inferred);
    assert!(argument.source.is_none());
    assert!(argument.loan.is_none());
    assert!(argument.evaluation.source_node.is_none());
    assert_eq!(
        graph
            .invocations
            .values()
            .filter(|invocation| invocation.callee.declaration_name == "make")
            .count(),
        1
    );
    assert!(checker.type_info.checked_input_body(owner)?.is_some());
    Ok(())
}

/// A field on an existing local remains a stored Read and never borrows fresh call-result authority.
#[test]
fn signature_block_keeps_stored_field_on_ordinary_read_route() -> Result<(), Box<dyn std::error::Error>> {
    let source = "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\ndef make() -> Description:\n    return Description(metadata=Metadata(value=\"value\"))\ndef observe(value: Metadata) -> int:\n    return len(value.value)\ndef inspect(seed: Owned[int]) -> int:\n    stored = make()\n    return observe(stored.metadata)\n";
    let graph = checked_graph(source, "inspect")?;
    let argument = graph
        .invocations
        .values()
        .find(|invocation| invocation.callee.declaration_name == "observe")
        .and_then(|invocation| invocation.arguments.first())
        .ok_or("stored field lost its ordinary invocation")?;
    let place = argument.source.as_ref().ok_or("stored field lost its source place")?;
    assert!(matches!(&place.root, flow::Root::Binding(identity)
        if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Local
            && identity.declaration_name == "stored"));
    assert_eq!(place.projection.len(), 1);
    assert!(argument.evaluation.source_node.is_some_and(|node| {
        matches!(graph.nodes.get(node).map(|node| &node.event),
            Some(flow::Event::Access { place: read, access: flow::Access::Read, through: None })
                if read == place)
    }));
    assert!(!graph.value_uses.contains_key(&argument.span));
    Ok(())
}

/// Nested, indexed, `?`-wrapped and method-call results cannot impersonate the one direct-call field shape.
#[test]
fn signature_block_refuses_other_call_result_projection_shapes() -> Result<(), Box<dyn std::error::Error>> {
    require_unavailable(
        "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Inner:\n    metadata: Metadata\n@derive(Clone)\nmodel Description:\n    inner: Inner\ndef make() -> Description:\n    return Description(inner=Inner(metadata=Metadata(value=\"value\")))\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(make().inner.metadata)\n",
        "inspect",
        "expression is not a stored place",
        "make()",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Description:\n    values: list[str]\ndef make() -> Description:\n    return Description(values=[\"value\"])\ndef observe(value: str) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(make().values[0])\n",
        "inspect",
        "expression is not a stored place",
        "make()",
    )?;
    require_unavailable(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\ndef make() -> Result[Description, Fault]:\n    return Ok(Description(metadata=Metadata(value=\"value\")))\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> Result[int, Fault]:\n    return Ok(observe(make()?.metadata))\n",
        "inspect",
        "expression is not a stored place",
        "make()?",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\nclass Factory:\n    pub marker: int\n    pub def make(self) -> Description:\n        return Description(metadata=Metadata(value=\"value\"))\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    factory = Factory(marker=1)\n    return observe(factory.make().metadata)\n",
        "inspect",
        "expression is not a stored place",
        "factory.make()",
    )
}

/// Computed properties and returned-view bases do not acquire fresh field-projection authority.
#[test]
fn signature_block_refuses_open_or_dependent_call_result_fields() -> Result<(), Box<dyn std::error::Error>> {
    require_unavailable(
        "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    raw: Metadata\n    property metadata -> Metadata:\n        return Metadata(value=self.raw.value)\ndef make() -> Description:\n    return Description(raw=Metadata(value=\"value\"))\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(make().metadata)\n",
        "inspect",
        "computed property access",
        "make().metadata",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\ndef view(value: Borrowed[Description]) -> Borrowed[Description] from value:\n    return value\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int], value: Borrowed[Description]) -> int:\n    return observe(view(value).metadata)\n",
        "inspect",
        "expression is not a stored place",
        "view(value)",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Metadata:\n    value: str\nmodel Envelope:\n    metadata: Borrowed[Metadata]\ndef make(value: Borrowed[Metadata]) -> Envelope from value:\n    return Envelope(metadata=value)\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int], value: Borrowed[Metadata]) -> int:\n    return observe(make(value).metadata)\n",
        "inspect",
        "expression is not a stored place",
        "make(value)",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\ndef make(value: Borrowed[Description]) -> Owned[Description] from value:\n    return Description(metadata=value.metadata)\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int], value: Borrowed[Description]) -> int:\n    return observe(make(value).metadata)\n",
        "inspect",
        "expression is not a stored place",
        "make(value)",
    )
}

/// The outer canonical Field identity is part of classification authority; changing it after checking must close the
/// route before any source-less CallResult is retained.
#[test]
fn signature_block_direct_call_field_identity_tamper_refuses() -> Result<(), Box<dyn std::error::Error>> {
    let source = "@derive(Clone)\nmodel Metadata:\n    value: str\n@derive(Clone)\nmodel Description:\n    metadata: Metadata\ndef make() -> Description:\n    return Description(metadata=Metadata(value=\"value\"))\ndef observe(value: Metadata) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(make().metadata)\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| {
            matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == "inspect")
        })
        .ok_or("missing inspect declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("inspect is not a function".into());
    };
    let mut checker = checked(source)?;
    let (owner, field_span) = {
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "inspect")
            .ok_or("missing inspect flow")?;
        let InputFlow::Checked { graph, .. } = flow else {
            return Err(format!("field projection baseline did not check: {flow:?}").into());
        };
        let field_span = *graph
            .value_uses
            .keys()
            .find(|span| source.get(span.start..span.end) == Some("make().metadata"))
            .ok_or("missing projected CallResult")?;
        (owner.clone(), field_span)
    };
    let mut tampered = checker
        .type_info
        .resolved_identity(field_span)
        .cloned()
        .ok_or("projected value has no canonical Field identity")?;
    assert_eq!(tampered.kind, incan_semantics_core::SemanticSourceTargetKind::Field);
    tampered.kind = incan_semantics_core::SemanticSourceTargetKind::Function;
    checker.type_info.record_resolved_identity(field_span, tampered);
    let unavailable = source::build(&checker, CallableSource::Function(function), declaration.span, &owner)
        .expect_err("tampered field identity retained projected CallResult authority");
    assert_eq!(unavailable.reason, "field lacks a checked stored identity");
    assert_eq!(
        source.get(unavailable.span.start..unavailable.span.end),
        Some("make().metadata")
    );
    Ok(())
}

/// Fresh payload-Variant admission depends on the exact full-expression Variant identity, not merely method-call
/// syntax or an equal result spelling.
#[test]
fn signature_block_payload_variant_identity_tamper_refuses_fresh_route() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Projection:\n    Named(str)\ndef observe(value: Projection) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(Projection.Named(\"value\"))\n";
    let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| {
            matches!(&declaration.node, crate::ast::Declaration::Function(function) if function.name == "inspect")
        })
        .ok_or("missing inspect declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("inspect is not a function".into());
    };
    let mut checker = checked(source)?;
    let (owner, payload_span) = {
        let (owner, flow) = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "inspect")
            .ok_or("missing inspect flow")?;
        let InputFlow::Checked { graph, .. } = flow else {
            return Err(format!("fresh payload baseline did not check: {flow:?}").into());
        };
        let payload_span = graph
            .invocations
            .values()
            .find(|invocation| invocation.callee.declaration_name == "observe")
            .and_then(|invocation| invocation.arguments.first())
            .map(|argument| argument.span)
            .ok_or("missing payload argument")?;
        (owner.clone(), payload_span)
    };
    let [statement] = function.body.as_slice() else {
        return Err("inspect body does not contain one return".into());
    };
    let crate::ast::Statement::Return(Some(returned)) = &statement.node else {
        return Err("inspect body is not one returned call".into());
    };
    let Expr::Call(_, _, arguments) = &returned.node else {
        return Err("inspect did not return observe(...)".into());
    };
    let [crate::ast::CallArg::Positional(payload)] = arguments.as_slice() else {
        return Err("observe did not retain one positional payload".into());
    };
    let Expr::MethodCall(_, _, _, _) = &payload.node else {
        return Err("observe payload is not a Variant method-call expression".into());
    };
    assert_eq!(payload.span, payload_span);
    let whole_identity = checker
        .type_info
        .resolved_identity(payload.span)
        .ok_or("payload call has no full-expression Variant identity")?;
    assert_eq!(
        whole_identity.kind,
        incan_semantics_core::SemanticSourceTargetKind::Variant
    );
    let mut tampered = checker
        .type_info
        .resolved_identity(payload_span)
        .cloned()
        .ok_or("payload call has no resolved Variant identity")?;
    tampered.kind = incan_semantics_core::SemanticSourceTargetKind::Function;
    checker.type_info.record_resolved_identity(payload_span, tampered);
    let unavailable = source::build(&checker, CallableSource::Function(function), declaration.span, &owner)
        .expect_err("tampered payload identity retained fresh ordinary admission");
    assert_eq!(
        unavailable.reason,
        "ordinary argument requires inferred access or mutable binding facts"
    );
    assert_eq!(
        source.get(unavailable.span.start..unavailable.span.end),
        Some("Projection.Named(\"value\")")
    );
    Ok(())
}

/// Stored locals and explicitly borrowed temporaries retain their established Read and Shared-loan routes rather
/// than acquiring source-less fresh ordinary authority.
#[test]
fn signature_block_preserves_stored_and_borrowed_ordinary_routes() -> Result<(), Box<dyn std::error::Error>> {
    let stored_source = "def observe(value: bytes) -> int:\n    return len(value)\ndef inspect(seed: Owned[int]) -> int:\n    stored = b\"family\"\n    return observe(stored)\n";
    let stored = checked_graph(stored_source, "inspect")?;
    let argument = stored
        .invocations
        .values()
        .find(|invocation| invocation.callee.declaration_name == "observe")
        .and_then(|invocation| invocation.arguments.first())
        .ok_or("stored ordinary argument lost its invocation")?;
    let place = argument
        .source
        .as_ref()
        .ok_or("stored ordinary argument lost its source place")?;
    assert!(
        matches!(&place.root, flow::Root::Binding(identity)
            if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Local
                && identity.declaration_name == "stored")
            && place.projection.is_empty(),
        "stored ordinary argument did not retain its exact Local source: {place:?}"
    );
    assert!(
        argument.evaluation.source_node.is_some_and(|source_node| {
            matches!(stored.nodes.get(source_node).map(|node| &node.event),
                Some(flow::Event::Access { place: read, access: flow::Access::Read, through: None })
                    if read == place)
        }),
        "stored ordinary argument did not retain its independent Read"
    );
    assert!(
        !stored
            .value_uses
            .keys()
            .any(|span| { stored_source.get(span.start..span.end) == Some("stored") }),
        "stored ordinary argument acquired fresh ValueUse authority"
    );

    let borrowed_source = "def observe(value: Borrowed[bytes]) -> int:\n    return len(value)\ndef inspect(seed: Owned[int]) -> int:\n    return observe(b\"family\")\n";
    let borrowed = checked_graph(borrowed_source, "inspect")?;
    let invocation = borrowed
        .invocations
        .values()
        .find(|invocation| invocation.callee.declaration_name == "observe")
        .ok_or("borrowed bytes argument lost its invocation")?;
    let argument = invocation
        .arguments
        .first()
        .ok_or("borrowed bytes invocation lost its argument")?;
    assert_eq!(argument.access, incan_semantics_core::IncanInputAccess::Shared);
    let loan = argument.loan.ok_or("borrowed bytes argument lost its Shared loan")?;
    let place = argument
        .source
        .as_ref()
        .ok_or("borrowed bytes argument lost its Temporary source")?;
    assert!(
        matches!(&place.root, flow::Root::Temporary { expression, invocation, .. }
            if borrowed_source.get(expression.start..expression.end) == Some("b\"family\"")
                && borrowed_source.get(invocation.start..invocation.end) == Some("observe(b\"family\")"))
            && place.projection.is_empty(),
        "borrowed bytes argument did not retain its exact Temporary source: {place:?}"
    );
    assert!(
        argument.evaluation.source_node.is_some_and(|source_node| {
            matches!(borrowed.nodes.get(source_node).map(|node| &node.event),
                Some(flow::Event::Borrow(id)) if *id == loan)
        }),
        "borrowed bytes argument did not retain its Borrow event"
    );
    assert!(
        invocation.final_accesses.iter().any(|final_access| {
            matches!(borrowed.nodes.get(*final_access).map(|node| &node.event),
                Some(flow::Event::Access {
                    place: read,
                    access: flow::Access::Read,
                    through: Some(id),
                }) if read == place && *id == loan)
        }),
        "borrowed bytes argument did not retain its final Read through the Shared loan"
    );
    assert!(
        !borrowed
            .value_uses
            .keys()
            .any(|span| { borrowed_source.get(span.start..span.end) == Some("b\"family\"") }),
        "borrowed bytes argument acquired fresh ValueUse authority"
    );
    Ok(())
}

/// An ordinary mutable slot may read and reuse only one exact whole cloneable mutable Local. The route preserves its
/// Local source and ordinary Read events without creating a loan, Move or stored ValueUse authority.
#[test]
fn signature_block_admits_only_exact_mutable_cloneable_local_reads() -> Result<(), Box<dyn std::error::Error>> {
    let source = "@derive(Clone)\nmodel Budget:\n    value: int\ndef charge(mut budget: Budget) -> int:\n    return budget.value\ndef inspect(seed: Owned[int]) -> int:\n    mut budget = Budget(value=1)\n    first = charge(budget)\n    return first + charge(budget)\n";
    let graph = checked_graph(source, "inspect")?;
    let arguments = graph
        .invocations
        .values()
        .filter(|invocation| invocation.callee.declaration_name == "charge")
        .filter_map(|invocation| invocation.arguments.first())
        .collect::<Vec<_>>();
    assert_eq!(arguments.len(), 2);
    for argument in arguments {
        assert_eq!(argument.access, incan_semantics_core::IncanInputAccess::Inferred);
        assert!(argument.loan.is_none());
        let place = argument
            .source
            .as_ref()
            .ok_or("mutable ordinary argument lost its Local source")?;
        assert!(
            matches!(&place.root, flow::Root::Binding(identity)
                if identity.kind == incan_semantics_core::SemanticSourceTargetKind::Local
                    && identity.declaration_name == "budget")
                && place.projection.is_empty(),
            "mutable ordinary argument retained the wrong source: {place:?}"
        );
        assert!(
            argument.evaluation.source_node.is_some_and(|source_node| {
                matches!(graph.nodes.get(source_node).map(|node| &node.event),
                    Some(flow::Event::Access { place: read, access: flow::Access::Read, through: None })
                        if read == place)
            }),
            "mutable ordinary argument lost its exact Read"
        );
        assert!(!graph.value_uses.contains_key(&argument.span));
    }

    require_unavailable(
        "@derive(Clone)\nmodel Budget:\n    value: int\ndef charge(mut budget: Budget) -> int:\n    return budget.value\ndef inspect(seed: Owned[int]) -> int:\n    budget = Budget(value=1)\n    return charge(budget)\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "budget",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Budget:\n    value: int\n@derive(Clone)\nmodel Holder:\n    budget: Budget\ndef charge(mut budget: Budget) -> int:\n    return budget.value\ndef inspect(seed: Owned[int]) -> int:\n    mut holder = Holder(budget=Budget(value=1))\n    return charge(holder.budget)\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "holder.budget",
    )?;
    require_unavailable(
        "def numbers() -> Generator[int]:\n    yield 1\ndef charge(mut values: Generator[int]) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    mut values = numbers()\n    return charge(values)\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "values",
    )?;

    require_unavailable(
        "@derive(Clone)\nmodel Budget:\n    value: int\ndef charge(mut budget: Budget) -> int:\n    return budget.value\ndef inspect(seed: Owned[int]) -> int:\n    mut budget = Budget(value=1)\n    held: Borrowed[Budget] = budget\n    result = charge(budget)\n    return result + held.value\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "budget",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Budget:\n    value: int\ndef take(value: Owned[Budget]) -> None:\n    pass\ndef charge(mut budget: Budget) -> int:\n    return budget.value\ndef inspect(seed: Owned[int]) -> int:\n    mut budget = Budget(value=1)\n    take(budget)\n    return charge(budget)\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "budget",
    )?;
    Ok(())
}

/// A mutable ordinary slot cannot borrow the source-less fresh scalar route.
#[test]
fn signature_block_refuses_mutable_fresh_ordinary_route() -> Result<(), Box<dyn std::error::Error>> {
    require_unavailable(
        "def observe(mut value: bytes) -> int:\n    return len(value)\ndef inspect(seed: Owned[int]) -> int:\n    return observe(b\"family\")\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "b\"family\"",
    )?;
    require_unavailable(
        "enum Projection:\n    Named(str)\ndef observe(mut value: Projection) -> int:\n    return 1\ndef inspect(seed: Owned[int]) -> int:\n    return observe(Projection.Named(\"value\"))\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "Projection.Named(\"value\")",
    )
}

/// A reference-carrying payload Variant cannot use the fresh ordinary route even when its constructor identity and
/// outer enum type are exact.
#[test]
fn signature_block_refuses_open_payload_variant_fresh_ordinary_route() -> Result<(), Box<dyn std::error::Error>> {
    require_unavailable(
        "enum Envelope:\n    Held(Borrowed[str])\ndef observe(value: Envelope) -> int:\n    return 1\ndef inspect(value: Borrowed[str], proof: Owned[int] = 0) -> int:\n    return observe(Envelope.Held(value))\n",
        "inspect",
        "ordinary argument requires inferred access or mutable binding facts",
        "Envelope.Held(value)",
    )
}

/// A payload-variant constructor used as a function value cannot impersonate a checked fieldless enum value.
#[test]
fn signature_block_refuses_payload_variant_callback_fresh_ordinary_route() -> Result<(), Box<dyn std::error::Error>> {
    require_unavailable(
        "enum Projection:\n    Named(str)\ndef invoke(factory: (str) -> Projection) -> Projection:\n    return factory(\"value\")\ndef inspect(seed: Owned[int]) -> Projection:\n    return invoke(Projection.Named)\n",
        "inspect",
        "call result needs retained dependency facts",
        "invoke(Projection.Named)",
    )
}

/// Nominal fallback never chooses between equal leaves from different canonical origins. Constructor and variant
/// classifiers share this resolver, so map iteration order cannot grant either route authority.
#[test]
fn signature_block_nominal_fallback_refuses_same_leaf_different_origins() -> Result<(), Box<dyn std::error::Error>> {
    let checker = checked("enum Family:\n    Local\n")?;
    let identity = checker
        .checked_nominal_type_identities()
        .values()
        .find(|identity| identity.declaration_name == "Family")
        .cloned()
        .ok_or("missing Family identity")?;
    let mut other = identity.clone();
    other.origin = incan_semantics_core::SymbolOrigin::Module(vec!["other".into()]);
    let scope = std::collections::BTreeMap::from([
        ("__first_family".to_string(), identity),
        ("__second_family".to_string(), other),
    ]);
    assert!(super::super::projection::checked_nominal_identity_in(&scope, "Family").is_none());
    assert!(!super::super::projection::same_checked_value_type_in(
        &scope,
        &crate::symbols::ResolvedType::Named("Family".into()),
        &crate::symbols::ResolvedType::Named("pub::other::Family".into()),
    ));
    Ok(())
}

/// Constructor fields consume exact whole transferable values once. Borrowed and explicitly owned parameters do not
/// borrow this route, and a second use after the admitted move remains a checked T0126 conflict.
#[test]
fn signature_block_classifies_enum_parameter_and_local_list_moves() -> Result<(), Box<dyn std::error::Error>> {
    let enum_value = checked(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Owned[int], family: Family) -> Field:\n    return Field(family=family)\n",
    )?;
    let list_value = checked(
        "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Owned[int]) -> Record:\n    mut fields: list[Field] = []\n    fields.append(Field(value=\"a\"))\n    return Record(fields=fields)\n",
    )?;
    let mutable_final_value = checked(
        "model Record:\n    profile: str\ndef collect(seed: Owned[int]) -> Record:\n    mut profile = \"\"\n    if seed > 0:\n        profile = \"positive\"\n    else:\n        profile = \"other\"\n    return Record(profile=profile)\n",
    )?;

    require_unavailable(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(family: Borrowed[Family], proof: Owned[int] = 0) -> Field:\n    return Field(family=family)\n",
        "collect",
        source::BORROWED_VALUE_IN_OWNED_FIELD,
        "family",
    )?;
    require_unavailable(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Owned[int], family: Owned[Family]) -> Field:\n    return Field(family=family)\n",
        "collect",
        "non-Copy constructor field needs checked ownership transfer materialization",
        "family",
    )?;

    for source in [
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Owned[int], family: Family) -> Field:\n    first = Field(family=family)\n    second = Field(family=family)\n    return second\n",
    ] {
        let checker = checked(source)?;
        assert!(
            !checker.errors.is_empty()
                && checker
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0126")),
            "move reuse did not remain a conflict: {:?}",
            checker.errors
        );
    }

    require_checked(&enum_value, "collect")?;
    require_checked(&list_value, "collect")?;
    require_checked(&mutable_final_value, "collect")
}

/// Compiler-selected builtin Clone calls retain an exact invocation even though their member has no source
/// declaration. A same-spelled user method remains an ordinary source call and acquires no builtin copy authority.
#[test]
fn signature_block_retains_builtin_clone_invocation() -> Result<(), Box<dyn std::error::Error>> {
    let source = "model Record:\n    text: str\ndef copy(value: Borrowed[str]) -> Owned[Record]:\n    return Record(text=value.clone())\n";
    let checker = checked(source)?;
    require_checked(&checker, "copy")?;
    let (clone_span, operation) = checker
        .type_info
        .calls
        .resolved_builtin_method_calls
        .iter()
        .find(|(_, operation)| {
            **operation
                == incan_lang::lang::builtins::BuiltinMethodId::Clone(
                    incan_lang::lang::surface::clone_methods::CloneMethodId::Clone,
                )
        })
        .ok_or("missing selected builtin Clone call")?;
    assert_eq!(operation.owner(), "Clone");
    let call = checker
        .type_info
        .protocols
        .explicit_input_calls
        .get(clone_span)
        .ok_or("builtin Clone call has no retained input fact")?;
    assert_eq!(call.kind, InputCallKind::Ordinary);
    assert_eq!(call.receiver, Some(crate::ast::Receiver::Immutable));
    assert!(call.parameters.is_empty());
    let flow = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "copy")
        .map(|(_, flow)| flow)
        .ok_or("missing copy flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err(format!("builtin Clone flow is unavailable: {flow:?}").into());
    };
    assert!(graph.invocations.contains_key(clone_span));

    let user_source = "model Copier:\n    def clone(self) -> str:\n        return \"user\"\ndef copy(seed: Owned[int]) -> str:\n    copier = Copier()\n    return copier.clone()\n";
    let user = checked(user_source)?;
    require_checked(&user, "copy")?;
    assert!(
        user.type_info
            .calls
            .resolved_builtin_method_calls
            .values()
            .all(|operation| !matches!(operation, incan_lang::lang::builtins::BuiltinMethodId::Clone(_)))
    );
    Ok(())
}

/// Matching a closed, nominal error from a direct call retains the evaluation of its argument and both arms.
#[test]
fn match_admits_a_closed_nominal_error_from_an_ordinary_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"model Fault:
    detail: str
def create(value: int) -> Result[None, Fault]:
    return Err(Fault(detail="failed"))
def inspect(seed: Owned[int]) -> bool:
    match create(seed):
        Ok(_) => return true
        Err(_) => return false
"#;
    require_checked(&checked(source)?, "inspect")
}

/// A selected dependency-free `Result` call may provide an exact constructor field through `?`. Stored carriers,
/// carrier constructors and returned views remain materialization refusals.
#[test]
fn signature_block_classifies_closed_try_call_results() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "enum Fault:\n    Bad\nenum Family:\n    Local\n@derive(Clone)\nmodel Row:\n    value: str\nmodel Record:\n    text: str\n    family: Family\n    rows: list[Row]\ndef text_value() -> Result[str, Fault]:\n    return Ok(\"value\")\ndef family_value() -> Result[Family, Fault]:\n    return Ok(Family.Local)\ndef row_values() -> Result[list[Row], Fault]:\n    return Ok([Row(value=\"value\")])\ndef decode(seed: Owned[int]) -> Result[Record, Fault]:\n    return Ok(Record(text=text_value()?, family=family_value()?, rows=row_values()?))\n",
    )?;

    require_unavailable(
        "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef decode(seed: Owned[int], value: str) -> Result[Record, Fault]:\n    result: Result[str, Fault] = Ok(value)\n    return Ok(Record(text=result?))\n",
        "decode",
        "non-Copy constructor field needs checked ownership transfer materialization",
        "result?",
    )?;
    require_unavailable(
        "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef decode(seed: Owned[int], value: str) -> Result[Record, Fault]:\n    return Ok(Record(text=Ok(value)?))\n",
        "decode",
        "non-Copy constructor field needs checked ownership transfer materialization",
        "Ok(value)?",
    )?;
    require_unavailable(
        "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef view(value: Borrowed[str]) -> Result[Borrowed[str], Fault] from value:\n    return Ok(value)\ndef decode(value: Borrowed[str]) -> Result[Record, Fault]:\n    return Ok(Record(text=view(value)?))\n",
        "decode",
        "non-Copy constructor field needs checked ownership transfer materialization",
        "view(value)?",
    )?;

    require_checked(&accepted, "decode")
}

/// One continuing enum-payload arm may move its payload into a local while a payload-free fallback terminates. A
/// borrowed subject and a second continuing arm remain refused, and post-extraction reuse remains a T0126 conflict.
#[test]
fn signature_block_classifies_match_payload_extraction() -> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Owned[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n";
    let accepted = checked(accepted_source)?;

    require_unavailable(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\ndef select(value: Borrowed[Payload], proof: Owned[int] = 0) -> Result[list[Cell], Fault]:\n    values = match value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
        "select",
        "expression needs checked call or dependency access facts",
        "match value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)",
    )?;
    require_unavailable(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n    Empty\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Owned[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        Payload.Empty => []\n    return Ok(values)\n",
        "select",
        "expression needs checked call or dependency access facts",
        "match row.value:\n        Payload.Record(fields) => fields\n        Payload.Empty => []",
    )?;

    let reused = checked(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Owned[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    again = row.value\n    return Ok(values)\n",
    )?;
    assert!(
        !reused.errors.is_empty()
            && reused
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0126")),
        "post-extraction reuse did not remain a conflict: {:?}",
        reused.errors
    );

    require_checked(&accepted, "decode")?;
    let flow = accepted
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "decode")
        .map(|(_, flow)| flow)
        .ok_or("missing match flow")?;
    let InputFlow::Checked { graph, .. } = flow else {
        return Err("match extraction did not retain a graph".into());
    };
    for spelling in ["row.value", "fields"] {
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| {
                    matches!(
                        node.event,
                        flow::Event::Access {
                            access: flow::Access::Move,
                            ..
                        }
                    ) && accepted_source.get(node.span.start..node.span.end) == Some(spelling)
                })
                .count(),
            1,
            "missing exact {spelling} move"
        );
    }
    Ok(())
}

/// The full record header shape combines snapshot reads, one closed `Try(Call)` field, one plain Enum Parameter move
/// and the final whole-list move. The grouped controls above remain the refusal matrix for each individual route.
#[test]
fn signature_block_classifies_full_header_record() -> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Family:\n    Local\nmodel CanonicalField:\n    value: str\nmodel Record:\n    profile: str\n    owner: str\n    root: str\n    family: Family\n    fields: list[CanonicalField]\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef from_native(seed: Owned[int], rows: list[list[str]]) -> Result[Record, str]:\n    header = rows[0]\n    mut fields: list[CanonicalField] = []\n    for row in rows[1:]:\n        fields.append(CanonicalField(value=row[0]))\n    record = Record(profile=header[4], owner=header[1], root=header[2], family=read_family(header[3])?, fields=fields)\n    return Ok(record)\ndef from_family(seed: Owned[int], rows: list[list[str]], family: Family) -> Record:\n    header = rows[0]\n    mut fields: list[CanonicalField] = []\n    for row in rows[1:]:\n        fields.append(CanonicalField(value=row[0]))\n    return Record(profile=header[4], owner=header[1], root=header[2], family=family, fields=fields)\n";
    let checker = checked(source)?;
    require_checked(&checker, "from_native")?;
    require_checked(&checker, "from_family")
}

include!("signature_block/str_encode.rs");
