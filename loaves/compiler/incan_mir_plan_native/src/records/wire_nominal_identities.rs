//! Projections of the native-plan profile's `nominal_identities` shapes onto Crumb values.
//!
//! Each projection writes or reads one stored shape in its declared field and alternative order; record fields
//! are read from their identity-ordered slots. A shape here and its description in `wire_schema.rs` change
//! together.

use crate::records::prelude::*;

/// Project one `NominalKind` alternative onto its identity and positional payload.
pub(crate) fn wire_nominal_kind(value: &NominalKind) -> Result<Wire, String> {
    Ok(match value {
        NominalKind::Model => unit(b"^,aL#\xf0"),
        NominalKind::Enum => unit(b"T\x10\x8d\xaf\xaa\xc5"),
        NominalKind::Static => unit(b"\x1d,s\xdc\x01\xf2"),
    })
}

/// Read one `NominalKind` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_nominal_kind(value: &Wire, positions: &WirePositions) -> Result<NominalKind, String> {
    let (identity, fields) = wire_variant(value)?;
    read_nominal_kind_alternative(identity, fields, positions)
}

/// Restore one `NominalKind` alternative from part 0 of its identity table.
fn read_nominal_kind_alternative(
    identity: &[u8],
    fields: &[WireField],
    _positions: &WirePositions,
) -> Result<NominalKind, String> {
    Ok(match identity {
        b"^,aL#\xf0" if fields.is_empty() => NominalKind::Model,
        b"T\x10\x8d\xaf\xaa\xc5" if fields.is_empty() => NominalKind::Enum,
        b"\x1d,s\xdc\x01\xf2" if fields.is_empty() => NominalKind::Static,
        _ => return Err("unknown or incomplete NominalKind alternative".to_owned()),
    })
}

/// Project one `ProductNominal` onto its schema-admitted Crumb record.
pub(crate) fn wire_product_nominal(value: &ProductNominal) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x0b'\xc1X\xfe\xb3", wire_nominal_kind(&value.kind)?),
        required(b"h\x9fjbs\x84", Wire::Text(value.identity.clone())),
        required(b"\x0b\xf4t\x89cc", Wire::Text(value.digest.clone())),
    ]))
}

/// Read one `ProductNominal` record in its identity-ordered field slots.
pub(crate) fn read_product_nominal(value: &Wire, positions: &WirePositions) -> Result<ProductNominal, String> {
    let fields = wire_fields(value, 3)?;
    Ok(ProductNominal {
        kind: read_nominal_kind(&fields[0].value, positions)?,
        identity: text(&fields[2].value)?,
        digest: text(&fields[1].value)?,
    })
}
