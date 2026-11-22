//! Shared Crumb field contracts for ranked compiler work and completed read witnesses.

use crate::crumb_rows::{admit_rows, encode_rows};
use crate::record_scope::{MemberSelection, PayloadSelection, RecordKey, RecordRequest, RecordSelection, valid_key};
use internal_models_bindings::{
    Budget, Description, DescriptionMode, IntegerValue, RowRecord, RowSet, Value, ValueField, binary_message,
    create_budget, crumb_message, read_field_at, read_record_at, row_count, row_descriptions,
};

pub const KEY_TYPE: &str = "tuple<string,string,string,string,string,string>";
pub const REQUEST_TYPE: &str =
    "tuple<tuple<string,string,string,string,string,string>,i64,string,list<string>,list<i64>>";
pub(crate) const MAX_GRAPH_RECORD_BYTES: i64 = 67_108_864;

/// Serialize exactly one typed compiler graph record through the public Crumb row codec.
pub fn encode_graph_row(key: &[u8], description: &Description, fields: Vec<ValueField>) -> Result<Vec<u8>, String> {
    let row = RowRecord {
        key: key.to_vec(),
        value: Value::Record(fields),
    };
    encode_rows(description, vec![row], MAX_GRAPH_RECORD_BYTES)
}

/// Require exactly the expected row, selector and field count before any domain projection.
pub fn decode_graph_row(
    encoded: &[u8],
    key: &[u8],
    description: &Description,
    count: usize,
) -> Result<Vec<ValueField>, String> {
    let mut budget = graph_read_budget()?;
    let rows = admit_graph_row(encoded, description, &mut budget, MAX_GRAPH_RECORD_BYTES)?;
    let row = read_record_at(&rows, 0, &mut budget).map_err(|error| binary_message(&error))?;
    if row.key != key {
        return Err("unexpected compiler graph Crumb row".into());
    }
    match row.value {
        Value::Record(fields) if fields.len() == count => Ok(fields),
        Value::Record(_) => Err("compiler graph Crumb row has the wrong field count".into()),
        _ => Err("expected compiler graph Crumb record".into()),
    }
}

/// Read one required text field without coercion.
pub fn text(value: &Value) -> Result<String, String> {
    match value {
        Value::Text(item) => Ok(item.clone()),
        _ => Err("expected compiler graph text".into()),
    }
}

/// Project one signed compiler integer to a Crumb i64.
pub fn integer(item: i64) -> Value {
    Value::Integer(IntegerValue::Signed(i128::from(item)))
}

/// Read a signed integer admitted under the i64 field contract.
pub fn number(value: &Value) -> Result<i64, String> {
    match value {
        // The field contract admits only i64 values; the reference keeps the low 64 bits.
        Value::Integer(IntegerValue::Signed(item)) => Ok(*item as i64),
        _ => Err("expected signed compiler graph integer".into()),
    }
}

/// Read one list; list and tuple have different Crumb representations.
pub fn items(value: &Value) -> Result<&Vec<Value>, String> {
    match value {
        Value::List(elements) => Ok(elements),
        _ => Err("expected compiler graph list".into()),
    }
}

/// Read a fixed tuple and verify its cardinality before indexing.
pub fn tuple_items(value: &Value, expected: usize) -> Result<&Vec<Value>, String> {
    match value {
        Value::Tuple(elements) if elements.len() == expected => Ok(elements),
        Value::Tuple(_) => Err("compiler graph tuple has the wrong arity".into()),
        _ => Err("expected compiler graph tuple".into()),
    }
}

/// By-value form of [`text`] for Incan callers: a field read from a list element passes as it is held.
pub fn text_value(value: Value) -> Result<String, String> {
    match value {
        Value::Text(item) => Ok(item),
        other => text(&other),
    }
}

/// By-value form of [`number`] for Incan callers.
pub fn number_value(value: Value) -> Result<i64, String> {
    number(&value)
}

/// By-value form of [`items`] for Incan callers; the elements move out of the value.
pub fn items_value(value: Value) -> Result<Vec<Value>, String> {
    match value {
        Value::List(elements) => Ok(elements),
        other => items(&other).cloned(),
    }
}

/// By-value form of [`tuple_items`] for Incan callers, whose arity is an Incan `int`.
pub fn tuple_items_value(value: Value, expected: i64) -> Result<Vec<Value>, String> {
    let expected = usize::try_from(expected).map_or(usize::MAX, |count| count);
    tuple_items(&value, expected)?;
    match value {
        Value::Tuple(elements) => Ok(elements),
        other => tuple_items(&other, expected).cloned(),
    }
}

/// Preserve each identity component independently, including the optional context.
pub fn key_value(key: &RecordKey) -> Value {
    Value::Tuple(vec![
        Value::Text(key.family.clone()),
        Value::Text(key.identity_profile.clone()),
        Value::Text(key.origin.clone()),
        Value::Text(key.identity.clone()),
        Value::Text(key.facet.clone()),
        Value::Text(key.context.clone()),
    ])
}

/// Decode a complete logical identity before selecting graph work.
pub fn read_key(value: &Value) -> Result<RecordKey, String> {
    let fields = tuple_items(value, 6)?;
    let key = RecordKey {
        family: text(&fields[0])?,
        identity_profile: text(&fields[1])?,
        origin: text(&fields[2])?,
        identity: text(&fields[3])?,
        facet: text(&fields[4])?,
        context: text(&fields[5])?,
    };
    if !valid_key(&key) {
        return Err("compiler record key authority fields must be nonempty".into());
    }
    Ok(key)
}

/// Keep selector arguments typed and preserve arbitrary strings without delimiters.
pub fn request_value(request: &RecordRequest) -> Value {
    let key = key_value(&request.key);
    let bound = integer(request.byte_limit);
    let (kind, strings, numbers) = match &request.selection {
        RecordSelection::Field(name) => ("field", vec![name.clone()], Vec::new()),
        RecordSelection::Payload(payload) => (
            "payload",
            vec![payload.reference.clone()],
            vec![integer(payload.offset), integer(payload.length)],
        ),
        RecordSelection::Members(members) => (
            "members",
            vec![
                members.selector.clone(),
                members.order.clone(),
                members.lower.clone(),
                members.upper.clone(),
                members.continuation.clone(),
            ],
            Vec::new(),
        ),
    };
    Value::Tuple(vec![
        key,
        bound,
        Value::Text(kind.into()),
        Value::List(strings.into_iter().map(Value::Text).collect()),
        Value::List(numbers),
    ])
}

/// Reconstruct only known selectors with their exact argument arities.
pub fn read_request(value: &Value) -> Result<RecordRequest, String> {
    let fields = tuple_items(value, 5)?;
    let key = read_key(&fields[0])?;
    let limit = number(&fields[1])?;
    if limit < 0 {
        return Err("compiler record byte limit must be nonnegative".into());
    }
    let kind = text(&fields[2])?;
    let strings = items(&fields[3])?;
    let numbers = items(&fields[4])?;
    let selection = match (kind.as_str(), strings.len(), numbers.len()) {
        ("field", 1, 0) => RecordSelection::Field(text(&strings[0])?),
        ("payload", 1, 2) => RecordSelection::Payload(PayloadSelection {
            reference: text(&strings[0])?,
            offset: number(&numbers[0])?,
            length: number(&numbers[1])?,
        }),
        ("members", 5, 0) => RecordSelection::Members(MemberSelection {
            selector: text(&strings[0])?,
            order: text(&strings[1])?,
            lower: text(&strings[2])?,
            upper: text(&strings[3])?,
            continuation: text(&strings[4])?,
        }),
        _ => return Err("invalid compiler record selection".into()),
    };
    Ok(RecordRequest {
        key,
        selection,
        byte_limit: limit,
    })
}

/// Read one projected field only after admission of the complete envelope and exact row type.
pub fn decode_graph_field(encoded: &[u8], description: &Description, field: &[u8]) -> Result<Value, String> {
    let mut budget = graph_read_budget()?;
    let rows = admit_graph_row(encoded, description, &mut budget, MAX_GRAPH_RECORD_BYTES)?;
    read_field_at(&rows, 0, &field.to_vec(), &mut budget).map_err(|error| binary_message(&error))
}

/// Use the same bound whether materializing a graph row or projecting a single field.
pub fn graph_read_budget() -> Result<Budget, String> {
    create_budget(MAX_GRAPH_RECORD_BYTES * 4, 4_194_304, 128).map_err(|error| crumb_message(&error))
}

/// Authenticate the sole dictionary schema before exposing any logical fields. The reference default maximum is
/// `MAX_GRAPH_RECORD_BYTES`.
pub fn admit_graph_row(
    encoded: &[u8],
    description: &Description,
    budget: &mut Budget,
    maximum: i64,
) -> Result<RowSet, String> {
    let rows = admit_rows(encoded, &vec![description.clone()], maximum, budget)?;
    let dictionary = row_descriptions(&rows, budget).map_err(|error| binary_message(&error))?;
    if row_count(&rows) != 1 || dictionary.len() != 1 {
        return Err("unexpected compiler graph Crumb row".into());
    }
    // The only referenced dictionary entry was resolved using our one supplied description.
    match dictionary[0].mode {
        DescriptionMode::Reference => Ok(rows),
        DescriptionMode::Inline => Err("unexpected inline compiler graph description".into()),
    }
}
