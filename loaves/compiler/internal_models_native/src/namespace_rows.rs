//! The native frontend's typed checked namespace projection, framed by the public Crumb row codec.

use crate::callable_schema::{CALLABLE_PRODUCT, callable_descriptions};
use crate::crumb_rows::{field_value, operation_budget, required_value};
use crate::graph_crumb_fields::{graph_read_budget, graph_read_budget_at_depth, items, text};
use crate::namespace_schema::{
    NAMESPACE_MODULE_NAME, NAMESPACE_MODULE_PATH, NAMESPACE_SCHEMA, namespace_descriptions,
    namespace_field_is_canonical, namespace_root,
};
use crate::plan_wire_rows::{
    PreparedWireReader, admit_wire, read_wire, wire_field, write_wire, write_wire_with_budget,
};
use internal_models_bindings::{
    Budget, IntegerValue, MapEntry, MappedView, Projection, RowAdmission, RowRead, Value, ValueField, binary_message,
    codec_allowance, crumb_message, depth_allowance, read_field_at, refusal, settle, value_hash,
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
    /// A signed logical integer outside the narrow checked declaration domain.
    WideSigned(i128),
    /// An unsigned logical integer outside the narrow checked declaration domain.
    WideUnsigned(u128),
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

// A single Body IR edge can cross a collection, a typed cell and a nominal record.
const EXECUTABLE_DEPTH: i64 = 512;

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

/// Convert compiler cells into the logical values required by the declared Crumb shape.
fn logical(cell: &NamespaceAtom) -> Result<Value, String> {
    Ok(match cell {
        NamespaceAtom::Text(s) => Value::Text(s.clone()),
        NamespaceAtom::Binary(data) => Value::Binary(data.clone()),
        NamespaceAtom::Flag(flag) => Value::Boolean(*flag),
        NamespaceAtom::Signed(number) => Value::Integer(IntegerValue::Signed(i128::from(*number))),
        NamespaceAtom::Unsigned(number) => Value::Integer(IntegerValue::Unsigned(u128::from(*number))),
        NamespaceAtom::WideSigned(number) => Value::Integer(IntegerValue::Signed(*number)),
        NamespaceAtom::WideUnsigned(number) => Value::Integer(IntegerValue::Unsigned(*number)),
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

/// Keep the ordinary namespace ABI for narrow values and retain exact bits for full-width constants.
fn signed(number: i128) -> NamespaceAtom {
    i64::try_from(number).map_or(NamespaceAtom::WideSigned(number), NamespaceAtom::Signed)
}

/// Keep the ordinary namespace ABI for narrow values and retain exact bits for full-width constants.
fn unsigned(number: u128) -> NamespaceAtom {
    u64::try_from(number).map_or(NamespaceAtom::WideUnsigned(number), NamespaceAtom::Unsigned)
}

/// Reconstruct the compiler ABI from an admitted logical value, retaining full-width integers.
fn atom(value: &Value) -> Result<NamespaceAtom, String> {
    Ok(match value {
        Value::Text(s) => NamespaceAtom::Text(s.clone()),
        Value::Binary(data) => NamespaceAtom::Binary(data.clone()),
        Value::Boolean(flag) => NamespaceAtom::Flag(*flag),
        Value::Integer(IntegerValue::Signed(number)) => signed(*number),
        Value::Integer(IntegerValue::Unsigned(number)) => unsigned(*number),
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
        3 => signed(nodes.signed()),
        4 => unsigned(nodes.unsigned()),
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
    let decoded = row.read_record(0, remaining_bytes, remaining_elements, depth_allowance(budget));
    settle(&decoded, budget).map_err(|error| binary_message(&error))?;
    restore_admitted_atoms(decoded)
}

/// Materialize a validated row from its preorder cursor.
fn restore_admitted_atoms(mut decoded: RowRead) -> Result<NamespaceAtom, String> {
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

/// Encode compiler logical cells using the caller's exact Crumb root and reference closure.
pub fn encode_checked_projection(
    root: &[u8],
    descriptions: &Vec<internal_models_bindings::Description>,
    value: &NamespaceAtom,
) -> Result<Vec<u8>, String> {
    write_wire_with_budget(
        root,
        descriptions,
        logical(value)?,
        &mut graph_read_budget_at_depth(EXECUTABLE_DEPTH)?,
    )
}

/// Admit the exact authored root and reference closure before restoring the compiler's logical cells.
pub fn decode_checked_projection(
    root: &[u8],
    descriptions: &Vec<internal_models_bindings::Description>,
    encoded: &[u8],
) -> Result<NamespaceAtom, String> {
    let reader = PreparedWireReader::create(root, descriptions, "checked executable")?;
    decode_prepared_checked_projection(&reader, encoded)
}

/// Authenticate owned executable bytes using a dictionary already prepared by the compiler process.
pub fn decode_prepared_checked_projection(
    reader: &PreparedWireReader,
    encoded: &[u8],
) -> Result<NamespaceAtom, String> {
    let mut budget = graph_read_budget_at_depth(EXECUTABLE_DEPTH)?;
    let row = reader.admit(encoded, &mut budget, &mut Vec::new())?;
    decode_admitted_executable(&row)
}

/// Reuse the admitted tree under the work already charged for this envelope.
fn decode_admitted_executable(row: &RowAdmission) -> Result<NamespaceAtom, String> {
    let decoded = row.read_record(0, row.bytes_charged, row.elements_charged, EXECUTABLE_DEPTH);
    if !decoded.succeeded() {
        return Err(binary_message(&refusal(
            decoded.category,
            decoded.offset,
            decoded.field,
            decoded.variant,
            decoded.cause,
        )));
    }
    restore_admitted_atoms(decoded)
}

/// Authenticate a mapped compiler root and materialize only the requested named fields.
pub fn checked_projection_fields(
    reader: &PreparedWireReader,
    view: &MappedView,
    identities: &[Vec<u8>],
) -> Result<Vec<NamespaceAtom>, String> {
    let mut budget = graph_read_budget_at_depth(EXECUTABLE_DEPTH)?;
    let row = reader.admit_view(view, &mut budget)?;
    identities
        .iter()
        .map(|identity| atom(&wire_field(&row, identity, &mut budget)?))
        .collect()
}

/// Open a fully needed executable value in its authenticated mapped backing.
pub fn decode_checked_projection_view(reader: &PreparedWireReader, view: &MappedView) -> Result<NamespaceAtom, String> {
    let mut budget = graph_read_budget_at_depth(EXECUTABLE_DEPTH)?;
    let row = reader.admit_view(view, &mut budget)?;
    decode_admitted_executable(&row)
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

#[cfg(test)]
mod executable_depth_tests {
    use super::*;
    use crate::crumb_rows::{ModelField, model_description};
    use internal_models_bindings::{complete_view, create_budget, mapped_bytes};

    const ROOT: &[u8] = b"nestd_\0";
    const CHILDREN: &[u8] = b"child_";
    const LABEL: &[u8] = b"label_";

    /// Build a finite chain in a recursive model, mirroring the named scopes in deeply nested Body IR.
    fn nested_value(depth: usize) -> Result<NamespaceAtom, String> {
        let children = selector_number(CHILDREN)?;
        let label = selector_number(LABEL)?;
        let node = |child| {
            NamespaceAtom::Fields(vec![
                NamespaceField {
                    identity: children,
                    value: NamespaceAtom::Items(child),
                },
                NamespaceField {
                    identity: label,
                    value: NamespaceAtom::Unsigned(7),
                },
            ])
        };
        Ok((0..depth).fold(node(Vec::new()), |child, _| node(vec![child])))
    }

    /// A deeper executable value must round-trip through both admission paths under the same finite budget.
    #[test]
    fn executable_projection_round_trips_nested_records() -> Result<(), String> {
        let descriptions = vec![model_description(
            ROOT,
            "DeepExecutableFixture",
            &[
                ModelField::new(CHILDREN, "children", "list<record@6e657374645f>"),
                ModelField::new(LABEL, "label", "u64"),
            ],
        )];
        let expected = nested_value(60)?;
        let encoded = encode_checked_projection(ROOT, &descriptions, &expected)?;
        assert_eq!(decode_checked_projection(ROOT, &descriptions, &encoded)?, expected);

        let encoded_again = encode_checked_projection(ROOT, &descriptions, &expected)?;
        assert_eq!(encoded, encoded_again);
        let reader = PreparedWireReader::create(ROOT, &descriptions, "deep executable fixture")?;
        let mapped = mapped_bytes(&encoded)?;
        let view = complete_view(&mapped).ok_or("deep executable fixture has no complete mapped view")?;
        assert_eq!(
            checked_projection_fields(&reader, &view, &[LABEL.to_vec()])?,
            [NamespaceAtom::Unsigned(7)]
        );
        assert_eq!(decode_checked_projection_view(&reader, &view)?, expected);

        let admitted = reader.admit_view(&view, &mut graph_read_budget_at_depth(EXECUTABLE_DEPTH)?)?;
        let mut exact = create_budget(admitted.bytes_charged, admitted.elements_charged, EXECUTABLE_DEPTH)
            .map_err(|error| crumb_message(&error))?;
        let paid = reader.admit_view(&view, &mut exact)?;
        assert_eq!(codec_allowance(&exact), (0, 0));
        assert!(decode_admitted(&paid, &mut exact).is_err());
        assert_eq!(decode_admitted_executable(&paid)?, expected);

        assert!(write_wire(ROOT, &descriptions, logical(&expected)?).is_err());
        let refused = encode_checked_projection(ROOT, &descriptions, &nested_value(176)?)
            .err()
            .ok_or("an over-depth executable frame was accepted")?;
        assert!(refused.contains("depth"), "{refused}");
        Ok(())
    }
}
