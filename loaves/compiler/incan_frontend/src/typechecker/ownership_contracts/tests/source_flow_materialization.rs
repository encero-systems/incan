//! Stored-value and call-result materialization boundaries retained by source flow.

use super::*;

/// An immutable ordinary slot may receive one selected call's fresh closed result. A returned view remains stored
/// caller-owned state and must not borrow this route merely because its presented value type matches the slot.
#[test]
fn ordinary_argument_accepts_only_fresh_closed_selected_call_results() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "def encode(value: Borrowed[int]) -> bytes:\n    return b\"row\"\ndef observe(value: bytes) -> int:\n    return len(value)\ndef inspect(value: Borrowed[int]) -> int:\n    return observe(encode(value))\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| { owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. }) })
    );

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
    assert_eq!(
        unavailable.reason,
        "ordinary argument requires inferred access or mutable binding facts"
    );
    assert_eq!(
        refused_source.get(unavailable.span.start..unavailable.span.end),
        Some("view(value)")
    );
    assert!(
        !refused.errors.is_empty()
            && refused
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
        "unrelated returned-view diagnostics: {:?}",
        refused.errors
    );
    Ok(())
}

/// A selected ordinary call returning a closed model is fresh list-element storage. A previously stored model keeps
/// its root and still needs retained-owner projection instead of acquiring transfer authority from the list literal.
#[test]
fn list_element_accepts_only_fresh_closed_selected_model_results() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "model Field:\n    value: bytes\ndef make(seed: int) -> Field:\n    return Field(value=b\"row\")\ndef collect(seed: Borrowed[int]) -> list[Field]:\n    return [make(seed)]\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| { owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. }) })
    );

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
        assert_eq!(
            refused_source.get(unavailable.span.start..unavailable.span.end),
            Some(stop),
            "{label}"
        );
        assert!(
            !refused.errors.is_empty()
                && refused
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }
    Ok(())
}

/// A constructor field may materialize one indexed value from a fresh builtin-slice loop row. An ordinary stored
/// list and a borrowed root remain outside that exact producer route, including when their element types match.
#[test]
fn constructor_field_index_requires_fresh_transferable_loop_row() -> Result<(), Box<dyn std::error::Error>> {
    let accepted = checked(
        "model Field:\n    value: str\ndef collect(rows: Borrowed[list[list[str]]]) -> list[Field]:\n    mut fields: list[Field] = []\n    for row in rows[1:]:\n        fields.append(Field(value=row[0]))\n    return fields\n",
    )?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| { owner.declaration_name == "collect" && matches!(flow, InputFlow::Checked { .. }) })
    );

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
        assert_eq!(
            source.get(unavailable.span.start..unavailable.span.end),
            Some(stop),
            "{label}"
        );
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
                && refused
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }
    Ok(())
}

/// Every canonical loop-pattern local is reinitialized at the poll, even when its type is cloneable and therefore
/// not an item-taking candidate. Reinitialization must not hide a same-iteration or outer-root use after transfer.
#[test]
fn cloneable_loop_pattern_reinitializes_without_suppressing_real_moves() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = "model Field:\n    value: str\ndef consume(value: Owned[str]) -> None:\n    pass\n";
    let accepted_source = format!(
        "{declarations}def inspect(fields: Borrowed[list[Field]]) -> str:\n    mut previous = \"\"\n    for field in fields:\n        if field.value == \"stop\":\n            consume(field.value)\n            return previous\n        previous = field.value\n    return previous\n"
    );
    let accepted = checked(&accepted_source)?;
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| { owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. }) })
    );

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
                && refused
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0126")),
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
        assert!(
            !analysis.conflicts.is_empty(),
            "{label} move lost its exact T0126 conflict"
        );
    }
    Ok(())
}

/// Parentheses around a nested `and` inside `or` preserve both short-circuit exits before a closed Borrowed
/// validation call runs through `?` for each fresh loop-pattern owner. A real same-iteration transfer and a
/// returned-view carrier keep their existing outcomes instead of borrowing the loop's per-poll replacement.
#[test]
fn parenthesized_short_circuit_loop_validation_preserves_exact_edges() -> Result<(), Box<dyn std::error::Error>> {
    let accepted_source = "enum Fault:\n    Bad\nmodel Field:\n    family: str\n    node: str\n    selector: str\ndef validate(field: Borrowed[Field]) -> Result[None, Fault]:\n    if field.family == \"bad\":\n        return Err(Fault.Bad)\n    return Ok(None)\ndef inspect(fields: Borrowed[list[Field]]) -> Result[str, Fault]:\n    mut previous_family = \"\"\n    mut previous_node = \"\"\n    mut previous_selector = \"\"\n    for field in fields:\n        if field.family < previous_family or (field.family == previous_family and field.node < previous_node):\n            return Err(Fault.Bad)\n        validate(field)?\n        previous_family = field.family\n        previous_node = field.node\n        previous_selector = field.selector\n    return Ok(previous_selector)\n";
    let accepted = checked(accepted_source)?;

    let moved_source = "enum Fault:\n    Bad\nmodel Field:\n    value: str\ndef consume(value: Owned[str]) -> Result[None, Fault]:\n    return Ok(None)\ndef inspect(fields: Borrowed[list[Field]]) -> Result[str, Fault]:\n    mut previous = \"\"\n    for field in fields:\n        consume(field.value)?\n        previous = field.value\n    return Ok(previous)\n";
    let moved = checked(moved_source)?;
    assert!(
        !moved.errors.is_empty()
            && moved
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0126")),
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
    assert!(
        !analysis.conflicts.is_empty(),
        "same-iteration move lost its exact T0126 conflict"
    );

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
            && dependent
                .errors
                .iter()
                .all(|error| error.stable_code() == Some("INCAN-T0123")),
        "returned-view carrier produced unrelated diagnostics: {:?}",
        dependent.errors
    );
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    assert!(
        accepted
            .type_info
            .protocols
            .explicit_input_flow
            .iter()
            .any(|(owner, flow)| { owner.declaration_name == "inspect" && matches!(flow, InputFlow::Checked { .. }) })
    );
    Ok(())
}

/// A builtin borrow-free slice of one fresh loop row may materialize an exact constructor field. This is a
/// constructor-only fact: stored and borrowed roots, user-selected slice operators, unproven element cloning,
/// closed cloneable non-scalar elements and the identical slice nested as a list element retain their refusals.
#[test]
fn constructor_field_slice_requires_fresh_transferable_loop_row() -> Result<(), Box<dyn std::error::Error>> {
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
        assert_eq!(
            source.get(unavailable.span.start..unavailable.span.end),
            Some(stop),
            "{label}"
        );
        assert!(
            !refused.errors.is_empty()
                && refused
                    .errors
                    .iter()
                    .all(|error| error.stable_code() == Some("INCAN-T0123")),
            "{label} produced unrelated diagnostics: {:?}",
            refused.errors
        );
    }
    assert!(accepted.errors.is_empty(), "{:?}", accepted.errors);
    for name in ["from_native", "from_family"] {
        assert!(
            accepted
                .type_info
                .protocols
                .explicit_input_flow
                .iter()
                .any(|(owner, flow)| { owner.declaration_name == name && matches!(flow, InputFlow::Checked { .. }) })
        );
    }
    Ok(())
}

