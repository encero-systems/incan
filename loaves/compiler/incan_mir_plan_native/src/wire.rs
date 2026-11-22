//! Crumb rows of the compiler-owned native-plan profile, through the standard library's native row codec.
//!
//! A plan record is one Crumb row: a root selector and one logical value, written with the complete closure of
//! referenced descriptions its shapes reach. Writing hands the native `RowWriteRequest` the same D-encoded descriptions
//! and pre-order value nodes the Incan row writer hands it, so the bytes are the reference's. Reading admits the
//! envelope against exactly the supplied descriptions, then materializes its one row. Refusals keep the reference's
//! diagnostic text: Crumb's `CrumbError` and `BinaryError` spellings, and the plan profile's own messages.

use std::sync::OnceLock;

// The standard library's native row codec, by the stable names the compiler bindings publish.
use internal_models_bindings::{RowAdmission, RowRead, RowRequest, RowWriteRequest};

/// The largest encoded plan or graph record, in bytes.
pub(crate) const MAX_RECORD_BYTES: i64 = 67_108_864;
/// Logical element allowance of one record operation.
const ELEMENTS: i64 = 4_194_304;
/// Nesting allowance of one record operation.
const DEPTH: i64 = 128;

// ---- Shapes and their D-encoded descriptions ----

/// One required field of a described record or enum alternative.
pub(crate) struct ShapeField {
    pub identity: &'static [u8],
    pub name: &'static str,
    pub storage: &'static str,
}

/// One described enum alternative and its positional payload fields.
pub(crate) struct ShapeVariant {
    pub identity: &'static [u8],
    pub name: &'static str,
    pub fields: &'static [ShapeField],
}

/// A described record's fields or a described enum's alternatives, in declaration order.
pub(crate) enum ShapeBody {
    Record(&'static [ShapeField]),
    Enum(&'static [ShapeVariant]),
}

/// One referenced Crumb description: its nominal selector, profile path and body.
pub(crate) struct Shape {
    pub selector: &'static [u8],
    pub path: &'static str,
    pub body: ShapeBody,
}

/// One required field description.
pub(crate) const fn field(identity: &'static [u8], name: &'static str, storage: &'static str) -> ShapeField {
    ShapeField {
        identity,
        name,
        storage,
    }
}

/// One alternative description and its positional payload fields.
pub(crate) const fn variant(
    identity: &'static [u8],
    name: &'static str,
    fields: &'static [ShapeField],
) -> ShapeVariant {
    ShapeVariant { identity, name, fields }
}

/// The complete description closure a root row is written and admitted with, D-encoded once per process.
pub(crate) struct Schema {
    shapes: &'static [&'static Shape],
    encoded: OnceLock<Vec<Vec<u8>>>,
}

impl Schema {
    /// Name a closure of shapes; nothing is encoded until a row first needs it.
    pub(crate) const fn new(shapes: &'static [&'static Shape]) -> Self {
        Self {
            shapes,
            encoded: OnceLock::new(),
        }
    }

    /// The number of descriptions in the closure.
    pub(crate) fn len(&self) -> usize {
        self.shapes.len()
    }

    /// The D encoding of each description, in closure order.
    pub(crate) fn descriptions(&self) -> &[Vec<u8>] {
        self.encoded
            .get_or_init(|| self.shapes.iter().map(|shape| encode_shape(shape)).collect())
    }
}

/// A metadata tree, as Crumb's description metadata holds it.
enum Meta<'a> {
    Boolean(bool),
    Text(&'a str),
    Binary(&'a [u8]),
    List(Vec<Meta<'a>>),
    Map(Vec<(&'static str, Meta<'a>)>),
}

/// D-encode one description the way `std.primitives.crumb.metadata_values.encode` does.
fn encode_shape(shape: &Shape) -> Vec<u8> {
    let metadata = match &shape.body {
        ShapeBody::Record(fields) => Meta::Map(vec![
            ("kind", Meta::Text("Record")),
            ("nominal", Meta::Binary(shape.selector)),
            ("path", Meta::Text(shape.path)),
            ("properties", Meta::Map(Vec::new())),
            ("fields", Meta::List(fields.iter().map(field_metadata).collect())),
        ]),
        ShapeBody::Enum(variants) => Meta::Map(vec![
            ("kind", Meta::Text("Enum")),
            ("nominal", Meta::Binary(shape.selector)),
            ("path", Meta::Text(shape.path)),
            ("properties", Meta::Map(Vec::new())),
            ("variants", Meta::List(variants.iter().map(variant_metadata).collect())),
        ]),
    };
    let mut output = Vec::new();
    emit(&metadata, &mut output);
    output
}

/// One required canonical field's complete metadata.
fn field_metadata(field: &ShapeField) -> Meta<'static> {
    Meta::Map(vec![
        ("id", Meta::Binary(field.identity)),
        ("name", Meta::Text(field.name)),
        ("type", Meta::Text(field.storage)),
        ("canonical", Meta::Boolean(true)),
        ("absent", Meta::Text("required")),
        ("properties", Meta::Map(Vec::new())),
    ])
}

/// One closed alternative's identity, name and positional payload metadata.
fn variant_metadata(variant: &ShapeVariant) -> Meta<'static> {
    Meta::Map(vec![
        ("id", Meta::Binary(variant.identity)),
        ("name", Meta::Text(variant.name)),
        (
            "fields",
            Meta::List(variant.fields.iter().map(field_metadata).collect()),
        ),
        ("properties", Meta::Map(Vec::new())),
    ])
}

/// Emit one metadata node: a type tag, then its framed payload; map entries in UTF-8 key order.
fn emit(meta: &Meta<'_>, output: &mut Vec<u8>) {
    match meta {
        Meta::Boolean(flag) => output.extend_from_slice(if *flag { &[1, 1] } else { &[1, 0] }),
        Meta::Text(text) => {
            output.push(3);
            framed(text.as_bytes(), output);
        }
        Meta::Binary(data) => {
            output.push(4);
            framed(data, output);
        }
        Meta::List(items) => {
            output.push(5);
            unsigned(items.len(), output);
            for item in items {
                emit(item, output);
            }
        }
        Meta::Map(entries) => {
            output.push(6);
            unsigned(entries.len(), output);
            let mut ordered: Vec<&(&'static str, Meta<'_>)> = entries.iter().collect();
            ordered.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
            for (key, value) in ordered {
                framed(key.as_bytes(), output);
                emit(value, output);
            }
        }
    }
}

/// A byte length in shortest native unsigned form, then the bytes.
fn framed(data: &[u8], output: &mut Vec<u8>) {
    unsigned(data.len(), output);
    output.extend_from_slice(data);
}

/// A count in Crumb's shortest native unsigned form: little-endian base-128 groups, high bit set on all but the last.
fn unsigned(count: usize, output: &mut Vec<u8>) {
    let mut value = count;
    loop {
        let group = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            output.push(group);
            return;
        }
        output.push(group | 0x80);
    }
}

// ---- Logical values ----

/// One logical Crumb value of the native-plan profile, in the Incan `Value` carrier's shape.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Wire {
    NoneValue,
    Some(Vec<Wire>),
    Boolean(bool),
    Signed(i128),
    Unsigned(u128),
    Float(i64, Vec<u8>),
    Text(String),
    Binary(Vec<u8>),
    List(Vec<Wire>),
    Tuple(Vec<Wire>),
    Record(Vec<WireField>),
    Enum(Vec<u8>, Vec<WireField>),
    /// A value of a kind this profile never declares; every accessor refuses it.
    Other,
}

/// One present, required, canonical field of a record or alternative.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WireField {
    pub identity: Vec<u8>,
    pub value: Wire,
}

impl Wire {
    /// A signed compiler integer as a Crumb i64.
    pub(crate) fn integer(value: i64) -> Self {
        Wire::Signed(i128::from(value))
    }

    /// IEEE binary64 bits as Crumb's logical big-endian float input.
    pub(crate) fn float(value: f64) -> Self {
        Wire::Float(64, value.to_bits().to_be_bytes().to_vec())
    }
}

/// Bind one present logical value to a required field contract.
pub(crate) fn required(identity: &[u8], value: Wire) -> WireField {
    WireField {
        identity: identity.to_vec(),
        value,
    }
}

/// The identities of positional payload fields `0` to `7`: the first six bytes of the sha256 of each index.
const POSITIONAL: [&[u8]; 8] = [
    b"_\xec\xebf\xff\xc8",
    b"k\x86\xb2s\xff4",
    b"\xd4s^:&^",
    b"N\x07@\x85b\xbe",
    b"K\x22ww\xd4\xdd",
    b"\xef-\x12}\xe3{",
    b"\xe7\xf6\xc0\x11wn",
    b"y\x02i\x9b\xe4,",
];

/// An alternative with no payload.
pub(crate) fn unit(identity: &[u8]) -> Wire {
    Wire::Enum(identity.to_vec(), Vec::new())
}

/// An alternative whose payload values take the positional field identities in order.
pub(crate) fn alternative(identity: &[u8], values: Vec<Wire>) -> Wire {
    let fields = values
        .into_iter()
        .zip(POSITIONAL)
        .map(|(value, position)| required(position, value))
        .collect();
    Wire::Enum(identity.to_vec(), fields)
}

/// A list of projected elements, refusing the first element that does not project.
pub(crate) fn list<T>(values: &[T], each: impl FnMut(&T) -> Result<Wire, String>) -> Result<Wire, String> {
    Ok(Wire::List(values.iter().map(each).collect::<Result<_, _>>()?))
}

/// Absence, or one present projected value.
pub(crate) fn optional<T>(value: &Option<T>, each: impl FnOnce(&T) -> Result<Wire, String>) -> Result<Wire, String> {
    Ok(match value {
        Some(item) => Wire::Some(vec![each(item)?]),
        None => Wire::NoneValue,
    })
}

/// Read each element of one list.
pub(crate) fn read_list<T>(value: &Wire, mut each: impl FnMut(&Wire) -> Result<T, String>) -> Result<Vec<T>, String> {
    let elements = items(value)?;
    // Sized up front: collecting through `Result` would grow the vector by doubling and keep the spare capacity.
    let mut read = Vec::with_capacity(elements.len());
    for element in elements {
        read.push(each(element)?);
    }
    Ok(read)
}

/// A schema-admitted record's fields in identity order.
pub(crate) fn wire_fields(value: &Wire, count: usize) -> Result<&[WireField], String> {
    match value {
        Wire::Record(fields) if fields.len() == count => Ok(fields),
        _ => Err("invalid native plan record".to_owned()),
    }
}

/// An admitted alternative's identity and its payload fields in identity order.
pub(crate) fn wire_variant(value: &Wire) -> Result<(&[u8], &[WireField]), String> {
    match value {
        Wire::Enum(identity, fields) => Ok((identity, fields)),
        _ => Err("invalid native plan alternative".to_owned()),
    }
}

/// One required boolean.
pub(crate) fn boolean_value(value: &Wire) -> Result<bool, String> {
    match value {
        Wire::Boolean(flag) => Ok(*flag),
        _ => Err("expected native plan boolean".to_owned()),
    }
}

/// A signed integer admitted under the i64 field contract.
pub(crate) fn number(value: &Wire) -> Result<i64, String> {
    match value {
        // The i64 field contract admits only values in range; the reference resizes with wrapping.
        Wire::Signed(item) => Ok(*item as i64),
        _ => Err("expected signed compiler graph integer".to_owned()),
    }
}

/// An unsigned integer that fits u64.
pub(crate) fn unsigned_value(value: &Wire) -> Result<u64, String> {
    match value {
        Wire::Unsigned(item) => u64::try_from(*item).map_err(|_| "expected native plan unsigned integer".to_owned()),
        _ => Err("expected native plan unsigned integer".to_owned()),
    }
}

/// One required text field, without coercion.
pub(crate) fn text(value: &Wire) -> Result<String, String> {
    match value {
        Wire::Text(item) => Ok(item.clone()),
        _ => Err("expected compiler graph text".to_owned()),
    }
}

/// One list; list and tuple have different Crumb representations.
pub(crate) fn items(value: &Wire) -> Result<&[Wire], String> {
    match value {
        Wire::List(elements) => Ok(elements),
        _ => Err("expected compiler graph list".to_owned()),
    }
}

/// A fixed tuple of exactly `expected` elements.
pub(crate) fn tuple_items(value: &Wire, expected: usize) -> Result<&[Wire], String> {
    match value {
        Wire::Tuple(elements) if elements.len() == expected => Ok(elements),
        Wire::Tuple(_) => Err("compiler graph tuple has the wrong arity".to_owned()),
        _ => Err("expected compiler graph tuple".to_owned()),
    }
}

/// IEEE binary64 from Crumb's big-endian float bits.
pub(crate) fn read_float(value: &Wire) -> Result<f64, String> {
    if let Wire::Float(64, data) = value
        && let Ok(bits) = <[u8; 8]>::try_from(data.as_slice())
    {
        return Ok(f64::from_bits(u64::from_be_bytes(bits)));
    }
    Err("expected native plan binary64".to_owned())
}

/// Absence, or exactly one present value read by `read`.
pub(crate) fn read_optional<T>(
    value: &Wire,
    read: impl FnOnce(&[Wire]) -> Result<T, String>,
) -> Result<Option<T>, String> {
    match value {
        Wire::NoneValue => Ok(None),
        Wire::Some(children) if children.len() == 1 => Ok(Some(read(children)?)),
        _ => Err("invalid native plan option".to_owned()),
    }
}

// ---- Budget and Crumb diagnostics ----

/// The caller-owned allowance of one record operation, as `std.primitives.crumb.Budget` spends it.
struct Budget {
    bytes: i64,
    elements: i64,
    depth: i64,
}

impl Budget {
    /// The graph record bound: four times the record byte limit, the shared element bound and nesting depth.
    fn graph() -> Self {
        Self {
            bytes: MAX_RECORD_BYTES * 4,
            elements: ELEMENTS,
            depth: DEPTH,
        }
    }

    /// Spend bytes, refusing a negative count or one past the allowance.
    fn charge_bytes(&mut self, count: i64) -> Result<(), CrumbError> {
        if count < 0 {
            return Err(CrumbError::InvalidInput("byte count"));
        }
        if count > self.bytes {
            return Err(CrumbError::Limit("bytes".to_owned()));
        }
        self.bytes -= count;
        Ok(())
    }

    /// Spend logical elements, refusing a negative count or one past the allowance.
    fn charge_elements(&mut self, count: i64) -> Result<(), CrumbError> {
        if count < 0 {
            return Err(CrumbError::InvalidInput("element count"));
        }
        if count > self.elements {
            return Err(CrumbError::Limit("elements".to_owned()));
        }
        self.elements -= count;
        Ok(())
    }
}

/// The two Crumb framing failures a plan record operation can meet, with `CrumbError.message()` text.
enum CrumbError {
    InvalidInput(&'static str),
    Limit(String),
}

impl CrumbError {
    /// The category prefix and detail, as Crumb spells them.
    fn message(&self) -> String {
        match self {
            CrumbError::InvalidInput(detail) => format!("invalid Crumb input for {detail}"),
            CrumbError::Limit(detail) => format!("Crumb request limit exceeded: {detail}"),
        }
    }

    /// The binary failure category a primitive failure becomes at a read boundary.
    fn binary_kind(&self) -> &'static str {
        match self {
            CrumbError::InvalidInput(_) => "invalid_cell",
            CrumbError::Limit(_) => "limit_exceeded",
        }
    }
}

/// Every binary failure category Crumb's reader names; any other native category reads as an invalid cell.
const BINARY_FAILURES: &[&str] = &[
    "bad_magic",
    "unsupported_profile",
    "reserved_flags",
    "noncanonical_integer",
    "integer_overflow",
    "truncated",
    "trailing_bytes",
    "missing_description",
    "description_mismatch",
    "duplicate_selector",
    "description_order",
    "description_index",
    "invalid_description",
    "invalid_selector",
    "duplicate_member",
    "member_position",
    "member_order",
    "invalid_offsets",
    "invalid_offset_width",
    "nonminimal_offset_width",
    "invalid_cell",
    "invalid_utf8",
    "bitmap_padding",
    "redundant_presence",
    "noncanonical_nan",
    "arity_mismatch",
    "duplicate_key",
    "collection_order",
    "union_index",
    "variant_index",
    "seal_required",
    "seal_count",
    "seal_mismatch",
    "limit_exceeded",
    "missing_field",
    "record_index",
];

/// `BinaryError.message()` for a positioned native refusal.
fn refusal(category: &str, offset: i64) -> String {
    let kind = BINARY_FAILURES
        .iter()
        .find(|known| **known == category)
        .copied()
        .unwrap_or("invalid_cell");
    binary_message(kind, offset)
}

/// Crumb's binary failure text: the category and byte position, never input contents.
fn binary_message(kind: &str, offset: i64) -> String {
    format!("Crumb binary {kind} at byte {offset}")
}

/// Charge a native read's allowance and surface its positioned refusal, as `row_values.settle` does.
fn settle(budget: &mut Budget, used: (i64, i64), category: &str, offset: i64) -> Result<(), String> {
    budget
        .charge_bytes(used.0)
        .map_err(|error| binary_message(error.binary_kind(), offset))?;
    budget
        .charge_elements(used.1)
        .map_err(|error| binary_message(error.binary_kind(), offset))?;
    if category.is_empty() {
        return Ok(());
    }
    Err(refusal(category, offset))
}

// ---- Writing ----

/// Write one row through the native writer under the graph record allowance, as `write_rows` does.
pub(crate) fn write_row(schema: &Schema, key: &[u8], value: Wire) -> Result<Vec<u8>, String> {
    let mut budget = Budget::graph();
    let mut request = RowWriteRequest::new(false);
    for description in schema.descriptions() {
        request.description(description.clone(), false);
    }
    request.record(key.to_vec());
    append(&mut request, value);
    request.allow(budget.bytes, budget.elements, budget.depth);
    let result = request.write();
    budget
        .charge_bytes(result.bytes_charged)
        .map_err(|error| error.message())?;
    budget
        .charge_elements(result.elements_charged)
        .map_err(|error| error.message())?;
    if result.category.is_empty() {
        return Ok(result.data);
    }
    if result.category == "limit_exceeded" {
        let cause = if result.cause.is_empty() {
            "native row writer".to_owned()
        } else {
            result.cause
        };
        return Err(CrumbError::Limit(cause).message());
    }
    Err(format!("invalid Crumb input for {}", result.category))
}

/// One native node with only a code and a count.
fn node(request: &mut RowWriteRequest, code: i64, count: usize) {
    let count = i64::try_from(count).unwrap_or(i64::MAX);
    request.node(code, count, false, 0, 0, 0, 0, Vec::new(), Vec::new(), String::new());
}

/// Append one complete logical value in pre-order, consuming it so payloads move into the request.
fn append(request: &mut RowWriteRequest, value: Wire) {
    match value {
        Wire::NoneValue => node(request, 0, 0),
        Wire::Some(items) => {
            node(request, 1, items.len());
            items.into_iter().for_each(|item| append(request, item));
        }
        Wire::Boolean(flag) => request.node(2, 0, flag, 0, 0, 0, 0, Vec::new(), Vec::new(), String::new()),
        Wire::Signed(number) => request.node(3, 0, false, number, 0, 0, 0, Vec::new(), Vec::new(), String::new()),
        Wire::Unsigned(number) => request.node(4, 0, false, 0, number, 0, 0, Vec::new(), Vec::new(), String::new()),
        Wire::Float(width, bits) => request.node(5, 0, false, 0, 0, width, 0, bits, Vec::new(), String::new()),
        Wire::Text(text) => request.node(7, 0, false, 0, 0, 0, 0, Vec::new(), Vec::new(), text),
        Wire::Binary(data) => request.node(8, 0, false, 0, 0, 0, 0, data, Vec::new(), String::new()),
        Wire::List(items) => {
            node(request, 10, items.len());
            items.into_iter().for_each(|item| append(request, item));
        }
        Wire::Tuple(items) => {
            node(request, 11, items.len());
            items.into_iter().for_each(|item| append(request, item));
        }
        Wire::Record(fields) => {
            node(request, 14, fields.len());
            append_fields(request, fields);
        }
        Wire::Enum(identity, fields) => {
            let count = i64::try_from(fields.len()).unwrap_or(i64::MAX);
            request.node(15, count, false, 0, 0, 0, 0, identity, Vec::new(), String::new());
            append_fields(request, fields);
        }
        Wire::Other => request.node(-1, 0, false, 0, 0, 0, 0, Vec::new(), Vec::new(), String::new()),
    }
}

/// Append each field node and its value.
fn append_fields(request: &mut RowWriteRequest, fields: Vec<WireField>) {
    for field in fields {
        request.node(
            18,
            0,
            true,
            0,
            0,
            0,
            0,
            field.identity,
            Vec::new(),
            "required".to_owned(),
        );
        append(request, field.value);
    }
}

// ---- Reading ----

/// One admitted envelope and the allowance left after admitting it and copying its dictionary.
pub(crate) struct AdmittedRow {
    admission: RowAdmission,
    budget: Budget,
}

/// The modes of an admitted envelope's description dictionary.
pub(crate) struct Dictionary {
    pub entries: usize,
    pub inline: bool,
}

/// Admit one complete envelope against `schema`, as `crumb_rows.admit_rows` does, and copy its dictionary modes.
pub(crate) fn admit_row(encoded: &[u8], schema: &Schema, maximum: i64) -> Result<(AdmittedRow, Dictionary), String> {
    if encoded.len() as u64 > maximum as u64 {
        return Err("encoded Crumb rows exceed their byte boundary".to_owned());
    }
    let mut budget = Budget::graph();
    let mut request = RowRequest::new(encoded.to_vec(), false, true);
    for description in schema.descriptions() {
        request.supply(description.clone());
    }
    request.allow(budget.bytes, budget.elements, budget.depth);
    let admission = request.admit();
    let used = (admission.bytes_charged, admission.elements_charged);
    settle(&mut budget, used, &admission.category, admission.offset)?;
    let mut dictionary = Dictionary {
        entries: 0,
        inline: false,
    };
    let count = admission.description_count();
    for index in 0..count {
        let read = admission.description(index, budget.bytes, budget.elements, budget.depth);
        settle(
            &mut budget,
            (read.bytes_charged, read.elements_charged),
            &read.category,
            read.offset,
        )?;
        dictionary.entries += 1;
        dictionary.inline |= read.supplied < 0;
    }
    Ok((AdmittedRow { admission, budget }, dictionary))
}

impl AdmittedRow {
    /// The admitted record count.
    pub(crate) fn records(&self) -> i64 {
        self.admission.records
    }

    /// Materialize the root row's selector and value under the remaining allowance.
    pub(crate) fn read_root(mut self) -> Result<(Vec<u8>, Wire), String> {
        let mut read = self
            .admission
            .read_record(0, self.budget.bytes, self.budget.elements, self.budget.depth);
        let used = (read.bytes_charged, read.elements_charged);
        settle(&mut self.budget, used, &read.category, read.offset)?;
        let value = build(&mut read)?;
        Ok((self.admission.key(0), value))
    }
}

/// Admit exactly one `root` row against a closed reference dictionary, as `plan_wire_rows.PreparedWireReader` does,
/// returning the dictionary slot each written description resolved to.
pub(crate) fn admit_root(
    encoded: &[u8],
    root: &[u8],
    schema: &Schema,
    name: &str,
) -> Result<(AdmittedRow, Vec<i64>), String> {
    if encoded.len() as u64 > MAX_RECORD_BYTES as u64 {
        return Err(format!("{name} exceeds its byte limit"));
    }
    let mut budget = Budget::graph();
    let mut request = RowRequest::new(encoded.to_vec(), false, true);
    for description in schema.descriptions() {
        request.supply(description.clone());
    }
    request.allow(budget.bytes, budget.elements, budget.depth);
    let admission = request.admit();
    budget
        .charge_bytes(admission.bytes_charged)
        .map_err(|error| error.message())?;
    budget
        .charge_elements(admission.elements_charged)
        .map_err(|error| error.message())?;
    if !admission.admitted() {
        return Err(refusal(&admission.category, admission.offset));
    }
    let count = admission.description_count();
    if admission.records != 1 || admission.key(0) != root || usize::try_from(count).ok() != Some(schema.len()) {
        return Err(format!(
            "{name} requires one root and its complete declaration dictionary"
        ));
    }
    let mut slots = Vec::with_capacity(schema.len());
    for index in 0..count {
        let slot = admission.reference_index(index);
        if slot < 0 {
            return Err(format!("{name} requires referenced compiler descriptions"));
        }
        slots.push(slot);
    }
    Ok((AdmittedRow { admission, budget }, slots))
}

/// The envelope range of one root field's cell: its origin and length, or `None` when the field is absent.
pub(crate) struct FieldCell {
    pub present: bool,
    pub origin: i64,
    pub length: i64,
}

impl AdmittedRow {
    /// Locate one root field's encoded cell under the remaining allowance.
    pub(crate) fn field_cell(&mut self, identity: &[u8]) -> Result<FieldCell, String> {
        let read = self.admission.field_cell(
            0,
            identity.to_vec(),
            self.budget.bytes,
            self.budget.elements,
            self.budget.depth,
        );
        settle(
            &mut self.budget,
            (read.bytes_charged, read.elements_charged),
            &read.category,
            read.offset,
        )?;
        Ok(FieldCell {
            present: read.present,
            origin: read.origin,
            length: read.length,
        })
    }

    /// Materialize one root field's value under the remaining allowance.
    pub(crate) fn read_field(&mut self, identity: &[u8]) -> Result<Wire, String> {
        let mut read = self.admission.read_field(
            0,
            identity.to_vec(),
            self.budget.bytes,
            self.budget.elements,
            self.budget.depth,
        );
        settle(
            &mut self.budget,
            (read.bytes_charged, read.elements_charged),
            &read.category,
            read.offset,
        )?;
        build(&mut read)
    }
}

/// Build one logical value from a successful native read's pre-order nodes.
fn build(read: &mut RowRead) -> Result<Wire, String> {
    let code = read.next();
    let count = usize::try_from(read.count()).unwrap_or(0);
    Ok(match code {
        0 => Wire::NoneValue,
        // A present option is followed by exactly one value.
        1 => Wire::Some(vec![build(read)?]),
        2 => Wire::Boolean(read.flag()),
        3 => Wire::Signed(read.signed()),
        4 => Wire::Unsigned(read.unsigned()),
        5 => Wire::Float(read.number(), read.bytes()),
        7 => Wire::Text(read.text()),
        8 => Wire::Binary(read.bytes()),
        10 => Wire::List(build_values(read, count)?),
        11 => Wire::Tuple(build_values(read, count)?),
        14 => Wire::Record(build_fields(read, count)?),
        15 => {
            let identity = read.bytes();
            Wire::Enum(identity, build_fields(read, count)?)
        }
        -1 => return Err(binary_message("invalid_cell", 0)),
        _ => Wire::Other,
    })
}

/// Build `count` consecutive values.
fn build_values(read: &mut RowRead, count: usize) -> Result<Vec<Wire>, String> {
    (0..count).map(|_| build(read)).collect()
}

/// Build `count` consecutive field nodes, each followed by its value.
fn build_fields(read: &mut RowRead, count: usize) -> Result<Vec<WireField>, String> {
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        if read.next() != 18 {
            return Err(binary_message("invalid_cell", 0));
        }
        let identity = read.bytes();
        fields.push(WireField {
            identity,
            value: build(read)?,
        });
    }
    Ok(fields)
}

// ---- Plan wire rows and compiler graph rows ----

/// Write one plan-profile row under its root selector, refusing a row past the record byte limit.
pub(crate) fn write_wire(key: &[u8], schema: &Schema, value: Wire) -> Result<Vec<u8>, String> {
    let encoded = write_row(schema, key, value)?;
    if encoded.len() as u64 > MAX_RECORD_BYTES as u64 {
        return Err("native plan wire exceeds its byte limit".to_owned());
    }
    Ok(encoded)
}

/// Authenticate every reachable schema and the sole root selector before interpreting the native payload.
pub(crate) fn read_wire(encoded: &[u8], key: &[u8], schema: &Schema) -> Result<Wire, String> {
    let (row, dictionary) = admit_row(encoded, schema, MAX_RECORD_BYTES)?;
    if row.records() != 1 || dictionary.entries != schema.len() {
        return Err("native plan wire requires one row and its exact declaration closure".to_owned());
    }
    if dictionary.inline {
        return Err("native plan wire requires referenced compiler schemas".to_owned());
    }
    let (selector, value) = row.read_root()?;
    if selector != key {
        return Err("unexpected native plan root selector".to_owned());
    }
    Ok(value)
}

/// Serialize exactly one typed compiler graph record, as `graph_crumb_fields.encode_graph_row` does.
pub(crate) fn encode_graph_row(key: &[u8], schema: &Schema, fields: Vec<WireField>) -> Result<Vec<u8>, String> {
    let encoded = write_row(schema, key, Wire::Record(fields))?;
    if encoded.len() as u64 > MAX_RECORD_BYTES as u64 {
        return Err(format!(
            "encoded Crumb rows need {} bytes, limit {MAX_RECORD_BYTES}",
            encoded.len()
        ));
    }
    Ok(encoded)
}

/// Require exactly the expected graph row, selector and field count before any domain projection.
pub(crate) fn decode_graph_row(
    encoded: &[u8],
    key: &[u8],
    schema: &Schema,
    count: usize,
) -> Result<Vec<WireField>, String> {
    let (row, dictionary) = admit_row(encoded, schema, MAX_RECORD_BYTES)?;
    if row.records() != 1 || dictionary.entries != 1 {
        return Err("unexpected compiler graph Crumb row".to_owned());
    }
    if dictionary.inline {
        return Err("unexpected inline compiler graph description".to_owned());
    }
    let (selector, value) = row.read_root()?;
    if selector != key {
        return Err("unexpected compiler graph Crumb row".to_owned());
    }
    match value {
        Wire::Record(fields) if fields.len() == count => Ok(fields),
        Wire::Record(_) => Err("compiler graph Crumb row has the wrong field count".to_owned()),
        _ => Err("expected compiler graph Crumb record".to_owned()),
    }
}
