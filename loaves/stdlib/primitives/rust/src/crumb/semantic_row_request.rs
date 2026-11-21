//! Native entry of the public Crumb row writer: D descriptions, record keys and one flat pre-order value stream.

use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex, MutexGuard};

use super::columns::Budget;
use super::hash::{self, Field, Node, Value};
use super::row_write;
use super::rows::{self, Stored};

/// One completed write, or a typed refusal, with the caller-owned allowance it consumed.
#[derive(Clone, Debug)]
pub struct RowWrite {
    /// Empty on success; otherwise a stable native refusal category.
    pub category: String,
    /// The exhausted caller-owned resource when the refusal is a limit.
    pub cause: String,
    /// Complete envelope bytes on success, empty on refusal.
    pub data: Vec<u8>,
    /// Bytes and elements consumed from the request's allowance.
    pub bytes_charged: i64,
    pub elements_charged: i64,
}

/// One node in the writer's flat pre-order logical-value stream.
#[derive(Clone, Debug)]
struct WriteNode {
    code: i64,
    count: i64,
    flag: bool,
    signed: i128,
    unsigned: u128,
    number: i64,
    second: i64,
    data: Vec<u8>,
    other: Vec<u8>,
    text: String,
}

/// Inputs of one public row write. Description D encoding and the flat nodes are transport, never admission proof.
#[derive(Clone, Debug)]
pub struct RowWriteRequest {
    state: Arc<Mutex<RowWriteState>>,
}

#[derive(Debug)]
struct RowWriteState {
    descriptions: Vec<(Vec<u8>, bool)>,
    records: Vec<Vec<u8>>,
    nodes: Vec<WriteNode>,
    kinds: Vec<(String, bool)>,
    sealed: bool,
    allowance: (u64, u64, u64),
}

impl RowWriteRequest {
    pub fn new(sealed: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(RowWriteState {
                descriptions: Vec::new(),
                records: Vec::new(),
                nodes: Vec::new(),
                kinds: Vec::new(),
                sealed,
                allowance: (0, 0, 0),
            })),
        }
    }

    fn locked(&self) -> MutexGuard<'_, RowWriteState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Add one complete D description and its requested inline (true) or reference (false) mode.
    pub fn description(&mut self, encoded: Vec<u8>, inline: bool) {
        self.locked().descriptions.push((encoded, inline));
    }

    /// Add one registered profile kind with its record (true) or enum (false) shape.
    pub fn register(&mut self, kind: String, record: bool) {
        self.locked().kinds.push((kind, record));
    }

    /// Start one record. Exactly one complete value must follow in the shared node stream.
    pub fn record(&mut self, key: Vec<u8>) {
        self.locked().records.push(key);
    }

    /// Append one pre-order node using the same codes and payload slots as `RowRead`.
    #[allow(clippy::too_many_arguments)]
    pub fn node(
        &mut self,
        code: i64,
        count: i64,
        flag: bool,
        signed: i128,
        unsigned: u128,
        number: i64,
        second: i64,
        data: Vec<u8>,
        other: Vec<u8>,
        text: String,
    ) {
        self.locked().nodes.push(WriteNode {
            code,
            count,
            flag,
            signed,
            unsigned,
            number,
            second,
            data,
            other,
            text,
        });
    }

    /// Set the caller's remaining byte, element and nesting allowance.
    pub fn allow(&mut self, bytes: i64, elements: i64, depth: i64) {
        let amount = |value: i64| u64::try_from(value).unwrap_or(0);
        self.locked().allowance = (amount(bytes), amount(elements), amount(depth));
    }

    /// Admit, validate and encode the complete request without exposing a partial prefix.
    pub fn write(&self) -> RowWrite {
        let state = self.locked();
        let (bytes, elements, depth) = state.allowance;
        let mut budget = Budget::new(bytes, elements, depth);
        let result = state.write_with(&mut budget);
        let (bytes_left, elements_left) = budget.left();
        let charged = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        match result {
            Ok(data) => RowWrite {
                category: String::new(),
                cause: String::new(),
                data,
                bytes_charged: charged(bytes - bytes_left),
                elements_charged: charged(elements - elements_left),
            },
            Err(category) => RowWrite {
                category: category.to_string(),
                cause: if category == "limit_exceeded" {
                    budget.cause().unwrap_or_default().to_string()
                } else {
                    String::new()
                },
                data: Vec::new(),
                bytes_charged: charged(bytes - bytes_left),
                elements_charged: charged(elements - elements_left),
            },
        }
    }
}

impl RowWriteState {
    fn write_with(&self, budget: &mut Budget) -> Result<Vec<u8>, &'static str> {
        let sources = self
            .descriptions
            .iter()
            .map(|(encoded, inline)| rows::read_description(encoded).map(|metadata| (metadata, *inline)))
            .collect::<Option<Vec<_>>>()
            .ok_or("invalid_description")?;
        let dictionary = rows::admit_written(sources, &self.kinds, budget)?;
        let values = parse_records(&self.nodes, self.records.len())?;
        let entries = dictionary
            .iter()
            .map(|entry| row_write::encode_entry_charged(entry, budget))
            .collect::<Result<Vec<_>, _>>()?;
        let mut slots = Vec::with_capacity(values.len());
        let mut cells = Vec::with_capacity(values.len());
        for (key, value) in self.records.iter().zip(&values) {
            budget.elements(1)?;
            let slot = dictionary_slot(&dictionary, key, budget)?;
            cells.push(row_write::write_root_charged(
                &dictionary[slot],
                value,
                &dictionary,
                budget,
            )?);
            slots.push(i64::try_from(slot).map_err(|_| "limit_exceeded")?);
        }
        let seal = logical_seal(&values, self.sealed, budget)?;
        let layout = crate::crumb::row_envelope(entries, slots, cells, self.sealed, seal);
        if layout.kind == 1 {
            return Err("invalid_cell");
        }
        if layout.kind != 0 {
            return Err("limit_exceeded");
        }
        budget.bytes(u64::try_from(layout.byte_charge).map_err(|_| "limit_exceeded")?)?;
        budget.elements(u64::try_from(layout.element_charge).map_err(|_| "limit_exceeded")?)?;
        Ok(layout.data)
    }
}

fn dictionary_slot(dictionary: &[Stored], key: &[u8], budget: &mut Budget) -> Result<usize, &'static str> {
    budget.elements(1)?;
    budget.bytes(2 * key.len() as u64)?;
    dictionary
        .iter()
        .position(|entry| entry.key == key)
        .ok_or("missing_description")
}

fn logical_seal(values: &[Value], sealed: bool, budget: &mut Budget) -> Result<Vec<u8>, &'static str> {
    if !sealed {
        return Ok(Vec::new());
    }
    hash::charge_seal_start(values.len(), budget)?;
    let mut preimage = b"crumb1\x00\x04".to_vec();
    hash::unsigned(&mut preimage, values.len() as u128);
    for value in values {
        let mut contribution = Vec::new();
        hash::emit_object_charged(value, &mut contribution, budget)?;
        hash::frame(&mut preimage, &contribution);
    }
    hash::charge_seal_digest(budget)?;
    Ok(Sha256::digest(preimage).to_vec())
}

fn parse_records(nodes: &[WriteNode], count: usize) -> Result<Vec<Value>, &'static str> {
    let mut cursor = NodeCursor { nodes, position: 0 };
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(cursor.value()?);
    }
    if cursor.position != nodes.len() {
        return Err("invalid_value_stream");
    }
    Ok(values)
}

struct NodeCursor<'a> {
    nodes: &'a [WriteNode],
    position: usize,
}

impl NodeCursor<'_> {
    fn next(&mut self) -> Result<&WriteNode, &'static str> {
        let node = self.nodes.get(self.position).ok_or("invalid_value_stream")?;
        self.position += 1;
        Ok(node)
    }

    fn values(&mut self, count: i64) -> Result<Vec<Value>, &'static str> {
        let count = usize::try_from(count).map_err(|_| "invalid_value_stream")?;
        (0..count).map(|_| self.value()).collect()
    }

    fn value(&mut self) -> Result<Value, &'static str> {
        let node = self.next()?.clone();
        Ok(match node.code {
            0 => Value::Absent,
            1 if node.count == 1 => Value::Some(Box::new(self.value()?)),
            2 => Value::Boolean(node.flag),
            3 => Value::Integer(node.signed < 0, node.signed.unsigned_abs()),
            4 => Value::Integer(false, node.unsigned),
            5 => Value::Float(float64_bits(node.number, &node.data)?),
            6 => Value::Decimal(node.signed, node.number),
            7 => Value::Text(node.text),
            8 => Value::Binary(node.data.into()),
            9 => Value::Withheld,
            10 => Value::List(self.values(node.count)?),
            11 => Value::Tuple(self.values(node.count)?),
            12 => Value::Set(self.values(node.count)?),
            13 => Value::Map(self.map(node.count)?),
            14 => Value::Record(self.fields(node.count)?),
            15 => Value::Enum(node.data, self.fields(node.count)?),
            16 if node.count == 1 => Value::Union(self.selector()?, Box::new(self.value()?)),
            17 => Value::Reference(node.data, node.other),
            _ => return Err("invalid_value_stream"),
        })
    }

    fn map(&mut self, count: i64) -> Result<Vec<(Value, Value)>, &'static str> {
        let count = usize::try_from(count).map_err(|_| "invalid_value_stream")?;
        (0..count).map(|_| Ok((self.value()?, self.value()?))).collect()
    }

    fn fields(&mut self, count: i64) -> Result<Vec<Field>, &'static str> {
        let count = usize::try_from(count).map_err(|_| "invalid_value_stream")?;
        let mut fields = Vec::with_capacity(count);
        for _ in 0..count {
            let node = self.next()?.clone();
            if node.code != 18 {
                return Err("invalid_value_stream");
            }
            let absent = match node.text.as_str() {
                "required" => 'r',
                "none" => 'n',
                "empty" => 'e',
                _ => return Err("invalid_value_stream"),
            };
            fields.push(Field {
                identity: node.data,
                canonical: node.flag,
                absent,
                value: self.value()?,
            });
        }
        Ok(fields)
    }

    fn selectors(&mut self, count: i64) -> Result<Vec<Node>, &'static str> {
        let count = usize::try_from(count).map_err(|_| "invalid_value_stream")?;
        (0..count).map(|_| self.selector()).collect()
    }

    fn selector(&mut self) -> Result<Node, &'static str> {
        let node = self.next()?.clone();
        Ok(match node.code {
            19 => Node::Scalar(node.text),
            20 => Node::Decimal(node.number, node.second),
            21 => Node::Container(node.text, self.selectors(node.count)?),
            22 => Node::Nominal(node.text, node.data, self.selectors(node.count)?),
            23 if node.count == 1 => Node::Reference(Box::new(self.selector()?)),
            _ => return Err("invalid_value_stream"),
        })
    }
}

fn float64_bits(width: i64, bits: &[u8]) -> Result<Vec<u8>, &'static str> {
    match width {
        64 if bits.len() == 8 => {
            let value = f64::from_bits(u64::from_be_bytes(bits.try_into().map_err(|_| "invalid_value_stream")?));
            Ok(if value.is_nan() {
                f64::NAN.to_bits().to_be_bytes().to_vec()
            } else {
                bits.to_vec()
            })
        }
        32 if bits.len() == 4 => {
            let value = f32::from_bits(u32::from_be_bytes(bits.try_into().map_err(|_| "invalid_value_stream")?));
            let wide = if value.is_nan() { f64::NAN } else { f64::from(value) };
            Ok(wide.to_bits().to_be_bytes().to_vec())
        }
        _ => Err("invalid_value_stream"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::rows::Meta;
    use super::*;

    fn metadata() -> Meta {
        let field = Meta::Map(vec![
            ("id".into(), Meta::Binary(b"field1".to_vec())),
            ("name".into(), Meta::Text("amount".into())),
            ("type".into(), Meta::Text("i8".into())),
            ("canonical".into(), Meta::Boolean(true)),
            ("absent".into(), Meta::Text("required".into())),
            ("properties".into(), Meta::Map(Vec::new())),
        ]);
        Meta::Map(vec![
            ("kind".into(), Meta::Text("Record".into())),
            ("nominal".into(), Meta::Binary(b"record\0".to_vec())),
            ("path".into(), Meta::Text("example.Stored".into())),
            ("fields".into(), Meta::List(vec![field])),
            ("properties".into(), Meta::Map(Vec::new())),
        ])
    }

    fn encoded_description() -> Vec<u8> {
        let mut budget = Budget::new(u64::MAX, u64::MAX, u64::MAX);
        let mut encoded = Vec::new();
        rows::encode_metadata(&metadata(), &mut encoded, &mut budget).expect("D encoding");
        encoded
    }

    fn node(request: &mut RowWriteRequest, code: i64, count: i64, data: &[u8], text: &str) {
        request.node(
            code,
            count,
            true,
            7,
            0,
            0,
            0,
            data.to_vec(),
            Vec::new(),
            text.to_string(),
        );
    }

    fn request(sealed: bool) -> RowWriteRequest {
        let mut request = RowWriteRequest::new(sealed);
        request.description(encoded_description(), true);
        request.record(b"record\0".to_vec());
        node(&mut request, 14, 1, &[], "");
        node(&mut request, 18, 0, b"field1", "required");
        node(&mut request, 3, 0, &[], "");
        request.allow(1_048_576, 1_048_576, 64);
        request
    }

    #[test]
    fn a_write_request_returns_complete_sealed_bytes_and_exact_charges() {
        let written = request(true).write();
        assert!(written.category.is_empty(), "write refused: {}", written.category);
        assert_eq!(&written.data[..7], b"CRMB\x01\x01\x01");
        let mut exact = request(true);
        exact.allow(written.bytes_charged, written.elements_charged, 64);
        let repeated = exact.write();
        assert!(
            repeated.category.is_empty(),
            "exact allowance refused: {}",
            repeated.category
        );
        assert_eq!(repeated.data, written.data);
        let mut short = request(true);
        short.allow(written.bytes_charged, written.elements_charged - 1, 64);
        let refused = short.write();
        assert_eq!(
            (refused.category.as_str(), refused.cause.as_str()),
            ("limit_exceeded", "elements")
        );
    }

    #[test]
    fn malformed_descriptions_streams_and_shape_mismatches_are_typed_refusals() {
        let negative_zero = (-0.0f64).to_bits().to_be_bytes();
        assert_eq!(float64_bits(64, &negative_zero), Ok(negative_zero.to_vec()));

        let mut malformed = RowWriteRequest::new(false);
        malformed.description(vec![9], true);
        malformed.allow(1_048_576, 1_048_576, 64);
        assert_eq!(malformed.write().category, "invalid_description");

        let mut stream = RowWriteRequest::new(false);
        stream.description(encoded_description(), true);
        stream.record(b"record\0".to_vec());
        node(&mut stream, 14, 1, &[], "");
        stream.allow(1_048_576, 1_048_576, 64);
        assert_eq!(stream.write().category, "invalid_value_stream");

        let mut shape = request(false);
        shape.locked().nodes.pop();
        node(&mut shape, 7, 0, &[], "wrong");
        assert_eq!(shape.write().category, "invalid_cell");
    }
}
