//! The native frontend's typed checked namespace projection, framed by the public Crumb row codec.

use crate::callable_schema::{CALLABLE_PRODUCT, callable_descriptions};
use crate::crumb_rows::{field_value, operation_budget, required_value};
use crate::graph_crumb_fields::{graph_read_budget, items, text};
use crate::namespace_schema::{
    NAMESPACE_MODULE_NAME, NAMESPACE_MODULE_PATH, NAMESPACE_SCHEMA, namespace_descriptions,
    namespace_field_is_canonical, namespace_root,
};
use crate::plan_wire_rows::{PreparedWireReader, admit_wire, read_wire, wire_field, write_wire};
use internal_models_bindings::{
    Budget, IntegerValue, MapEntry, MappedView, Projection, RowAdmission, RowRead, Value, ValueField, binary_message,
    codec_allowance, crumb_message, depth_allowance, read_field_at, settle, value_hash,
};

/// One identity-addressed field of a namespace record or alternative.
#[derive(Clone, PartialEq)]
pub struct NamespaceField {
    pub identity: u64,
    pub value: NamespaceAtom,
}
crate::incan_debug::incan_debug!(NamespaceField { identity, value });

/// One key and value of a namespace mapping.
#[derive(Clone, PartialEq)]
pub struct NamespaceEntry {
    pub key: NamespaceAtom,
    pub value: NamespaceAtom,
}
crate::incan_debug::incan_debug!(NamespaceEntry { key, value });

/// Logical ABI between the Rust-owned checker declarations and their storage contract.
#[derive(Clone, Debug, PartialEq)]
pub enum NamespaceAtom {
    Text(String),
    Binary(Vec<u8>),
    Flag(bool),
    Signed(i64),
    Unsigned(u64),
    Real(f64),
    Missing,
    Present(Vec<NamespaceAtom>),
    Items(Vec<NamespaceAtom>),
    TupleItems(Vec<NamespaceAtom>),
    Mapping(Vec<NamespaceEntry>),
    Fields(Vec<NamespaceField>),
    Alternative(u64, Vec<NamespaceField>),
}

const INVALID_CELL: &str = "invalid logical cell in a checked namespace";

fn selector(number: u64) -> Result<Vec<u8>, String> {
    if number > 0xffff_ffff_ffff {
        return Err("namespace member identity exceeds six bytes".into());
    }
    Ok(number.to_be_bytes()[2..].to_vec())
}

fn selector_number(value: &[u8]) -> Result<u64, String> {
    if value.len() != 6 {
        return Err("namespace member identity must have six bytes".into());
    }
    Ok(value
        .iter()
        .fold(0_u64, |result, byte| (result << 8) | u64::from(*byte)))
}

fn fields(values: &[NamespaceField]) -> Result<Vec<ValueField>, String> {
    values
        .iter()
        .map(|field| {
            let key = selector(field.identity)?;
            let canonical = namespace_field_is_canonical(&key);
            Ok(field_value(&key, logical(&field.value)?, canonical))
        })
        .collect()
}

fn logical(cell: &NamespaceAtom) -> Result<Value, String> {
    Ok(match cell {
        NamespaceAtom::Text(s) => Value::Text(s.clone()),
        NamespaceAtom::Binary(data) => Value::Binary(data.clone()),
        NamespaceAtom::Flag(flag) => Value::Boolean(*flag),
        NamespaceAtom::Signed(number) => Value::Integer(IntegerValue::Signed(i128::from(*number))),
        NamespaceAtom::Unsigned(number) => Value::Integer(IntegerValue::Unsigned(u128::from(*number))),
        NamespaceAtom::Real(number) => Value::Float(64, number.to_bits().to_be_bytes().to_vec()),
        NamespaceAtom::Missing => Value::NoneValue,
        NamespaceAtom::Present(items) => match items.as_slice() {
            [item] => Value::Some(vec![logical(item)?]),
            _ => return Err("namespace option needs one value".into()),
        },
        NamespaceAtom::Items(items) => Value::List(items.iter().map(logical).collect::<Result<_, _>>()?),
        NamespaceAtom::TupleItems(items) => Value::Tuple(items.iter().map(logical).collect::<Result<_, _>>()?),
        NamespaceAtom::Mapping(pairs) => Value::Map(
            pairs
                .iter()
                .map(|entry| {
                    Ok(MapEntry {
                        key: logical(&entry.key)?,
                        value: logical(&entry.value)?,
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        NamespaceAtom::Fields(items) => Value::Record(fields(items)?),
        NamespaceAtom::Alternative(identity, items) => Value::Enum(selector(*identity)?, fields(items)?),
    })
}

fn read_fields(values: &[ValueField]) -> Result<Vec<NamespaceField>, String> {
    values
        .iter()
        .map(|field| {
            Ok(NamespaceField {
                identity: selector_number(&field.identity)?,
                value: atom(&field.value)?,
            })
        })
        .collect()
}

fn float_bits(data: &[u8]) -> Option<f64> {
    let bytes: [u8; 8] = data.try_into().ok()?;
    Some(f64::from_bits(u64::from_be_bytes(bytes)))
}

fn atom(value: &Value) -> Result<NamespaceAtom, String> {
    Ok(match value {
        Value::Text(s) => NamespaceAtom::Text(s.clone()),
        Value::Binary(data) => NamespaceAtom::Binary(data.clone()),
        Value::Boolean(flag) => NamespaceAtom::Flag(*flag),
        Value::Integer(IntegerValue::Signed(number)) => {
            NamespaceAtom::Signed(i64::try_from(*number).map_err(|_| INVALID_CELL.to_owned())?)
        }
        Value::Integer(IntegerValue::Unsigned(number)) => {
            NamespaceAtom::Unsigned(u64::try_from(*number).map_err(|_| INVALID_CELL.to_owned())?)
        }
        Value::Float(64, data) => NamespaceAtom::Real(float_bits(data).ok_or_else(|| INVALID_CELL.to_owned())?),
        Value::NoneValue => NamespaceAtom::Missing,
        Value::Some(items) if items.len() == 1 => NamespaceAtom::Present(vec![atom(&items[0])?]),
        Value::List(items) => NamespaceAtom::Items(items.iter().map(atom).collect::<Result<_, _>>()?),
        Value::Tuple(items) => NamespaceAtom::TupleItems(items.iter().map(atom).collect::<Result<_, _>>()?),
        Value::Map(entries) => NamespaceAtom::Mapping(
            entries
                .iter()
                .map(|entry| {
                    Ok(NamespaceEntry {
                        key: atom(&entry.key)?,
                        value: atom(&entry.value)?,
                    })
                })
                .collect::<Result<_, String>>()?,
        ),
        Value::Record(items) => NamespaceAtom::Fields(read_fields(items)?),
        Value::Enum(identity, items) => NamespaceAtom::Alternative(selector_number(identity)?, read_fields(items)?),
        _ => return Err(INVALID_CELL.into()),
    })
}

/// Decode the compiler field projection from the validated Crumb node stream.
fn atom_fields(nodes: &mut RowRead, count: i64) -> Result<Vec<NamespaceField>, String> {
    let mut fields = Vec::new();
    for _ in 0..count {
        if nodes.next() != 18 {
            return Err("invalid field in a checked namespace".into());
        }
        let identity = selector_number(&nodes.bytes())?;
        if !matches!(nodes.text().as_str(), "required" | "none" | "empty") {
            return Err("invalid absent policy in a checked namespace".into());
        }
        fields.push(NamespaceField {
            identity,
            value: read_atom(nodes)?,
        });
    }
    Ok(fields)
}

/// Build the compiler carrier directly from one settled Crumb record in preorder.
fn read_atom(nodes: &mut RowRead) -> Result<NamespaceAtom, String> {
    let code = nodes.next();
    let invalid = || INVALID_CELL.to_owned();
    Ok(match code {
        0 => NamespaceAtom::Missing,
        1 => NamespaceAtom::Present(vec![read_atom(nodes)?]),
        2 => NamespaceAtom::Flag(nodes.flag()),
        3 => NamespaceAtom::Signed(i64::try_from(nodes.signed()).map_err(|_| invalid())?),
        4 => NamespaceAtom::Unsigned(u64::try_from(nodes.unsigned()).map_err(|_| invalid())?),
        5 => {
            let data = nodes.bytes();
            if nodes.number() != 64 {
                return Err(invalid());
            }
            NamespaceAtom::Real(float_bits(&data).ok_or_else(invalid)?)
        }
        7 => NamespaceAtom::Text(nodes.text()),
        8 => NamespaceAtom::Binary(nodes.bytes()),
        10 | 11 => {
            let mut values = Vec::new();
            for _ in 0..nodes.count() {
                values.push(read_atom(nodes)?);
            }
            if code == 10 {
                NamespaceAtom::Items(values)
            } else {
                NamespaceAtom::TupleItems(values)
            }
        }
        13 => {
            let mut entries = Vec::new();
            for _ in 0..nodes.count() {
                let key = read_atom(nodes)?;
                entries.push(NamespaceEntry {
                    key,
                    value: read_atom(nodes)?,
                });
            }
            NamespaceAtom::Mapping(entries)
        }
        14 => {
            let count = nodes.count();
            NamespaceAtom::Fields(atom_fields(nodes, count)?)
        }
        15 => {
            let identity = selector_number(&nodes.bytes())?;
            let count = nodes.count();
            NamespaceAtom::Alternative(identity, atom_fields(nodes, count)?)
        }
        _ => return Err(invalid()),
    })
}

/// The immutable encoded compiler descriptions shared by namespace reads in one driver process.
#[derive(Clone)]
pub struct PreparedNamespaceReader {
    pub(crate) reader: PreparedWireReader,
}
crate::incan_debug::incan_debug!(PreparedNamespaceReader { reader });

impl PreparedNamespaceReader {
    /// Encode the namespace description dictionary once.
    pub fn create() -> Result<Self, String> {
        Ok(Self {
            reader: PreparedWireReader::create(&namespace_root(), &namespace_descriptions(), "checked namespace")?,
        })
    }

    /// Decode owned namespace bytes.
    pub fn decode(&self, encoded: &[u8]) -> Result<NamespaceAtom, String> {
        let mut budget = graph_read_budget()?;
        let row = self.reader.admit(encoded, &mut budget, &mut Vec::new())?;
        decode_admitted(&row, &mut budget)
    }

    /// Decode the namespace in place through the same referenced dictionary.
    pub fn decode_view(&self, view: &MappedView) -> Result<NamespaceAtom, String> {
        let mut budget = graph_read_budget()?;
        let row = self.reader.admit_view(view, &mut budget)?;
        decode_admitted(&row, &mut budget)
    }

    /// Read only the advertised identity fields of owned bytes.
    pub fn identity(&self, encoded: &[u8], schema: u32) -> Result<NamespaceIdentity, String> {
        let mut budget = graph_read_budget()?;
        let row = self.reader.admit(encoded, &mut budget, &mut Vec::new())?;
        identity_admitted(&row, schema, &mut budget)
    }

    /// Recover the exact identity from an older namespace envelope already held by a mapped view.
    pub fn identity_view(&self, view: &MappedView, schema: u32) -> Result<NamespaceIdentity, String> {
        let mut budget = graph_read_budget()?;
        let row = self.reader.admit_view(view, &mut budget)?;
        identity_admitted(&row, schema, &mut budget)
    }
}

/// Project the admitted namespace into the frontend's cells under the remaining allowance.
fn decode_admitted(row: &RowAdmission, budget: &mut Budget) -> Result<NamespaceAtom, String> {
    let (remaining_bytes, remaining_elements) = codec_allowance(budget);
    let mut decoded = row.read_record(0, remaining_bytes, remaining_elements, depth_allowance(budget));
    settle(&decoded, budget).map_err(|error| binary_message(&error))?;
    let root = read_atom(&mut decoded)?;
    if decoded.next() != -1 {
        return Err("checked namespace row has trailing logical cells".into());
    }
    Ok(root)
}

/// Require the namespace schema version the reader expects.
fn check_schema(version: &Value, schema: u32) -> Result<(), String> {
    match version {
        Value::Integer(IntegerValue::Unsigned(number)) if *number != u128::from(schema) => {
            Err("unsupported checked namespace schema".into())
        }
        Value::Integer(IntegerValue::Unsigned(_)) => Ok(()),
        _ => Err("invalid checked namespace schema".into()),
    }
}

/// Read the module path segments of a namespace identity.
fn module_path(segments: &Value) -> Result<Vec<String>, String> {
    items(segments)?.iter().map(text).collect()
}

/// Read only the advertised identity fields and verify the frontend schema.
fn identity_admitted(row: &RowAdmission, schema: u32, budget: &mut Budget) -> Result<NamespaceIdentity, String> {
    let name = text(&wire_field(row, NAMESPACE_MODULE_NAME, budget)?)?;
    check_schema(&wire_field(row, NAMESPACE_SCHEMA, budget)?, schema)?;
    let path = module_path(&wire_field(row, NAMESPACE_MODULE_PATH, budget)?)?;
    Ok(NamespaceIdentity { name, path })
}

/// Prepare the shared namespace reader.
pub fn prepare_namespace_reader() -> Result<PreparedNamespaceReader, String> {
    PreparedNamespaceReader::create()
}

/// Decode owned namespace bytes through a prepared reader.
pub fn decode_prepared_namespace(reader: &PreparedNamespaceReader, encoded: &[u8]) -> Result<NamespaceAtom, String> {
    reader.decode(encoded)
}

/// Read a namespace identity from owned bytes through a prepared reader.
pub fn identify_prepared_namespace(
    reader: &PreparedNamespaceReader,
    encoded: &[u8],
    schema: u32,
) -> Result<NamespaceIdentity, String> {
    reader.identity(encoded, schema)
}

/// Decode a bounded interface directly from an immutable view.
pub fn decode_prepared_namespace_view(
    reader: &PreparedNamespaceReader,
    view: &MappedView,
) -> Result<NamespaceAtom, String> {
    reader.decode_view(view)
}

/// Read the identity from a mapped interface when the caller has no separate proof.
pub fn identify_prepared_namespace_view(
    reader: &PreparedNamespaceReader,
    view: &MappedView,
    schema: u32,
) -> Result<NamespaceIdentity, String> {
    reader.identity_view(view, schema)
}

/// Encode a checked namespace interface.
pub fn encode_namespace_interface(root: &NamespaceAtom) -> Result<Vec<u8>, String> {
    write_wire(&namespace_root(), &namespace_descriptions(), logical(root)?)
}

/// Decode a checked namespace interface without a prepared reader.
pub fn decode_namespace_interface(encoded: &[u8]) -> Result<NamespaceAtom, String> {
    atom(&read_wire(encoded, &namespace_root(), &namespace_descriptions())?)
}

/// The module name and path a checked namespace advertises.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespaceIdentity {
    pub name: String,
    pub path: Vec<String>,
}
crate::incan_debug::incan_debug!(NamespaceIdentity { name, path });

/// Read the checked namespace identity without constructing declarations for an unused lookup table.
pub fn namespace_identity(encoded: &[u8], schema: u32) -> Result<NamespaceIdentity, String> {
    let mut budget = graph_read_budget()?;
    let rows = admit_wire(encoded, &namespace_descriptions(), &mut budget)?;
    // Only CheckedNamespaceInterface declares module_name in the closed namespace dictionary.
    let mut field = |identity: &[u8]| {
        read_field_at(&rows, 0, &identity.to_vec(), &mut budget).map_err(|error| binary_message(&error))
    };
    let name = text(&field(NAMESPACE_MODULE_NAME)?)?;
    check_schema(&field(NAMESPACE_SCHEMA)?, schema)?;
    let path = module_path(&field(NAMESPACE_MODULE_PATH)?)?;
    Ok(NamespaceIdentity { name, path })
}

/// The full installable namespace and its RFC 124 canonical logical-value digest.
#[derive(Clone, PartialEq, Eq)]
pub struct NamespacePublication {
    pub interface: Vec<u8>,
    pub semantic_interface: Vec<u8>,
}
crate::incan_debug::incan_debug!(NamespacePublication {
    interface,
    semantic_interface
});

/// Hash the same checked rows the compiler emits; canonical field selection preserves expansion identity.
pub fn publish_namespace_interface(root: &NamespaceAtom) -> Result<NamespacePublication, String> {
    let value = logical(root)?;
    let interface = write_wire(&namespace_root(), &namespace_descriptions(), value.clone())?;
    let mut budget = operation_budget(0)?;
    let semantic = value_hash(value, &Projection::Canonical, &mut budget, false).map_err(|e| crumb_message(&e))?;
    Ok(NamespacePublication {
        interface,
        semantic_interface: semantic.digest,
    })
}

/// Keep the full checked product and its integrity witness in the same schema-bound Crumb row.
pub fn encode_callable_inventory(root: &NamespaceAtom) -> Result<Vec<u8>, String> {
    let value = logical(root)?;
    let mut budget = operation_budget(0)?;
    let checksum = value_hash(value.clone(), &Projection::Object, &mut budget, false).map_err(|e| crumb_message(&e))?;
    let envelope = Value::Record(vec![
        required_value(b"digest", Value::Binary(checksum.digest)),
        required_value(b"produc", value),
    ]);
    write_wire(CALLABLE_PRODUCT, &callable_descriptions(), envelope)
}

/// Authenticate all fields, including the current source positions, before rebuilding any callable.
pub fn decode_callable_inventory(encoded: &[u8]) -> Result<NamespaceAtom, String> {
    let envelope = read_wire(encoded, CALLABLE_PRODUCT, &callable_descriptions())?;
    let Value::Record(fields) = envelope else {
        return Err("invalid callable publication".into());
    };
    if fields.len() != 2 || fields[0].identity != b"digest" || fields[1].identity != b"produc" {
        return Err("invalid callable publication fields".into());
    }
    let mut budget = operation_budget(0)?;
    let checksum =
        value_hash(fields[1].value.clone(), &Projection::Object, &mut budget, false).map_err(|e| crumb_message(&e))?;
    match &fields[0].value {
        Value::Binary(stored) if *stored != checksum.digest => {
            return Err("callable publication checksum differs".into());
        }
        Value::Binary(_) => {}
        _ => return Err("missing callable publication checksum".into()),
    }
    atom(&fields[1].value)
}
