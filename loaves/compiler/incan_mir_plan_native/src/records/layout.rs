//! Retain every concrete native layout field, checked carrier type and generic instance request without deriving
//! storage or semantic identities during decoding.

use crate::plan::{EnumDeclaration, ModelDeclaration, ZipCarrier};
use crate::positions::WirePositions;
use crate::record_types::CheckedType;
use crate::records::wire_layout_records::{
    read_enum_declaration, read_model_declaration, read_zip_carrier, wire_enum_declaration, wire_model_declaration,
    wire_zip_carrier,
};
use crate::records::wire_layout_type_records::{
    StoredInstanceRequest, read_stored_instance_request, wire_checked_type, wire_stored_instance_request,
};
use crate::records::wire_schema::{
    STORED_INSTANCE_REQUEST_SCHEMA, STORED_LAYOUT_CHECKED_TYPE_SCHEMA, STORED_LAYOUT_ENUM_DECLARATION_SCHEMA,
    STORED_LAYOUT_MODEL_DECLARATION_SCHEMA, STORED_LAYOUT_ZIP_CARRIER_SCHEMA,
};
use crate::types::{SourceSpan, source_span};
use crate::wire::{Wire, number, read_wire, required, text, wire_fields, write_wire};

/// The root selector of one model layout.
const MODEL_ROOT: &[u8] = b"\x9c\xff\xb7\x00\xc4\x1f\x00";
/// The root selector of one enum layout.
const ENUM_ROOT: &[u8] = b"\x22x[\xf6\xa0\xa6\x00";
/// The root selector of one zip carrier.
const ZIP_ROOT: &[u8] = b">i\xa6U\x16 \x00";
/// The root selector of one whole checked carrier type.
const CHECKED_LAYOUT_ROOT: &[u8] = b"\xd3\x06\xf4\xa0\x6a\xec\x00";
/// The root selector of one generic instance request.
const INSTANCE_REQUEST_ROOT: &[u8] = b"\x78\x18\xc1\xb2\x6b\x0b\x00";

/// Encode complete model inventories in their native order.
pub fn encode_models(values: &[ModelDeclaration]) -> Result<Vec<Vec<u8>>, String> {
    values
        .iter()
        .map(|value| {
            write_wire(
                MODEL_ROOT,
                &STORED_LAYOUT_MODEL_DECLARATION_SCHEMA,
                wire_model_declaration(value)?,
            )
        })
        .collect()
}

/// Restore complete model inventories without checking or lowering source.
pub fn decode_models(values: &[Vec<u8>]) -> Result<Vec<ModelDeclaration>, String> {
    values
        .iter()
        .map(|value| {
            let row = read_wire(value, MODEL_ROOT, &STORED_LAYOUT_MODEL_DECLARATION_SCHEMA)?;
            read_model_declaration(&row, &WirePositions::preserve())
        })
        .collect()
}

/// Encode complete enum inventories in their native order.
pub fn encode_enums(values: &[EnumDeclaration]) -> Result<Vec<Vec<u8>>, String> {
    values
        .iter()
        .map(|value| {
            write_wire(
                ENUM_ROOT,
                &STORED_LAYOUT_ENUM_DECLARATION_SCHEMA,
                wire_enum_declaration(value)?,
            )
        })
        .collect()
}

/// Restore complete enum inventories without checking or lowering source.
pub fn decode_enums(values: &[Vec<u8>]) -> Result<Vec<EnumDeclaration>, String> {
    values
        .iter()
        .map(|value| {
            let row = read_wire(value, ENUM_ROOT, &STORED_LAYOUT_ENUM_DECLARATION_SCHEMA)?;
            read_enum_declaration(&row, &WirePositions::preserve())
        })
        .collect()
}

/// Encode complete zip-carrier inventories in their native order.
pub fn encode_zip_carriers(values: &[ZipCarrier]) -> Result<Vec<Vec<u8>>, String> {
    values
        .iter()
        .map(|value| write_wire(ZIP_ROOT, &STORED_LAYOUT_ZIP_CARRIER_SCHEMA, wire_zip_carrier(value)?))
        .collect()
}

/// Restore complete zip-carrier inventories without checking or lowering source.
pub fn decode_zip_carriers(values: &[Vec<u8>]) -> Result<Vec<ZipCarrier>, String> {
    values
        .iter()
        .map(|value| {
            let row = read_wire(value, ZIP_ROOT, &STORED_LAYOUT_ZIP_CARRIER_SCHEMA)?;
            read_zip_carrier(&row, &WirePositions::preserve())
        })
        .collect()
}

/// Authenticate a whole checked carrier type with its real closed native-plan wire schema.
pub fn encode_checked_layout(value: &CheckedType) -> Result<Vec<u8>, String> {
    write_wire(
        CHECKED_LAYOUT_ROOT,
        &STORED_LAYOUT_CHECKED_TYPE_SCHEMA,
        wire_checked_type(value)?,
    )
}

/// Retain the declaration identity, closed argument structures, callable slots and role together.
pub fn encode_instance_request(
    declaration: &str,
    arguments: &[CheckedType],
    slots: &[i64],
    original: bool,
) -> Result<Vec<u8>, String> {
    let stored = StoredInstanceRequest {
        declaration: declaration.to_owned(),
        arguments: arguments.to_vec(),
        owned_callable_slots: slots.to_vec(),
        original_callable: original,
    };
    write_wire(
        INSTANCE_REQUEST_ROOT,
        &STORED_INSTANCE_REQUEST_SCHEMA,
        wire_stored_instance_request(&stored)?,
    )
}

/// Restore only the compiler's exact stored generic request and checked argument types.
pub fn decode_instance_request(encoded: &[u8]) -> Result<(String, Vec<CheckedType>, Vec<i64>, bool), String> {
    let row = read_wire(encoded, INSTANCE_REQUEST_ROOT, &STORED_INSTANCE_REQUEST_SCHEMA)?;
    let stored = read_stored_instance_request(&row, &WirePositions::preserve())?;
    Ok((
        stored.declaration,
        stored.arguments,
        stored.owned_callable_slots,
        stored.original_callable,
    ))
}

/// Allocation-free coordinates of a concrete layout declaration or field.
pub(crate) fn wire_source_span(value: &SourceSpan) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b";\x9c5\x8f6\xf0", Wire::Text(value.file.as_str().to_owned())),
        required(b"8\xa9\xc1\xe7!X", Wire::integer(value.line)),
        required(b"\x88\x1d_\xb6\x00\x90", Wire::integer(value.column)),
    ]))
}

/// Restore native shared-file storage through its owning constructor, after the read's coordinate mapping.
pub(crate) fn read_source_span(value: &Wire, positions: &WirePositions) -> Result<SourceSpan, String> {
    let fields = wire_fields(value, 3)?;
    let (file, line, column) = positions.apply(
        text(&fields[1].value)?,
        number(&fields[0].value)?,
        number(&fields[2].value)?,
    )?;
    Ok(source_span(&file, line, column))
}
