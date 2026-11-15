//! The native row set behind an admitted envelope: records, single fields, field cells and written descriptions,
//! read on request under the caller's remaining allowance and handed to Incan in its carriers' own shapes.
//!
//! `row_reader.RowSet` reads a record or field again from the admitted bytes under the caller's budget; so does
//! each method here, charging only the allowance it is given and positioning any refusal as the Incan reader does.
//! A materialized value, written description or union selector arrives as a `RowRead` cursor over its nodes in
//! pre-order, from which Incan builds `Value`, `Metadata` and `TypeNode` trees without decoding any envelope bytes.

use super::columns::{self, Budget};
use super::hash::{self, Node, Value};
use super::row_cells::{self, read_selected, select_field};
use super::row_cursor::{Place, Positioned, Refusal, at};
use super::row_entry::RowAdmission;
use super::rows::{Meta, Stored, StoredField};

/// One node of a materialized value, written description or selector, in its Incan carrier's shape.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    NoneValue,
    Some,
    Boolean(bool),
    Signed(i128),
    Unsigned(u128),
    Float(i64, Vec<u8>),
    Decimal(i128, i64),
    Text(String),
    Binary(Vec<u8>),
    Withheld,
    List(usize),
    Tuple(usize),
    Set(usize),
    Map(usize),
    Record(usize),
    Enum(Vec<u8>, usize),
    Union,
    Reference(Vec<u8>, Vec<u8>),
    Field(Vec<u8>, bool, char),
    Scalar(String),
    DecimalType(i64, i64),
    Container(String, usize),
    Nominal(String, Vec<u8>, usize),
    ReferenceType(usize),
    Null,
    Flag(bool),
    Integer(i128),
    String(String),
    Bytes(Vec<u8>),
    Symbol(String),
    Items(usize),
    Entries(usize),
    Key(String),
}

impl Item {
    /// The node's code, as `RowRead.next` reports it.
    fn code(&self) -> i64 {
        match self {
            Item::NoneValue => 0,
            Item::Some => 1,
            Item::Boolean(_) => 2,
            Item::Signed(_) => 3,
            Item::Unsigned(_) => 4,
            Item::Float(..) => 5,
            Item::Decimal(..) => 6,
            Item::Text(_) => 7,
            Item::Binary(_) => 8,
            Item::Withheld => 9,
            Item::List(_) => 10,
            Item::Tuple(_) => 11,
            Item::Set(_) => 12,
            Item::Map(_) => 13,
            Item::Record(_) => 14,
            Item::Enum(..) => 15,
            Item::Union => 16,
            Item::Reference(..) => 17,
            Item::Field(..) => 18,
            Item::Scalar(_) => 19,
            Item::DecimalType(..) => 20,
            Item::Container(..) => 21,
            Item::Nominal(..) => 22,
            Item::ReferenceType(_) => 23,
            Item::Null => 24,
            Item::Flag(_) => 25,
            Item::Integer(_) => 26,
            Item::String(_) => 27,
            Item::Bytes(_) => 28,
            Item::Symbol(_) => 29,
            Item::Items(_) => 30,
            Item::Entries(_) => 31,
            Item::Key(_) => 32,
        }
    }
}

/// One read from an admitted row set: a positioned refusal, or the nodes of what was read, and the allowance used.
///
/// Node codes: 0 none, 1 some (one value follows), 2 boolean, 3 signed and 4 unsigned integer, 5 float (width and
/// big-endian bits), 6 decimal (coefficient and scale), 7 text, 8 binary, 9 withheld, 10 list, 11 tuple and 12 set
/// (a count of values follows), 13 map (a count of key and value pairs), 14 record and 15 enum (an identity and a count
/// of fields), 16 union (a selector and one value follow), 17 reference (target and identity), 18 field (identity,
/// canonical flag and absent policy; one value follows); selectors 19 scalar, 20 decimal (precision and scale), 21
/// container (kind and argument count), 22 nominal (kind, identity and argument count), 23 reference (one argument);
/// metadata 24 null, 25 boolean, 26 integer, 27 text, 28 binary, 29 symbol, 30 list (a count of items), 31 map (a
/// count of entries, each a 32 key and one item).
#[derive(Clone)]
pub struct RowRead {
    /// Empty when the read succeeded; otherwise the refusal category as `BinaryFailure` spells it.
    pub category: String,
    /// The refusal's envelope offset.
    pub offset: i64,
    /// The stored field identity a refusal is in, when known.
    pub field: Option<Vec<u8>>,
    /// The enum variant identity a refusal is in, when known.
    pub variant: Option<Vec<u8>>,
    /// The exhausted caller-owned resource when a budget charge caused the refusal.
    pub cause: String,
    /// Bytes of the allowance this read used.
    pub bytes_charged: i64,
    /// Elements of the allowance this read used.
    pub elements_charged: i64,
    /// For a field cell: whether the field is present, and its cell's envelope offset and length.
    pub present: bool,
    pub origin: i64,
    pub length: i64,
    /// For a written description: whether it was written inline.
    pub inline: bool,
    /// For a reference description: the position of the supplied description it resolved to; otherwise -1.
    pub supplied: i64,
    items: Vec<Item>,
    position: usize,
}

impl std::fmt::Debug for RowRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RowRead")
            .field("category", &self.category)
            .field("offset", &self.offset)
            .field("nodes", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl RowRead {
    /// Whether the read succeeded.
    pub fn succeeded(&self) -> bool {
        self.category.is_empty()
    }

    /// Advance to the next node and return its code; -1 after the last node.
    pub fn next(&mut self) -> i64 {
        self.position += 1;
        self.current().map_or(-1, Item::code)
    }

    /// The count of a collection, record, enum, selector argument list or metadata list or map.
    pub fn count(&self) -> i64 {
        let count = match self.current() {
            Some(
                Item::List(count)
                | Item::Tuple(count)
                | Item::Set(count)
                | Item::Map(count)
                | Item::Record(count)
                | Item::Enum(_, count)
                | Item::Container(_, count)
                | Item::Nominal(_, _, count)
                | Item::ReferenceType(count)
                | Item::Items(count)
                | Item::Entries(count),
            ) => *count,
            _ => 0,
        };
        i64::try_from(count).unwrap_or(i64::MAX)
    }

    /// A boolean, a field's canonical flag, or a metadata boolean.
    pub fn flag(&self) -> bool {
        matches!(
            self.current(),
            Some(Item::Boolean(true) | Item::Field(_, true, _) | Item::Flag(true))
        )
    }

    /// A signed integer, a decimal coefficient, or a metadata integer.
    pub fn signed(&self) -> i128 {
        match self.current() {
            Some(Item::Signed(value) | Item::Decimal(value, _) | Item::Integer(value)) => *value,
            _ => 0,
        }
    }

    /// An unsigned integer.
    pub fn unsigned(&self) -> u128 {
        match self.current() {
            Some(Item::Unsigned(value)) => *value,
            _ => 0,
        }
    }

    /// A float's width in bits, a decimal's scale, or a decimal selector's precision.
    pub fn number(&self) -> i64 {
        match self.current() {
            Some(Item::Float(width, _)) => *width,
            Some(Item::Decimal(_, scale)) => *scale,
            Some(Item::DecimalType(precision, _)) => *precision,
            _ => 0,
        }
    }

    /// A decimal selector's scale.
    pub fn second(&self) -> i64 {
        match self.current() {
            Some(Item::DecimalType(_, scale)) => *scale,
            _ => 0,
        }
    }

    /// A float's bits, binary data, an enum, field or nominal identity, a reference target, or metadata binary.
    pub fn bytes(&self) -> Vec<u8> {
        match self.current() {
            Some(
                Item::Float(_, data)
                | Item::Binary(data)
                | Item::Enum(data, _)
                | Item::Reference(data, _)
                | Item::Field(data, _, _)
                | Item::Nominal(_, data, _)
                | Item::Bytes(data),
            ) => data.clone(),
            _ => Vec::new(),
        }
    }

    /// A reference's identity.
    pub fn other(&self) -> Vec<u8> {
        match self.current() {
            Some(Item::Reference(_, identity)) => identity.clone(),
            _ => Vec::new(),
        }
    }

    /// Text, a scalar or container selector's name, a nominal selector's kind, a field's absent policy
    /// (`required`, `none` or `empty`), or metadata text, symbol or map key.
    pub fn text(&self) -> String {
        match self.current() {
            Some(
                Item::Text(text)
                | Item::Scalar(text)
                | Item::Container(text, _)
                | Item::Nominal(text, _, _)
                | Item::String(text)
                | Item::Symbol(text)
                | Item::Key(text),
            ) => text.clone(),
            Some(Item::Field(_, _, absent)) => absent_policy(*absent).to_string(),
            _ => String::new(),
        }
    }

    /// The node `next` last reached.
    fn current(&self) -> Option<&Item> {
        self.position
            .checked_sub(1)
            .and_then(|position| self.items.get(position))
    }
}

/// The policy name `AbsentPolicy` spells.
fn absent_policy(absent: char) -> &'static str {
    match absent {
        'n' => "none",
        'e' => "empty",
        _ => "required",
    }
}

/// A read's result from its outcome and the allowance it used.
fn finish(result: Positioned<RowRead>, allowance: (u64, u64), budget: &Budget) -> RowRead {
    let (bytes_left, elements_left) = budget.left();
    let charged = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
    let mut read = result.unwrap_or_else(refused);
    read.bytes_charged = charged(allowance.0 - bytes_left);
    read.elements_charged = charged(allowance.1 - elements_left);
    if read.category == "limit_exceeded" {
        read.cause = budget.cause().unwrap_or_default().to_string();
    }
    read
}

/// An empty successful read with these nodes.
fn items(items: Vec<Item>) -> RowRead {
    RowRead {
        category: String::new(),
        offset: 0,
        field: None,
        variant: None,
        cause: String::new(),
        bytes_charged: 0,
        elements_charged: 0,
        present: false,
        origin: 0,
        length: 0,
        inline: false,
        supplied: -1,
        items,
        position: 0,
    }
}

/// A refused read.
fn refused(refusal: Refusal) -> RowRead {
    let mut read = items(Vec::new());
    read.category = refusal.category.to_string();
    read.offset = i64::try_from(refusal.offset).unwrap_or(i64::MAX);
    read.field = refusal.field;
    read.variant = refusal.variant;
    read
}

/// A budget of the caller's remaining allowance; a negative amount is none.
fn allowance(bytes: i64, elements: i64, depth: i64) -> ((u64, u64), Budget) {
    let amount = |value: i64| u64::try_from(value).unwrap_or(0);
    let (bytes, elements) = (amount(bytes), amount(elements));
    ((bytes, elements), Budget::new(bytes, elements, amount(depth)))
}

impl RowAdmission {
    /// The record's written key K; empty for an index past the set.
    pub fn key(&self, index: i64) -> Vec<u8> {
        self.row(index)
            .map(|(rows, record)| rows.dictionary[record.slot].key.clone())
            .unwrap_or_default()
    }

    /// Materialize one record again from the admitted bytes, as `RowSet.read_record` does.
    pub fn read_record(&self, index: i64, bytes: i64, elements: i64, depth: i64) -> RowRead {
        let (limits, mut budget) = allowance(bytes, elements, depth);
        let result = self.record_items(index, &mut budget);
        finish(result, limits, &budget)
    }

    /// One field's encoded cell, as `RowSet.field_view` does; a declared absence is not present.
    pub fn field_cell(&self, index: i64, identity: Vec<u8>, bytes: i64, elements: i64, depth: i64) -> RowRead {
        let (limits, mut budget) = allowance(bytes, elements, depth);
        let result = self.row(index).and_then(|(rows, record)| {
            let (entry, raw) = (&rows.dictionary[record.slot], self.raw(record));
            let selected = select_field(entry, raw, record.origin, &identity, &mut budget)?;
            let mut read = items(Vec::new());
            if let Some((view, origin)) = selected.cell {
                read.present = true;
                read.origin = i64::try_from(origin).unwrap_or(i64::MAX);
                read.length = i64::try_from(view.len()).unwrap_or(i64::MAX);
            }
            Ok(read)
        });
        finish(result, limits, &budget)
    }

    /// Materialize one field's final stored value, as `RowSet.read_field` does.
    pub fn read_field(&self, index: i64, identity: Vec<u8>, bytes: i64, elements: i64, depth: i64) -> RowRead {
        let (limits, mut budget) = allowance(bytes, elements, depth);
        let result = self.row(index).and_then(|(rows, record)| {
            let (entry, raw) = (&rows.dictionary[record.slot], self.raw(record));
            let selected = select_field(entry, raw, record.origin, &identity, &mut budget)?;
            let value = read_selected(&selected, &identity, &rows.dictionary, &mut budget)?;
            let mut nodes = Vec::new();
            flatten(&selected.field.storage, &value, &rows.dictionary, &mut nodes).place(record.origin)?;
            Ok(items(nodes))
        });
        finish(result, limits, &budget)
    }

    /// The number of written descriptions.
    pub fn description_count(&self) -> i64 {
        self.rows
            .as_ref()
            .map_or(0, |rows| i64::try_from(rows.dictionary.len()).unwrap_or(i64::MAX))
    }

    /// One complete written description in dictionary order, charged as `RowSet.descriptions` charges its copy.
    pub fn description(&self, index: i64, bytes: i64, elements: i64, depth: i64) -> RowRead {
        let (limits, mut budget) = allowance(bytes, elements, depth);
        let result = self
            .rows
            .as_ref()
            .and_then(|rows| rows.dictionary.get(usize::try_from(index).ok()?))
            .ok_or_else(|| at("record_index", 0))
            .and_then(|entry| {
                charge_copy(&entry.metadata, &mut budget).place(0)?;
                let mut nodes = Vec::new();
                flatten_metadata(&entry.metadata, &mut nodes);
                let mut read = items(nodes);
                read.inline = entry.inline;
                read.supplied = entry
                    .supplied
                    .map_or(-1, |position| i64::try_from(position).unwrap_or(-1));
                Ok(read)
            });
        finish(result, limits, &budget)
    }

    /// The admitted rows and one record of them; an index past the set is `record_index` at 0.
    fn row(&self, index: i64) -> Positioned<(&super::rows::Rows, &super::rows::Record)> {
        let rows = self.rows.as_ref().ok_or_else(|| at("record_index", 0))?;
        let record = usize::try_from(index)
            .ok()
            .and_then(|index| rows.records.get(index))
            .ok_or_else(|| at("record_index", 0))?;
        Ok((rows, record))
    }

    /// A record's row frame within the admitted bytes.
    fn raw(&self, record: &super::rows::Record) -> &[u8] {
        &self.data[record.origin..record.origin + record.length]
    }

    /// Read a record again under `budget`, then charge its key as `RowSet.read_record` does.
    fn record_items(&self, index: i64, budget: &mut Budget) -> Positioned<RowRead> {
        let (rows, record) = self.row(index)?;
        let entry = &rows.dictionary[record.slot];
        let value = row_cells::read_root(entry, self.raw(record), record.origin, &rows.dictionary, budget)?;
        budget.bytes(entry.key.len() as u64).place(record.origin)?;
        let mut nodes = Vec::new();
        flatten_root(entry, &value, &rows.dictionary, &mut nodes).place(record.origin)?;
        Ok(items(nodes))
    }
}

/// Charge one complete metadata copy as `metadata_values.charge_copy` does.
fn charge_copy(value: &Meta, budget: &mut Budget) -> Result<(), columns::Refused> {
    budget.elements(1)?;
    budget.enter()?;
    let result = charge_inner(value, budget);
    budget.leave();
    result
}

/// Charge one metadata node's retained storage.
fn charge_inner(value: &Meta, budget: &mut Budget) -> Result<(), columns::Refused> {
    match value {
        Meta::Null | Meta::Boolean(_) => budget.bytes(1),
        Meta::Integer(_) => budget.bytes(16),
        Meta::Text(text) | Meta::Symbol(text) => charge_text(text, budget),
        Meta::Binary(data) => budget.bytes(data.len() as u64),
        Meta::List(children) => children.iter().try_for_each(|child| charge_copy(child, budget)),
        Meta::Map(entries) => entries.iter().try_for_each(|(key, child)| {
            budget.elements(1)?;
            charge_text(key, budget)?;
            charge_copy(child, budget)
        }),
    }
}

/// Reserve a UTF-8 upper bound for one retained name or text, as `metadata_values.charge_text` does.
fn charge_text(text: &str, budget: &mut Budget) -> Result<(), columns::Refused> {
    (0..4).try_for_each(|_| budget.bytes(text.len() as u64))
}

/// Flatten one root record or enum value through its written description.
fn flatten_root(entry: &Stored, value: &Value, dictionary: &[Stored], out: &mut Vec<Item>) -> Result<(), &'static str> {
    match value {
        Value::Record(fields) if entry.record => {
            out.push(Item::Record(fields.len()));
            flatten_fields(&entry.fields, fields, dictionary, out)
        }
        Value::Enum(identity, fields) if !entry.record => {
            let variant = entry
                .variants
                .iter()
                .find(|variant| &variant.identity == identity)
                .ok_or("invalid_cell")?;
            out.push(Item::Enum(identity.clone(), fields.len()));
            flatten_fields(&variant.fields, fields, dictionary, out)
        }
        _ => Err("invalid_cell"),
    }
}

/// Flatten stored fields in identity order, each through its written storage type.
fn flatten_fields(
    shape: &[StoredField],
    fields: &[hash::Field],
    dictionary: &[Stored],
    out: &mut Vec<Item>,
) -> Result<(), &'static str> {
    if shape.len() != fields.len() {
        return Err("invalid_cell");
    }
    for (stored, field) in shape.iter().zip(fields) {
        out.push(Item::Field(field.identity.clone(), field.canonical, field.absent));
        flatten(&stored.storage, &field.value, dictionary, out)?;
    }
    Ok(())
}

/// Flatten one value through its written storage type, which decides integer signedness and float width.
fn flatten(node: &Node, value: &Value, dictionary: &[Stored], out: &mut Vec<Item>) -> Result<(), &'static str> {
    match (node, value) {
        (Node::Scalar(kind), _) => out.push(scalar(kind, value)?),
        (Node::Decimal(..), Value::Decimal(coefficient, scale)) => out.push(Item::Decimal(*coefficient, *scale)),
        (Node::Reference(_), Value::Reference(target, identity)) => {
            out.push(Item::Reference(target.clone(), identity.clone()))
        }
        (Node::Nominal(..), _) => {
            let key = hash::closed_selector(node).ok_or("invalid_selector")?;
            let entry = dictionary
                .iter()
                .find(|entry| entry.key == key)
                .ok_or("invalid_selector")?;
            flatten_root(entry, value, dictionary, out)?;
        }
        (Node::Container(kind, arguments), _) => container(kind, arguments, value, dictionary, out)?,
        _ => return Err("invalid_cell"),
    }
    Ok(())
}

/// One scalar node in its Incan carrier's shape.
fn scalar(kind: &str, value: &Value) -> Result<Item, &'static str> {
    if let Some(columns::KeyType::Integer(signed, _)) = columns::key_type(kind) {
        let Value::Integer(negative, magnitude) = value else {
            return Err("invalid_cell");
        };
        if !signed {
            return Ok(Item::Unsigned(*magnitude));
        }
        let signed = *magnitude as i128;
        return Ok(Item::Signed(if *negative { signed.wrapping_neg() } else { signed }));
    }
    Ok(match (kind, value) {
        ("fp64", Value::Float(bits)) => Item::Float(64, bits.clone()),
        ("fp32", Value::Float(bits)) => {
            let wide = f64::from_be_bytes(bits.as_slice().try_into().map_err(|_| "invalid_cell")?);
            let narrow = if wide.is_nan() {
                0x7fc0_0000
            } else {
                (wide as f32).to_bits()
            };
            Item::Float(32, narrow.to_be_bytes().to_vec())
        }
        ("boolean", Value::Boolean(flag)) => Item::Boolean(*flag),
        ("string", Value::Text(text)) => Item::Text(text.clone()),
        ("binary", Value::Binary(data)) => Item::Binary(data.clone()),
        ("secret_string" | "secret_bytes", Value::Withheld) => Item::Withheld,
        ("reference", Value::Reference(target, identity)) => Item::Reference(target.clone(), identity.clone()),
        _ => return Err("invalid_cell"),
    })
}

/// Flatten an option, union, list, tuple, set or map value through its element types.
fn container(
    kind: &str,
    arguments: &[Node],
    value: &Value,
    dictionary: &[Stored],
    out: &mut Vec<Item>,
) -> Result<(), &'static str> {
    match (kind, value) {
        ("option", Value::Absent) => out.push(Item::NoneValue),
        ("option", Value::Some(child)) => {
            out.push(Item::Some);
            flatten(arguments.first().ok_or("invalid_selector")?, child, dictionary, out)?;
        }
        ("union", Value::Union(candidate, child)) => {
            out.push(Item::Union);
            selector(candidate, out)?;
            flatten(candidate, child, dictionary, out)?;
        }
        ("list" | "set", Value::List(values) | Value::Set(values)) => {
            out.push(if kind == "list" {
                Item::List(values.len())
            } else {
                Item::Set(values.len())
            });
            let element = arguments.first().ok_or("invalid_selector")?;
            values
                .iter()
                .try_for_each(|value| flatten(element, value, dictionary, out))?;
        }
        ("tuple", Value::Tuple(values)) if values.len() == arguments.len() => {
            out.push(Item::Tuple(values.len()));
            arguments
                .iter()
                .zip(values)
                .try_for_each(|(element, value)| flatten(element, value, dictionary, out))?;
        }
        ("map", Value::Map(entries)) if arguments.len() == 2 => {
            out.push(Item::Map(entries.len()));
            for (key, value) in entries {
                flatten(&arguments[0], key, dictionary, out)?;
                flatten(&arguments[1], value, dictionary, out)?;
            }
        }
        _ => return Err("invalid_cell"),
    }
    Ok(())
}

/// Flatten a written union member's selector as `selectors.parse_type` builds its `TypeNode`.
fn selector(node: &Node, out: &mut Vec<Item>) -> Result<(), &'static str> {
    match node {
        Node::Scalar(name) => out.push(Item::Scalar(name.clone())),
        Node::Decimal(precision, scale) => out.push(Item::DecimalType(*precision, *scale)),
        Node::Container(kind, arguments) => {
            out.push(Item::Container(kind.clone(), arguments.len()));
            arguments.iter().try_for_each(|argument| selector(argument, out))?;
        }
        Node::Nominal(kind, identity, arguments) => {
            out.push(Item::Nominal(kind.clone(), identity.clone(), arguments.len()));
            arguments.iter().try_for_each(|argument| selector(argument, out))?;
        }
        Node::Reference(inner) => {
            out.push(Item::ReferenceType(1));
            selector(inner, out)?;
        }
        _ => return Err("invalid_selector"),
    }
    Ok(())
}

/// Flatten one complete metadata tree, keeping every map entry in read order.
fn flatten_metadata(value: &Meta, out: &mut Vec<Item>) {
    match value {
        Meta::Null => out.push(Item::Null),
        Meta::Boolean(flag) => out.push(Item::Flag(*flag)),
        Meta::Integer(number) => out.push(Item::Integer(*number)),
        Meta::Text(text) => out.push(Item::String(text.clone())),
        Meta::Binary(data) => out.push(Item::Bytes(data.clone())),
        Meta::Symbol(name) => out.push(Item::Symbol(name.clone())),
        Meta::List(children) => {
            out.push(Item::Items(children.len()));
            children.iter().for_each(|child| flatten_metadata(child, out));
        }
        Meta::Map(entries) => {
            out.push(Item::Entries(entries.len()));
            for (key, child) in entries {
                out.push(Item::Key(key.clone()));
                flatten_metadata(child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::row_entry::RowRequest;
    use super::super::rows;
    use super::*;

    const IDENTITY: [u8; 6] = [0x01, 0x3f, 0x54, 0x40, 0x0c, 0x82];

    /// The corpus's one-field reference description `example.Value`, with this field type.
    fn described(storage: &str) -> Vec<u8> {
        let notation = format!(
            r#"{{"kind":"map","entries":[[{{"kind":"string","value":"nominal"}},{{"kind":"binary","hex":"1ddb0b3ce6d300"}}],[{{"kind":"string","value":"kind"}},{{"kind":"string","value":"Record"}}],[{{"kind":"string","value":"path"}},{{"kind":"string","value":"example.Value"}}],[{{"kind":"string","value":"fields"}},{{"kind":"list","items":[{{"kind":"map","entries":[[{{"kind":"string","value":"id"}},{{"kind":"binary","hex":"013f54400c82"}}],[{{"kind":"string","value":"name"}},{{"kind":"string","value":"age"}}],[{{"kind":"string","value":"type"}},{{"kind":"string","value":"{storage}"}}],[{{"kind":"string","value":"canonical"}},{{"kind":"boolean","value":true}}],[{{"kind":"string","value":"absent"}},{{"kind":"string","value":"required"}}],[{{"kind":"string","value":"properties"}},{{"kind":"map","entries":[]}}]]}}]}}],[{{"kind":"string","value":"properties"}},{{"kind":"map","entries":[]}}]]}}"#
        );
        let description = rows::notation(&serde_json::from_str(&notation).expect("notation")).expect("description");
        let mut encoded = Vec::new();
        let mut unlimited = Budget::new(u64::MAX, u64::MAX, u64::MAX);
        rows::encode_metadata(&description, &mut encoded, &mut unlimited).expect("D encoding");
        encoded
    }

    /// Admit a corpus envelope whose one reference description has this field type.
    fn admitted(input: &str, storage: &str) -> RowAdmission {
        let mut request = RowRequest::new(columns::hex(input).expect("hex"), false, true);
        request.supply(described(storage));
        request.allow(16_777_216, 1_048_576, 128);
        let admission = request.admit();
        assert!(admission.admitted(), "corpus envelope refused: {}", admission.category);
        admission
    }

    fn field(value: Item) -> Vec<Item> {
        vec![Item::Record(1), Item::Field(IDENTITY.to_vec(), true, 'r'), value]
    }

    const BOOLEAN: &str = "43524d42010100310101071ddb0b3ce6d3001da133a298ed40ff061c1219f095f290c49ba4ed00e4c9dad4afcf37eb2519ae01000401000101";

    #[test]
    fn records_arrive_in_their_incan_carriers_shape() {
        let negative = "43524d42010100310101071ddb0b3ce6d300cdc6d397d5ecca21865e6132d7929c2951a49c6094b85504e67f4f2ff3a09ae1010004010001ff";
        let nan = "43524d42010100340101071ddb0b3ce6d30076b8e223de0229780edd3b61535050ae5e6678388ac7b0a065c29196c41d816b0100070100040000c07f";
        let union = "43524d42010100320101071ddb0b3ce6d300be267290480c1f4b128fb0c6d3f45abf012bcf4759d1a1b0b2d74187899618510100050100020107";
        let read = admitted(negative, "i8").read_record(0, 65_536, 65_536, 64);
        assert!(read.succeeded(), "{}", read.category);
        assert_eq!(read.items, field(Item::Signed(-1)));
        let read = admitted(nan, "fp32").read_record(0, 65_536, 65_536, 64);
        assert_eq!(read.items, field(Item::Float(32, vec![0x7f, 0xc0, 0, 0])));
        let mut read = admitted(union, "union<string,i8>").read_record(0, 65_536, 65_536, 64);
        assert_eq!(
            read.items,
            [
                field(Item::Union),
                vec![Item::Scalar("i8".to_string()), Item::Signed(7)]
            ]
            .concat()
        );
        assert_eq!((read.next(), read.count()), (14, 1));
        assert_eq!(
            (read.next(), read.bytes(), read.flag(), read.text()),
            (18, IDENTITY.to_vec(), true, "required".to_string())
        );
        assert_eq!(
            (read.next(), read.next(), read.text(), read.next(), read.signed()),
            (16, 19, "i8".to_string(), 3, 7)
        );
        assert_eq!(read.next(), -1);
        assert!(read.bytes_charged > 0 && read.elements_charged > 0);
    }

    #[test]
    fn a_field_is_selected_by_identity_and_a_record_is_read_under_the_callers_allowance() {
        let rows = admitted(BOOLEAN, "boolean");
        assert_eq!(rows.key(0), vec![0x1d, 0xdb, 0x0b, 0x3c, 0xe6, 0xd3, 0x00]);
        let cell = rows.field_cell(0, IDENTITY.to_vec(), 64, 64, 16);
        assert!(cell.succeeded() && cell.present, "{}", cell.category);
        assert_eq!((cell.origin, cell.length), (56, 1));
        assert_eq!(
            rows.read_field(0, IDENTITY.to_vec(), 64, 64, 16).items,
            vec![Item::Boolean(true)]
        );
        let unknown = rows.read_field(0, b"zzzzzz".to_vec(), 64, 64, 16);
        assert_eq!((unknown.category.as_str(), unknown.offset), ("missing_field", 53));
        let short = rows.field_cell(0, b"short".to_vec(), 64, 64, 16);
        assert_eq!(short.category, "missing_field");
        let short = rows.read_record(0, 0, 64, 16);
        assert_eq!(short.category, "limit_exceeded");
        assert_eq!(short.cause, "bytes");
        assert_eq!(rows.read_record(1, 64, 64, 16).category, "record_index");
        let description = rows.description(0, 65_536, 65_536, 64);
        assert!(description.succeeded() && !description.inline);
        assert_eq!(description.supplied, 0);
        assert_eq!((description.items[0].code(), description.items.len()), (31, 24));
        assert_eq!(rows.description(0, 0, 65_536, 64).category, "limit_exceeded");
    }
}
