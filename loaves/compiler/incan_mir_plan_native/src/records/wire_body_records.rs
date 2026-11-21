//! Projections of the native-plan profile's `body_records` shapes onto Crumb values.
//!
//! Each projection writes or reads one stored shape in its declared field and alternative order; record fields
//! are read from their identity-ordered slots. A shape here and its description in `wire_schema.rs` change
//! together.

use crate::records::prelude::*;

/// The stored form of `StoredConstant` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredConstant {
    Int(i64),
    Float(f64),
    Bool(bool),
    Unit,
    Text(String),
    Numeric(String, StoredType),
    TypeToken(Vec<StoredType>),
}

/// Project one `StoredConstant` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_constant(value: &StoredConstant) -> Result<Wire, String> {
    Ok(match value {
        StoredConstant::Int(field0) => alternative(b"\x0b_`\x80p\xc6", vec![Wire::integer(*field0)]),
        StoredConstant::Float(field0) => alternative(b"\x1ai<\x00\xc4\x0c", vec![Wire::float(*field0)]),
        StoredConstant::Bool(field0) => alternative(b"I\xf4\x11\xf0\xa1\xa7", vec![Wire::Boolean(*field0)]),
        StoredConstant::Unit => unit(b"NTY`\xf1\xbf"),
        StoredConstant::Text(field0) => alternative(b"q\x98\x8cM\x8e\x08", vec![Wire::Text(field0.clone())]),
        StoredConstant::Numeric(field0, field1) => alternative(
            b"6u-l\xd1G",
            vec![Wire::Text(field0.clone()), wire_stored_type(field1)?],
        ),
        StoredConstant::TypeToken(field0) => alternative(b"\xeb\x90\x0dwd\xd0", vec![list(field0, wire_stored_type)?]),
    })
}

/// Read one `StoredConstant` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_constant(value: &Wire, positions: &WirePositions) -> Result<StoredConstant, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_constant_alternative(identity, fields, positions)
}

/// Restore one `StoredConstant` alternative from part 0 of its identity table.
fn read_stored_constant_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredConstant, String> {
    Ok(match identity {
        b"\x0b_`\x80p\xc6" if fields.len() == 1 => StoredConstant::Int(number(&fields[0].value)?),
        b"\x1ai<\x00\xc4\x0c" if fields.len() == 1 => StoredConstant::Float(read_float(&fields[0].value)?),
        b"I\xf4\x11\xf0\xa1\xa7" if fields.len() == 1 => StoredConstant::Bool(boolean_value(&fields[0].value)?),
        b"NTY`\xf1\xbf" if fields.is_empty() => StoredConstant::Unit,
        b"q\x98\x8cM\x8e\x08" if fields.len() == 1 => StoredConstant::Text(text(&fields[0].value)?),
        b"6u-l\xd1G" if fields.len() == 2 => {
            StoredConstant::Numeric(text(&fields[0].value)?, read_stored_type(&fields[1].value, positions)?)
        }
        b"\xeb\x90\x0dwd\xd0" if fields.len() == 1 => {
            StoredConstant::TypeToken(read_list(&fields[0].value, |item0| read_stored_type(item0, positions))?)
        }
        _ => return Err("unknown or incomplete StoredConstant alternative".to_owned()),
    })
}

/// The stored form of `StoredFieldStep` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredFieldStep {
    pub(crate) slot: i64,
    pub(crate) ty: StoredType,
    pub(crate) variant: i64,
    pub(crate) dereferences: i64,
}

/// Project one `StoredFieldStep` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_field_step(value: &StoredFieldStep) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"eX\x83\x831\xb7", Wire::integer(value.slot)),
        required(b"O\x9d\xb7\xf8\xe5\x06", wire_stored_type(&value.ty)?),
        required(b"\xcb{\xf5bB\x0b", Wire::integer(value.variant)),
        required(b"3\x8e0\xd7\xaa\x17", Wire::integer(value.dereferences)),
    ]))
}

/// Read one `StoredFieldStep` record in its identity-ordered field slots.
pub(crate) fn read_stored_field_step(value: &Wire, positions: &WirePositions) -> Result<StoredFieldStep, String> {
    let fields = wire_fields(value, 4)?;
    Ok(StoredFieldStep {
        slot: number(&fields[2].value)?,
        ty: read_stored_type(&fields[1].value, positions)?,
        variant: number(&fields[3].value)?,
        dereferences: number(&fields[0].value)?,
    })
}

/// The stored form of `StoredOperand` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredOperand {
    pub(crate) kind: StoredRead,
    pub(crate) span: StoredSpan,
}

/// Project one `StoredOperand` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_operand(value: &StoredOperand) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x0b'\xc1X\xfe\xb3", wire_stored_read(&value.kind)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
    ]))
}

/// Read one `StoredOperand` record in its identity-ordered field slots.
pub(crate) fn read_stored_operand(value: &Wire, positions: &WirePositions) -> Result<StoredOperand, String> {
    let fields = wire_fields(value, 2)?;
    Ok(StoredOperand {
        kind: read_stored_read(&fields[0].value, positions)?,
        span: read_stored_span(&fields[1].value, positions)?,
    })
}

/// The stored form of `StoredPlace` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredPlace {
    pub(crate) local: i64,
    pub(crate) projection: StoredProjection,
    pub(crate) span: StoredSpan,
}

/// Project one `StoredPlace` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_place(value: &StoredPlace) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"%\xbf\x8e\x1a#\x93", Wire::integer(value.local)),
        required(b"\x1b%\x0e\xa1\x99\xbe", wire_stored_projection(&value.projection)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
    ]))
}

/// Read one `StoredPlace` record in its identity-ordered field slots.
pub(crate) fn read_stored_place(value: &Wire, positions: &WirePositions) -> Result<StoredPlace, String> {
    let fields = wire_fields(value, 3)?;
    Ok(StoredPlace {
        local: number(&fields[1].value)?,
        projection: read_stored_projection(&fields[0].value, positions)?,
        span: read_stored_span(&fields[2].value, positions)?,
    })
}

/// The stored form of `StoredProjection` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredProjection {
    Whole,
    Value,
    Overflow,
    NumericValue(StoredType),
    VariantField(i64, i64, StoredType),
    Field(i64, StoredType),
    Deref(StoredType),
    DerefField(i64, StoredType),
    Fields(Vec<StoredFieldStep>),
    DerefFields(Vec<StoredFieldStep>),
}

/// Project one `StoredProjection` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_projection(value: &StoredProjection) -> Result<Wire, String> {
    Ok(match value {
        StoredProjection::Whole => unit(b"\xfdU\x05`\x93\x8c"),
        StoredProjection::Value => unit(b"\x8e7\x95=#\xda"),
        StoredProjection::Overflow => unit(b"\x17\xb8\xdc\xf5>K"),
        StoredProjection::NumericValue(field0) => alternative(b"\x04I\xf5\xd2N?", vec![wire_stored_type(field0)?]),
        StoredProjection::VariantField(field0, field1, field2) => alternative(
            b"\xc0\x01\x8e\x9e\x9f\x16",
            vec![
                Wire::integer(*field0),
                Wire::integer(*field1),
                wire_stored_type(field2)?,
            ],
        ),
        StoredProjection::Field(field0, field1) => alternative(
            b"\xf4_\xc1\xdf\xdc\x96",
            vec![Wire::integer(*field0), wire_stored_type(field1)?],
        ),
        StoredProjection::Deref(field0) => alternative(b"\x12/\x9b\xd7h\xa6", vec![wire_stored_type(field0)?]),
        StoredProjection::DerefField(field0, field1) => alternative(
            b"\x84\xdd\xaa\x99\xf0\x90",
            vec![Wire::integer(*field0), wire_stored_type(field1)?],
        ),
        StoredProjection::Fields(field0) => {
            alternative(b"aoH\xc1\xdb\x19", vec![list(field0, wire_stored_field_step)?])
        }
        StoredProjection::DerefFields(field0) => {
            alternative(b"g\x93\xd8#\xbc\xbe", vec![list(field0, wire_stored_field_step)?])
        }
    })
}

/// Read one `StoredProjection` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_projection(value: &Wire, positions: &WirePositions) -> Result<StoredProjection, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_projection_alternative(identity, fields, positions)
}

/// Restore one `StoredProjection` alternative from part 0 of its identity table.
fn read_stored_projection_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredProjection, String> {
    Ok(match identity {
        b"\xfdU\x05`\x93\x8c" if fields.is_empty() => StoredProjection::Whole,
        b"\x8e7\x95=#\xda" if fields.is_empty() => StoredProjection::Value,
        b"\x17\xb8\xdc\xf5>K" if fields.is_empty() => StoredProjection::Overflow,
        b"\x04I\xf5\xd2N?" if fields.len() == 1 => {
            StoredProjection::NumericValue(read_stored_type(&fields[0].value, positions)?)
        }
        b"\xc0\x01\x8e\x9e\x9f\x16" if fields.len() == 3 => StoredProjection::VariantField(
            number(&fields[0].value)?,
            number(&fields[1].value)?,
            read_stored_type(&fields[2].value, positions)?,
        ),
        b"\xf4_\xc1\xdf\xdc\x96" if fields.len() == 2 => StoredProjection::Field(
            number(&fields[0].value)?,
            read_stored_type(&fields[1].value, positions)?,
        ),
        b"\x12/\x9b\xd7h\xa6" if fields.len() == 1 => {
            StoredProjection::Deref(read_stored_type(&fields[0].value, positions)?)
        }
        b"\x84\xdd\xaa\x99\xf0\x90" if fields.len() == 2 => StoredProjection::DerefField(
            number(&fields[0].value)?,
            read_stored_type(&fields[1].value, positions)?,
        ),
        b"aoH\xc1\xdb\x19" if fields.len() == 1 => StoredProjection::Fields(read_list(&fields[0].value, |item0| {
            read_stored_field_step(item0, positions)
        })?),
        b"g\x93\xd8#\xbc\xbe" if fields.len() == 1 => {
            StoredProjection::DerefFields(read_list(&fields[0].value, |item0| {
                read_stored_field_step(item0, positions)
            })?)
        }
        _ => return Err("unknown or incomplete StoredProjection alternative".to_owned()),
    })
}

/// The stored form of `StoredRead` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredRead {
    Copy(StoredPlace),
    Move(StoredPlace),
    Literal(StoredConstant),
}

/// Project one `StoredRead` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_read(value: &StoredRead) -> Result<Wire, String> {
    Ok(match value {
        StoredRead::Copy(field0) => alternative(b"\xe2\x1f\x93_\x11\xd7", vec![wire_stored_place(field0)?]),
        StoredRead::Move(field0) => alternative(b"n\xcc=\xf6\xbf\xfd", vec![wire_stored_place(field0)?]),
        StoredRead::Literal(field0) => alternative(b"\xa6#\x0c*\x81\xab", vec![wire_stored_constant(field0)?]),
    })
}

/// Read one `StoredRead` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_read(value: &Wire, positions: &WirePositions) -> Result<StoredRead, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_read_alternative(identity, fields, positions)
}

/// Restore one `StoredRead` alternative from part 0 of its identity table.
fn read_stored_read_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredRead, String> {
    Ok(match identity {
        b"\xe2\x1f\x93_\x11\xd7" if fields.len() == 1 => {
            StoredRead::Copy(read_stored_place(&fields[0].value, positions)?)
        }
        b"n\xcc=\xf6\xbf\xfd" if fields.len() == 1 => StoredRead::Move(read_stored_place(&fields[0].value, positions)?),
        b"\xa6#\x0c*\x81\xab" if fields.len() == 1 => {
            StoredRead::Literal(read_stored_constant(&fields[0].value, positions)?)
        }
        _ => return Err("unknown or incomplete StoredRead alternative".to_owned()),
    })
}

/// The stored form of `StoredRvalue` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredRvalue {
    Use(StoredOperand),
    Binary(i64, StoredOperand, StoredOperand),
    Unary(i64, StoredOperand),
    IntToFloat(StoredOperand),
    FloatToInt(StoredOperand),
    BoolToInt(StoredOperand),
    Array(Vec<StoredOperand>),
    Borrow(StoredPlace),
    UnsizeSlice(StoredOperand),
    NativeAlias(StoredOperand, StoredType, StoredType),
    FrozenText(StoredOperand),
    FrozenBytes(Vec<i64>),
    NumericCast(StoredOperand, StoredType, StoredType),
    Enum(i64, i64, Vec<StoredOperand>),
    Discriminant(StoredPlace),
    TagToInt(StoredOperand),
    Model(i64, Vec<StoredOperand>),
    Tuple(Vec<StoredOperand>),
    Range(Vec<StoredOperand>),
    MutBorrow(StoredPlace),
    IsVariant(StoredPlace, i64),
    ReifyFunction(StoredOperand),
    FunctionAddress(StoredOperand),
    ClosureObject(StoredPlace),
    PlannedFunctionItem(String, StoredSpan),
    FrozenCollection(StoredType, Vec<StoredFrozenValue>),
}

/// Project one `StoredRvalue` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_rvalue(value: &StoredRvalue) -> Result<Wire, String> {
    Ok(match value {
        StoredRvalue::Use(field0) => alternative(b"\xc3m\x81\x9e{\xc6", vec![wire_stored_operand(field0)?]),
        StoredRvalue::Binary(field0, field1, field2) => alternative(
            b"\xee\x1c\x82w\xca\xf5",
            vec![
                Wire::integer(*field0),
                wire_stored_operand(field1)?,
                wire_stored_operand(field2)?,
            ],
        ),
        StoredRvalue::Unary(field0, field1) => alternative(
            b"~\x98\x83P\x9d\x02",
            vec![Wire::integer(*field0), wire_stored_operand(field1)?],
        ),
        StoredRvalue::IntToFloat(field0) => alternative(b"9\x15+\x9f}\xe3", vec![wire_stored_operand(field0)?]),
        StoredRvalue::FloatToInt(field0) => alternative(b"\x8b\xc2\xe2\xc7a\x1f", vec![wire_stored_operand(field0)?]),
        StoredRvalue::BoolToInt(field0) => alternative(b"01\xa6\xb1\xeb^", vec![wire_stored_operand(field0)?]),
        StoredRvalue::Array(field0) => alternative(b"\xf0\xcf9\xd0\xbe>", vec![list(field0, wire_stored_operand)?]),
        StoredRvalue::Borrow(field0) => alternative(b".\xdf\xb1h\xb3\x8b", vec![wire_stored_place(field0)?]),
        StoredRvalue::UnsizeSlice(field0) => alternative(b"\xecWL\xe3\xa7d", vec![wire_stored_operand(field0)?]),
        StoredRvalue::NativeAlias(field0, field1, field2) => alternative(
            b"\xdd\xb0\x0b=6\xb4",
            vec![
                wire_stored_operand(field0)?,
                wire_stored_type(field1)?,
                wire_stored_type(field2)?,
            ],
        ),
        StoredRvalue::FrozenText(field0) => alternative(b"\x0fAV\x17\xca$", vec![wire_stored_operand(field0)?]),
        StoredRvalue::FrozenBytes(field0) => alternative(
            b"\xa5\x1aU\xd1;\xee",
            vec![list(field0, |item0| Ok(Wire::integer(*item0)))?],
        ),
        StoredRvalue::NumericCast(field0, field1, field2) => alternative(
            b"\xdeJ\xe4\xd3\x93\xf0",
            vec![
                wire_stored_operand(field0)?,
                wire_stored_type(field1)?,
                wire_stored_type(field2)?,
            ],
        ),
        StoredRvalue::Enum(field0, field1, field2) => alternative(
            b"T\x10\x8d\xaf\xaa\xc5",
            vec![
                Wire::integer(*field0),
                Wire::integer(*field1),
                list(field2, wire_stored_operand)?,
            ],
        ),
        StoredRvalue::Discriminant(field0) => alternative(b"x\xbc\x9c&\x03\x1a", vec![wire_stored_place(field0)?]),
        StoredRvalue::TagToInt(field0) => alternative(b"\xe7#\xb0\xbd\x03\xf7", vec![wire_stored_operand(field0)?]),
        StoredRvalue::Model(field0, field1) => alternative(
            b"^,aL#\xf0",
            vec![Wire::integer(*field0), list(field1, wire_stored_operand)?],
        ),
        StoredRvalue::Tuple(field0) => alternative(b"\xf16\xeex\xc3;", vec![list(field0, wire_stored_operand)?]),
        StoredRvalue::Range(field0) => alternative(b"]\xe7J\x81\xb1\x92", vec![list(field0, wire_stored_operand)?]),
        StoredRvalue::MutBorrow(field0) => alternative(b"\xa87!2_\xde", vec![wire_stored_place(field0)?]),
        StoredRvalue::IsVariant(field0, field1) => {
            alternative(b"ivP2h\xd9", vec![wire_stored_place(field0)?, Wire::integer(*field1)])
        }
        StoredRvalue::ReifyFunction(field0) => alternative(b"S\xe4\x16\xfd%\xcf", vec![wire_stored_operand(field0)?]),
        StoredRvalue::FunctionAddress(field0) => {
            alternative(b"\xc4\xe4\xd4\xc6~\xe2", vec![wire_stored_operand(field0)?])
        }
        StoredRvalue::ClosureObject(field0) => alternative(b"s!\xe9q\x11\xd7", vec![wire_stored_place(field0)?]),
        StoredRvalue::PlannedFunctionItem(field0, field1) => alternative(
            b"^H\x983G\x01",
            vec![Wire::Text(field0.clone()), wire_stored_span(field1)?],
        ),
        StoredRvalue::FrozenCollection(field0, field1) => alternative(
            b"`ar\x85\xa4\xcb",
            vec![wire_stored_type(field0)?, list(field1, wire_stored_frozen_value)?],
        ),
    })
}

/// Read one `StoredRvalue` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_rvalue(value: &Wire, positions: &WirePositions) -> Result<StoredRvalue, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_rvalue_alternative(identity, fields, positions)
}

/// Restore one `StoredRvalue` alternative from part 0 of its identity table.
fn read_stored_rvalue_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredRvalue, String> {
    Ok(match identity {
        b"\xc3m\x81\x9e{\xc6" if fields.len() == 1 => {
            StoredRvalue::Use(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\xee\x1c\x82w\xca\xf5" if fields.len() == 3 => StoredRvalue::Binary(
            number(&fields[0].value)?,
            read_stored_operand(&fields[1].value, positions)?,
            read_stored_operand(&fields[2].value, positions)?,
        ),
        b"~\x98\x83P\x9d\x02" if fields.len() == 2 => StoredRvalue::Unary(
            number(&fields[0].value)?,
            read_stored_operand(&fields[1].value, positions)?,
        ),
        b"9\x15+\x9f}\xe3" if fields.len() == 1 => {
            StoredRvalue::IntToFloat(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\x8b\xc2\xe2\xc7a\x1f" if fields.len() == 1 => {
            StoredRvalue::FloatToInt(read_stored_operand(&fields[0].value, positions)?)
        }
        b"01\xa6\xb1\xeb^" if fields.len() == 1 => {
            StoredRvalue::BoolToInt(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\xf0\xcf9\xd0\xbe>" if fields.len() == 1 => StoredRvalue::Array(read_list(&fields[0].value, |item0| {
            read_stored_operand(item0, positions)
        })?),
        b".\xdf\xb1h\xb3\x8b" if fields.len() == 1 => {
            StoredRvalue::Borrow(read_stored_place(&fields[0].value, positions)?)
        }
        b"\xecWL\xe3\xa7d" if fields.len() == 1 => {
            StoredRvalue::UnsizeSlice(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\xdd\xb0\x0b=6\xb4" if fields.len() == 3 => StoredRvalue::NativeAlias(
            read_stored_operand(&fields[0].value, positions)?,
            read_stored_type(&fields[1].value, positions)?,
            read_stored_type(&fields[2].value, positions)?,
        ),
        b"\x0fAV\x17\xca$" if fields.len() == 1 => {
            StoredRvalue::FrozenText(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\xa5\x1aU\xd1;\xee" if fields.len() == 1 => StoredRvalue::FrozenBytes(read_list(&fields[0].value, number)?),
        b"\xdeJ\xe4\xd3\x93\xf0" if fields.len() == 3 => StoredRvalue::NumericCast(
            read_stored_operand(&fields[0].value, positions)?,
            read_stored_type(&fields[1].value, positions)?,
            read_stored_type(&fields[2].value, positions)?,
        ),
        b"T\x10\x8d\xaf\xaa\xc5" if fields.len() == 3 => StoredRvalue::Enum(
            number(&fields[0].value)?,
            number(&fields[1].value)?,
            read_list(&fields[2].value, |item0| read_stored_operand(item0, positions))?,
        ),
        b"x\xbc\x9c&\x03\x1a" if fields.len() == 1 => {
            StoredRvalue::Discriminant(read_stored_place(&fields[0].value, positions)?)
        }
        b"\xe7#\xb0\xbd\x03\xf7" if fields.len() == 1 => {
            StoredRvalue::TagToInt(read_stored_operand(&fields[0].value, positions)?)
        }
        b"^,aL#\xf0" if fields.len() == 2 => StoredRvalue::Model(
            number(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_stored_operand(item0, positions))?,
        ),
        b"\xf16\xeex\xc3;" if fields.len() == 1 => StoredRvalue::Tuple(read_list(&fields[0].value, |item0| {
            read_stored_operand(item0, positions)
        })?),
        b"]\xe7J\x81\xb1\x92" if fields.len() == 1 => StoredRvalue::Range(read_list(&fields[0].value, |item0| {
            read_stored_operand(item0, positions)
        })?),
        b"\xa87!2_\xde" if fields.len() == 1 => {
            StoredRvalue::MutBorrow(read_stored_place(&fields[0].value, positions)?)
        }
        b"ivP2h\xd9" if fields.len() == 2 => StoredRvalue::IsVariant(
            read_stored_place(&fields[0].value, positions)?,
            number(&fields[1].value)?,
        ),
        b"S\xe4\x16\xfd%\xcf" if fields.len() == 1 => {
            StoredRvalue::ReifyFunction(read_stored_operand(&fields[0].value, positions)?)
        }
        b"\xc4\xe4\xd4\xc6~\xe2" if fields.len() == 1 => {
            StoredRvalue::FunctionAddress(read_stored_operand(&fields[0].value, positions)?)
        }
        b"s!\xe9q\x11\xd7" if fields.len() == 1 => {
            StoredRvalue::ClosureObject(read_stored_place(&fields[0].value, positions)?)
        }
        b"^H\x983G\x01" if fields.len() == 2 => {
            StoredRvalue::PlannedFunctionItem(text(&fields[0].value)?, read_stored_span(&fields[1].value, positions)?)
        }
        b"`ar\x85\xa4\xcb" if fields.len() == 2 => StoredRvalue::FrozenCollection(
            read_stored_type(&fields[0].value, positions)?,
            read_list(&fields[1].value, |item0| read_stored_frozen_value(item0, positions))?,
        ),
        _ => return Err("unknown or incomplete StoredRvalue alternative".to_owned()),
    })
}

/// The stored form of `StoredFrozenValue` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredFrozenValue {
    Scalar(StoredConstant),
    Bytes(Vec<i64>),
    Collection(StoredType, Vec<StoredFrozenValue>),
}

/// Project one `StoredFrozenValue` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_frozen_value(value: &StoredFrozenValue) -> Result<Wire, String> {
    Ok(match value {
        StoredFrozenValue::Scalar(field0) => alternative(b"C\x81\x9c`\xb3\xbd", vec![wire_stored_constant(field0)?]),
        StoredFrozenValue::Bytes(field0) => alternative(
            b"M\x80\x000\x1f\xcc",
            vec![list(field0, |item0| Ok(Wire::integer(*item0)))?],
        ),
        StoredFrozenValue::Collection(field0, field1) => alternative(
            b"{y\x07\x08\xff\x1b",
            vec![wire_stored_type(field0)?, list(field1, wire_stored_frozen_value)?],
        ),
    })
}

/// Read one `StoredFrozenValue` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_frozen_value(value: &Wire, positions: &WirePositions) -> Result<StoredFrozenValue, String> {
    let (identity, fields) = wire_variant(value)?;
    Ok(match identity {
        b"C\x81\x9c`\xb3\xbd" if fields.len() == 1 => {
            StoredFrozenValue::Scalar(read_stored_constant(&fields[0].value, positions)?)
        }
        b"M\x80\x000\x1f\xcc" if fields.len() == 1 => StoredFrozenValue::Bytes(read_list(&fields[0].value, number)?),
        b"{y\x07\x08\xff\x1b" if fields.len() == 2 => StoredFrozenValue::Collection(
            read_stored_type(&fields[0].value, positions)?,
            read_list(&fields[1].value, |item0| read_stored_frozen_value(item0, positions))?,
        ),
        _ => return Err("unknown or incomplete StoredFrozenValue alternative".to_owned()),
    })
}

/// The stored form of `StoredSpan` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredSpan {
    pub(crate) file: String,
    pub(crate) line: i64,
    pub(crate) column: i64,
}

/// Project one `StoredSpan` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_span(value: &StoredSpan) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b";\x9c5\x8f6\xf0", Wire::Text(value.file.clone())),
        required(b"8\xa9\xc1\xe7!X", Wire::integer(value.line)),
        required(b"\x88\x1d_\xb6\x00\x90", Wire::integer(value.column)),
    ]))
}

/// Read one `StoredSpan` record in its identity-ordered field slots.
pub(crate) fn read_stored_span(value: &Wire, positions: &WirePositions) -> Result<StoredSpan, String> {
    let fields = wire_fields(value, 3)?;
    let (file, line, column) = positions.apply(
        text(&fields[1].value)?,
        number(&fields[0].value)?,
        number(&fields[2].value)?,
    )?;
    Ok(StoredSpan { file, line, column })
}

/// The stored form of `StoredStatement` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredStatement {
    pub(crate) kind: StoredStatementKind,
    pub(crate) span: StoredSpan,
}

/// Project one `StoredStatement` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_statement(value: &StoredStatement) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\x0b'\xc1X\xfe\xb3", wire_stored_statement_kind(&value.kind)?),
        required(b"7\xa0\x02Z\xa1\xf2", wire_stored_span(&value.span)?),
    ]))
}

/// Read one `StoredStatement` record in its identity-ordered field slots.
pub(crate) fn read_stored_statement(value: &Wire, positions: &WirePositions) -> Result<StoredStatement, String> {
    let fields = wire_fields(value, 2)?;
    Ok(StoredStatement {
        kind: read_stored_statement_kind(&fields[0].value, positions)?,
        span: read_stored_span(&fields[1].value, positions)?,
    })
}

/// The stored form of `StoredStatementKind` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredStatementKind {
    Assign(StoredPlace, StoredRvalue, StoredSpan),
    Live(i64),
    Dead(i64),
}

/// Project one `StoredStatementKind` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_statement_kind(value: &StoredStatementKind) -> Result<Wire, String> {
    Ok(match value {
        StoredStatementKind::Assign(field0, field1, field2) => alternative(
            b"\x8e\xce\x89\x5c\x9b2",
            vec![
                wire_stored_place(field0)?,
                wire_stored_rvalue(field1)?,
                wire_stored_span(field2)?,
            ],
        ),
        StoredStatementKind::Live(field0) => alternative(b"\xb6J\xc0_\x17\xe6", vec![Wire::integer(*field0)]),
        StoredStatementKind::Dead(field0) => alternative(b"\xec\x9b\x10\xa4\xa7\x9c", vec![Wire::integer(*field0)]),
    })
}

/// Read one `StoredStatementKind` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_statement_kind(
    value: &Wire,
    positions: &WirePositions,
) -> Result<StoredStatementKind, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_statement_kind_alternative(identity, fields, positions)
}

/// Restore one `StoredStatementKind` alternative from part 0 of its identity table.
fn read_stored_statement_kind_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredStatementKind, String> {
    Ok(match identity {
        b"\x8e\xce\x89\x5c\x9b2" if fields.len() == 3 => StoredStatementKind::Assign(
            read_stored_place(&fields[0].value, positions)?,
            read_stored_rvalue(&fields[1].value, positions)?,
            read_stored_span(&fields[2].value, positions)?,
        ),
        b"\xb6J\xc0_\x17\xe6" if fields.len() == 1 => StoredStatementKind::Live(number(&fields[0].value)?),
        b"\xec\x9b\x10\xa4\xa7\x9c" if fields.len() == 1 => StoredStatementKind::Dead(number(&fields[0].value)?),
        _ => return Err("unknown or incomplete StoredStatementKind alternative".to_owned()),
    })
}

/// The stored form of `StoredType` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StoredType {
    Int,
    Float,
    Bool,
    Unit,
    CheckedInt,
    Never,
    Text,
    LiteralTextView,
    TextView,
    TextValues(i64),
    LiteralTextValues(i64),
    TextValuesView(i64),
    LiteralTextValuesView(i64),
    TextSequence,
    LiteralTextSequence,
    Layout(PlanType),
}

/// Project one `StoredType` alternative onto its identity and positional payload.
pub(crate) fn wire_stored_type(value: &StoredType) -> Result<Wire, String> {
    Ok(match value {
        StoredType::Int => unit(b"\x0b_`\x80p\xc6"),
        StoredType::Float => unit(b"\x1ai<\x00\xc4\x0c"),
        StoredType::Bool => unit(b"I\xf4\x11\xf0\xa1\xa7"),
        StoredType::Unit => unit(b"NTY`\xf1\xbf"),
        StoredType::CheckedInt => unit(b"\xc3\xf45\xc7(Q"),
        StoredType::Never => unit(b"c\x00\xef\x80\x0b\xb8"),
        StoredType::Text => unit(b"q\x98\x8cM\x8e\x08"),
        StoredType::LiteralTextView => unit(b"\xd0Y?\xda\x9f\xb6"),
        StoredType::TextView => unit(b"pL \xb1\x8c\x8e"),
        StoredType::TextValues(field0) => alternative(b"\xd1\xbf\xdd\xae\x10V", vec![Wire::integer(*field0)]),
        StoredType::LiteralTextValues(field0) => alternative(b"\xf7\xbfU\x17\x8c\x06", vec![Wire::integer(*field0)]),
        StoredType::TextValuesView(field0) => alternative(b"/\x98A\xdf\x82\x04", vec![Wire::integer(*field0)]),
        StoredType::LiteralTextValuesView(field0) => alternative(b"\xe6Zg\x16y\x07", vec![Wire::integer(*field0)]),
        StoredType::TextSequence => unit(b"\xdb\x01{)\xdaG"),
        StoredType::LiteralTextSequence => unit(b"\x931\xa0y+U"),
        StoredType::Layout(field0) => alternative(b"\xa5\x11\x90\x91a\xe1", vec![wire_plan_type(field0)?]),
    })
}

/// Read one `StoredType` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_stored_type(value: &Wire, positions: &WirePositions) -> Result<StoredType, String> {
    let (identity, fields) = wire_variant(value)?;
    read_stored_type_alternative(identity, fields, positions)
}

/// Restore one `StoredType` alternative from part 0 of its identity table.
fn read_stored_type_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<StoredType, String> {
    Ok(match identity {
        b"\x0b_`\x80p\xc6" if fields.is_empty() => StoredType::Int,
        b"\x1ai<\x00\xc4\x0c" if fields.is_empty() => StoredType::Float,
        b"I\xf4\x11\xf0\xa1\xa7" if fields.is_empty() => StoredType::Bool,
        b"NTY`\xf1\xbf" if fields.is_empty() => StoredType::Unit,
        b"\xc3\xf45\xc7(Q" if fields.is_empty() => StoredType::CheckedInt,
        b"c\x00\xef\x80\x0b\xb8" if fields.is_empty() => StoredType::Never,
        b"q\x98\x8cM\x8e\x08" if fields.is_empty() => StoredType::Text,
        b"\xd0Y?\xda\x9f\xb6" if fields.is_empty() => StoredType::LiteralTextView,
        b"pL \xb1\x8c\x8e" if fields.is_empty() => StoredType::TextView,
        b"\xd1\xbf\xdd\xae\x10V" if fields.len() == 1 => StoredType::TextValues(number(&fields[0].value)?),
        b"\xf7\xbfU\x17\x8c\x06" if fields.len() == 1 => StoredType::LiteralTextValues(number(&fields[0].value)?),
        b"/\x98A\xdf\x82\x04" if fields.len() == 1 => StoredType::TextValuesView(number(&fields[0].value)?),
        b"\xe6Zg\x16y\x07" if fields.len() == 1 => StoredType::LiteralTextValuesView(number(&fields[0].value)?),
        b"\xdb\x01{)\xdaG" if fields.is_empty() => StoredType::TextSequence,
        b"\x931\xa0y+U" if fields.is_empty() => StoredType::LiteralTextSequence,
        b"\xa5\x11\x90\x91a\xe1" if fields.len() == 1 => {
            StoredType::Layout(read_plan_type(&fields[0].value, positions)?)
        }
        _ => return Err("unknown or incomplete StoredType alternative".to_owned()),
    })
}
