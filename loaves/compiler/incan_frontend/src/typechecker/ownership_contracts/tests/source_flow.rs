//! Source flow over local modes, owned initializers, observers, branches, loops and loans.

use super::*;

/// Immutable module constants are values outside a callable's storage graph. Mutable module static storage and
/// every other unsupported root keep the existing exact refusal instead of acquiring synthetic local permissions.
#[test]
fn module_constants_are_values_but_static_storage_remains_refused() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "const LIMIT: int = 1\ndef cap(limit: int) -> int:\n    return limit\ndef inspect(value: Borrowed[int]) -> int:\n    return value + cap(LIMIT)\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| { owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. }) })
    );

    let refused = checked(
        "static LIMIT: int = 1\ndef cap(limit: int) -> int:\n    return limit\ndef inspect(value: Borrowed[int]) -> int:\n    return value + cap(LIMIT)\n",
    )?;
    let flow = refused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, flow)| flow)
        .ok_or("missing static source flow")?;
    let InputFlow::Unavailable(unavailable) = flow else {
        return Err(format!("unsupported static storage admitted: {flow:?}").into());
    };
    assert_eq!(unavailable.reason, "place is not a prepared parameter or checked local");
    assert!(
        refused
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123"))
    );
    Ok(())
}

/// One explicit-input route may call a checked private helper, read supported fields and extend a checked local list.
/// The compound update must not grant static storage, an alias or an unproved element transfer local permissions.
#[test]
fn explicit_input_uses_private_helper_fields_and_checked_local_compound_assignment()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "const LIMIT: int = 1\nmodel Record:\n    value: int\ndef record_value(record: Borrowed[Record]) -> int:\n    return record.value\ndef above(value: int, limit: int) -> bool:\n    return value > limit\ndef summarize(values: list[int], limit: int) -> int:\n    return values[0] + limit\ndef encode_record(record: Borrowed[Record]) -> int:\n    mut values: list[int] = []\n    values += [record.value]\n    value = record_value(record)\n    if above(record.value, LIMIT):\n        return 0\n    return summarize([value], LIMIT)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
    assert!(
        checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| {
                owner.declaration_name == "encode_record" && matches!(flow, InputFlow::Checked { .. })
            })
    );
    Ok(())
}

/// Ordinary immutable helper slots retain inferred access to exact checked parameter/field places. Explicit owned
/// slots must still refuse a projection rooted in a borrowed input instead of manufacturing a transfer.
#[test]
fn explicit_input_infers_only_checked_parameter_and_field_reads() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "enum Family:\n    Local\nmodel Record:\n    root: str\n    family: Family\ndef nonempty(value: str) -> bool:\n    return value != \"\"\ndef family_name(family: Family) -> str:\n    match family:\n        Family.Local => return \"local\"\ndef inspect(record: Borrowed[Record]) -> bool:\n    if not nonempty(record.root):\n        return false\n    return family_name(record.family) == \"local\"\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));

    let refused = checked(
        "model Record:\n    root: str\ndef consume(value: Owned[str]) -> None:\n    pass\ndef inspect(record: Borrowed[Record]) -> None:\n    consume(record.root)\n",
    )?;
    assert!(
        refused
            .errors
            .iter()
            .any(|error| {
                error.stable_code() == Some("INCAN-T0126")
                    && error.message == "This operation requires access not granted by its ownership contract"
            }),
        "borrowed field acquired an owned transfer: {:?}",
        refused.errors
    );
    Ok(())
}

/// A checker-selected call returning a closed scalar is fresh variant storage. A place projected from a borrowed
/// input remains unavailable as an owned payload without checked transfer materialization.
#[test]
fn explicit_input_variant_payload_accepts_only_fresh_closed_calls() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "enum Value:\n    Text(str)\ndef inspect(values: Borrowed[list[str]]) -> Value:\n    return Value.Text(str(len(values)))\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));

    let refused_source = "enum Value:\n    Item(list[str])\ndef inspect(value: Owned[list[str]]) -> Value:\n    mut held: BorrowedMut[list[str]] = value\n    return Value.Item(held)\n";
    let refused = checked(refused_source)?;
    let state = refused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, state)| state)
        .ok_or("missing borrowed-payload source flow")?;
    let InputFlow::Unavailable(unavailable) = state else {
        return Err(format!("borrowed payload admitted: {state:?}").into());
    };
    assert_eq!(unavailable.reason, source::BORROWED_VALUE_IN_OWNED_FIELD);
    assert_eq!(refused_source.get(unavailable.span.start..unavailable.span.end), Some("held"));
    assert!(refused.errors.iter().any(|error| error.stable_code() == Some("INCAN-T0123")));
    Ok(())
}

/// A builtin list slice with a closed borrow-free result reads its checked base and bounds. A user-selected slice
/// operator continues through the selected-call refusal rather than borrowing builtin list semantics.
#[test]
fn explicit_input_list_slice_requires_builtin_borrow_free_facts() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "def inspect(rows: Borrowed[list[list[str]]]) -> int:\n    return len(rows[1:])\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));

    let refused = checked(
        "class Shelf with Sliceable[int]:\n    pub values: list[int]\n    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[int]:\n        return self.values\ndef inspect(shelf: Borrowed[Shelf]) -> int:\n    return len(shelf[1:])\n",
    )?;
    let state = refused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, state)| state)
        .ok_or("missing selected-slice source flow")?;
    let InputFlow::Unavailable(unavailable) = state else {
        return Err(format!("user-selected slice admitted as builtin: {state:?}").into());
    };
    assert_eq!(unavailable.reason, "operator needs selected callable access facts");
    assert!(refused.errors.iter().any(|error| error.stable_code() == Some("INCAN-T0123")));
    Ok(())
}

/// An owned parameter can transfer a checked field into fresh aggregate storage. The same field under a borrowed
/// parameter must remain refused; the list literal cannot hide a clone or partial ownership change.
#[test]
fn explicit_input_list_elements_require_transferable_field_roots() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "model Record:\n    value: str\ndef collect(record: Owned[Record]) -> list[str]:\n    return [record.value]\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
    }));

    let refused = [
        (
            "borrowed root",
            "model Record:\n    value: str\ndef collect(record: Borrowed[Record]) -> list[str]:\n    return [record.value]\n",
            "list element needs checked retained-owner projection",
        ),
        (
            "borrowed alias",
            "model Record:\n    value: str\ndef collect(record: Owned[Record]) -> list[str]:\n    let alias: Borrowed[Record] = record\n    return [alias.value]\n",
            "list element needs checked retained-owner projection",
        ),
        (
            "indexed element",
            "def collect(values: Owned[list[str]]) -> list[str]:\n    return [values[0]]\n",
            "list element needs checked retained-owner projection",
        ),
        (
            "static root",
            "model Record:\n    value: str\nstatic STORED: Record = Record(value=\"stored\")\ndef collect(seed: Borrowed[int]) -> list[str]:\n    return [STORED.value]\n",
            "place is not a prepared parameter or checked local",
        ),
    ];
    for (label, source, expected) in refused {
        let checker = checked(source)?;
        let state = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} source flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered owned list storage: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}: {:?}", checker.errors);
        assert!(
            !checker.errors.is_empty()
                && checker.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced an unrelated diagnostic: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// A builtin method may receive `?` applied to a checked fresh call result. A stored borrowed value remains refused
/// as a non-Copy append argument and cannot acquire transfer materialization from the surrounding method call.
#[test]
fn explicit_input_builtin_method_accepts_only_fresh_try_call_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "enum Fault:\n    Bad\ndef make() -> Result[bytes, Fault]:\n    return Ok(b\"row\")\ndef collect(seed: Borrowed[int]) -> Result[list[bytes], Fault]:\n    mut rows: list[bytes] = []\n    rows.append(make()?)\n    return Ok(rows)\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
    }));

    for (label, source) in [
        (
            "stored borrowed value",
            "def collect(value: Borrowed[bytes]) -> list[bytes]:\n    mut rows: list[bytes] = []\n    rows.append(value)\n    return rows\n",
        ),
        (
            "returned borrowed view",
            "enum Fault:\n    Bad\ndef view(value: Borrowed[bytes]) -> Result[Borrowed[bytes], Fault] from value:\n    return Ok(value)\ndef collect(value: Borrowed[bytes]) -> Result[list[bytes], Fault]:\n    mut rows: list[bytes] = []\n    rows.append(view(value)?)\n    return Ok(rows)\n",
        ),
    ] {
        let checker = checked(source)?;
        let state = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} append source flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} admitted as fresh append storage: {state:?}").into());
        };
        assert_eq!(
            unavailable.reason,
            "a non-Copy builtin method argument needs checked ownership transfer materialization",
            "{label}: {:?}",
            checker.errors
        );
        assert!(
            !checker.errors.is_empty()
                && checker.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced an unrelated diagnostic: {:?}",
            checker.errors
        );
    }
    Ok(())
}

/// A closed source enum parameter may be observed by an immutable ordinary helper without widening the whole-value
/// route for borrowed strings or other stored parameters.
#[test]
fn explicit_input_whole_parameter_requires_closed_enum_facts() -> Result<(), Box<dyn std::error::Error>> {
    let positive = checked(
        "enum Family:\n    Local\n    Final\ndef family_name(family: Family) -> str:\n    match family:\n        Family.Local => return \"local\"\n        Family.Final => return \"final\"\ndef inspect(rows: Borrowed[list[list[str]]], family: Family) -> str:\n    if len(rows) == 0:\n        return \"empty\"\n    return family_name(family)\n",
    )?;
    let refused = checked(
        "def inspect(value: Borrowed[str]) -> None:\n    user_print(value)\ndef user_print(value: str) -> None:\n    pass\n",
    )?;
    let state = refused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, state)| state)
        .ok_or("missing whole borrowed observer flow")?;
    let InputFlow::Unavailable(unavailable) = state else {
        return Err(format!("whole borrowed observer admitted: {state:?}").into());
    };
    assert_eq!(unavailable.reason, "ordinary argument requires inferred access or mutable binding facts");
    assert!(!refused.errors.is_empty());
    assert!(refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")));
    assert!(positive.errors.is_empty(), "{:?}", positive.errors);
    assert!(
        positive
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. }))
    );
    Ok(())
}

/// A builtin list concatenation creates fresh list storage after reading both exact list operands. A stored list or
/// checker-selected user `__add__` result must not acquire transfer authority from the append boundary.
#[test]
fn explicit_input_builtin_list_concat_is_fresh_without_transfer_authority() -> Result<(), Box<dyn std::error::Error>> {
    let positive = checked(
        "model Field:\n    family: str\n    node: str\n    values: list[str]\ndef collect(field: Borrowed[Field]) -> list[list[str]]:\n    mut rows: list[list[str]] = []\n    rows.append([field.family, field.node] + field.values)\n    return rows\n",
    )?;
    for (label, source) in [
        (
            "stored list field",
            "model Field:\n    values: list[str]\ndef collect(field: Borrowed[Field]) -> list[list[str]]:\n    mut rows: list[list[str]] = []\n    rows.append(field.values)\n    return rows\n",
        ),
        (
            "selected user addition",
            "model Words:\n    values: list[str]\n    def __add__(self, other: Words) -> list[str]:\n        return []\ndef collect(left: Borrowed[Words], right: Borrowed[Words]) -> list[list[str]]:\n    mut rows: list[list[str]] = []\n    rows.append(left + right)\n    return rows\n",
        ),
    ] {
        let checker = checked(source)?;
        let state = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} acquired append transfer authority: {state:?}").into());
        };
        assert_eq!(
            unavailable.reason,
            "a non-Copy builtin method argument needs checked ownership transfer materialization",
            "{label}: {:?}",
            checker.errors
        );
        assert!(!checker.errors.is_empty(), "{label} produced no refusal");
        assert!(checker.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")));
    }
    assert!(positive.errors.is_empty(), "{:?}", positive.errors);
    assert!(
        positive
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. }))
    );
    Ok(())
}

/// `?` may unwrap a selected call only after the call result itself is proven to carry no retained dependency. A
/// returned view in the success payload remains refused at the builtin append boundary.
#[test]
fn explicit_input_try_call_requires_closed_carrier_dependency_facts() -> Result<(), Box<dyn std::error::Error>> {
    let positive = checked(
        "enum Fault:\n    Bad(str)\ndef encode(value: int) -> Result[bytes, Fault]:\n    if value < 0:\n        return Err(Fault.Bad(\"negative\"))\n    return Ok(b\"row\")\ndef collect(seed: Borrowed[int]) -> Result[list[bytes], Fault]:\n    value = 1\n    mut rows: list[bytes] = []\n    rows.append(encode(value)?)\n    return Ok(rows)\n",
    )?;
    let refused = checked(
        "enum Fault:\n    Bad\ndef view(value: Borrowed[bytes]) -> Result[Borrowed[bytes], Fault] from value:\n    return Ok(value)\ndef collect(value: Borrowed[bytes]) -> Result[list[bytes], Fault]:\n    mut rows: list[bytes] = []\n    rows.append(view(value)?)\n    return Ok(rows)\n",
    )?;
    let state = refused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "collect")
        .map(|(_, state)| state)
        .ok_or("missing returned-view carrier flow")?;
    let InputFlow::Unavailable(unavailable) = state else {
        return Err(format!("returned view bypassed dependency facts: {state:?}").into());
    };
    assert_eq!(
        unavailable.reason,
        "a non-Copy builtin method argument needs checked ownership transfer materialization"
    );
    assert!(!refused.errors.is_empty());
    assert!(refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")));
    assert!(positive.errors.is_empty(), "{:?}", positive.errors);
    assert!(
        positive
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. }))
    );
    Ok(())
}

/// Resource-observer fixtures must reach cleanup through represented call boundaries. An ordinary helper
/// retains its selected header but still needs non-Copy argument or nominal returned-dependency facts.
#[test]
fn resource_probe_wrappers_retain_exact_source_boundary_refusals() -> Result<(), Box<dyn std::error::Error>> {
    for (source, call, reason) in [
        (
            "def observe(label: str) -> None:\n    print(label)\ndef consume(value: Owned[str]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = \"initial\"\n    if flag:\n        consume(value)\n    observe(\"join\")\n",
            "\"join\"",
            "ordinary argument requires inferred access or mutable binding facts",
        ),
        (
            "class Holder:\n    pub value: int\ndef make(value: int) -> Holder:\n    return Holder(value=value)\ndef consume(value: Owned[Holder]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = make(1)\n    if flag:\n        consume(value)\n",
            "make(1)",
            "call result needs retained dependency facts",
        ),
        (
            "class Holder:\n    pub value: int\ndef make(value: Owned[int]) -> Holder:\n    return Holder(value=value)\ndef consume(value: Owned[Holder]) -> None:\n    pass\ndef main(flag: bool) -> None:\n    value = make(1)\n    if flag:\n        consume(value)\n",
            "make(1)",
            "call result needs retained dependency facts",
        ),
    ] {
        let checker = checked(source)?;
        let state = checker
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "main")
            .map(|(_, state)| state)
            .ok_or("missing caller source flow")?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("expected exact unavailable boundary: {state:?}").into());
        };
        assert_eq!(unavailable.reason, reason);
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(call));
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123"))
        );
    }
    Ok(())
}

/// An ordinary value parameter can take a fresh model constructor because its checked field binding authenticates
/// both the nominal allocation and its inputs. A constructor containing a non-Copy place remains refused.
#[test]
fn ordinary_calls_accept_only_authenticated_fresh_model_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "model Label:\n    pub name: str\nclass Graph:\n    pub labels: list[Label]\n    pub def add(mut self, label: Label) -> None:\n        self.labels.append(label)\n    pub def read_view(self) -> Borrowed[Graph] from self:\n        return self\ndef main() -> None:\n    mut graph = Graph(labels=[])\n    graph.add(Label(name=\"editor\"))\n    reader = graph.read_view()\n    println(reader.labels[0].name)\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| owner.declaration_name == "main" && matches!(flow, InputFlow::Checked { .. }))
    );

    let refused = "model Label:\n    pub name: str\nclass Graph:\n    pub labels: list[Label]\n    pub def add(mut self, label: Label) -> None:\n        self.labels.append(label)\n    pub def read_view(self) -> Borrowed[Graph] from self:\n        return self\ndef main() -> None:\n    name = \"editor\"\n    mut graph = Graph(labels=[])\n    graph.add(Label(name=name))\n    reader = graph.read_view()\n    println(reader.labels[0].name)\n";
    let checker = checked(refused)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0123")),
        "unsupported constructor admitted: {refused}"
    );
    Ok(())
}

/// Local modes keep their callable owner through settlement, and an ordinary following function does not
/// inherit a previous body's ownership profile, including in the legacy cold-baseline comparison route.
#[test]
fn local_mode_facts_stay_with_their_callable() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def borrowed() -> int:\n    mut value = 1\n    mut view: BorrowedMut[int] = value\n    view = 2\n    return value\ndef ordinary() -> int:\n    return 3\n";
    for legacy in [false, true] {
        let tokens = crate::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.legacy_body_checking = legacy;
        let _ = checker.check_program(&program);
        assert_eq!(checker.type_info.protocols.explicit_local_modes.len(), 1);
        assert_eq!(checker.type_info.protocols.explicit_input_flow.len(), 1);
        assert!(
            checker
                .type_info
                .protocols
                .explicit_input_flow
                .keys()
                .all(|owner| owner.declaration_name == "borrowed")
        );
    }
    Ok(())
}

/// A builtin collection-length read observes a transferred owner just as a primitive read does. Literal
/// construction cannot hide that transfer, and source-defined length hooks retain their separate refusal.
#[test]
fn collection_length_observes_transferred_storage() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def consume(values: Owned[list[int]]) -> int:\n    return len(values)\ndef main() -> None:\n    values = [1, 2, 3]\n    consume(values)\n    assert len(values) == 3\n";
    let checker = checked(source)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126")),
        "{:?}",
        checker.errors
    );
    for source in [
        "def collect(value: Borrowed[str]) -> None:\n    values = [value]\n",
        "def collect(values: Borrowed[list[int]]) -> None:\n    copied = [*values]\n",
    ] {
        let checker = checked(source)?;
        assert!(
            checker
                .errors
                .iter()
                .any(|error| error.stable_code() == Some("INCAN-T0123")),
            "unsupported collection admitted: {source}"
        );
    }
    Ok(())
}

/// Owned local destinations retain their exact initializer event; a later read of the same place cannot stand
/// in for that event, and a second destination cannot acquire the first destination's initializer authority.
#[test]
fn owned_local_initializers_retain_exact_source_events() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def inspect(value: Owned[int]) -> int:\n    let held: Owned[int] = (value)\n    assert value == 1\n    return held\n";
    let graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(graph.owned_locals.len(), 1);
    assert!(
        flow::analyze(&graph)
            .map_err(|error| format!("{error:?}"))?
            .conflicts
            .is_empty()
    );
    let (destination, initializer) = graph
        .owned_locals
        .first_key_value()
        .ok_or("missing owned initializer")?;
    assert_eq!(destination.declaration_name, "held");
    let stored = initializer.source.as_ref().ok_or("missing stored source")?;
    let index = initializer.access_node.ok_or("missing initializer event")?;
    let other = graph.nodes.iter().enumerate().find_map(|(candidate, node)| {
            (candidate != index && matches!(&node.event, flow::Event::Access { place, access: flow::Access::Read, .. } if place == stored)).then_some(candidate)
        }).ok_or("missing later read of the same place")?;
    let mut wrong = graph.clone();
    wrong
        .owned_locals
        .get_mut(destination)
        .ok_or("missing destination")?
        .access_node = Some(other);
    assert!(flow::analyze(&wrong).is_err());
    let mut absent = graph.clone();
    absent
        .owned_locals
        .get_mut(destination)
        .ok_or("missing destination")?
        .access_node = None;
    assert!(flow::analyze(&absent).is_err());
    let mut outside = graph.clone();
    outside
        .owned_locals
        .get_mut(destination)
        .ok_or("missing destination")?
        .access_node = Some(graph.nodes.len());
    assert!(flow::analyze(&outside).is_err());
    let mut missing_owner = graph.clone();
    missing_owner
        .permissions
        .remove(&flow::Root::Binding(destination.clone()));
    assert!(flow::analyze(&missing_owner).is_err());
    Ok(())
}

/// Registry-selected primitive observers retain reads after transfer; user functions and nominal hooks do not
/// acquire the builtin's access contract merely by being named print or len.
#[test]
fn primitive_observers_keep_checked_reads() -> Result<(), Box<dyn std::error::Error>> {
    for source in [
        "def inspect(value: Borrowed[str], flag: bool) -> None:\n    assert flag, value\n    print(value)\n",
        "def inspect(value: Borrowed[str]) -> None:\n    assert false, f\"failed: {value}\"\n    print(value)\n",
        "def inspect(value: Borrowed[str]) -> None:\n    assert true, \"literal\"\n    assert false, value\n",
        "def inspect(value: Borrowed[str], flag: bool) -> None:\n    assert flag, f\"first: {value}\"\n    assert flag, f\"second: {value}\"\n    return\n    print(value)\n",
        "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(value: Borrowed[str], mut other: BorrowedMut[str], flag: bool) -> None:\n    assert flag, f\"{value}{replace(other)}\"\n    print(value)\n",
        "def replace(mut value: BorrowedMut[str]) -> str:\n    value = \"changed\"\n    return \"changed\"\ndef inspect(mut other: BorrowedMut[str], flag: bool) -> None:\n    assert flag, replace(other)\n    print(other)\n",
        "def inspect(flag: bool) -> None:\n    assert flag, consume(\"fresh\")\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n",
        "def inspect(value: Owned[int], flag: bool) -> None:\n    assert flag, consume(value)\n    print(value)\ndef consume(value: Owned[int]) -> str:\n    return \"failed\"\n",
        "def inspect(flag: bool) -> None:\n    value = \"stored\"\n    assert flag, consume(value)\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n",
        "def inspect(flag: bool) -> None:\n    let value: Owned[str] = \"stored\"\n    assert flag, consume(value)\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n",
        "def inspect(value: Owned[str], flag: bool) -> None:\n    assert flag, consume(value)\ndef consume(value: Owned[str]) -> str:\n    return \"failed\"\n",
    ] {
        let checker = checked(source)?;
        assert!(checker.errors.is_empty(), "{source}\n{:?}", checker.errors);
    }
    let source = "def inspect(value: Borrowed[str]) -> None:\n    print(value)\n    assert len(value) > 0, \"empty\"\n";
    let graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
    assert!(
        flow::analyze(&graph)
            .map_err(|error| format!("{error:?}"))?
            .conflicts
            .is_empty()
    );
    let source = "def inspect(value: Owned[str]) -> None:\n    consume(value)\n    assert len(value) > 0\ndef consume(value: Owned[str]) -> None:\n    pass\n";
    let graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
    assert!(
        !flow::analyze(&graph)
            .map_err(|error| format!("{error:?}"))?
            .conflicts
            .is_empty()
    );
    let checker = checked(source)?;
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.stable_code() == Some("INCAN-T0126"))
    );
    for source in [
        "def inspect(value: Borrowed[str]) -> None:\n    user_print(value)\ndef user_print(value: str) -> None:\n    pass\n",
        "def inspect(value: Borrowed[Measure]) -> int:\n    return len(value)\nclass Measure:\n    def __len__(self) -> int:\n        return 1\n",
    ] {
        assert!(
            source_graph(source)?.is_err(),
            "unprojected observer admitted: {source}"
        );
    }
    let shadowed = checked(
        "def inspect(value: Borrowed[str]) -> None:\n    print(value)\ndef print(value: str) -> None:\n    pass\n",
    )?;
    assert!(
        shadowed
            .errors
            .iter()
            .any(|error| error.stable_code() != Some("INCAN-T0123")),
        "protected builtin shadowing must remain rejected"
    );
    Ok(())
}

/// Source branch and back-edge projections preserve consuming uses and distinguish Copy reads from transfers.
#[test]
fn source_flow_tracks_inputs_through_branches_and_loops() -> Result<(), Box<dyn std::error::Error>> {
    for (source, conflict) in [
        ("def add(value: Borrowed[int]) -> int:\n    return value + 1\n", false),
        ("def transfer(value: Owned[str]) -> str:\n    return value\n", false),
        ("def copy(value: Borrowed[str]) -> str:\n    return value\n", true),
        (
            "def branch(value: Owned[str], flag: bool) -> str:\n    if flag:\n        taken = value\n    return value\n",
            true,
        ),
        (
            "def repeat(value: Owned[str]) -> None:\n    loop:\n        taken = value\n        continue\n",
            true,
        ),
        (
            "def restore(mut value: Owned[str]) -> str:\n    taken = value\n    value = \"new\"\n    return value\n",
            false,
        ),
        (
            "def local(mut value: Owned[int]) -> int:\n    mut view: BorrowedMut[int] = value\n    view = view + 1\n    return value\n",
            false,
        ),
        (
            "def local_only() -> int:\n    mut value = 1\n    mut view: BorrowedMut[int] = value\n    view = 2\n    return value\n",
            false,
        ),
        (
            "def conflict(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    value = 2\n    return view\n",
            true,
        ),
        (
            "def readonly(mut value: Owned[int]) -> int:\n    mut view: Borrowed[int] = value\n    view = 2\n    return value\n",
            true,
        ),
        (
            "def reborrow(mut value: Owned[int]) -> int:\n    mut parent: BorrowedMut[int] = value\n    mut child: BorrowedMut[int] = parent\n    parent = 2\n    return child\n",
            true,
        ),
        (
            "def branch_release(mut value: Owned[int], flag: bool) -> int:\n    let view: Borrowed[int] = value\n    if flag:\n        return view\n    value = 2\n    return value\n",
            false,
        ),
        (
            "def shadow(mut value: Owned[int]) -> int:\n    let view: Borrowed[int] = value\n    Borrowed = 2\n    value = 3\n    return view\n",
            true,
        ),
        (
            "def nested(mut value: BorrowedMut[Customer]) -> int:\n    mut view: BorrowedMut[int] = value.address.count\n    view = 2\n    return value.address.count\nmodel Address:\n    count: int\nmodel Customer:\n    address: Address\n",
            false,
        ),
        (
            "def parent(value: Owned[Customer]) -> int:\n    let view: Borrowed[int] = value.address.count\n    let taken: Owned[Address] = value.address\n    return view\nmodel Address:\n    count: int\nmodel Customer:\n    address: Address\n",
            true,
        ),
        (
            "def sibling(mut value: BorrowedMut[Pair]) -> int:\n    let view: Borrowed[int] = value.left\n    value.right = 2\n    return view\nmodel Pair:\n    left: int\n    right: int\n",
            false,
        ),
        (
            "def overlapping(mut value: BorrowedMut[Pair]) -> int:\n    let view: Borrowed[int] = value.left\n    value.left = 2\n    return view\nmodel Pair:\n    left: int\n    right: int\n",
            true,
        ),
        (
            "def compound(mut value: BorrowedMut[int]) -> int:\n    value += 1\n    return value\n",
            false,
        ),
        (
            "def branch(flag: bool) -> None:\n    value = \"owned\"\n    if flag:\n        consume(value)\n    print(\"after join\")\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            false,
        ),
        (
            "def branch(flag: bool) -> None:\n    let value: Owned[str] = \"owned\"\n    if flag:\n        consume(value)\n    print(\"after join\")\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            false,
        ),
        (
            "def branch(value: Owned[str], flag: bool) -> None:\n    if flag:\n        consume(value)\n    else:\n        return\n    print(\"after join\")\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            false,
        ),
        (
            "def branch(flag: bool) -> None:\n    mut value = \"owned\"\n    if flag:\n        consume(value)\n    value = \"new\"\n    print(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            false,
        ),
        (
            "def branch(flag: bool) -> None:\n    value = \"owned\"\n    if flag:\n        consume(value)\n    print(value)\ndef consume(value: Owned[str]) -> None:\n    pass\n",
            true,
        ),
        // A loop shares its stored iterable for every poll: items are read freely, the iterable is not edited
        // until the loop ends.
        (
            "def total(values: Borrowed[list[int]]) -> int:\n    mut sum = 0\n    for value in values:\n        sum += value\n    return sum\n",
            false,
        ),
        (
            "def reset(mut values: BorrowedMut[list[int]]) -> None:\n    for value in values:\n        values = [value]\n",
            true,
        ),
        (
            "def after(mut values: BorrowedMut[list[int]]) -> None:\n    mut sum = 0\n    for value in values:\n        sum += value\n    values = [sum]\n",
            false,
        ),
    ] {
        let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
        let analysis = flow::analyze(&graph).map_err(|error| format!("{source}\n{error:?}"))?;
        assert_eq!(
            !analysis.conflicts.is_empty(),
            conflict,
            "{source}\n{:?}",
            analysis.conflicts
        );
    }
    let dictionary = source_graph(
        "def present(streams: Borrowed[dict[str, Generator[int]]], key: str) -> bool:\n    match streams.get(key):\n        Some(_) => return true\n        None => return false\n",
    )?
    .map_err(|error| format!("{error:?}"))?;
    let dictionary_analysis = flow::analyze(&dictionary).map_err(|error| format!("{error:?}"))?;
    assert_eq!(dictionary.dictionary_lookups.len(), 1);
    assert_eq!(
        super::super::dictionary::read_only_lookups(&dictionary, &dictionary_analysis).len(),
        1
    );
    Ok(())
}

/// Calls, aggregate results and temporary owners remain unavailable until their exact checked access
/// and dependency facts exist; a partial graph cannot count as validation of the whole callable.
#[test]
fn source_flow_refuses_unprojected_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    let builtin = source_graph("def show(value: Borrowed[int]) -> int:\n    return abs(value)\n")?;
    assert!(
        builtin.is_ok(),
        "a selected core builtin must retain its checked operands"
    );
    for source in [
        "def local(value: Borrowed[str]) -> None:\n    let view: Reference[str] = value\n",
        "def temporary() -> None:\n    let view: Borrowed[str] = \"temporary\"\n",
    ] {
        assert!(
            source_graph(source)?.is_err(),
            "unsupported dependency admitted: {source}"
        );
    }
    Ok(())
}

/// Transfers follow actual Copy evidence, including derives and composite carriers. An exclusive reference
/// never becomes Copy merely because the legacy caller-adaptation helper groups all reference types together.
#[test]
fn explicit_transfer_uses_copy_evidence() -> Result<(), Box<dyn std::error::Error>> {
    for (source, conflict) in [
        (
            "def exclusive(value: Owned[&mut int]) -> &mut int:\n    first = value\n    return value\n",
            true,
        ),
        (
            "def shared(value: Owned[&int]) -> &int:\n    first = value\n    return value\n",
            false,
        ),
        (
            "def derived(value: Owned[Point]) -> Point:\n    first = value\n    return value\n@derive(Copy)\nmodel Point:\n    x: int\n",
            false,
        ),
        (
            "def optional(value: Owned[Option[int]]) -> Option[int]:\n    first = value\n    return value\n",
            false,
        ),
    ] {
        let graph = source_graph(source)?.map_err(|error| format!("{source}\n{error:?}"))?;
        let analysis = flow::analyze(&graph).map_err(|error| format!("{source}\n{error:?}"))?;
        assert_eq!(!analysis.conflicts.is_empty(), conflict, "{source}\n{analysis:?}");
    }
    Ok(())
}

/// Canonical destinations retain the exact reborrow chain; lowering must not rediscover aliases by name.
#[test]
fn local_loan_destinations_keep_checked_sources() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def read(mut value: Owned[int]) -> int:\n    mut parent: BorrowedMut[int] = value\n    let child: Borrowed[int] = parent\n    return child\n";
    let mut graph = source_graph(source)?.map_err(|error| format!("{error:?}"))?;
    let parent = graph
        .local_loans
        .iter()
        .find(|(local, _)| local.declaration_name == "parent")
        .map(|(_, loan)| *loan)
        .ok_or("missing parent destination")?;
    let (child, child_loan) = graph
        .local_loans
        .iter()
        .find(|(local, _)| local.declaration_name == "child")
        .map(|(local, loan)| (local.clone(), *loan))
        .ok_or("missing child destination")?;
    assert_eq!(graph.local_loans.len(), 2);
    assert_eq!(graph.loans[child_loan.0].parent, Some(parent));
    assert!(
        flow::analyze(&graph)
            .map_err(|error| format!("{error:?}"))?
            .conflicts
            .is_empty()
    );
    let mut dual = graph.clone();
    dual.permissions.insert(
        flow::Root::Binding(child.clone()),
        flow::Permission {
            exclusive: false,
            writable: false,
            transferable: true,
        },
    );
    assert!(flow::analyze(&dual).is_err());
    let mut duplicate = graph.clone();
    let mut other = child.clone();
    other.declaration_name = "other".into();
    duplicate.local_loans.insert(other, child_loan);
    assert!(flow::analyze(&duplicate).is_err());
    graph.local_loans.insert(child.clone(), flow::LoanId(graph.loans.len()));
    assert!(flow::analyze(&graph).is_err());
    graph.local_loans.insert(child.clone(), child_loan);
    let mut invalid = child;
    invalid.kind = incan_semantics_core::SemanticSourceTargetKind::Parameter;
    graph.local_loans.insert(invalid, child_loan);
    assert!(flow::analyze(&graph).is_err());
    Ok(())
}

/// Generic trait defaults retain an exact conditional result obligation; removal or type replacement cannot
/// turn the source proof into an unconditional permission to release caller-owned loans.
#[test]
fn generic_trait_result_requires_closed_dependency_proof() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"trait Visit:
    def visit[E, W with Sink[E]](self, mut sink: BorrowedMut[W]) -> Result[None, E]:
        pass

trait Sink[E]:
    def feed[V with Visit](mut self, value: Borrowed[V]) -> Result[None, E]:
        return value.visit(self)

class Value with Visit:
    pub count: int
    def visit[E, W with Sink[E]](self, mut sink: W) -> Result[None, E]:
        return Ok(None)

class Reader with Sink[str]:
    pub count: int

def main() -> Result[None, str]:
    value = Value(count=7)
    mut reader = Reader(count=0)
    reader.feed(value)?
    assert value.count == 7
    return Ok(None)
"#;
    let checker = checked(source)?;
    assert!(
        checker.errors.is_empty(),
        "errors={:?}; calls={:?}",
        checker.errors,
        checker
            .type_info
            .protocols
            .explicit_input_calls
            .iter()
            .filter(|(_, call)| call
                .owner
                .as_ref()
                .is_some_and(|owner| owner.declaration_name == "feed"))
            .map(|(span, call)| (
                call,
                checker.type_info.expr_type(*span),
                checker.type_info.selected_input_contract(call)
            ))
            .collect::<Vec<_>>()
    );
    let (owner, state) = checker
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "feed")
        .ok_or("missing feed flow")?;
    let InputFlow::Checked { graph, .. } = state else {
        return Err(format!("{state:?}").into());
    };
    let (span, invocation) = graph
        .invocations
        .iter()
        .find(|(_, call)| call.result_requirement.is_some())
        .ok_or("generic call lost its conditional obligation")?;
    assert_eq!(
        invocation.result_requirement.as_ref(),
        checker.type_info.expr_type(*span)
    );
    assert!(checker.type_info.checked_input_body(owner)?.is_some());
    for replacement in [None, Some(ResolvedType::Str)] {
        let mut corrupted = checker.type_info.clone();
        let Some(InputFlow::Checked { graph, .. }) = corrupted.protocols.explicit_input_flow.get_mut(owner) else {
            return Err("missing copied graph".into());
        };
        graph
            .invocations
            .get_mut(span)
            .ok_or("missing copied invocation")?
            .result_requirement = replacement;
        assert!(corrupted.checked_input_body(owner).is_err());
    }
    Ok(())
}

/// An immutable ordinary slot may receive one selected call's fresh closed result. A returned view remains stored
/// caller-owned state and must not borrow this route merely because its presented value type matches the slot.
#[test]
fn ordinary_argument_accepts_only_fresh_closed_selected_call_results()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "def encode(value: Borrowed[int]) -> bytes:\n    return b\"row\"\ndef observe(value: bytes) -> int:\n    return len(value)\ndef inspect(value: Borrowed[int]) -> int:\n    return observe(encode(value))\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));

    let refused_source = "def view(value: Borrowed[bytes]) -> Borrowed[bytes] from value:\n    return value\ndef observe(value: bytes) -> int:\n    return len(value)\ndef inspect(value: Borrowed[bytes]) -> int:\n    return observe(view(value))\n";
    let refused = checked(refused_source)?;
    let state = refused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, state)| state)
        .ok_or("missing returned-view argument flow")?;
    let InputFlow::Unavailable(unavailable) = state else {
        return Err(format!("returned view entered ordinary closed-result route: {state:?}").into());
    };
    assert_eq!(unavailable.reason, "ordinary argument requires inferred access or mutable binding facts");
    assert_eq!(refused_source.get(unavailable.span.start..unavailable.span.end), Some("view(value)"));
    assert!(
        !refused.errors.is_empty()
            && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
        "unrelated returned-view diagnostics: {:?}",
        refused.errors
    );
    Ok(())
}

/// A selected ordinary call returning a closed model is fresh list-element storage. A previously stored model keeps
/// its root and still needs retained-owner projection instead of acquiring transfer authority from the list literal.
#[test]
fn list_element_accepts_only_fresh_closed_selected_model_results()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "model Field:\n    value: bytes\ndef make(seed: int) -> Field:\n    return Field(value=b\"row\")\ndef collect(seed: Borrowed[int]) -> list[Field]:\n    return [make(seed)]\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
    }));

    for (label, refused_source, reason, stop) in [
        (
            "stored model",
            "model Field:\n    value: bytes\ndef collect(seed: Borrowed[int]) -> list[Field]:\n    field = Field(value=b\"row\")\n    return [field]\n",
            "list element needs checked retained-owner projection",
            "field",
        ),
        (
            "dependent call result",
            "def view(value: Borrowed[bytes]) -> Borrowed[bytes] from value:\n    return value\ndef collect(value: Borrowed[bytes]) -> list[bytes]:\n    return [view(value)]\n",
            "a returned view needs a local binding, a receiver or a returned view position",
            "view(value)",
        ),
    ] {
        let refused = checked(refused_source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} list flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered fresh list route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, reason, "{label}");
        assert_eq!(refused_source.get(unavailable.span.start..unavailable.span.end), Some(stop), "{label}");
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }
    Ok(())
}

/// A constructor field may materialize one indexed value from a fresh builtin-slice loop row. An ordinary stored
/// list and a borrowed root remain outside that exact producer route, including when their element types match.
#[test]
fn constructor_field_index_requires_fresh_transferable_loop_row()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "model Field:\n    value: str\ndef collect(rows: Borrowed[list[list[str]]]) -> list[Field]:\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(value=row[0]))\n    return fields\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
    }));

    for (label, source, expected, stop) in [
        (
            "stored local row",
            "model Field:\n    value: str\ndef collect(seed: Borrowed[int]) -> Field:\n    row = [\"stored\"]\n    return Field(value=row[0])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "row[0]",
        ),
        (
            "borrowed row",
            "model Field:\n    value: str\ndef collect(row: Borrowed[list[str]]) -> Field:\n    return Field(value=row[0])\n",
            source::BORROWED_VALUE_IN_OWNED_FIELD,
            "row[0]",
        ),
        (
            "selected user index",
            "class Row:\n    pub value: str\n    def __getitem__(self, index: int) -> str:\n        return self.value\nmodel Field:\n    value: str\ndef collect(row: Owned[Row]) -> Field:\n    return Field(value=row[0])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "row[0]",
        ),
        (
            "unproven Clone element",
            "model Holder[T]:\n    value: T\ndef collect[T](tokens: Owned[list[T]]) -> Holder[T]:\n    return Holder(value=tokens[0])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "tokens[0]",
        ),
        (
            "non-constructor list element",
            "def collect(rows: Owned[list[str]]) -> list[str]:\n    return [rows[0]]\n",
            "list element needs checked retained-owner projection",
            "rows[0]",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} constructor flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered fresh loop-row route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}");
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(stop), "{label}");
        if label == "selected user index" {
            assert!(
                refused
                    .type_info
                    .calls
                    .resolved_operator_calls
                    .contains_key(&unavailable.span),
                "user index fixture did not retain its selected operator"
            );
        }
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }
    Ok(())
}

/// Every canonical loop-pattern local is reinitialized at the poll, even when its type is cloneable and therefore
/// not an item-taking candidate. Reinitialization must not hide a same-iteration or outer-root use after transfer.
#[test]
fn cloneable_loop_pattern_reinitializes_without_suppressing_real_moves()
-> Result<(), Box<dyn std::error::Error>> {
    let declarations = "model Field:\n    value: str\ndef consume(value: Owned[str]) -> None:\n    pass\n";
    let accepted_source = format!(
        "{declarations}def inspect(fields: Borrowed[list[Field]]) -> str:\n    mut previous = \"\"\n    for field in fields:\n        if field.value == \"stop\":\n            consume(field.value)\n            return previous\n        previous = field.value\n    return previous\n"
    );
    let accepted = checked(&accepted_source)?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));

    for (label, body) in [
        (
            "same iteration",
            "def inspect(fields: Borrowed[list[Field]]) -> str:\n    for field in fields:\n        consume(field.value)\n        if field.value == \"again\":\n            return field.value\n    return \"\"\n",
        ),
        (
            "outer root",
            "def inspect(fields: Borrowed[list[Field]]) -> str:\n    outer = Field(value=\"outer\")\n    for field in fields:\n        consume(outer.value)\n    return outer.value\n",
        ),
    ] {
        let source = format!("{declarations}{body}");
        let refused = checked(&source)?;
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0126")),
            "{label} move produced unrelated diagnostics: {:?}",
            refused.errors
        );
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "inspect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} move flow"))?;
        let InputFlow::Checked { analysis, .. } = state else {
            return Err(format!("{label} move lost its checked conflict graph: {state:?}").into());
        };
        assert!(!analysis.conflicts.is_empty(), "{label} move lost its exact T0126 conflict");
    }
    Ok(())
}

/// Parentheses around a nested `and` inside `or` preserve both short-circuit exits before a closed Borrowed
/// validation call runs through `?` for each fresh loop-pattern owner. A real same-iteration transfer and a
/// returned-view carrier keep their existing outcomes instead of borrowing the loop's per-poll replacement.
#[test]
fn parenthesized_short_circuit_loop_validation_preserves_exact_edges()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Fault:\n    Bad\nmodel Field:\n    family: str\n    node: str\n    selector: str\ndef validate(field: Borrowed[Field]) -> Result[None, Fault]:\n    if field.family == \"bad\":\n        return Err(Fault.Bad)\n    return Ok(None)\ndef inspect(fields: Borrowed[list[Field]]) -> Result[str, Fault]:\n    mut previous_family = \"\"\n    mut previous_node = \"\"\n    mut previous_selector = \"\"\n    for field in fields:\n        if field.family < previous_family or (field.family == previous_family and field.node < previous_node):\n            return Err(Fault.Bad)\n        validate(field)?\n        previous_family = field.family\n        previous_node = field.node\n        previous_selector = field.selector\n    return Ok(previous_selector)\n";
    let accepted = checked(accepted_source)?;

    let moved_source = "enum Fault:\n    Bad\nmodel Field:\n    value: str\ndef consume(value: Owned[str]) -> Result[None, Fault]:\n    return Ok(None)\ndef inspect(fields: Borrowed[list[Field]]) -> Result[str, Fault]:\n    mut previous = \"\"\n    for field in fields:\n        consume(field.value)?\n        previous = field.value\n    return Ok(previous)\n";
    let moved = checked(moved_source)?;
    assert!(
        !moved.errors.is_empty() && moved.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0126")),
        "same-iteration move produced unrelated diagnostics: {:?}",
        moved.errors
    );
    let moved_state = moved
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, state)| state)
        .ok_or("missing same-iteration validation flow")?;
    let InputFlow::Checked { analysis, .. } = moved_state else {
        return Err(format!("same-iteration move lost its checked graph: {moved_state:?}").into());
    };
    assert!(!analysis.conflicts.is_empty(), "same-iteration move lost its exact T0126 conflict");

    let dependent_source = "enum Fault:\n    Bad\nmodel Field:\n    value: str\ndef view(value: Borrowed[str]) -> Result[Borrowed[str], Fault] from value:\n    return Ok(value)\ndef inspect(fields: Borrowed[list[Field]]) -> Result[list[str], Fault]:\n    mut values: list[str] = []\n    for field in fields:\n        values.append(view(field.value)?)\n    return Ok(values)\n";
    let dependent = checked(dependent_source)?;
    let dependent_state = dependent
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "inspect")
        .map(|(_, state)| state)
        .ok_or("missing returned-view validation flow")?;
    let InputFlow::Unavailable(unavailable) = dependent_state else {
        return Err(format!("returned-view carrier bypassed validation dependencies: {dependent_state:?}").into());
    };
    assert_eq!(
        unavailable.reason,
        "a non-Copy builtin method argument needs checked ownership transfer materialization"
    );
    assert_eq!(
        dependent_source.get(unavailable.span.start..unavailable.span.end),
        Some("view(field.value)?")
    );
    assert!(
        !dependent.errors.is_empty()
            && dependent.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
        "returned-view carrier produced unrelated diagnostics: {:?}",
        dependent.errors
    );
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. })
    }));
    Ok(())
}

/// A builtin borrow-free slice of one fresh loop row may materialize an exact constructor field. This is a
/// constructor-only fact: stored and borrowed roots, user-selected slice operators, unproven element cloning,
/// closed cloneable non-scalar elements and the identical slice nested as a list element retain their refusals.
#[test]
fn constructor_field_slice_requires_fresh_transferable_loop_row()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "model Field:\n    family: str\n    values: list[str]\ndef from_native(rows: Borrowed[list[list[str]]]) -> list[Field]:\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(family=row[0], values=row[3:]))\n    return fields\ndef from_family(rows: Borrowed[list[list[str]]]) -> list[Field]:\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(family=row[0], values=row[3:]))\n    return fields\n";
    let accepted = checked(accepted_source)?;

    for (label, source, expected, stop) in [
        (
            "stored row",
            "model Field:\n    values: list[str]\ndef collect(seed: Borrowed[int]) -> Field:\n    row = [\"a\", \"b\", \"c\", \"d\"]\n    return Field(values=row[3:])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "row[3:]",
        ),
        (
            "borrowed row",
            "model Field:\n    values: list[str]\ndef collect(row: Borrowed[list[str]]) -> Field:\n    return Field(values=row[3:])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "row[3:]",
        ),
        (
            "selected user slice",
            "class Shelf:\n    pub values: list[int]\n    def __getslice__(self, start: Option[int], end: Option[int], step: Option[int]) -> list[int]:\n        return self.values\nmodel Field:\n    values: list[int]\ndef collect(shelf: Owned[Shelf]) -> Field:\n    return Field(values=shelf[3:])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "shelf[3:]",
        ),
        (
            "unproven Clone element",
            "model Field[T]:\n    values: list[T]\ndef collect[T](rows: Owned[list[list[T]]]) -> list[Field[T]]:\n    mut fields: list[Field[T]] = []\n    for row in rows:\n        fields.append(Field(values=row[1:]))\n    return fields\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "row[1:]",
        ),
        (
            "closed cloneable model element",
            "@derive(Clone)\nmodel Token:\n    value: str\nmodel Field:\n    values: list[Token]\ndef collect(rows: Borrowed[list[list[Token]]]) -> list[Field]:\n    mut fields: list[Field] = []\n    for row in rows:\n        fields.append(Field(values=row[1:]))\n    return fields\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "row[1:]",
        ),
        (
            "non-constructor list element",
            "def collect(rows: Owned[list[list[str]]]) -> list[list[str]]:\n    for row in rows:\n        return [row[3:]]\n    return []\n",
            "list element needs checked retained-owner projection",
            "row[3:]",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} constructor-slice flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered constructor-slice materialization: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}: {:?}", refused.errors);
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(stop), "{label}");
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    for name in ["from_native", "from_family"] {
        assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
            owner.declaration_name == name && matches!(flow, InputFlow::Checked { .. })
        }));
    }
    Ok(())
}

/// An immutable pre-loop local may retain the exact owned snapshot moved from one builtin parameter index. A later
/// constructor may clone a scalar element from that authenticated snapshot, without granting arbitrary stored,
/// mutable, borrowed, overloaded, non-cloneable or returned-view locals the same authority.
#[test]
fn constructor_field_index_requires_immutable_parameter_snapshot()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "model Field:\n    owner: str\n    root: str\n    values: list[str]\ndef from_native(seed: Borrowed[int], rows: list[list[str]]) -> list[Field]:\n    header = rows[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(owner=header[1], root=header[2], values=row[3:]))\n    return fields\ndef from_family(seed: Borrowed[int], rows: list[list[str]]) -> list[Field]:\n    header = rows[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(owner=header[1], root=header[2], values=row[3:]))\n    return fields\n";
    let accepted = checked(accepted_source)?;

    for (label, source, expected, stop) in [
        (
            "literal stored local",
            "model Field:\n    owner: str\ndef collect(seed: Borrowed[int]) -> Field:\n    header = [\"header\", \"owner\"]\n    return Field(owner=header[1])\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "header[1]",
        ),
        (
            "mutable reassigned snapshot",
            "model Field:\n    owner: str\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> list[Field]:\n    mut header = rows[0]\n    header = rows[1]\n    mut fields: list[Field] = []\n    for row in rows[2:]:\n        fields.append(Field(owner=header[1]))\n    return fields\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "header[1]",
        ),
        (
            "borrowed header",
            "model Field:\n    owner: str\ndef collect(header: Borrowed[list[str]], rows: list[list[str]]) -> list[Field]:\n    mut fields: list[Field] = []\n    for row in rows:\n        fields.append(Field(owner=header[1]))\n    return fields\n",
            source::BORROWED_VALUE_IN_OWNED_FIELD,
            "header[1]",
        ),
        (
            "selected user index",
            "class Header:\n    pub owner: str\n    def __getitem__(self, index: int) -> str:\n        return self.owner\nmodel Field:\n    owner: str\ndef collect(seed: Borrowed[int], rows: list[Header]) -> list[Field]:\n    header = rows[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(owner=header[1]))\n    return fields\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "header[1]",
        ),
        (
            "non-cloneable element",
            "model Token:\n    value: str\nmodel Field:\n    token: Token\ndef collect(seed: Borrowed[int], rows: list[list[Token]]) -> list[Field]:\n    header = rows[0]\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(token=header[1]))\n    return fields\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "header[1]",
        ),
        (
            "returned-view initializer",
            "model Field:\n    owner: str\ndef view(value: Borrowed[list[str]]) -> Borrowed[list[str]] from value:\n    return value\ndef collect(source: Borrowed[list[str]], rows: list[list[str]]) -> list[Field]:\n    header = view(source)\n    mut fields: list[Field] = []\n    for row in rows:\n        fields.append(Field(owner=header[1]))\n    return fields\n",
            source::BORROWED_VALUE_IN_OWNED_FIELD,
            "header[1]",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} snapshot flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered immutable snapshot route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}: {:?}", refused.errors);
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(stop), "{label}");
        if label == "selected user index" {
            assert!(
                refused.type_info.calls.resolved_operator_calls.contains_key(&unavailable.span),
                "user index fixture did not retain its selected operator"
            );
        }
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }

    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    for name in ["from_native", "from_family"] {
        assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
            owner.declaration_name == name && matches!(flow, InputFlow::Checked { .. })
        }));
    }
    Ok(())
}

/// A scalar index of one authenticated immutable parameter snapshot may feed only an immutable ordinary helper slot.
/// An explicit Owned source parameter and a mutable ordinary slot retain the existing argument refusal.
#[test]
fn snapshot_index_argument_requires_immutable_ordinary_read()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n";
    let accepted = checked(accepted_source)?;

    for (label, source) in [
        (
            "explicit Owned source",
            "enum Family:\n    Local\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: Owned[list[list[str]]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
        ),
        (
            "mutable ordinary slot",
            "enum Family:\n    Local\ndef read_family(mut value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> Result[Family, str]:\n    header = rows[0]\n    family = read_family(header[3])?\n    return Ok(family)\n",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} snapshot-argument flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered snapshot ordinary-argument route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, "ordinary argument requires inferred access or mutable binding facts");
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some("header[3]"));
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }

    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
    }), "{}", format!("{:?}", flow_summary(&accepted)));
    Ok(())
}

/// Full header construction keeps the direct Try(Call) and closed Enum Parameter constructor routes visible to their
/// own focused contracts instead of letting either hide the snapshot-argument boundary above.
#[test]
fn full_header_constructor_shape_retains_snapshot_and_family_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "enum Family:\n    Local\nmodel CanonicalField:\n    value: str\nmodel Record:\n    profile: str\n    owner: str\n    root: str\n    family: Family\n    fields: list[CanonicalField]\ndef read_family(value: str) -> Result[Family, str]:\n    return Ok(Family.Local)\ndef from_native(seed: Borrowed[int], rows: list[list[str]]) -> Result[Record, str]:\n    header = rows[0]\n    mut fields: list[CanonicalField] = []\n    for row in rows[1:]:\n        fields.append(CanonicalField(value=row[0]))\n    record = Record(profile=header[4], owner=header[1], root=header[2], family=read_family(header[3])?, fields=fields)\n    return Ok(record)\ndef from_family(seed: Borrowed[int], rows: list[list[str]], family: Family) -> Record:\n    header = rows[0]\n    mut fields: list[CanonicalField] = []\n    for row in rows[1:]:\n        fields.append(CanonicalField(value=row[0]))\n    return Record(profile=header[4], owner=header[1], root=header[2], family=family, fields=fields)\n";
    let checker = checked(source)?;
    assert!(checker.errors.is_empty(), "{:?}", checker.errors);
    for name in ["from_native", "from_family"] {
        assert!(checker.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
            owner.declaration_name == name && matches!(flow, InputFlow::Checked { .. })
        }), "{name}: {}", format!("{:?}", flow_summary(&checker)));
    }
    Ok(())
}

/// One exact immutable whole Enum Parameter may transfer into an equal constructor field. Borrowed values, stored
/// locals and model parameters remain refused, while a second use after the admitted transfer remains a real T0126.
#[test]
fn constructor_field_enum_parameter_requires_exact_transfer()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    return Field(family=family)\n";
    let accepted = checked(accepted_source)?;

    for (label, source, expected) in [
        (
            "Borrowed enum",
            "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(family: Borrowed[Family]) -> Field:\n    return Field(family=family)\n",
            source::BORROWED_VALUE_IN_OWNED_FIELD,
        ),
        (
            "stored enum local",
            "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int]) -> Field:\n    family = Family.Local\n    return Field(family=family)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "model parameter",
            "model Family:\n    value: str\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    return Field(family=family)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "explicit Owned enum parameter",
            "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Owned[Family]) -> Field:\n    return Field(family=family)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} constructor flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered Enum Parameter transfer route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}: {:?}", refused.errors);
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some("family"), "{label}");
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }

    let reused = checked(
        "enum Family:\n    Local\nmodel Field:\n    family: Family\ndef collect(seed: Borrowed[int], family: Family) -> Field:\n    first = Field(family=family)\n    second = Field(family=family)\n    return second\n",
    )?;
    assert!(
        !reused.errors.is_empty()
            && reused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0126")),
        "reused Enum Parameter produced unrelated diagnostics: {:?}",
        reused.errors
    );
    let state = reused
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "collect")
        .map(|(_, state)| state)
        .ok_or("missing reused Enum Parameter flow")?;
    let InputFlow::Checked { analysis, .. } = state else {
        return Err(format!("reused Enum Parameter lost its checked conflict graph: {state:?}").into());
    };
    assert!(!analysis.conflicts.is_empty(), "reused Enum Parameter lost its exact T0126 conflict");

    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
    }));
    Ok(())
}

/// An exact whole builtin-list Local may transfer once into an equal constructor field. Borrowed, dependent,
/// non-list and projected storage remain refused; a read or second move after transfer remains a checked T0126.
#[test]
fn constructor_field_whole_local_list_requires_exact_transfer()
-> Result<(), Box<dyn std::error::Error>> {
    let mutable_positive = checked(
        "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    mut fields: list[Field] = []\n    for value in [\"a\", \"b\"]:\n        fields.append(Field(value=\"field\"))\n    return Record(fields=fields)\n",
    )?;
    let immutable_positive = checked(
        "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    fields = [Field(value=\"a\")]\n    return Record(fields=fields)\n",
    )?;

    for (label, source, expected) in [
        (
            "returned-view list",
            "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef view(values: Borrowed[list[Field]]) -> Borrowed[list[Field]] from values:\n    return values\ndef collect(source: Borrowed[list[Field]]) -> Record:\n    fields = view(source)\n    return Record(fields=fields)\n",
            source::BORROWED_VALUE_IN_OWNED_FIELD,
        ),
        (
            "dependent owned list",
            "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef copy_fields(values: Borrowed[list[Field]]) -> Owned[list[Field]] from values:\n    return values.clone()\ndef collect(source: Borrowed[list[Field]]) -> Record:\n    fields = copy_fields(source)\n    return Record(fields=fields)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "stored model local",
            "model Family:\n    value: str\nmodel Record:\n    family: Family\ndef collect(seed: Borrowed[int]) -> Record:\n    family = Family(value=\"local\")\n    return Record(family=family)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "stored enum local",
            "enum Family:\n    Local\nmodel Record:\n    family: Family\ndef collect(seed: Borrowed[int]) -> Record:\n    family = Family.Local\n    return Record(family=family)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "explicit Owned list local",
            "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    fields: Owned[list[Field]] = [Field(value=\"a\")]\n    return Record(fields=fields)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "snapshot list local",
            "model Record:\n    fields: list[str]\ndef collect(seed: Borrowed[int], rows: list[list[str]]) -> Record:\n    fields = rows[0]\n    return Record(fields=fields)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
        (
            "projected list",
            "@derive(Clone)\nmodel Field:\n    value: str\nmodel Store:\n    fields: list[Field]\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    store = Store(fields=[Field(value=\"a\")])\n    return Record(fields=store.fields)\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} constructor flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered whole-local list transfer route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}: {:?}", refused.errors);
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
        if label == "dependent owned list" {
            let call = refused
                .type_info
                .protocols
                .explicit_input_calls
                .values()
                .find(|call| call.callee.declaration_name == "copy_fields")
                .ok_or("dependent list control lost its selected call")?;
            let contract = refused
                .type_info
                .selected_input_contract(call)?
                .ok_or("dependent list control lost its selected contract")?;
            assert!(crate::ownership_metadata::owned_result_source(Some(contract))?.is_some());
        }
    }

    for (label, source) in [
        (
            "move then read",
            "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> int:\n    fields = [Field(value=\"a\")]\n    record = Record(fields=fields)\n    return len(fields)\n",
        ),
        (
            "double move",
            "@derive(Clone)\nmodel Field:\n    value: str\nmodel Record:\n    fields: list[Field]\ndef collect(seed: Borrowed[int]) -> Record:\n    fields = [Field(value=\"a\")]\n    first = Record(fields=fields)\n    second = Record(fields=fields)\n    return second\n",
        ),
    ] {
        let refused = checked(source)?;
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0126")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "collect")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} flow"))?;
        let InputFlow::Checked { analysis, .. } = state else {
            return Err(format!("{label} lost its checked conflict graph: {state:?}").into());
        };
        assert!(!analysis.conflicts.is_empty(), "{label} lost its exact T0126 conflict");
    }

    for (label, accepted) in [("mutable", mutable_positive), ("immutable", immutable_positive)] {
        assert!(
            accepted.errors.is_empty(),
            "{label}: {:?}; {}",
            accepted.errors,
            format!("{:?}", flow_summary(&accepted))
        );
        assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
            owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. })
        }), "{label}: {}", format!("{:?}", flow_summary(&accepted)));
    }
    Ok(())
}

/// One exact source-enum payload arm may continue while a payload-free wildcard terminates. The subject field and
/// payload binding each move once; guards, nested/named/multiple payloads, a second continuation, transformed bodies
/// and borrowed/dependent subjects remain refused before the deferred positive assertion.
#[test]
fn match_expression_assignment_requires_exact_independent_enum_payload()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n";
    let accepted = checked(accepted_source)?;

    for (label, source, expected) in [
        (
            "borrowed subject",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\ndef select(value: Borrowed[Payload]) -> Result[list[Cell], Fault]:\n    values = match value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment subject needs one independent stored enum place",
        ),
        (
            "dependent returned-view subject",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\ndef view(value: Borrowed[Payload]) -> Borrowed[Payload] from value:\n    return value\ndef select(value: Borrowed[Payload]) -> Result[list[Cell], Fault]:\n    held = view(value)\n    values = match held:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment subject needs one independent stored enum place",
        ),
        (
            "guarded extraction",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([]))\n    values = match row.value:\n        Payload.Record(fields) if true => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment extraction does not admit guards",
        ),
        (
            "nested payload pattern",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Inner:\n    Record(list[Cell])\n@derive(Clone)\nenum Payload:\n    Nested(Inner)\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Nested(Inner.Record([])))\n    values = match row.value:\n        Payload.Nested(Inner.Record(fields)) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment extraction needs one immutable payload binding",
        ),
        (
            "named model payload pattern",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nmodel Payload:\n    fields: list[Cell]\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    payload = Payload(fields=[])\n    values = match payload:\n        Payload(fields=fields) => fields\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment subject needs one independent stored enum place",
        ),
        (
            "multiple payload bindings",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Pair(list[Cell], list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Pair([], []))\n    values = match row.value:\n        Payload.Pair(left, right) => left\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment has another continuing or payload-binding arm",
        ),
        (
            "second continuing arm",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n    Empty\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        Payload.Empty => []\n    return Ok(values)\n",
            "match assignment has another continuing or payload-binding arm",
        ),
        (
            "mismatched body",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([]))\n    other: list[Cell] = []\n    values = match row.value:\n        Payload.Record(fields) => other\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment extraction body differs from its payload binding",
        ),
        (
            "transformed payload body",
            "enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef select(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([]))\n    values = match row.value:\n        Payload.Record(fields) => fields + []\n        _ => return Err(Fault.Bad)\n    return Ok(values)\n",
            "match assignment extraction body differs from its payload binding",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "select")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} match-expression flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered match-expression assignment route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}");
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label}: {:?}",
            refused.errors
        );
    }

    let (owner, state) = accepted
        .type_info
        .protocols
        .explicit_input_flow
        .iter()
        .find(|(owner, _)| owner.declaration_name == "decode")
        .ok_or("missing accepted match-expression flow")?;
    let InputFlow::Checked { graph, .. } = state else {
        return Err(format!("match-expression assignment was not checked: {state:?}").into());
    };
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    for spelling in ["row.value", "fields"] {
        assert_eq!(
            graph.nodes.iter().filter(|node| {
                matches!(node.event, flow::Event::Access { access: flow::Access::Move, .. })
                    && accepted_source.get(node.span.start..node.span.end) == Some(spelling)
            }).count(),
            1,
            "match extraction lost its exact {spelling} Move: {:?}",
            graph.nodes
        );
    }
    let subject_span = graph.nodes.iter().find_map(|node| {
        (matches!(node.event, flow::Event::Access { access: flow::Access::Move, .. })
            && accepted_source.get(node.span.start..node.span.end) == Some("row.value")).then_some(node.span)
    }).ok_or("match extraction lost its subject Move")?;
    let mut missing_loan_free = checked(accepted_source)?;
    missing_loan_free.type_info.protocols.loan_free_scrutinees.remove(&subject_span);
    let tokens = crate::lexer::lex(accepted_source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let (declaration_span, function) = program.declarations.iter().find_map(|declaration| match &declaration.node {
        crate::ast::Declaration::Function(function) if function.name == "decode" => Some((declaration.span, function)),
        _ => None,
    }).ok_or("missing accepted match-expression declaration")?;
    let Err(missing_fact) = source::build(
        &missing_loan_free,
        CallableSource::Function(function),
        declaration_span,
        owner,
    ) else {
        return Err("source selection ignored a missing no-loan scrutinee fact".into());
    };
    assert_eq!(
        missing_fact.reason,
        "match assignment subject needs one independent stored enum place"
    );
    assert!(
        missing_loan_free.type_info.checked_input_body(owner)?
            .ok_or("missing match-expression proof after fact removal")?
            .validate_operation_coverage(CallableSource::Function(function))
            .is_ok(),
        "current match replay still trusts the retained no-loan scrutinee marker"
    );

    let reused = checked("enum Fault:\n    Bad\n@derive(Clone)\nmodel Cell:\n    value: str\n@derive(Clone)\nenum Payload:\n    Record(list[Cell])\n@derive(Clone)\nmodel Row:\n    value: Payload\ndef decode(seed: Borrowed[int]) -> Result[list[Cell], Fault]:\n    row = Row(value=Payload.Record([Cell(value=\"a\")]))\n    values = match row.value:\n        Payload.Record(fields) => fields\n        _ => return Err(Fault.Bad)\n    again = row.value\n    return Ok(values)\n")?;
    assert!(
        !reused.errors.is_empty()
            && reused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0126")),
        "post-extraction reuse produced unrelated diagnostics: {:?}",
        reused.errors
    );
    let state = reused.type_info.protocols.explicit_input_flow.iter()
        .find(|(owner, _)| owner.declaration_name == "decode")
        .map(|(_, state)| state)
        .ok_or("missing post-extraction reuse flow")?;
    let InputFlow::Checked { analysis, .. } = state else {
        return Err(format!("post-extraction reuse lost its checked conflict graph: {state:?}").into());
    };
    assert!(!analysis.conflicts.is_empty(), "post-extraction reuse lost its exact T0126 conflict");
    Ok(())
}

/// A direct selected Try(Call) may fill an exact closed constructor field only when its result carries no view,
/// stored carrier, user-selected method or Native call selection. Scalar, enum and list[Model] fields share the
/// same selected-call boundary; every negative executes before the deferred positive assertion.
#[test]
fn constructor_field_try_call_requires_closed_selected_result()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Fault:\n    Bad\nenum Family:\n    Local\n@derive(Clone)\nmodel Row:\n    value: str\nmodel Record:\n    text: str\n    family: Family\n    rows: list[Row]\ndef text_value() -> Result[str, Fault]:\n    return Ok(\"value\")\ndef family_value() -> Result[Family, Fault]:\n    return Ok(Family.Local)\ndef row_values() -> Result[list[Row], Fault]:\n    return Ok([Row(value=\"value\")])\ndef decode(seed: Borrowed[int]) -> Result[Record, Fault]:\n    return Ok(Record(text=text_value()?, family=family_value()?, rows=row_values()?))\n";
    let accepted = checked(accepted_source)?;

    for (label, source, expected, stop) in [
        (
            "returned view",
            "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef view(value: Borrowed[str]) -> Result[Borrowed[str], Fault] from value:\n    return Ok(value)\ndef decode(value: Borrowed[str]) -> Result[Record, Fault]:\n    return Ok(Record(text=view(value)?))\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "view(value)?",
        ),
        (
            "stored Result",
            "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef decode(seed: Borrowed[int], value: str) -> Result[Record, Fault]:\n    result: Result[str, Fault] = Ok(value)\n    return Ok(Record(text=result?))\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "result?",
        ),
        (
            "non-call carrier",
            "enum Fault:\n    Bad\nmodel Record:\n    text: str\ndef decode(seed: Borrowed[int], value: str) -> Result[Record, Fault]:\n    return Ok(Record(text=Ok(value)?))\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "Ok(value)?",
        ),
        (
            "user-selected method",
            "enum Fault:\n    Bad\nclass Decoder:\n    pub def text(self, value: str) -> Result[str, Fault]:\n        return Ok(value)\nmodel Record:\n    text: str\ndef decode(seed: Borrowed[int], value: str) -> Result[Record, Fault]:\n    decoder = Decoder()\n    return Ok(Record(text=decoder.text(value)?))\n",
            "non-Copy constructor field needs checked ownership transfer materialization",
            "decoder.text(value)?",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "decode")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} constructor Try(Call) flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered closed constructor Try(Call) route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, expected, "{label}: {:?}", refused.errors);
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(stop), "{label}");
        if label == "returned view" {
            let call = refused
                .type_info
                .protocols
                .explicit_input_calls
                .values()
                .find(|call| call.callee.declaration_name == "view")
                .ok_or("returned-view control lost its selected call")?;
            let contract = refused
                .type_info
                .selected_input_contract(call)?
                .ok_or("returned-view control lost its selected contract")?;
            assert!(crate::ownership_metadata::returned_view(Some(contract))?.is_some());
        }
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }

    let mut native = checked(accepted_source)?;
    let native_call = native
        .type_info
        .protocols
        .explicit_input_calls
        .values_mut()
        .find(|call| call.callee.declaration_name == "text_value")
        .ok_or("missing selected text_value call for Native call-kind control")?;
    native_call.kind = InputCallKind::Native;
    let tokens = crate::lexer::lex(accepted_source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| matches!(&declaration.node,
            crate::ast::Declaration::Function(function) if function.name == "decode"))
        .ok_or("missing Native call-kind control declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("Native call-kind control is not a function".into());
    };
    let owner = native
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "decode")
        .ok_or("missing Native call-kind control owner")?;
    let Err(native_unavailable) = source::build(&native, CallableSource::Function(function), declaration.span, owner)
    else {
        return Err("native Result call entered closed constructor Try(Call) route".into());
    };
    assert_eq!(
        native_unavailable.reason,
        "non-Copy constructor field needs checked ownership transfer materialization"
    );
    assert_eq!(
        accepted_source.get(native_unavailable.span.start..native_unavailable.span.end),
        Some("text_value()?")
    );
    assert!(
        native.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
        "Native call-kind control produced unrelated diagnostics: {:?}",
        native.errors
    );

    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "decode" && matches!(flow, InputFlow::Checked { .. })
    }));
    Ok(())
}

/// An immutable unaliased local may expose one exact builtin Element+Field path to an immutable ordinary helper only
/// when the selected field is Clone, borrow-free and exactly matches its slot. Borrowed, mutable, overloaded,
/// non-Clone and returned-view storage remains refused before the deferred positive assertion. Native-bearing values
/// remain outside this route-specific matrix; production admission must retain its fail-closed native-loan guard.
#[test]
fn ordinary_argument_element_field_requires_immutable_clone_read()
-> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(seed: Borrowed[int]) -> str:\n    values = [Cell(value=\"a\")]\n    return keep(values[0].value)\n";
    let accepted = checked(accepted_source)?;

    for (label, source, stop) in [
        (
            "borrowed values",
            "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(values: Borrowed[list[Cell]]) -> str:\n    return keep(values[0].value)\n",
            "values[0].value",
        ),
        (
            "mutable values",
            "@derive(Clone)\nmodel Cell:\n    value: str\ndef keep(value: str) -> str:\n    return value\ndef decode(seed: Borrowed[int]) -> str:\n    mut values = [Cell(value=\"a\")]\n    return keep(values[0].value)\n",
            "values[0].value",
        ),
        (
            "selected user index",
            "@derive(Clone)\nmodel Cell:\n    value: str\nclass Shelf:\n    pub item: Cell\n    def __getitem__(self, index: int) -> Cell:\n        return self.item\ndef keep(value: str) -> str:\n    return value\ndef decode(seed: Borrowed[int]) -> str:\n    shelf = Shelf(item=Cell(value=\"a\"))\n    return keep(shelf[0].value)\n",
            "shelf[0].value",
        ),
        (
            "non-Clone field",
            "from rust::std::thread import JoinHandle\nmodel Cell:\n    token: JoinHandle[int]\ndef keep(value: JoinHandle[int]) -> None:\n    pass\ndef decode(seed: Borrowed[int]) -> None:\n    values: list[Cell] = []\n    keep(values[0].token)\n",
            "values[0].token",
        ),
        (
            "returned-view initializer",
            "@derive(Clone)\nmodel Cell:\n    value: str\ndef view(values: Borrowed[list[Cell]]) -> Borrowed[list[Cell]] from values:\n    return values\ndef keep(value: str) -> str:\n    return value\ndef decode(source: Borrowed[list[Cell]]) -> str:\n    values = view(source)\n    return keep(values[0].value)\n",
            "values[0].value",
        ),
    ] {
        let refused = checked(source)?;
        let state = refused
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .find(|(owner, _)| owner.declaration_name == "decode")
            .map(|(_, state)| state)
            .ok_or_else(|| format!("missing {label} Element+Field argument flow"))?;
        let InputFlow::Unavailable(unavailable) = state else {
            return Err(format!("{label} entered immutable Element+Field argument route: {state:?}").into());
        };
        assert_eq!(unavailable.reason, "ordinary argument requires inferred access or mutable binding facts", "{label}");
        assert_eq!(source.get(unavailable.span.start..unavailable.span.end), Some(stop), "{label}");
        if label == "selected user index" {
            assert!(refused.type_info.calls.resolved_operator_calls.keys().any(|span| {
                unavailable.span.start <= span.start && span.end <= unavailable.span.end
            }));
        }
        assert!(
            !refused.errors.is_empty()
                && refused.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }

    let mut mismatch = checked(accepted_source)?;
    let mismatch_call = mismatch
        .type_info
        .protocols
        .explicit_input_calls
        .values_mut()
        .find(|call| call.callee.declaration_name == "keep")
        .ok_or("missing keep call for exact slot mismatch control")?;
    let [parameter] = mismatch_call.parameters.as_mut_slice() else {
        return Err("keep mismatch control has the wrong arity".into());
    };
    parameter.ty = crate::symbols::ResolvedType::Bytes;
    let tokens = crate::lexer::lex(accepted_source).map_err(|errors| format!("{errors:?}"))?;
    let program = crate::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let declaration = program
        .declarations
        .iter()
        .find(|declaration| matches!(&declaration.node,
            crate::ast::Declaration::Function(function) if function.name == "decode"))
        .ok_or("missing exact slot mismatch declaration")?;
    let crate::ast::Declaration::Function(function) = &declaration.node else {
        return Err("exact slot mismatch control is not a function".into());
    };
    let owner = mismatch
        .type_info
        .protocols
        .explicit_input_flow
        .keys()
        .find(|owner| owner.declaration_name == "decode")
        .ok_or("missing exact slot mismatch owner")?;
    let Err(mismatch_unavailable) = source::build(
        &mismatch,
        CallableSource::Function(function),
        declaration.span,
        owner,
    ) else {
        return Err("mismatched Element+Field argument entered immutable read route".into());
    };
    assert_eq!(mismatch_unavailable.reason, "ordinary invocation differs from its selected call-site header");
    assert_eq!(
        accepted_source.get(mismatch_unavailable.span.start..mismatch_unavailable.span.end),
        Some("keep(values[0].value)")
    );
    assert!(
        mismatch.errors.iter().all(|error| error.stable_code() == Some("INCAN-T0123")),
        "exact slot mismatch produced unrelated diagnostics: {:?}",
        mismatch.errors
    );

    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(accepted.type_info.protocols.explicit_input_flow.iter().any(|(owner, flow)| {
        owner.declaration_name == "decode" && matches!(flow, InputFlow::Checked { .. })
    }));
    Ok(())
}
