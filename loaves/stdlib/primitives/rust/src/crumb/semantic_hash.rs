//! Native crumb1 object, canonical, model, description and seal digests over the corpus notation.
//!
//! The logical tree is rebuilt from the independent fixture notation, ordered by Crumb's stored comparison and
//! streamed through the same tagged V, S and D contributions as `std.primitives.crumb.hashes`.

use std::cmp::Ordering;
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;

use serde_json::Value as Json;
use sha2::{Digest, Sha256};

/// The codec borrows immutable bytes independently of the native mapping implementation used by its caller.
trait RowBacking: AsRef<[u8]> + Debug + Send + Sync {}
impl<T: AsRef<[u8]> + Debug + Send + Sync> RowBacking for T {}

/// A bounded view keeps its envelope's backing alive and only copies the shared handle when sliced.
#[derive(Clone, Debug)]
pub(super) struct RowView {
    backing: Arc<dyn RowBacking>,
    range: Range<usize>,
}

impl RowView {
    /// Share an already-owned immutable input. Platform views carry their own mapping lifetime.
    #[allow(
        dead_code,
        reason = "the shared standalone codegraph codec also accepts borrowed slices"
    )]
    pub(super) fn new(backing: impl AsRef<[u8]> + Debug + Send + Sync + 'static) -> Self {
        let size = backing.as_ref().len();
        Self {
            backing: Arc::new(backing),
            range: 0..size,
        }
    }

    /// Borrow the bytes within this view's checked extent.
    pub(super) fn as_bytes(&self) -> &[u8] {
        &self.backing.as_ref().as_ref()[self.range.clone()]
    }

    /// Select an extent relative to this view, retaining only the shared backing handle.
    pub(super) fn slice(&self, offset: usize, length: usize) -> Option<Self> {
        let end = offset.checked_add(length)?;
        if end > self.range.len() {
            return None;
        }
        let start = self.range.start;
        Some(Self {
            backing: Arc::clone(&self.backing),
            range: start + offset..start + end,
        })
    }
}

/// Binary cells are byte values regardless of whether they own bytes or retain part of an immutable row.
#[derive(Clone)]
pub(super) enum Binary {
    Owned(Vec<u8>),
    Mapped(RowView),
}

impl From<Vec<u8>> for Binary {
    /// Move newly produced bytes into a logical cell.
    fn from(bytes: Vec<u8>) -> Self {
        Self::Owned(bytes)
    }
}

impl AsRef<[u8]> for Binary {
    /// Borrow the logical bytes for comparisons, hashing and explicit materialization.
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Owned(bytes) => bytes,
            Self::Mapped(view) => view.as_bytes(),
        }
    }
}

/// One logical value after notation translation, mirroring the Incan `Value` carrier.
#[derive(Clone)]
pub(super) enum Value {
    Absent,
    Some(Box<Value>),
    Boolean(bool),
    Integer(bool, u128),
    Float(Vec<u8>),
    Decimal(i128, i64),
    Text(String),
    Binary(Binary),
    Withheld,
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Set(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Record(Vec<Field>),
    Enum(Vec<u8>, Vec<Field>),
    Union(Node, Box<Value>),
    Reference(Vec<u8>, Vec<u8>),
}

/// One stored field with its identity, canonical participation and declared absence.
#[derive(Clone)]
pub(super) struct Field {
    pub(super) identity: Vec<u8>,
    pub(super) canonical: bool,
    pub(super) absent: char,
    pub(super) value: Value,
}

/// One type node, mirroring the Incan `TypeNode` forms the corpus notation can spell.
#[derive(Clone)]
pub(super) enum Node {
    Scalar(String),
    Decimal(i64, i64),
    Container(String, Vec<Node>),
    Nominal(String, Vec<u8>, Vec<Node>),
    Record(String, Vec<Shape>),
    Enum(String, Vec<(Vec<u8>, Vec<Shape>)>),
    Recursive(String),
    Reference(Box<Node>),
}

/// One structural field shape used by S spellings.
#[derive(Clone)]
pub(super) struct Shape {
    pub(super) identity: Vec<u8>,
    pub(super) storage: Node,
    pub(super) canonical: bool,
    pub(super) absent: char,
}

/// Object keeps every stored field; canonical omits fields excluded by their description.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Projection {
    Object,
    Canonical,
}

pub(super) const SCALARS: [&str; 18] = [
    "boolean",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "fp32",
    "fp64",
    "string",
    "binary",
    "secret_string",
    "secret_bytes",
    "reference",
];

/// Derive one complete preimage and digest for a hash-preimage corpus document.
pub(super) fn hash_preimage(document: &Json) -> Option<(Vec<u8>, [u8; 32])> {
    let input = document.get("input")?;
    let preimage = match document.get("projection")?.as_str()? {
        "object" => value_preimage(&logical(input)?, Projection::Object)?,
        "canonical" => value_preimage(&logical(input)?, Projection::Canonical)?,
        "model" => model_preimage(&node(input.get("type")?)?)?,
        "description" => description_preimage(input.get("description")?)?,
        "seal" => seal_preimage(&logical_items(input.get("values")?)?)?,
        _ => return None,
    };
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&Sha256::digest(&preimage));
    Some((preimage, digest))
}

/// Validate one value and stream its object or canonical V contribution behind its domain prefix.
pub(super) fn value_preimage(value: &Value, projection: Projection) -> Option<Vec<u8>> {
    validate(value)?;
    let mut output = b"crumb1\x00".to_vec();
    output.push(if projection == Projection::Object { 1 } else { 2 });
    emit(value, projection, &mut output)?;
    Some(output)
}

/// Frame the structural S spelling behind the model domain prefix.
pub(super) fn model_preimage(node: &Node) -> Option<Vec<u8>> {
    let spelling = spell(node, false, &mut Vec::new())?;
    let mut output = b"crumb1\x00\x03".to_vec();
    frame(&mut output, spelling.as_bytes());
    Some(output)
}

/// Commit to ordered object contributions, each framed by its exact length.
fn seal_preimage(values: &[Value]) -> Option<Vec<u8>> {
    for value in values {
        validate(value)?;
    }
    let mut output = b"crumb1\x00\x04".to_vec();
    unsigned(&mut output, values.len() as u128);
    for value in values {
        let mut contribution = Vec::new();
        emit(value, Projection::Object, &mut contribution)?;
        frame(&mut output, &contribution);
    }
    Some(output)
}

/// Encode the complete tagged metadata tree behind the description domain prefix.
fn description_preimage(description: &Json) -> Option<Vec<u8>> {
    let mut output = b"crumb1\x00\x05".to_vec();
    metadata(description, &mut output)?;
    Some(output)
}

/// Translate one tagged corpus value without sorting or merging its original entries.
pub(super) fn logical(value: &Json) -> Option<Value> {
    Some(match value.get("kind")?.as_str()? {
        "none" => Value::Absent,
        "some" => Value::Some(Box::new(logical(value.get("value")?)?)),
        "boolean" => Value::Boolean(value.get("value")?.as_bool()?),
        "int" => integer(value)?,
        "float" => Value::Float(float64_bits(value.get("width")?.as_i64()?, &hex(value.get("bits")?)?)?),
        "decimal" => Value::Decimal(
            value.get("coefficient")?.as_str()?.parse().ok()?,
            value.get("scale")?.as_i64()?,
        ),
        "string" => Value::Text(value.get("value")?.as_str()?.to_string()),
        "binary" => Value::Binary(hex(value.get("hex")?)?.into()),
        "withheld" => Value::Withheld,
        "list" => Value::List(logical_items(value.get("items")?)?),
        "tuple" => Value::Tuple(logical_items(value.get("items")?)?),
        "set" => Value::Set(logical_items(value.get("items")?)?),
        "map" => Value::Map(entries(value.get("entries")?)?),
        "record" => Value::Record(fields(value.get("fields")?)?),
        "enum" => match logical(value.get("payload")?)? {
            Value::Record(payload) => Value::Enum(hex(value.get("variant_id")?)?, payload),
            _ => return None,
        },
        "union" => Value::Union(
            member(value.get("member_type")?)?,
            Box::new(logical(value.get("value")?)?),
        ),
        "reference" => Value::Reference(opaque(value, "target_type")?, opaque(value, "identity")?),
        _ => return None,
    })
}

/// Translate ordered items one by one.
pub(super) fn logical_items(items: &Json) -> Option<Vec<Value>> {
    items.as_array()?.iter().map(logical).collect()
}

/// Parse an exact integer into the signed or unsigned carrier the Incan fixture adapter selects.
fn integer(value: &Json) -> Option<Value> {
    let spelling = value.get("value")?.as_str()?;
    let unsigned = value
        .get("type")
        .and_then(Json::as_str)
        .is_some_and(|kind| kind.starts_with('u'));
    if unsigned {
        return Some(Value::Integer(false, spelling.parse::<u128>().ok()?));
    }
    match spelling.parse::<i128>() {
        Ok(number) => Some(Value::Integer(number < 0, number.unsigned_abs())),
        Err(_) => Some(Value::Integer(false, spelling.parse::<u128>().ok()?)),
    }
}

/// Keep map entries as explicit pairs so duplicates are refused rather than overwritten.
fn entries(items: &Json) -> Option<Vec<(Value, Value)>> {
    let mut result = Vec::new();
    for item in items.as_array()? {
        let pair = item.as_array()?;
        if pair.len() != 2 {
            return None;
        }
        result.push((logical(&pair[0])?, logical(&pair[1])?));
    }
    Some(result)
}

/// Translate record fields with their declared absence and canonical participation.
fn fields(items: &Json) -> Option<Vec<Field>> {
    let mut result = Vec::new();
    for item in items.as_array()? {
        result.push(Field {
            identity: hex(item.get("id")?)?,
            canonical: item.get("canonical")?.as_bool()?,
            absent: absence(item.get("absent")?)?,
            value: logical(item.get("value")?)?,
        });
    }
    Some(result)
}

/// Translate the declared absence policy into its one-letter S code.
fn absence(value: &Json) -> Option<char> {
    match value.as_str()? {
        "required" => Some('r'),
        "none" => Some('n'),
        "empty" => Some('e'),
        _ => None,
    }
}

/// Use exact identity bytes when present; textual labels are opaque UTF-8.
fn opaque(value: &Json, key: &str) -> Option<Vec<u8>> {
    if let Some(encoded) = value.get(format!("{key}_hex")) {
        return hex(encoded);
    }
    Some(value.get(key)?.as_str()?.as_bytes().to_vec())
}

/// Translate one closed union member into its nominal selector node.
fn member(value: &Json) -> Option<Node> {
    let kind = value.get("kind")?.as_str()?;
    if !matches!(kind, "record" | "enum") {
        return None;
    }
    let arguments = match value.get("arguments") {
        Some(items) => nodes(items)?,
        None => Vec::new(),
    };
    Some(Node::Nominal(
        kind.to_string(),
        hex(value.get("nominal_hex")?)?,
        arguments,
    ))
}

/// Translate ordered type nodes.
fn nodes(items: &Json) -> Option<Vec<Node>> {
    items.as_array()?.iter().map(node).collect()
}

/// Translate one scalar, container, record, enum or recursive type node.
pub(super) fn node(value: &Json) -> Option<Node> {
    if let Some(spelling) = value.as_str() {
        return selector_node(spelling);
    }
    let kind = value.get("kind")?.as_str()?;
    Some(match kind {
        "option" | "list" | "set" => Node::Container(kind.to_string(), vec![node(value.get("item")?)?]),
        "map" => Node::Container(
            kind.to_string(),
            vec![node(value.get("key_type")?)?, node(value.get("value_type")?)?],
        ),
        "tuple" | "union" => Node::Container(kind.to_string(), nodes(value.get("items")?)?),
        "record" => Node::Record(value.get("key")?.as_str()?.to_string(), shapes(value)?),
        "enum" => {
            let mut variants = Vec::new();
            for variant in value.get("variants")?.as_array()? {
                variants.push((hex(variant.get("id")?)?, shapes(variant)?));
            }
            Node::Enum(value.get("key")?.as_str()?.to_string(), variants)
        }
        "recur" => Node::Recursive(value.get("key")?.as_str()?.to_string()),
        _ => return None,
    })
}

/// Translate declared field shapes without sorting or repairing their identities.
pub(super) fn shapes(value: &Json) -> Option<Vec<Shape>> {
    let mut result = Vec::new();
    for item in value.get("fields")?.as_array()? {
        result.push(Shape {
            identity: hex(item.get("id")?)?,
            storage: node(item.get("type")?)?,
            canonical: item.get("canonical")?.as_bool()?,
            absent: absence(item.get("absent")?)?,
        });
    }
    Some(result)
}

/// Decode the corpus's scalar, decimal and closed nominal selector strings.
fn selector_node(spelling: &str) -> Option<Node> {
    if let Some(inner) = spelling
        .strip_prefix("decimal<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        let (precision, scale) = inner.split_once(',')?;
        return Some(Node::Decimal(precision.parse().ok()?, scale.parse().ok()?));
    }
    let (kind, rest) = if let Some(rest) = spelling.strip_prefix("record@") {
        ("record", rest)
    } else if let Some(rest) = spelling.strip_prefix("enum@") {
        ("enum", rest)
    } else {
        return SCALARS.contains(&spelling).then(|| Node::Scalar(spelling.to_string()));
    };
    let identity = hex_text(rest.get(..12)?)?;
    let tail = &rest[12..];
    let mut arguments = Vec::new();
    if !tail.is_empty() {
        let inner = tail.strip_prefix('<')?.strip_suffix('>')?;
        for part in split_arguments(inner)? {
            arguments.push(selector_node(part)?);
        }
    }
    Some(Node::Nominal(kind.to_string(), identity, arguments))
}

/// Split only top-level commas, refusing empty or unbalanced arguments.
fn split_arguments(contents: &str) -> Option<Vec<&str>> {
    let mut depth = 0i64;
    let mut start = 0;
    let mut parts = Vec::new();
    for (index, character) in contents.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            ',' if depth == 0 => {
                if index == start {
                    return None;
                }
                parts.push(&contents[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 || start == contents.len() {
        return None;
    }
    parts.push(&contents[start..]);
    Some(parts)
}

/// Decode strict lowercase hexadecimal from a JSON string.
fn hex(value: &Json) -> Option<Vec<u8>> {
    hex_text(value.as_str()?)
}

/// Decode strict hexadecimal octets.
fn hex_text(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(value.get(index..index + 2)?, 16).ok())
        .collect()
}

/// Widen binary32 exactly and normalize every NaN while retaining signed zero and infinities.
fn float64_bits(width: i64, bits: &[u8]) -> Option<Vec<u8>> {
    const NAN: [u8; 8] = [0x7f, 0xf8, 0, 0, 0, 0, 0, 0];
    let wide = match (width, bits.len()) {
        (64, 8) => f64::from_bits(u64::from_be_bytes(bits.try_into().ok()?)),
        (32, 4) => f64::from(f32::from_bits(u32::from_be_bytes(bits.try_into().ok()?))),
        _ => return None,
    };
    if wide.is_nan() {
        return Some(NAN.to_vec());
    }
    Some(wide.to_bits().to_be_bytes().to_vec())
}

/// Check format invariants, including values a canonical digest will omit.
fn validate(value: &Value) -> Option<()> {
    match value {
        Value::Some(item) => validate(item),
        Value::Decimal(coefficient, scale) => normalize_decimal(*coefficient, *scale).map(|_| ()),
        Value::List(items) | Value::Tuple(items) => items.iter().try_for_each(validate),
        Value::Set(items) => {
            items.iter().try_for_each(validate)?;
            let order = order_values(items, Projection::Object)?;
            unique(
                order
                    .windows(2)
                    .map(|pair| compare(&items[pair[0]], &items[pair[1]], Projection::Object)),
            )
        }
        Value::Map(entries) => {
            for (key, item) in entries {
                validate(key)?;
                validate(item)?;
            }
            let order = order_entries(entries, Projection::Object)?;
            unique(
                order
                    .windows(2)
                    .map(|pair| compare(&entries[pair[0]].0, &entries[pair[1]].0, Projection::Object)),
            )
        }
        Value::Record(fields) => validate_fields(fields),
        Value::Enum(identity, fields) => (identity.len() == 6)
            .then_some(())
            .and_then(|_| validate_fields(fields)),
        Value::Union(member, item) => {
            selector_spelling(member)?;
            validate(item)
        }
        _ => Some(()),
    }
}

/// Refuse any adjacent pair of sorted elements that compares equal.
fn unique(mut comparisons: impl Iterator<Item = Option<Ordering>>) -> Option<()> {
    comparisons.try_for_each(|result| (result? != Ordering::Equal).then_some(()))
}

/// Field identities are six octets and unique before any value is visited.
fn validate_fields(fields: &[Field]) -> Option<()> {
    let mut seen = std::collections::BTreeSet::new();
    for field in fields {
        if field.identity.len() != 6 || !seen.insert(field.identity.as_slice()) {
            return None;
        }
        validate(&field.value)?;
    }
    Some(())
}

/// Stream one tagged V contribution; present optional layers vanish and absent depth is explicit.
fn emit(value: &Value, projection: Projection, output: &mut Vec<u8>) -> Option<()> {
    match value {
        Value::Absent | Value::Some(_) => match absence_depth(value) {
            Some(depth) => {
                output.push(0);
                unsigned(output, depth as u128);
            }
            None => {
                let Value::Some(item) = value else { return None };
                emit(item, projection, output)?;
            }
        },
        Value::Boolean(flag) => output.extend_from_slice(&[1, u8::from(*flag)]),
        Value::Integer(negative, magnitude) => {
            output.push(2);
            zigzag(output, *magnitude, *negative);
        }
        Value::Float(bits) => {
            output.push(3);
            output.extend_from_slice(bits);
        }
        Value::Decimal(coefficient, scale) => {
            let (coefficient, scale) = normalize_decimal(*coefficient, *scale)?;
            output.push(4);
            zigzag(output, coefficient.unsigned_abs(), coefficient < 0);
            zigzag(output, scale.unsigned_abs() as u128, scale < 0);
        }
        Value::Text(text) => {
            output.push(5);
            frame(output, text.as_bytes());
        }
        Value::Binary(data) => {
            output.push(6);
            frame(output, data.as_ref());
        }
        Value::Withheld => output.push(7),
        Value::List(items) => sequence(8, items, projection, output)?,
        Value::Tuple(items) => sequence(9, items, projection, output)?,
        Value::Set(items) => {
            output.push(10);
            unsigned(output, items.len() as u128);
            for index in order_values(items, projection)? {
                emit(&items[index], projection, output)?;
            }
        }
        Value::Map(entries) => {
            output.push(11);
            unsigned(output, entries.len() as u128);
            for index in order_entries(entries, projection)? {
                emit(&entries[index].0, projection, output)?;
                emit(&entries[index].1, projection, output)?;
            }
        }
        Value::Record(fields) => emit_record(fields, projection, output)?,
        Value::Enum(identity, fields) => {
            output.push(13);
            output.extend_from_slice(identity);
            emit_record(fields, projection, output)?;
        }
        Value::Union(member, item) => {
            output.push(14);
            frame(output, selector_spelling(member)?.as_bytes());
            emit(item, projection, output)?;
        }
        Value::Reference(target, identity) => {
            output.push(15);
            frame(output, target);
            frame(output, identity);
        }
    }
    Some(())
}

/// Emit an ordered sequence's tag, element count and elements.
fn sequence(tag: u8, items: &[Value], projection: Projection, output: &mut Vec<u8>) -> Option<()> {
    output.push(tag);
    unsigned(output, items.len() as u128);
    items.iter().try_for_each(|item| emit(item, projection, output))
}

/// Emit projected fields in identity order, each identity followed by its contribution.
fn emit_record(fields: &[Field], projection: Projection, output: &mut Vec<u8>) -> Option<()> {
    let indices = fields_in_order(fields, projection);
    output.push(12);
    unsigned(output, indices.len() as u128);
    for index in indices {
        output.extend_from_slice(&fields[index].identity);
        emit(&fields[index].value, projection, output)?;
    }
    Some(())
}

/// Only declared outer absence or collection emptiness omits a field, before recursive projection.
fn omitted(field: &Field, projection: Projection) -> bool {
    if projection == Projection::Canonical && !field.canonical {
        return true;
    }
    match &field.value {
        Value::Absent => field.absent == 'n',
        Value::List(items) | Value::Set(items) => field.absent == 'e' && items.is_empty(),
        Value::Map(entries) => field.absent == 'e' && entries.is_empty(),
        _ => false,
    }
}

/// Return contributing field indices by exact identity.
fn fields_in_order(fields: &[Field], projection: Projection) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..fields.len())
        .filter(|index| !omitted(&fields[*index], projection))
        .collect();
    indices.sort_by(|left, right| {
        fields[*left]
            .identity
            .cmp(&fields[*right].identity)
            .then(left.cmp(right))
    });
    indices
}

/// Count nested optional absence; a present value under any number of layers has none.
fn absence_depth(value: &Value) -> Option<u64> {
    match value {
        Value::Absent => Some(0),
        Value::Some(item) => absence_depth(item).map(|depth| depth + 1),
        _ => None,
    }
}

/// Stably sort set elements by stored comparison, keeping projected-equal elements distinct.
fn order_values(items: &[Value], projection: Projection) -> Option<Vec<usize>> {
    stable_order(items.len(), |left, right| {
        compare(&items[left], &items[right], projection)
    })
}

/// Stably sort map entries by full key comparison, then value.
fn order_entries(entries: &[(Value, Value)], projection: Projection) -> Option<Vec<usize>> {
    stable_order(entries.len(), |left, right| {
        match compare(&entries[left].0, &entries[right].0, projection)? {
            Ordering::Equal => compare(&entries[left].1, &entries[right].1, projection),
            ordering => Some(ordering),
        }
    })
}

/// Sort indices with a fallible comparator; any unrelated pair refuses the whole order.
fn stable_order(count: usize, compare: impl Fn(usize, usize) -> Option<Ordering>) -> Option<Vec<usize>> {
    let mut failed = false;
    let mut indices: Vec<usize> = (0..count).collect();
    indices.sort_by(|left, right| {
        compare(*left, *right).unwrap_or_else(|| {
            failed = true;
            Ordering::Equal
        })
    });
    (!failed).then_some(indices)
}

/// Compare complete values: absence first, then present values by type-specific stored order.
pub(super) fn compare(left: &Value, right: &Value, projection: Projection) -> Option<Ordering> {
    match (absence_depth(left), absence_depth(right)) {
        (Some(a), Some(b)) => return Some(a.cmp(&b)),
        (Some(_), None) => return Some(Ordering::Less),
        (None, Some(_)) => return Some(Ordering::Greater),
        _ => {}
    }
    if let Value::Some(item) = left {
        return compare(item, right, projection);
    }
    if let Value::Some(item) = right {
        return compare(left, item, projection);
    }
    match (left, right) {
        (Value::Boolean(a), Value::Boolean(b)) => Some(a.cmp(b)),
        (Value::Integer(a_negative, a), Value::Integer(b_negative, b)) => {
            Some(signed_order(*a_negative, *a, *b_negative, *b))
        }
        (Value::Float(a), Value::Float(b)) => Some(float_key(a).cmp(&float_key(b))),
        (Value::Decimal(a, a_scale), Value::Decimal(b, b_scale)) => compare_decimals(*a, *a_scale, *b, *b_scale),
        (Value::Text(a), Value::Text(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
        (Value::Binary(a), Value::Binary(b)) => Some(a.as_ref().cmp(b.as_ref())),
        (Value::Withheld, Value::Withheld) => Some(Ordering::Equal),
        (Value::List(a), Value::List(b)) | (Value::Tuple(a), Value::Tuple(b)) => compare_sequences(a, b, projection),
        (Value::Set(a), Value::Set(b)) => {
            let a_order = order_values(a, projection)?;
            let b_order = order_values(b, projection)?;
            let a_items: Vec<&Value> = a_order.iter().map(|index| &a[*index]).collect();
            let b_items: Vec<&Value> = b_order.iter().map(|index| &b[*index]).collect();
            compare_refs(&a_items, &b_items, projection)
        }
        (Value::Map(a), Value::Map(b)) => compare_maps(a, b, projection),
        (Value::Record(a), Value::Record(b)) => compare_fields(a, b, projection),
        (Value::Enum(a_id, a), Value::Enum(b_id, b)) => match a_id.cmp(b_id) {
            Ordering::Equal => compare_fields(a, b, projection),
            ordering => Some(ordering),
        },
        (Value::Union(a_type, a), Value::Union(b_type, b)) => match selector_key(a_type)?.cmp(&selector_key(b_type)?) {
            Ordering::Equal => compare(a, b, projection),
            ordering => Some(ordering),
        },
        (Value::Reference(a_type, a_id), Value::Reference(b_type, b_id)) => {
            Some(a_type.cmp(b_type).then(a_id.cmp(b_id)))
        }
        _ => None,
    }
}

/// Compare signed or unsigned magnitudes by mathematical value.
fn signed_order(a_negative: bool, a: u128, b_negative: bool, b: u128) -> Ordering {
    match (a_negative, b_negative) {
        (false, false) => a.cmp(&b),
        (true, true) => b.cmp(&a),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    }
}

/// Lexicographic sequence order; length decides only an equal prefix.
fn compare_sequences(left: &[Value], right: &[Value], projection: Projection) -> Option<Ordering> {
    let left: Vec<&Value> = left.iter().collect();
    let right: Vec<&Value> = right.iter().collect();
    compare_refs(&left, &right, projection)
}

/// Lexicographic order over borrowed elements.
fn compare_refs(left: &[&Value], right: &[&Value], projection: Projection) -> Option<Ordering> {
    for (a, b) in left.iter().zip(right) {
        let result = compare(a, b, projection)?;
        if result != Ordering::Equal {
            return Some(result);
        }
    }
    Some(left.len().cmp(&right.len()))
}

/// Compare projected fields by identity, then value, then contributing count.
fn compare_fields(left: &[Field], right: &[Field], projection: Projection) -> Option<Ordering> {
    let a_order = fields_in_order(left, projection);
    let b_order = fields_in_order(right, projection);
    for (a, b) in a_order.iter().zip(&b_order) {
        let (a, b) = (&left[*a], &right[*b]);
        let result = a.identity.cmp(&b.identity);
        if result != Ordering::Equal {
            return Some(result);
        }
        let contents = compare(&a.value, &b.value, projection)?;
        if contents != Ordering::Equal {
            return Some(contents);
        }
    }
    Some(a_order.len().cmp(&b_order.len()))
}

/// Compare ordered key/value pairs; projected key ties compare projected values.
fn compare_maps(left: &[(Value, Value)], right: &[(Value, Value)], projection: Projection) -> Option<Ordering> {
    let a_order = order_entries(left, projection)?;
    let b_order = order_entries(right, projection)?;
    for (a, b) in a_order.iter().zip(&b_order) {
        let (a, b) = (&left[*a], &right[*b]);
        let result = compare(&a.0, &b.0, projection)?;
        if result != Ordering::Equal {
            return Some(result);
        }
        let contents = compare(&a.1, &b.1, projection)?;
        if contents != Ordering::Equal {
            return Some(contents);
        }
    }
    Some(a_order.len().cmp(&b_order.len()))
}

/// Order nominal members by framed K, others by hashed N with a full spelling tie-break.
pub(super) fn selector_key(node: &Node) -> Option<(u8, Vec<u8>, String)> {
    match node {
        Node::Nominal(kind, ..) => Some((1, closed_selector(node)?, kind.clone())),
        _ => {
            let spelling = selector_spelling(node)?;
            Some((0, Sha256::digest(spelling.as_bytes()).to_vec(), spelling))
        }
    }
}

/// Frame a nominal identity, argument count and exact argument spellings.
pub(super) fn closed_selector(node: &Node) -> Option<Vec<u8>> {
    selector_spelling(node)?;
    let Node::Nominal(_, identity, arguments) = node else {
        return None;
    };
    let mut output = identity.clone();
    unsigned(&mut output, arguments.len() as u128);
    for argument in arguments {
        frame(&mut output, selector_spelling(argument)?.as_bytes());
    }
    Some(output)
}

/// Frame a closed selector while settling the same owned-output work as the public implementation.
pub(super) fn closed_selector_charged(
    node: &Node,
    budget: &mut super::columns::Budget,
) -> Result<Vec<u8>, super::columns::Refused> {
    charged_selector_spelling(node, budget)?;
    let Node::Nominal(_, identity, arguments) = node else {
        return Err("invalid_cell");
    };
    let mut output = Vec::new();
    append_charged(&mut output, identity, budget)?;
    let count = crate::crumb::encode_native_unsigned(arguments.len() as u128);
    budget.bytes(count.byte_charge)?;
    budget.elements(count.element_charge)?;
    append_charged(&mut output, &count.data, budget)?;
    for argument in arguments {
        let spelling = charged_selector_spelling(argument, budget)?;
        let framed = crate::crumb::frame(spelling.into_bytes());
        budget.bytes(framed.byte_charge)?;
        budget.elements(framed.element_charge)?;
        append_charged(&mut output, &framed.data, budget)?;
    }
    Ok(output)
}

/// Spell one selector while charging every recursive node and composed output fragment.
pub(super) fn charged_selector_spelling(
    node: &Node,
    budget: &mut super::columns::Budget,
) -> Result<String, super::columns::Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = charged_spelling_inner(node, budget);
    budget.leave();
    result
}

fn charged_spelling_inner(node: &Node, budget: &mut super::columns::Budget) -> Result<String, super::columns::Refused> {
    match node {
        Node::Scalar(kind) => {
            budget.bytes(kind.len() as u64)?;
            Ok(kind.clone())
        }
        Node::Decimal(..) => charged_composed(node, budget),
        Node::Container(kind, items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(charged_selector_spelling(item, budget)?);
            }
            if kind == "union" {
                for part in &parts {
                    budget.bytes(part.len() as u64)?;
                    budget.elements(1)?;
                }
            }
            charged_composed(node, budget)
        }
        Node::Nominal(kind, identity, arguments) => {
            budget.bytes((kind.len() + 1 + 2 * identity.len()) as u64)?;
            for argument in arguments {
                charged_selector_spelling(argument, budget)?;
            }
            if arguments.is_empty() {
                selector_spelling(node).ok_or("invalid_cell")
            } else {
                charged_composed(node, budget)
            }
        }
        Node::Reference(item) => {
            charged_selector_spelling(item, budget)?;
            charged_composed(node, budget)
        }
        Node::Record(..) | Node::Enum(..) | Node::Recursive(..) => Err("invalid_cell"),
    }
}

fn charged_composed(node: &Node, budget: &mut super::columns::Budget) -> Result<String, super::columns::Refused> {
    let spelling = selector_spelling(node).ok_or("invalid_cell")?;
    budget.bytes(spelling.len() as u64)?;
    Ok(spelling)
}

fn append_charged(
    output: &mut Vec<u8>,
    value: &[u8],
    budget: &mut super::columns::Budget,
) -> Result<(), super::columns::Refused> {
    budget.bytes(value.len() as u64)?;
    output.extend_from_slice(value);
    Ok(())
}

/// Transform normalized IEEE bits into bytewise numeric order, including negative zero and NaN.
fn float_key(bits: &[u8]) -> Vec<u8> {
    let negative = bits[0] >= 128;
    bits.iter()
        .enumerate()
        .map(|(index, byte)| match (negative, index) {
            (true, _) => 255 - byte,
            (false, 0) => byte.wrapping_add(128),
            _ => *byte,
        })
        .collect()
}

/// Return the unique trailing-zero-free coefficient and scale.
fn normalize_decimal(coefficient: i128, scale: i64) -> Option<(i128, i64)> {
    if coefficient == 0 {
        return Some((0, 0));
    }
    let (mut coefficient, mut scale) = (coefficient, scale);
    while coefficient % 10 == 0 {
        scale = scale.checked_sub(1)?;
        coefficient /= 10;
    }
    Some((coefficient, scale))
}

/// Compare exact decimals by aligning their digit strings at a common scale.
fn compare_decimals(left: i128, left_scale: i64, right: i128, right_scale: i64) -> Option<Ordering> {
    let (a, a_scale) = normalize_decimal(left, left_scale)?;
    let (b, b_scale) = normalize_decimal(right, right_scale)?;
    if (a < 0) != (b < 0) {
        return Some(if a < 0 { Ordering::Less } else { Ordering::Greater });
    }
    let common = a_scale.max(b_scale);
    let digits = |value: i128, scale: i64| {
        let mut text = value.unsigned_abs().to_string();
        if value != 0 {
            text.push_str(&"0".repeat((common - scale) as usize));
        }
        text
    };
    let (a_digits, b_digits) = (digits(a, a_scale), digits(b, b_scale));
    let magnitude = a_digits
        .len()
        .cmp(&b_digits.len())
        .then_with(|| a_digits.cmp(&b_digits));
    Some(if a < 0 { magnitude.reverse() } else { magnitude })
}

/// Return the selector spelling N; bound definitions never expand into it.
pub(super) fn selector_spelling(node: &Node) -> Option<String> {
    spell(node, true, &mut Vec::new())
}

/// Return the structural spelling S; a bare nominal selector supplies no shape.
pub(super) fn shape_spelling(node: &Node) -> Option<String> {
    spell(node, false, &mut Vec::new())
}

/// Spell one node as N (nominal) or S (structural) under the active definition stack.
fn spell(node: &Node, nominal: bool, active: &mut Vec<String>) -> Option<String> {
    match node {
        Node::Scalar(kind) => Some(kind.clone()),
        Node::Decimal(precision, scale) => (*precision > 0 && *precision <= 38 && *scale >= 0 && scale <= precision)
            .then(|| format!("decimal<{precision},{scale}>")),
        Node::Container(kind, items) => container(kind, items, nominal, active),
        Node::Nominal(kind, identity, arguments) => {
            if !nominal || identity.len() != 6 {
                return None;
            }
            let stem = format!("{kind}@{}", hex_string(identity));
            if arguments.is_empty() {
                return Some(stem);
            }
            let parts: Option<Vec<String>> = arguments
                .iter()
                .map(|argument| spell(argument, true, &mut Vec::new()))
                .collect();
            Some(format!("{stem}<{}>", parts?.join(",")))
        }
        Node::Record(key, fields) => {
            enter(key, active)?;
            let result = record_fields(fields, nominal, active);
            active.pop();
            result
        }
        Node::Enum(key, variants) => {
            enter(key, active)?;
            let result = variant_fields(variants, nominal, active);
            active.pop();
            result
        }
        Node::Reference(item) => {
            if nominal {
                Some(format!("ref<{}>", spell(item, true, active)?))
            } else {
                Some("reference".to_string())
            }
        }
        Node::Recursive(key) => {
            let offset = active.iter().rev().position(|item| item == key)?;
            Some(format!("^{offset}"))
        }
    }
}

/// Push one active definition, refusing a duplicate on the current path.
fn enter(key: &str, active: &mut Vec<String>) -> Option<()> {
    if active.iter().any(|item| item == key) {
        return None;
    }
    active.push(key.to_string());
    Some(())
}

/// Validate constructor arity and sort unions by hashed spelling with full tie-breaks.
fn container(kind: &str, items: &[Node], nominal: bool, active: &mut Vec<String>) -> Option<String> {
    let arity = match kind {
        "list" | "set" | "option" => items.len() == 1,
        "map" => items.len() == 2,
        "union" => !items.is_empty(),
        "tuple" => true,
        _ => false,
    };
    if !arity {
        return None;
    }
    let mut parts = items
        .iter()
        .map(|item| spell(item, nominal, active))
        .collect::<Option<Vec<String>>>()?;
    if kind == "union" {
        let mut keyed = Vec::with_capacity(parts.len());
        for part in parts {
            if nominal && keyed.iter().any(|(_, seen)| seen == &part) {
                return None;
            }
            keyed.push((hex_string(&Sha256::digest(part.as_bytes())), part));
        }
        keyed.sort();
        parts = keyed.into_iter().map(|(_, part)| part).collect();
    }
    Some(format!("{kind}<{}>", parts.join(",")))
}

/// Spell identity-sorted fields with their canonical flag and absence code.
fn record_fields(fields: &[Shape], nominal: bool, active: &mut Vec<String>) -> Option<String> {
    let mut ordered: Vec<(String, &Shape)> = Vec::with_capacity(fields.len());
    for field in fields {
        let identity = hex_string(&field.identity);
        if field.identity.len() != 6 || ordered.iter().any(|(seen, _)| seen == &identity) {
            return None;
        }
        ordered.push((identity, field));
    }
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut parts = Vec::with_capacity(ordered.len());
    for (identity, field) in ordered {
        let spelling = spell(&field.storage, nominal, active)?;
        let canonical = if field.canonical { '1' } else { '0' };
        parts.push(format!("{identity}:{spelling}:{canonical}:{}", field.absent));
    }
    Some(format!("record{{{}}}", parts.join(";")))
}

/// Spell identity-sorted variants with their payload records.
fn variant_fields(variants: &[(Vec<u8>, Vec<Shape>)], nominal: bool, active: &mut Vec<String>) -> Option<String> {
    let mut ordered: Vec<(String, &Vec<Shape>)> = Vec::with_capacity(variants.len());
    for (identity, fields) in variants {
        let name = hex_string(identity);
        if identity.len() != 6 || ordered.iter().any(|(seen, _)| seen == &name) {
            return None;
        }
        ordered.push((name, fields));
    }
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut parts = Vec::with_capacity(ordered.len());
    for (identity, fields) in ordered {
        parts.push(format!("{identity}:{}", record_fields(fields, nominal, active)?));
    }
    Some(format!("enum{{{}}}", parts.join(";")))
}

/// Encode one tagged metadata node, sorting unique property-name map keys by UTF-8 bytes.
fn metadata(value: &Json, output: &mut Vec<u8>) -> Option<()> {
    match value.get("kind")?.as_str()? {
        "null" => output.push(0),
        "boolean" => output.extend_from_slice(&[1, u8::from(value.get("value")?.as_bool()?)]),
        "int" => {
            let number: i128 = value.get("value")?.as_str()?.parse().ok()?;
            output.push(2);
            zigzag(output, number.unsigned_abs(), number < 0);
        }
        "string" => {
            output.push(3);
            frame(output, value.get("value")?.as_str()?.as_bytes());
        }
        "binary" => {
            output.push(4);
            frame(output, &hex(value.get("hex")?)?);
        }
        "symbol" => {
            output.push(7);
            frame(output, value.get("name")?.as_str()?.as_bytes());
        }
        "list" => {
            let items = value.get("items")?.as_array()?;
            output.push(5);
            unsigned(output, items.len() as u128);
            items.iter().try_for_each(|item| metadata(item, output))?;
        }
        "map" => metadata_map(value.get("entries")?.as_array()?, output)?,
        _ => return None,
    }
    Some(())
}

/// Refuse duplicate or non-name keys before emitting the sorted property table.
fn metadata_map(entries: &[Json], output: &mut Vec<u8>) -> Option<()> {
    let mut keyed: Vec<(&str, &Json)> = Vec::with_capacity(entries.len());
    for entry in entries {
        let pair = entry.as_array()?;
        if pair.len() != 2 || pair[0].get("kind")?.as_str()? != "string" {
            return None;
        }
        let key = pair[0].get("value")?.as_str()?;
        if !property_name(key) || keyed.iter().any(|(seen, _)| *seen == key) {
            return None;
        }
        keyed.push((key, &pair[1]));
    }
    keyed.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    output.push(6);
    unsigned(output, keyed.len() as u128);
    for (key, item) in keyed {
        frame(output, key.as_bytes());
        metadata(item, output)?;
    }
    Some(())
}

/// Admit one identifier-shaped property name: a letter or underscore, then letters, digits or underscores.
fn property_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_')
        && characters.all(|character| character.is_alphanumeric() || character == '_')
}

/// Append one shortest base-128 unsigned count.
pub(super) fn unsigned(output: &mut Vec<u8>, mut value: u128) {
    loop {
        let mut group = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            group |= 0x80;
        }
        output.push(group);
        if value == 0 {
            return;
        }
    }
}

/// Append one Z integer: twice a nonnegative value, or twice a negative magnitude minus one, over 129 bits.
pub(super) fn zigzag(output: &mut Vec<u8>, magnitude: u128, negative: bool) {
    let mut high = magnitude >> 127;
    let mut low = magnitude << 1;
    if negative && magnitude != 0 {
        if low == 0 {
            high -= 1;
            low = u128::MAX;
        } else {
            low -= 1;
        }
    }
    loop {
        let mut group = (low & 0x7f) as u8;
        low = (low >> 7) | (high << 121);
        high = 0;
        if low != 0 {
            group |= 0x80;
        }
        output.push(group);
        if low == 0 {
            return;
        }
    }
}

/// Append one shortest length and the bytes it frames.
pub(super) fn frame(output: &mut Vec<u8>, data: &[u8]) {
    unsigned(output, data.len() as u128);
    output.extend_from_slice(data);
}

/// Spell octets as lowercase hexadecimal.
pub(super) fn hex_string(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Append one value's object contribution without validating it again.
pub(super) fn emit_object(value: &Value, output: &mut Vec<u8>) -> Option<()> {
    emit(value, Projection::Object, output)
}

/// Append one object contribution while replaying `append_seal`'s count and hash traversals on the caller's budget.
pub(super) fn emit_object_charged(
    value: &Value,
    output: &mut Vec<u8>,
    budget: &mut super::columns::Budget,
) -> Result<(), super::columns::Refused> {
    let start = output.len();
    emit_object(value, output).ok_or("invalid_cell")?;
    let length = output.len() - start;
    charge_emit(value, budget)?;
    budget.bytes(length as u64)?;
    charge_unsigned(length, budget)?;
    charge_emit(value, budget)?;
    budget.bytes(length as u64)
}

/// Charge the seal domain and record count written by `start_seal`.
pub(super) fn charge_seal_start(
    count: usize,
    budget: &mut super::columns::Budget,
) -> Result<(), super::columns::Refused> {
    budget.bytes(8)?;
    charge_unsigned(count, budget)
}

/// Charge the independent digest returned by `HashOutput.digest`.
pub(super) fn charge_seal_digest(budget: &mut super::columns::Budget) -> Result<(), super::columns::Refused> {
    budget.bytes(32)
}

/// Charge a shortest unsigned value once to encode it and once to write it.
fn charge_unsigned(value: usize, budget: &mut super::columns::Budget) -> Result<(), super::columns::Refused> {
    let encoded = crate::crumb::encode_native_unsigned(value as u128);
    budget.elements(encoded.element_charge)?;
    budget.bytes(encoded.byte_charge + encoded.data.len() as u64)
}

/// Replay one value traversal's depth, work and retained ordering keys without rebuilding its bytes.
fn charge_emit(value: &Value, budget: &mut super::columns::Budget) -> Result<(), super::columns::Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = match value {
        Value::Some(item) | Value::Union(_, item) => charge_emit(item, budget),
        Value::List(items) | Value::Tuple(items) | Value::Set(items) => {
            items.iter().try_for_each(|item| charge_emit(item, budget))
        }
        Value::Map(entries) => entries.iter().try_for_each(|(key, value)| {
            charge_emit(key, budget)?;
            charge_emit(value, budget)
        }),
        Value::Integer(negative, magnitude) => {
            let encoded = if *negative {
                let signed = if *magnitude == 1u128 << 127 {
                    i128::MIN
                } else {
                    -i128::try_from(*magnitude).map_err(|_| "invalid_cell")?
                };
                crate::crumb::encode_native_signed(signed)
            } else {
                crate::crumb::encode_integer_unsigned(*magnitude)
            };
            charge_encoded(encoded, budget)
        }
        Value::Record(fields) | Value::Enum(_, fields) => {
            charge_encoded(crate::crumb::encode_native_unsigned(fields.len() as u128), budget)?;
            fields.iter().try_for_each(|field| {
                budget.elements(1)?;
                budget.bytes(28)?;
                charge_emit(&field.value, budget)
            })
        }
        _ => Ok(()),
    };
    budget.leave();
    result
}

fn charge_encoded(
    encoded: crate::crumb::Encoded,
    budget: &mut super::columns::Budget,
) -> Result<(), super::columns::Refused> {
    budget.bytes(encoded.byte_charge)?;
    budget.elements(encoded.element_charge)
}
