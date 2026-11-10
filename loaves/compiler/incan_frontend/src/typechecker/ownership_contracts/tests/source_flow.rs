//! Source flow over local modes, owned initializers, observers, branches, loops and loans.

use super::*;

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
