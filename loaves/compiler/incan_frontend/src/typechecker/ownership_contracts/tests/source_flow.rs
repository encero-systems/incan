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
