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
fn require_unavailable(
    source: &str,
    owner: &str,
    reason: &str,
    stop: &str,
) -> Result<(), Box<dyn std::error::Error>> {
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
        "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
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
        .find(|(_, value_use)| value_use.accesses.first().is_some_and(|access| access.node == first.node))
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
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
        "decode",
    )?;
    let match_use = matched
        .value_uses
        .values_mut()
        .find(|value_use| {
            value_use.accesses.iter().any(|access| access.role == flow::ValueUseRole::MatchSubject)
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
fn signature_block_classifies_snapshot_and_element_field_reads()
-> Result<(), Box<dyn std::error::Error>> {
    let snapshot = checked(
        "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
    )?;
    let element_field = checked(
        "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(seed: Borrowed[int]) -> str:\n    values = [Cell(value=\"a\")]\n    return keep(values[0].value)\n",
    )?;

    require_unavailable(
        "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: Owned[list[list[str]]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
        "collect",
        "ordinary argument requires inferred access or mutable binding facts",
        "header[3]",
    )?;
    require_unavailable(
        "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(values: Borrowed[list[Cell]]) -> str:\n    return keep(values[0].value)\n",
        "decode",
        "ordinary argument requires inferred access or mutable binding facts",
        "values[0].value",
    )?;

    require_checked(&snapshot, "collect")?;
    require_checked(&element_field, "decode")
}

/// Constructor fields consume exact whole transferable values once. Borrowed and explicitly owned parameters do not
/// borrow this route, and a second use after the admitted move remains a checked T0126 conflict.
#[test]
fn signature_block_classifies_enum_parameter_and_local_list_moves()
-> Result<(), Box<dyn std::error::Error>> {
    let enum_value = checked(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    return Field(family=family)\n",
    )?;
    let list_value = checked(
        "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    mut fields: list[Field] = []\n    fields.append(Field(value=\"a\"))\n    return Record(fields=fields)\n",
    )?;

    require_unavailable(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(family: Borrowed[Family]) -> Field:\n    return Field(family=family)\n",
        "collect",
        source::BORROWED_VALUE_IN_OWNED_FIELD,
        "family",
    )?;
    require_unavailable(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Owned[Family]) -> Field:\n    return Field(family=family)\n",
        "collect",
        "non-Copy constructor field needs checked ownership transfer materialization",
        "family",
    )?;

    for source in [
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    first = Field(family=family)\n    second = Field(family=family)\n    return second\n",
        "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    fields = [Field(value=\"a\")]\n    first = Record(fields=fields)\n    second = Record(fields=fields)\n    return second\n",
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
    require_checked(&list_value, "collect")
}

/// A selected dependency-free `Result` call may provide an exact constructor field through `?`. Stored carriers,
/// carrier constructors and returned views remain materialization refusals.
#[test]
fn signature_block_classifies_closed_try_call_results()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "enum Fault:\n    Bad\nenum Family:\n    Local\n@derive(Clone)\nmodel Row:\n    value: str\nmodel Record:\n    text: str\n    family: Family\n    rows: list[Row]\ndef text_value() -> Result[str, Fault]:\n    return Ok(\"value\")\ndef family_value() -> Result[Family, Fault]:\n    return Ok(Family.Local)\ndef row_values() -> Result[list[Row], Fault]:\n    return Ok([Row(value=\"value\")])\ndef decode(seed: Borrowed[int]) -> Result[Record, Fault]:\n    return Ok(Record(text=text_value()?, family=family_value()?, rows=row_values()?))\n",
    )?;

    require_unavailable(
        "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef decode(seed: Borrowed[int], value: str) -> Result[Record, Fault]:\n    result: Result[str, Fault] = Ok(value)\n    return Ok(Record(text=result?))\n",
        "decode",
        "non-Copy constructor field needs checked ownership transfer materialization",
        "result?",
    )?;
    require_unavailable(
        "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef decode(seed: Borrowed[int], value: str) -> Result[Record, Fault]:\n    return Ok(Record(text=Ok(value)?))\n",
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
fn signature_block_classifies_match_payload_extraction()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n";
    let accepted = checked(accepted_source)?;

    require_unavailable(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\ndef select(value: Borrowed[Payload]) -> Result[list[Cell], Fault]:\n    values = match value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
        "select",
        "expression needs checked call or dependency access facts",
        "match value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)",
    )?;
    require_unavailable(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n    Empty\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        Payload.Empty => []\n    return Ok(values)\n",
        "select",
        "expression needs checked call or dependency access facts",
        "match row.value:\n        Payload.Record(fields) => fields\n        Payload.Empty => []",
    )?;

    let reused = checked(
        "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    again = row.value\n    return Ok(values)\n",
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
                    matches!(node.event, flow::Event::Access { access: flow::Access::Move, .. })
                        && accepted_source.get(node.span.start..node.span.end) == Some(spelling)
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
fn signature_block_classifies_full_header_record()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Family:\n    Local\nmodel CanonicalField:\n    value: str\nmodel Record:\n    profile: str\n    owner: str\n    root: str\n    family: Family\n    fields: list[CanonicalField]\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef from_native(seed: Borrowed[int], rows: list[list[str]]) -> Result[Record, str]:\n    header = rows[0]\n    mut fields: list[CanonicalField] = []\n    for row in rows[1:]:\n        fields.append(CanonicalField(value=row[0]))\n    record = Record(profile=header[4], owner=header[1], root=header[2], family=read_family(header[3])?, fields=fields)\n    return Ok(record)\ndef from_family(seed: Borrowed[int], rows: list[list[str]], family: Family) -> Record:\n    header = rows[0]\n    mut fields: list[CanonicalField] = []\n    for row in rows[1:]:\n        fields.append(CanonicalField(value=row[0]))\n    return Record(profile=header[4], owner=header[1], root=header[2], family=family, fields=fields)\n";
    let checker = checked(source)?;
    require_checked(&checker, "from_native")?;
    require_checked(&checker, "from_family")
}
