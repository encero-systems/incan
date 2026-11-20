//! Projections of the native-plan profile's `layout_type_records` shapes onto Crumb values.
//!
//! Each projection writes or reads one stored shape in its declared field and alternative order; record fields
//! are read from their identity-ordered slots. A shape here and its description in `wire_schema.rs` change
//! together.

use crate::records::prelude::*;

/// The stored form of `StoredInstanceRequest` in the native-plan profile.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredInstanceRequest {
    pub(crate) declaration: String,
    pub(crate) arguments: Vec<CheckedType>,
    pub(crate) owned_callable_slots: Vec<i64>,
    pub(crate) original_callable: bool,
}

/// Project one `StoredInstanceRequest` onto its schema-admitted Crumb record.
pub(crate) fn wire_stored_instance_request(value: &StoredInstanceRequest) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(b"\xab\xcd\x0e\xe1\xfc,", Wire::Text((&value.declaration).clone())),
        required(b"\xb1\x92\x01}\xa4\x7f", list(&value.arguments, wire_checked_type)?),
        required(
            b"\xfc\x7f\x8co\xaf\xff",
            list(&value.owned_callable_slots, |item0| Ok(Wire::integer(*item0)))?,
        ),
        required(b"\x9b\xf1\x17\x8c4{", Wire::Boolean(*&value.original_callable)),
    ]))
}

/// Read one `StoredInstanceRequest` record in its identity-ordered field slots.
pub(crate) fn read_stored_instance_request(
    value: &Wire,
    positions: &WirePositions,
) -> Result<StoredInstanceRequest, String> {
    let fields = wire_fields(value, 4)?;
    Ok(StoredInstanceRequest {
        declaration: text(&fields[1].value)?,
        arguments: read_list(&fields[2].value, |item0| read_checked_type(item0, positions))?,
        owned_callable_slots: read_list(&fields[3].value, |item0| Ok(number(item0)?))?,
        original_callable: boolean_value(&fields[0].value)?,
    })
}

/// Project one `StoredLayoutCarrier` alternative onto its identity and positional payload.
pub(crate) fn wire_carrier(value: &Carrier) -> Result<Wire, String> {
    Ok(match value {
        Carrier::Declared => unit(b"\xdcV\xdd&\x87\xa3"),
        Carrier::Option => unit(b"E\xaa\xac\xba~\xa1"),
        Carrier::Result => unit(b"n}P\xe8OG"),
    })
}

/// Read one `StoredLayoutCarrier` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_carrier(value: &Wire, positions: &WirePositions) -> Result<Carrier, String> {
    let (identity, fields) = wire_variant(value)?;
    read_carrier_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCarrier` alternative from part 0 of its identity table.
fn read_carrier_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<Carrier, String> {
    let _ = positions;
    Ok(match identity {
        b"\xdcV\xdd&\x87\xa3" if fields.len() == 0 => Carrier::Declared,
        b"E\xaa\xac\xba~\xa1" if fields.len() == 0 => Carrier::Option,
        b"n}P\xe8OG" if fields.len() == 0 => Carrier::Result,
        _ => return Err("unknown or incomplete StoredLayoutCarrier alternative".to_owned()),
    })
}

/// Project one `StoredLayoutCheckedCallableParam` onto its schema-admitted Crumb record.
pub(crate) fn wire_checked_callable_param(value: &CheckedCallableParam) -> Result<Wire, String> {
    Ok(Wire::Record(vec![
        required(
            b"\x82\xa3S\x7f\xf0\xdb",
            optional(&value.name, |item0| Ok(Wire::Text((item0).clone())))?,
        ),
        required(b"O\x9d\xb7\xf8\xe5\x06", wire_checked_type(&value.ty)?),
        required(b"\x0b'\xc1X\xfe\xb3", wire_checked_callable_param_kind(&value.kind)?),
        required(b"h*w|#(", Wire::Boolean(*&value.has_default)),
        required(b"\xc2\xea58V\xa6", Wire::Boolean(*&value.is_partial_preset)),
        required(b"\xc5}!\xc0\xf9[", Wire::Boolean(*&value.is_mut)),
        required(b"\xa6\xcb\xca1|\x1a", Wire::Boolean(*&value.native_owned)),
        required(b"+#{X\xd0r", wire_checked_input_access(&value.input_access)?),
    ]))
}

/// Read one `StoredLayoutCheckedCallableParam` record in its identity-ordered field slots.
pub(crate) fn read_checked_callable_param(
    value: &Wire,
    positions: &WirePositions,
) -> Result<CheckedCallableParam, String> {
    let fields = wire_fields(value, 8)?;
    Ok(CheckedCallableParam {
        name: read_optional(&fields[4].value, |children| Ok(text(&children[0])?))?,
        ty: read_checked_type(&fields[2].value, positions)?,
        kind: read_checked_callable_param_kind(&fields[0].value, positions)?,
        has_default: boolean_value(&fields[3].value)?,
        is_partial_preset: boolean_value(&fields[6].value)?,
        is_mut: boolean_value(&fields[7].value)?,
        native_owned: boolean_value(&fields[5].value)?,
        input_access: read_checked_input_access(&fields[1].value, positions)?,
    })
}

/// Project one `StoredLayoutCheckedCallableParamKind` alternative onto its identity and positional payload.
pub(crate) fn wire_checked_callable_param_kind(value: &CheckedCallableParamKind) -> Result<Wire, String> {
    Ok(match value {
        CheckedCallableParamKind::Normal => unit(b"\xa7$\x8e\xebE\xeb"),
        CheckedCallableParamKind::RestPositional => unit(b"\xed\xe4!\xe1h\xfb"),
        CheckedCallableParamKind::RestKeyword => unit(b"\xff\xad\x057D!"),
    })
}

/// Read one `StoredLayoutCheckedCallableParamKind` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_checked_callable_param_kind(
    value: &Wire,
    positions: &WirePositions,
) -> Result<CheckedCallableParamKind, String> {
    let (identity, fields) = wire_variant(value)?;
    read_checked_callable_param_kind_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCheckedCallableParamKind` alternative from part 0 of its identity table.
fn read_checked_callable_param_kind_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<CheckedCallableParamKind, String> {
    let _ = positions;
    Ok(match identity {
        b"\xa7$\x8e\xebE\xeb" if fields.len() == 0 => CheckedCallableParamKind::Normal,
        b"\xed\xe4!\xe1h\xfb" if fields.len() == 0 => CheckedCallableParamKind::RestPositional,
        b"\xff\xad\x057D!" if fields.len() == 0 => CheckedCallableParamKind::RestKeyword,
        _ => return Err("unknown or incomplete StoredLayoutCheckedCallableParamKind alternative".to_owned()),
    })
}

/// Project one `StoredLayoutCheckedInputAccess` alternative onto its identity and positional payload.
pub(crate) fn wire_checked_input_access(value: &CheckedInputAccess) -> Result<Wire, String> {
    Ok(match value {
        CheckedInputAccess::Inferred => unit(b"\xd4a\xcb\xb4\x90f"),
        CheckedInputAccess::Shared => unit(b"\xe3\xc4\xb3\x9dmP"),
        CheckedInputAccess::Exclusive => unit(b"H\x83\xb7\xdf\xff\xb0"),
        CheckedInputAccess::Owned => unit(b"\x17\xb7`\xc4\x1c\xd2"),
        CheckedInputAccess::InferredShared => unit(b"1\xca\xf1d\xbb&"),
    })
}

/// Read one `StoredLayoutCheckedInputAccess` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_checked_input_access(value: &Wire, positions: &WirePositions) -> Result<CheckedInputAccess, String> {
    let (identity, fields) = wire_variant(value)?;
    read_checked_input_access_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCheckedInputAccess` alternative from part 0 of its identity table.
fn read_checked_input_access_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<CheckedInputAccess, String> {
    let _ = positions;
    Ok(match identity {
        b"\xd4a\xcb\xb4\x90f" if fields.len() == 0 => CheckedInputAccess::Inferred,
        b"\xe3\xc4\xb3\x9dmP" if fields.len() == 0 => CheckedInputAccess::Shared,
        b"H\x83\xb7\xdf\xff\xb0" if fields.len() == 0 => CheckedInputAccess::Exclusive,
        b"\x17\xb7`\xc4\x1c\xd2" if fields.len() == 0 => CheckedInputAccess::Owned,
        b"1\xca\xf1d\xbb&" if fields.len() == 0 => CheckedInputAccess::InferredShared,
        _ => return Err("unknown or incomplete StoredLayoutCheckedInputAccess alternative".to_owned()),
    })
}

/// Project one `StoredLayoutCheckedNumericType` alternative onto its identity and positional payload.
pub(crate) fn wire_checked_numeric_type(value: &CheckedNumericType) -> Result<Wire, String> {
    Ok(match value {
        CheckedNumericType::I8 => unit(b"1Fb\x8c\x11\x8c"),
        CheckedNumericType::I16 => unit(b"\xb9\xff?nq\xda"),
        CheckedNumericType::I32 => unit(b"\xbe-,\xd01}"),
        CheckedNumericType::I64 => unit(b"\xc1\xc7\xe0-\xd6\xcb"),
        CheckedNumericType::I128 => unit(b"G\xeck\xdaM\xa3"),
        CheckedNumericType::U8 => unit(b"4s\xd3\x8a\x12A"),
        CheckedNumericType::U16 => unit(b"\xef\x09\xbc\x80\xcdb"),
        CheckedNumericType::U32 => unit(b"\x80\x07$\x86\xf2\xea"),
        CheckedNumericType::U64 => unit(b"\x84\xb8\x9e\x22ET"),
        CheckedNumericType::U128 => unit(b"\x0c\x80\x0d\xc4\xf9I"),
        CheckedNumericType::F32 => unit(b"\x82%Z\xeel\xcc"),
        CheckedNumericType::F64 => unit(b"\xa4M\x7ft\xc8)"),
        CheckedNumericType::ISize => unit(b"\xec|D\x91\xf6\x86"),
        CheckedNumericType::USize => unit(b"\xbaL\xadu\xa6Z"),
        CheckedNumericType::Bool => unit(b"I\xf4\x11\xf0\xa1\xa7"),
    })
}

/// Read one `StoredLayoutCheckedNumericType` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_checked_numeric_type(value: &Wire, positions: &WirePositions) -> Result<CheckedNumericType, String> {
    let (identity, fields) = wire_variant(value)?;
    read_checked_numeric_type_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCheckedNumericType` alternative from part 0 of its identity table.
fn read_checked_numeric_type_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<CheckedNumericType, String> {
    let _ = positions;
    Ok(match identity {
        b"1Fb\x8c\x11\x8c" if fields.len() == 0 => CheckedNumericType::I8,
        b"\xb9\xff?nq\xda" if fields.len() == 0 => CheckedNumericType::I16,
        b"\xbe-,\xd01}" if fields.len() == 0 => CheckedNumericType::I32,
        b"\xc1\xc7\xe0-\xd6\xcb" if fields.len() == 0 => CheckedNumericType::I64,
        b"G\xeck\xdaM\xa3" if fields.len() == 0 => CheckedNumericType::I128,
        b"4s\xd3\x8a\x12A" if fields.len() == 0 => CheckedNumericType::U8,
        b"\xef\x09\xbc\x80\xcdb" if fields.len() == 0 => CheckedNumericType::U16,
        b"\x80\x07$\x86\xf2\xea" if fields.len() == 0 => CheckedNumericType::U32,
        b"\x84\xb8\x9e\x22ET" if fields.len() == 0 => CheckedNumericType::U64,
        b"\x0c\x80\x0d\xc4\xf9I" if fields.len() == 0 => CheckedNumericType::U128,
        b"\x82%Z\xeel\xcc" if fields.len() == 0 => CheckedNumericType::F32,
        b"\xa4M\x7ft\xc8)" if fields.len() == 0 => CheckedNumericType::F64,
        b"\xec|D\x91\xf6\x86" if fields.len() == 0 => CheckedNumericType::ISize,
        b"\xbaL\xadu\xa6Z" if fields.len() == 0 => CheckedNumericType::USize,
        b"I\xf4\x11\xf0\xa1\xa7" if fields.len() == 0 => CheckedNumericType::Bool,
        _ => return Err("unknown or incomplete StoredLayoutCheckedNumericType alternative".to_owned()),
    })
}

/// Project one `StoredLayoutCheckedPrimitiveType` alternative onto its identity and positional payload.
pub(crate) fn wire_checked_primitive_type(value: &CheckedPrimitiveType) -> Result<Wire, String> {
    Ok(match value {
        CheckedPrimitiveType::Int => unit(b"\x0b_`\x80p\xc6"),
        CheckedPrimitiveType::Float => unit(b"\x1ai<\x00\xc4\x0c"),
        CheckedPrimitiveType::Numeric(field0) => alternative(b"6u-l\xd1G", vec![wire_checked_numeric_type(field0)?]),
        CheckedPrimitiveType::Bool => unit(b"I\xf4\x11\xf0\xa1\xa7"),
        CheckedPrimitiveType::Str => unit(b"\x80\x84\xa5\x1b<d"),
        CheckedPrimitiveType::Bytes => unit(b"M\x80\x000\x1f\xcc"),
        CheckedPrimitiveType::FrozenStr => unit(b"O\xab\xe1W\xe8\xf9"),
        CheckedPrimitiveType::FrozenBytes => unit(b"\xa5\x1aU\xd1;\xee"),
        CheckedPrimitiveType::Unit => unit(b"NTY`\xf1\xbf"),
    })
}

/// Read one `StoredLayoutCheckedPrimitiveType` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_checked_primitive_type(
    value: &Wire,
    positions: &WirePositions,
) -> Result<CheckedPrimitiveType, String> {
    let (identity, fields) = wire_variant(value)?;
    read_checked_primitive_type_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCheckedPrimitiveType` alternative from part 0 of its identity table.
fn read_checked_primitive_type_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<CheckedPrimitiveType, String> {
    let _ = positions;
    Ok(match identity {
        b"\x0b_`\x80p\xc6" if fields.len() == 0 => CheckedPrimitiveType::Int,
        b"\x1ai<\x00\xc4\x0c" if fields.len() == 0 => CheckedPrimitiveType::Float,
        b"6u-l\xd1G" if fields.len() == 1 => {
            CheckedPrimitiveType::Numeric(read_checked_numeric_type(&fields[0].value, positions)?)
        }
        b"I\xf4\x11\xf0\xa1\xa7" if fields.len() == 0 => CheckedPrimitiveType::Bool,
        b"\x80\x84\xa5\x1b<d" if fields.len() == 0 => CheckedPrimitiveType::Str,
        b"M\x80\x000\x1f\xcc" if fields.len() == 0 => CheckedPrimitiveType::Bytes,
        b"O\xab\xe1W\xe8\xf9" if fields.len() == 0 => CheckedPrimitiveType::FrozenStr,
        b"\xa5\x1aU\xd1;\xee" if fields.len() == 0 => CheckedPrimitiveType::FrozenBytes,
        b"NTY`\xf1\xbf" if fields.len() == 0 => CheckedPrimitiveType::Unit,
        _ => return Err("unknown or incomplete StoredLayoutCheckedPrimitiveType alternative".to_owned()),
    })
}

/// Project one `StoredLayoutCheckedRuntimeBuiltin` alternative onto its identity and positional payload.
pub(crate) fn wire_checked_runtime_builtin(value: &CheckedRuntimeBuiltin) -> Result<Wire, String> {
    Ok(match value {
        CheckedRuntimeBuiltin::ValidationError => unit(b"\xbd\xfa\xef\x14\xbe\x91"),
    })
}

/// Read one `StoredLayoutCheckedRuntimeBuiltin` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_checked_runtime_builtin(
    value: &Wire,
    positions: &WirePositions,
) -> Result<CheckedRuntimeBuiltin, String> {
    let (identity, fields) = wire_variant(value)?;
    read_checked_runtime_builtin_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCheckedRuntimeBuiltin` alternative from part 0 of its identity table.
fn read_checked_runtime_builtin_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<CheckedRuntimeBuiltin, String> {
    let _ = positions;
    Ok(match identity {
        b"\xbd\xfa\xef\x14\xbe\x91" if fields.len() == 0 => CheckedRuntimeBuiltin::ValidationError,
        _ => return Err("unknown or incomplete StoredLayoutCheckedRuntimeBuiltin alternative".to_owned()),
    })
}

/// Project one `StoredLayoutCheckedType` alternative onto its identity and positional payload.
pub(crate) fn wire_checked_type(value: &CheckedType) -> Result<Wire, String> {
    Ok(match value {
        CheckedType::Never => unit(b"c\x00\xef\x80\x0b\xb8"),
        CheckedType::Primitive(field0) => {
            alternative(b"\x09V\x17<\xa4\x15", vec![wire_checked_primitive_type(field0)?])
        }
        CheckedType::Named(field0) => alternative(b"\x86\x05`Z\xe0&", vec![Wire::Text((field0).clone())]),
        CheckedType::Generic(field0, field1) => alternative(
            b"\x02(\xc6\xd4\x8e\xcf",
            vec![Wire::Text((field0).clone()), list(field1, wire_checked_type)?],
        ),
        CheckedType::Decimal(field0, field1) => alternative(
            b"\x18Q}3-\x1b",
            vec![Wire::Unsigned(u128::from(*field0)), Wire::Unsigned(u128::from(*field1))],
        ),
        CheckedType::Function(field0, field1) => alternative(
            b"\xc8\x03q\x03\x02\xd5",
            vec![
                list(field0, wire_checked_callable_param)?,
                list(field1, wire_checked_type)?,
            ],
        ),
        CheckedType::TypeToken(field0) => alternative(b"\xeb\x90\x0dwd\xd0", vec![list(field0, wire_checked_type)?]),
        CheckedType::Tuple(field0) => alternative(b"\xf16\xeex\xc3;", vec![list(field0, wire_checked_type)?]),
        CheckedType::TypeVar(field0) => alternative(b"\xc7\xa4\xa1AK\xe4", vec![Wire::Text((field0).clone())]),
        CheckedType::SelfType => unit(b"\x800$\x93\xf2\xd4"),
        CheckedType::Ref(field0) => alternative(b"\x9bn\x88\xe8\x94:", vec![list(field0, wire_checked_type)?]),
        CheckedType::RefMut(field0) => alternative(b"\xff\xc4\x90\xa6\xa7\xb3", vec![list(field0, wire_checked_type)?]),
        CheckedType::RustInteropPath(field0) => alternative(b"\xe7u\x7f\x0d\xdfV", vec![Wire::Text((field0).clone())]),
        CheckedType::NativeBuiltin(field0) => {
            alternative(b"o\xc7B\xf9\xdc\xbd", vec![wire_checked_runtime_builtin(field0)?])
        }
        CheckedType::Unsupported(field0) => alternative(b"T2FX\xe2\xeb", vec![Wire::Text((field0).clone())]),
        CheckedType::Infer => unit(b"'@\x0f\xf4\xb8\x09"),
        CheckedType::Unknown => unit(b"\xb7d\xcd\xc0\xea\xb7"),
        CheckedType::NativeArray(field0, field1) => alternative(
            b"0?\xb1)\xac\xe5",
            vec![list(field0, wire_checked_type)?, Wire::Unsigned(u128::from(*field1))],
        ),
        CheckedType::NativeTraitObject(field0, field1, field2) => alternative(
            b"\x5cz%\xe2\x9cR",
            vec![
                Wire::Text((field0).clone()),
                Wire::Boolean(*field1),
                Wire::Boolean(*field2),
            ],
        ),
        CheckedType::NativePointer(field0, field1) => alternative(
            b"\xa6\x9d@\x085\xc9",
            vec![Wire::Boolean(*field0), list(field1, wire_checked_type)?],
        ),
        CheckedType::NativeCChar => unit(b"\xed\x5c1CFF"),
        _ => return Err("native-interface refuses an unsupported CheckedType alternative".to_owned()),
    })
}

/// Read one `StoredLayoutCheckedType` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_checked_type(value: &Wire, positions: &WirePositions) -> Result<CheckedType, String> {
    let (identity, fields) = wire_variant(value)?;
    read_checked_type_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutCheckedType` alternative from part 0 of its identity table.
fn read_checked_type_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<CheckedType, String> {
    let _ = positions;
    Ok(match identity {
        b"c\x00\xef\x80\x0b\xb8" if fields.len() == 0 => CheckedType::Never,
        b"\x09V\x17<\xa4\x15" if fields.len() == 1 => {
            CheckedType::Primitive(read_checked_primitive_type(&fields[0].value, positions)?)
        }
        b"\x86\x05`Z\xe0&" if fields.len() == 1 => CheckedType::Named(text(&fields[0].value)?),
        b"\x02(\xc6\xd4\x8e\xcf" if fields.len() == 2 => CheckedType::Generic(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_checked_type(item0, positions))?,
        ),
        b"\x18Q}3-\x1b" if fields.len() == 2 => CheckedType::Decimal(
            unsigned_value(&fields[0].value)? as u8,
            unsigned_value(&fields[1].value)? as u8,
        ),
        b"\xc8\x03q\x03\x02\xd5" if fields.len() == 2 => CheckedType::Function(
            read_list(&fields[0].value, |item0| read_checked_callable_param(item0, positions))?,
            read_list(&fields[1].value, |item0| read_checked_type(item0, positions))?,
        ),
        b"\xeb\x90\x0dwd\xd0" if fields.len() == 1 => CheckedType::TypeToken(read_list(&fields[0].value, |item0| {
            read_checked_type(item0, positions)
        })?),
        b"\xf16\xeex\xc3;" if fields.len() == 1 => CheckedType::Tuple(read_list(&fields[0].value, |item0| {
            read_checked_type(item0, positions)
        })?),
        b"\xc7\xa4\xa1AK\xe4" if fields.len() == 1 => CheckedType::TypeVar(text(&fields[0].value)?),
        b"\x800$\x93\xf2\xd4" if fields.len() == 0 => CheckedType::SelfType,
        b"\x9bn\x88\xe8\x94:" if fields.len() == 1 => CheckedType::Ref(read_list(&fields[0].value, |item0| {
            read_checked_type(item0, positions)
        })?),
        b"\xff\xc4\x90\xa6\xa7\xb3" if fields.len() == 1 => {
            CheckedType::RefMut(read_list(&fields[0].value, |item0| {
                read_checked_type(item0, positions)
            })?)
        }
        b"\xe7u\x7f\x0d\xdfV" if fields.len() == 1 => CheckedType::RustInteropPath(text(&fields[0].value)?),
        b"o\xc7B\xf9\xdc\xbd" if fields.len() == 1 => {
            CheckedType::NativeBuiltin(read_checked_runtime_builtin(&fields[0].value, positions)?)
        }
        b"T2FX\xe2\xeb" if fields.len() == 1 => CheckedType::Unsupported(text(&fields[0].value)?),
        b"'@\x0f\xf4\xb8\x09" if fields.len() == 0 => CheckedType::Infer,
        b"\xb7d\xcd\xc0\xea\xb7" if fields.len() == 0 => CheckedType::Unknown,
        b"0?\xb1)\xac\xe5" if fields.len() == 2 => CheckedType::NativeArray(
            read_list(&fields[0].value, |item0| read_checked_type(item0, positions))?,
            unsigned_value(&fields[1].value)?,
        ),
        b"\x5cz%\xe2\x9cR" if fields.len() == 3 => CheckedType::NativeTraitObject(
            text(&fields[0].value)?,
            boolean_value(&fields[1].value)?,
            boolean_value(&fields[2].value)?,
        ),
        b"\xa6\x9d@\x085\xc9" if fields.len() == 2 => CheckedType::NativePointer(
            boolean_value(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_checked_type(item0, positions))?,
        ),
        b"\xed\x5c1CFF" if fields.len() == 0 => CheckedType::NativeCChar,
        _ => return Err("unknown or incomplete StoredLayoutCheckedType alternative".to_owned()),
    })
}

/// Project one `StoredLayoutListLeaf` alternative onto its identity and positional payload.
pub(crate) fn wire_list_leaf(value: &ListLeaf) -> Result<Wire, String> {
    Ok(match value {
        ListLeaf::Int => unit(b"\x0b_`\x80p\xc6"),
        ListLeaf::Float => unit(b"\x1ai<\x00\xc4\x0c"),
        ListLeaf::Bool => unit(b"I\xf4\x11\xf0\xa1\xa7"),
        ListLeaf::Str => unit(b"\x80\x84\xa5\x1b<d"),
        ListLeaf::Tuple(field0) => alternative(b"\xf16\xeex\xc3;", vec![list(field0, wire_tuple_element)?]),
        ListLeaf::Model(field0, field1) => {
            alternative(b"^,aL#\xf0", vec![Wire::integer(*field0), Wire::Text((field1).clone())])
        }
        ListLeaf::Enum(field0, field1) => alternative(
            b"T\x10\x8d\xaf\xaa\xc5",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        ListLeaf::U8 => unit(b"4s\xd3\x8a\x12A"),
        ListLeaf::Unit => unit(b"NTY`\xf1\xbf"),
        ListLeaf::Decimal => unit(b"\x18Q}3-\x1b"),
        ListLeaf::FrozenStr => unit(b"O\xab\xe1W\xe8\xf9"),
        ListLeaf::FrozenBytes => unit(b"\xa5\x1aU\xd1;\xee"),
        ListLeaf::FrozenCollection(field0, field1) => alternative(
            b"`ar\x85\xa4\xcb",
            vec![Wire::Text((field0).clone()), list(field1, wire_plan_type)?],
        ),
        ListLeaf::FunctionPointer(field0) => alternative(b"\xa7\x0a\xca\xf0X`", vec![list(field0, wire_plan_type)?]),
        ListLeaf::TypeToken(field0) => alternative(b"\xeb\x90\x0dwd\xd0", vec![list(field0, wire_plan_type)?]),
        ListLeaf::List(field0, field1) => alternative(
            b"o /T\xa7\xb2",
            vec![list(field0, wire_list_leaf)?, Wire::integer(*field1)],
        ),
        ListLeaf::Dict(field0) => alternative(b"\x96\x11\xd3\xd8\x00\xfc", vec![list(field0, wire_list_leaf)?]),
        ListLeaf::Set(field0) => alternative(b"\xb6\xf6\xf3\xad\x07\xb3", vec![list(field0, wire_list_leaf)?]),
        ListLeaf::USize => unit(b"\xbaL\xadu\xa6Z"),
        ListLeaf::Numeric(field0) => alternative(b"6u-l\xd1G", vec![wire_sized_numeric(field0)?]),
        ListLeaf::Reference(field0) => alternative(b"q\xbf\x90\x93_\xc2", vec![list(field0, wire_plan_type)?]),
    })
}

/// Read one `StoredLayoutListLeaf` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_list_leaf(value: &Wire, positions: &WirePositions) -> Result<ListLeaf, String> {
    let (identity, fields) = wire_variant(value)?;
    read_list_leaf_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutListLeaf` alternative from part 0 of its identity table.
fn read_list_leaf_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<ListLeaf, String> {
    let _ = positions;
    Ok(match identity {
        b"\x0b_`\x80p\xc6" if fields.len() == 0 => ListLeaf::Int,
        b"\x1ai<\x00\xc4\x0c" if fields.len() == 0 => ListLeaf::Float,
        b"I\xf4\x11\xf0\xa1\xa7" if fields.len() == 0 => ListLeaf::Bool,
        b"\x80\x84\xa5\x1b<d" if fields.len() == 0 => ListLeaf::Str,
        b"\xf16\xeex\xc3;" if fields.len() == 1 => ListLeaf::Tuple(read_list(&fields[0].value, |item0| {
            read_tuple_element(item0, positions)
        })?),
        b"^,aL#\xf0" if fields.len() == 2 => ListLeaf::Model(number(&fields[0].value)?, text(&fields[1].value)?),
        b"T\x10\x8d\xaf\xaa\xc5" if fields.len() == 2 => {
            ListLeaf::Enum(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"4s\xd3\x8a\x12A" if fields.len() == 0 => ListLeaf::U8,
        b"NTY`\xf1\xbf" if fields.len() == 0 => ListLeaf::Unit,
        b"\x18Q}3-\x1b" if fields.len() == 0 => ListLeaf::Decimal,
        b"O\xab\xe1W\xe8\xf9" if fields.len() == 0 => ListLeaf::FrozenStr,
        b"\xa5\x1aU\xd1;\xee" if fields.len() == 0 => ListLeaf::FrozenBytes,
        b"`ar\x85\xa4\xcb" if fields.len() == 2 => ListLeaf::FrozenCollection(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        b"\xa7\x0a\xca\xf0X`" if fields.len() == 1 => {
            ListLeaf::FunctionPointer(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xeb\x90\x0dwd\xd0" if fields.len() == 1 => {
            ListLeaf::TypeToken(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"o /T\xa7\xb2" if fields.len() == 2 => ListLeaf::List(
            read_list(&fields[0].value, |item0| read_list_leaf(item0, positions))?,
            number(&fields[1].value)?,
        ),
        b"\x96\x11\xd3\xd8\x00\xfc" if fields.len() == 1 => {
            ListLeaf::Dict(read_list(&fields[0].value, |item0| read_list_leaf(item0, positions))?)
        }
        b"\xb6\xf6\xf3\xad\x07\xb3" if fields.len() == 1 => {
            ListLeaf::Set(read_list(&fields[0].value, |item0| read_list_leaf(item0, positions))?)
        }
        b"\xbaL\xadu\xa6Z" if fields.len() == 0 => ListLeaf::USize,
        b"6u-l\xd1G" if fields.len() == 1 => ListLeaf::Numeric(read_sized_numeric(&fields[0].value, positions)?),
        b"q\xbf\x90\x93_\xc2" if fields.len() == 1 => {
            ListLeaf::Reference(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        _ => return Err("unknown or incomplete StoredLayoutListLeaf alternative".to_owned()),
    })
}

/// Project one `StoredLayoutPlanType` alternative onto its identity and positional payload.
pub(crate) fn wire_plan_type(value: &PlanType) -> Result<Wire, String> {
    Ok(match value {
        PlanType::Int => unit(b"\x0b_`\x80p\xc6"),
        PlanType::Float => unit(b"\x1ai<\x00\xc4\x0c"),
        PlanType::I8 => unit(b"1Fb\x8c\x11\x8c"),
        PlanType::I16 => unit(b"\xb9\xff?nq\xda"),
        PlanType::I32 => unit(b"\xbe-,\xd01}"),
        PlanType::I128 => unit(b"G\xeck\xdaM\xa3"),
        PlanType::ISize => unit(b"\xec|D\x91\xf6\x86"),
        PlanType::USize => unit(b"\xbaL\xadu\xa6Z"),
        PlanType::U8 => unit(b"4s\xd3\x8a\x12A"),
        PlanType::U16 => unit(b"\xef\x09\xbc\x80\xcdb"),
        PlanType::U32 => unit(b"\x80\x07$\x86\xf2\xea"),
        PlanType::U64 => unit(b"\x84\xb8\x9e\x22ET"),
        PlanType::U128 => unit(b"\x0c\x80\x0d\xc4\xf9I"),
        PlanType::F32 => unit(b"\x82%Z\xeel\xcc"),
        PlanType::F64 => unit(b"\xa4M\x7ft\xc8)"),
        PlanType::Bool => unit(b"I\xf4\x11\xf0\xa1\xa7"),
        PlanType::Unit => unit(b"NTY`\xf1\xbf"),
        PlanType::CheckedNumeric(field0) => alternative(b"\xa0?\xf2#F\x9f", vec![wire_sized_numeric(field0)?]),
        PlanType::Never => unit(b"c\x00\xef\x80\x0b\xb8"),
        PlanType::UnitFunction => unit(b"\xe6\xf5\xc36\x16\xf4"),
        PlanType::EnumTag => unit(b"\x03\xa6\xc5\x01j\x86"),
        PlanType::CheckedInt => unit(b"\xc3\xf45\xc7(Q"),
        PlanType::Tuple(field0) => alternative(b"\xf16\xeex\xc3;", vec![list(field0, wire_tuple_element)?]),
        PlanType::String => unit(b"\xb2\xef#\x0e\x7fO"),
        PlanType::TupleRef(field0) => alternative(b"\xbd\xb4\xa2v\x05;", vec![list(field0, wire_tuple_element)?]),
        PlanType::TupleMutRef(field0) => alternative(b"\xaeV\xd77[\xff", vec![list(field0, wire_tuple_element)?]),
        PlanType::StringRef => unit(b"\xbfkl\x9f\xdb\xc5"),
        PlanType::ScalarRef(field0) => alternative(b"\x97 \xde\x22\xd7\xd4", vec![list(field0, wire_plan_type)?]),
        PlanType::ScalarMutRef(field0) => alternative(b"\xcbeb\x95hA", vec![list(field0, wire_plan_type)?]),
        PlanType::StringMutRef => unit(b"\xa45\x1e\xa3g\xb4"),
        PlanType::Decimal => unit(b"\x18Q}3-\x1b"),
        PlanType::FrozenStr => unit(b"O\xab\xe1W\xe8\xf9"),
        PlanType::FrozenBytes => unit(b"\xa5\x1aU\xd1;\xee"),
        PlanType::FrozenCollection(field0, field1) => alternative(
            b"`ar\x85\xa4\xcb",
            vec![Wire::Text((field0).clone()), list(field1, wire_plan_type)?],
        ),
        PlanType::FrozenCollectionRef(field0, field1) => alternative(
            b"T\xf9p\x0cZ3",
            vec![Wire::Text((field0).clone()), list(field1, wire_plan_type)?],
        ),
        PlanType::TypeToken(field0) => alternative(b"\xeb\x90\x0dwd\xd0", vec![list(field0, wire_plan_type)?]),
        _ => return wire_plan_type_1(value),
    })
}

/// Project one `StoredLayoutPlanType` alternative onto its identity and positional payload.
fn wire_plan_type_1(value: &PlanType) -> Result<Wire, String> {
    Ok(match value {
        PlanType::StaticText => unit(b"\xd5\xdf\x07x\xa5\x0a"),
        PlanType::StaticBytes => unit(b"_\x93\x11xcg"),
        PlanType::ValidationError => unit(b"\xbd\xfa\xef\x14\xbe\x91"),
        PlanType::ValidationErrorRef => unit(b"\x1ejG\x9a\xae\xf1"),
        PlanType::StrRef => unit(b"(+u\x8f\x05\x8a"),
        PlanType::StringArray(field0) => alternative(b"\x85~Q\x14\x0d\x18", vec![Wire::integer(*field0)]),
        PlanType::StrArray(field0) => alternative(b"\x8a\xe3R\x0a\xe0\xcb", vec![Wire::integer(*field0)]),
        PlanType::StringArrayRef(field0) => alternative(b"\xeb2P\xd3\xfc<", vec![Wire::integer(*field0)]),
        PlanType::StrArrayRef(field0) => alternative(b"\xb1\x90\x07D|\x13", vec![Wire::integer(*field0)]),
        PlanType::Enum(field0, field1) => alternative(
            b"T\x10\x8d\xaf\xaa\xc5",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        PlanType::EnumRef(field0, field1) => alternative(
            b"\xc0\xa35\x8f\x22\x01",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        PlanType::EnumMutRef(field0, field1) => {
            alternative(b"zq\xac~]|", vec![Wire::integer(*field0), Wire::Text((field1).clone())])
        }
        PlanType::Model(field0, field1) => {
            alternative(b"^,aL#\xf0", vec![Wire::integer(*field0), Wire::Text((field1).clone())])
        }
        PlanType::ModelRef(field0, field1) => alternative(
            b"C\xc3U\x17\xae\x15",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        PlanType::ModelMutRef(field0, field1) => alternative(
            b"\xb0\x02\x8e\xa2$\x98",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        PlanType::StringSlice => unit(b"\x10\x99\xe21t\xe8"),
        PlanType::StrSlice => unit(b"\xb7\xe7\x93\x84\xb2\xfb"),
        PlanType::List(field0, field1) => {
            alternative(b"o /T\xa7\xb2", vec![wire_list_leaf(field0)?, Wire::integer(*field1)])
        }
        PlanType::ListRef(field0, field1) => alternative(
            b"W\x16\x82\x93!\xd4",
            vec![wire_list_leaf(field0)?, Wire::integer(*field1)],
        ),
        PlanType::ListMutRef(field0, field1) => alternative(
            b"/@\x05\xe8\xd2Y",
            vec![wire_list_leaf(field0)?, Wire::integer(*field1)],
        ),
        PlanType::ListIntoIter(field0, field1) => alternative(
            b"\xfb\xa4\xbc\xc3,\x87",
            vec![wire_list_leaf(field0)?, Wire::integer(*field1)],
        ),
        PlanType::ListIntoIterMutRef(field0, field1) => alternative(
            b"W\xdeO;\xa8\x13",
            vec![wire_list_leaf(field0)?, Wire::integer(*field1)],
        ),
        PlanType::Set(field0) => alternative(b"\xb6\xf6\xf3\xad\x07\xb3", vec![wire_list_leaf(field0)?]),
        PlanType::SetRef(field0) => alternative(b"\x8a\xc6\x9b\x0e\xb7\xbb", vec![wire_list_leaf(field0)?]),
        PlanType::SetMutRef(field0) => alternative(b"\xcaydbR ", vec![wire_list_leaf(field0)?]),
        PlanType::Dict(field0, field1) => alternative(
            b"\x96\x11\xd3\xd8\x00\xfc",
            vec![wire_list_leaf(field0)?, wire_list_leaf(field1)?],
        ),
        PlanType::DictRef(field0, field1) => alternative(
            b"\xc2\x1fVr6\xdd",
            vec![wire_list_leaf(field0)?, wire_list_leaf(field1)?],
        ),
        PlanType::DictMutRef(field0, field1) => alternative(
            b"b\xbe\xe3\xf3\xd1\xdb",
            vec![wire_list_leaf(field0)?, wire_list_leaf(field1)?],
        ),
        PlanType::RangeValue => unit(b"\xaf\xd5\xff\xa5\xa9\x15"),
        PlanType::ZipIterator(field0, field1) => alternative(
            b"\xda\x10<v\xc1;",
            vec![wire_tuple_element(field0)?, wire_tuple_element(field1)?],
        ),
        PlanType::ZipIteratorRef(field0, field1) => alternative(
            b"\xc8O\xb0\x89\x03!",
            vec![wire_tuple_element(field0)?, wire_tuple_element(field1)?],
        ),
        PlanType::ZipIteratorMutRef(field0, field1) => alternative(
            b"|\x0a\x10\xd6\xf4\x8f",
            vec![wire_tuple_element(field0)?, wire_tuple_element(field1)?],
        ),
        PlanType::Generator(field0, field1) => alternative(
            b"\x1a\xde`\xab\xe9?",
            vec![wire_list_leaf(field0)?, Wire::integer(*field1)],
        ),
        PlanType::GeneratorMutRef(field0, field1) => alternative(
            b"A\xe1l\xc9\xd0E",
            vec![wire_list_leaf(field0)?, Wire::integer(*field1)],
        ),
        PlanType::GeneratorYield(field0, field1) => {
            alternative(b"\xb9H#:\xc32", vec![wire_list_leaf(field0)?, Wire::integer(*field1)])
        }
        PlanType::GeneratorYieldRef(field0, field1) => {
            alternative(b"=-X\x17O\xe0", vec![wire_list_leaf(field0)?, Wire::integer(*field1)])
        }
        _ => return wire_plan_type_2(value),
    })
}

/// Project one `StoredLayoutPlanType` alternative onto its identity and positional payload.
fn wire_plan_type_2(value: &PlanType) -> Result<Wire, String> {
    Ok(match value {
        PlanType::FunctionPointer(field0) => alternative(b"\xa7\x0a\xca\xf0X`", vec![list(field0, wire_plan_type)?]),
        PlanType::FunctionItem(field0, field1) => alternative(
            b"\xff\x92\x01\x14t\x9f",
            vec![Wire::Text((field0).clone()), list(field1, wire_plan_type)?],
        ),
        PlanType::Closure(field0) => alternative(b"\xe9\x0da}+U", vec![list(field0, wire_plan_type)?]),
        PlanType::NativeClosure(field0, field1, field2) => alternative(
            b"Y\x0f\xc7\xc6\x8e\x97",
            vec![
                list(field0, wire_plan_type)?,
                Wire::Boolean(*field1),
                Wire::Boolean(*field2),
            ],
        ),
        PlanType::NativeTraitObject(field0, field1, field2) => alternative(
            b"\x5cz%\xe2\x9cR",
            vec![
                Wire::Text((field0).clone()),
                Wire::Boolean(*field1),
                Wire::Boolean(*field2),
            ],
        ),
        PlanType::SharedCallable(field0) => alternative(b"V\xfe9\xc0\xf7\xb9", vec![list(field0, wire_plan_type)?]),
        PlanType::ClosureOnce(field0) => alternative(b"\xd3\x9eqf\x1f.", vec![list(field0, wire_plan_type)?]),
        PlanType::ClosureRef(field0) => alternative(b"\x7f&q\xb2)\xed", vec![list(field0, wire_plan_type)?]),
        PlanType::BorrowingClosure(field0) => alternative(b"`\xa1v\x08\xb0\xf4", vec![list(field0, wire_plan_type)?]),
        PlanType::BorrowingClosureRef(field0) => {
            alternative(b"\xec\xdb\xa9E\xf7\xde", vec![list(field0, wire_plan_type)?])
        }
        PlanType::CallableArguments(field0) => {
            alternative(b"\xb7\xe5\xaa\xe2\xfbm", vec![list(field0, wire_plan_type)?])
        }
        PlanType::Future(field0) => alternative(b"acl\xde\xf5G", vec![list(field0, wire_plan_type)?]),
        PlanType::NativeStr => unit(b"\x89\xd2qrhj"),
        PlanType::NativeSlice(field0) => alternative(b"\x1f\x04\x02YL\xb8", vec![list(field0, wire_plan_type)?]),
        PlanType::SliceRef(field0) => alternative(b"\xa1\xf0\x0b\xce\x97\xfb", vec![list(field0, wire_plan_type)?]),
        PlanType::SliceMutRef(field0) => alternative(b"\xabf`]\xf9\xcc", vec![list(field0, wire_plan_type)?]),
        PlanType::NativeArray(field0, field1) => alternative(
            b"0?\xb1)\xac\xe5",
            vec![list(field0, wire_plan_type)?, Wire::integer(*field1)],
        ),
        PlanType::NativeArrayRef(field0, field1) => alternative(
            b"x?\xbd{\xe2\x8f",
            vec![list(field0, wire_plan_type)?, Wire::integer(*field1)],
        ),
        PlanType::NativeArrayMutRef(field0, field1) => alternative(
            b"o\xda\x01L\xed0",
            vec![list(field0, wire_plan_type)?, Wire::integer(*field1)],
        ),
        PlanType::CChar => unit(b"D,\xa28\xaa\xb8"),
        PlanType::RawPointer(field0, field1) => alternative(
            b"'\xee\xe0\xd7P=",
            vec![Wire::Boolean(*field0), list(field1, wire_plan_type)?],
        ),
        PlanType::Boxed(field0) => alternative(b"\x1e\x82j\xeb<\xa2", vec![list(field0, wire_plan_type)?]),
        _ => return Err("native-interface refuses an unsupported PlanType alternative".to_owned()),
    })
}

/// Read one `StoredLayoutPlanType` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_plan_type(value: &Wire, positions: &WirePositions) -> Result<PlanType, String> {
    let (identity, fields) = wire_variant(value)?;
    read_plan_type_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutPlanType` alternative from part 0 of its identity table.
fn read_plan_type_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<PlanType, String> {
    let _ = positions;
    Ok(match identity {
        b"\x0b_`\x80p\xc6" if fields.len() == 0 => PlanType::Int,
        b"\x1ai<\x00\xc4\x0c" if fields.len() == 0 => PlanType::Float,
        b"1Fb\x8c\x11\x8c" if fields.len() == 0 => PlanType::I8,
        b"\xb9\xff?nq\xda" if fields.len() == 0 => PlanType::I16,
        b"\xbe-,\xd01}" if fields.len() == 0 => PlanType::I32,
        b"G\xeck\xdaM\xa3" if fields.len() == 0 => PlanType::I128,
        b"\xec|D\x91\xf6\x86" if fields.len() == 0 => PlanType::ISize,
        b"\xbaL\xadu\xa6Z" if fields.len() == 0 => PlanType::USize,
        b"4s\xd3\x8a\x12A" if fields.len() == 0 => PlanType::U8,
        b"\xef\x09\xbc\x80\xcdb" if fields.len() == 0 => PlanType::U16,
        b"\x80\x07$\x86\xf2\xea" if fields.len() == 0 => PlanType::U32,
        b"\x84\xb8\x9e\x22ET" if fields.len() == 0 => PlanType::U64,
        b"\x0c\x80\x0d\xc4\xf9I" if fields.len() == 0 => PlanType::U128,
        b"\x82%Z\xeel\xcc" if fields.len() == 0 => PlanType::F32,
        b"\xa4M\x7ft\xc8)" if fields.len() == 0 => PlanType::F64,
        b"I\xf4\x11\xf0\xa1\xa7" if fields.len() == 0 => PlanType::Bool,
        b"NTY`\xf1\xbf" if fields.len() == 0 => PlanType::Unit,
        b"\xa0?\xf2#F\x9f" if fields.len() == 1 => {
            PlanType::CheckedNumeric(read_sized_numeric(&fields[0].value, positions)?)
        }
        b"c\x00\xef\x80\x0b\xb8" if fields.len() == 0 => PlanType::Never,
        b"\xe6\xf5\xc36\x16\xf4" if fields.len() == 0 => PlanType::UnitFunction,
        b"\x03\xa6\xc5\x01j\x86" if fields.len() == 0 => PlanType::EnumTag,
        b"\xc3\xf45\xc7(Q" if fields.len() == 0 => PlanType::CheckedInt,
        b"\xf16\xeex\xc3;" if fields.len() == 1 => PlanType::Tuple(read_list(&fields[0].value, |item0| {
            read_tuple_element(item0, positions)
        })?),
        b"\xb2\xef#\x0e\x7fO" if fields.len() == 0 => PlanType::String,
        b"\xbd\xb4\xa2v\x05;" if fields.len() == 1 => PlanType::TupleRef(read_list(&fields[0].value, |item0| {
            read_tuple_element(item0, positions)
        })?),
        b"\xaeV\xd77[\xff" if fields.len() == 1 => PlanType::TupleMutRef(read_list(&fields[0].value, |item0| {
            read_tuple_element(item0, positions)
        })?),
        b"\xbfkl\x9f\xdb\xc5" if fields.len() == 0 => PlanType::StringRef,
        b"\x97 \xde\x22\xd7\xd4" if fields.len() == 1 => {
            PlanType::ScalarRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xcbeb\x95hA" if fields.len() == 1 => {
            PlanType::ScalarMutRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xa45\x1e\xa3g\xb4" if fields.len() == 0 => PlanType::StringMutRef,
        b"\x18Q}3-\x1b" if fields.len() == 0 => PlanType::Decimal,
        b"O\xab\xe1W\xe8\xf9" if fields.len() == 0 => PlanType::FrozenStr,
        b"\xa5\x1aU\xd1;\xee" if fields.len() == 0 => PlanType::FrozenBytes,
        b"`ar\x85\xa4\xcb" if fields.len() == 2 => PlanType::FrozenCollection(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        b"T\xf9p\x0cZ3" if fields.len() == 2 => PlanType::FrozenCollectionRef(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        b"\xeb\x90\x0dwd\xd0" if fields.len() == 1 => {
            PlanType::TypeToken(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        _ => return read_plan_type_alternative_1(identity, fields, positions),
    })
}

/// Restore one `StoredLayoutPlanType` alternative from part 1 of its identity table.
fn read_plan_type_alternative_1(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<PlanType, String> {
    let _ = positions;
    Ok(match identity {
        b"\xd5\xdf\x07x\xa5\x0a" if fields.len() == 0 => PlanType::StaticText,
        b"_\x93\x11xcg" if fields.len() == 0 => PlanType::StaticBytes,
        b"\xbd\xfa\xef\x14\xbe\x91" if fields.len() == 0 => PlanType::ValidationError,
        b"\x1ejG\x9a\xae\xf1" if fields.len() == 0 => PlanType::ValidationErrorRef,
        b"(+u\x8f\x05\x8a" if fields.len() == 0 => PlanType::StrRef,
        b"\x85~Q\x14\x0d\x18" if fields.len() == 1 => PlanType::StringArray(number(&fields[0].value)?),
        b"\x8a\xe3R\x0a\xe0\xcb" if fields.len() == 1 => PlanType::StrArray(number(&fields[0].value)?),
        b"\xeb2P\xd3\xfc<" if fields.len() == 1 => PlanType::StringArrayRef(number(&fields[0].value)?),
        b"\xb1\x90\x07D|\x13" if fields.len() == 1 => PlanType::StrArrayRef(number(&fields[0].value)?),
        b"T\x10\x8d\xaf\xaa\xc5" if fields.len() == 2 => {
            PlanType::Enum(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"\xc0\xa35\x8f\x22\x01" if fields.len() == 2 => {
            PlanType::EnumRef(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"zq\xac~]|" if fields.len() == 2 => PlanType::EnumMutRef(number(&fields[0].value)?, text(&fields[1].value)?),
        b"^,aL#\xf0" if fields.len() == 2 => PlanType::Model(number(&fields[0].value)?, text(&fields[1].value)?),
        b"C\xc3U\x17\xae\x15" if fields.len() == 2 => {
            PlanType::ModelRef(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"\xb0\x02\x8e\xa2$\x98" if fields.len() == 2 => {
            PlanType::ModelMutRef(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"\x10\x99\xe21t\xe8" if fields.len() == 0 => PlanType::StringSlice,
        b"\xb7\xe7\x93\x84\xb2\xfb" if fields.len() == 0 => PlanType::StrSlice,
        b"o /T\xa7\xb2" if fields.len() == 2 => {
            PlanType::List(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"W\x16\x82\x93!\xd4" if fields.len() == 2 => {
            PlanType::ListRef(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"/@\x05\xe8\xd2Y" if fields.len() == 2 => {
            PlanType::ListMutRef(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"\xfb\xa4\xbc\xc3,\x87" if fields.len() == 2 => {
            PlanType::ListIntoIter(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"W\xdeO;\xa8\x13" if fields.len() == 2 => {
            PlanType::ListIntoIterMutRef(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"\xb6\xf6\xf3\xad\x07\xb3" if fields.len() == 1 => PlanType::Set(read_list_leaf(&fields[0].value, positions)?),
        b"\x8a\xc6\x9b\x0e\xb7\xbb" if fields.len() == 1 => {
            PlanType::SetRef(read_list_leaf(&fields[0].value, positions)?)
        }
        b"\xcaydbR " if fields.len() == 1 => PlanType::SetMutRef(read_list_leaf(&fields[0].value, positions)?),
        b"\x96\x11\xd3\xd8\x00\xfc" if fields.len() == 2 => PlanType::Dict(
            read_list_leaf(&fields[0].value, positions)?,
            read_list_leaf(&fields[1].value, positions)?,
        ),
        b"\xc2\x1fVr6\xdd" if fields.len() == 2 => PlanType::DictRef(
            read_list_leaf(&fields[0].value, positions)?,
            read_list_leaf(&fields[1].value, positions)?,
        ),
        b"b\xbe\xe3\xf3\xd1\xdb" if fields.len() == 2 => PlanType::DictMutRef(
            read_list_leaf(&fields[0].value, positions)?,
            read_list_leaf(&fields[1].value, positions)?,
        ),
        b"\xaf\xd5\xff\xa5\xa9\x15" if fields.len() == 0 => PlanType::RangeValue,
        b"\xda\x10<v\xc1;" if fields.len() == 2 => PlanType::ZipIterator(
            read_tuple_element(&fields[0].value, positions)?,
            read_tuple_element(&fields[1].value, positions)?,
        ),
        b"\xc8O\xb0\x89\x03!" if fields.len() == 2 => PlanType::ZipIteratorRef(
            read_tuple_element(&fields[0].value, positions)?,
            read_tuple_element(&fields[1].value, positions)?,
        ),
        b"|\x0a\x10\xd6\xf4\x8f" if fields.len() == 2 => PlanType::ZipIteratorMutRef(
            read_tuple_element(&fields[0].value, positions)?,
            read_tuple_element(&fields[1].value, positions)?,
        ),
        b"\x1a\xde`\xab\xe9?" if fields.len() == 2 => {
            PlanType::Generator(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"A\xe1l\xc9\xd0E" if fields.len() == 2 => {
            PlanType::GeneratorMutRef(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"\xb9H#:\xc32" if fields.len() == 2 => {
            PlanType::GeneratorYield(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"=-X\x17O\xe0" if fields.len() == 2 => {
            PlanType::GeneratorYieldRef(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        _ => return read_plan_type_alternative_2(identity, fields, positions),
    })
}

/// Restore one `StoredLayoutPlanType` alternative from part 2 of its identity table.
fn read_plan_type_alternative_2(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<PlanType, String> {
    let _ = positions;
    Ok(match identity {
        b"\xa7\x0a\xca\xf0X`" if fields.len() == 1 => {
            PlanType::FunctionPointer(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xff\x92\x01\x14t\x9f" if fields.len() == 2 => PlanType::FunctionItem(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        b"\xe9\x0da}+U" if fields.len() == 1 => {
            PlanType::Closure(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"Y\x0f\xc7\xc6\x8e\x97" if fields.len() == 3 => PlanType::NativeClosure(
            read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?,
            boolean_value(&fields[1].value)?,
            boolean_value(&fields[2].value)?,
        ),
        b"\x5cz%\xe2\x9cR" if fields.len() == 3 => PlanType::NativeTraitObject(
            text(&fields[0].value)?,
            boolean_value(&fields[1].value)?,
            boolean_value(&fields[2].value)?,
        ),
        b"V\xfe9\xc0\xf7\xb9" if fields.len() == 1 => {
            PlanType::SharedCallable(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xd3\x9eqf\x1f." if fields.len() == 1 => {
            PlanType::ClosureOnce(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\x7f&q\xb2)\xed" if fields.len() == 1 => {
            PlanType::ClosureRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"`\xa1v\x08\xb0\xf4" if fields.len() == 1 => {
            PlanType::BorrowingClosure(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xec\xdb\xa9E\xf7\xde" if fields.len() == 1 => {
            PlanType::BorrowingClosureRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xb7\xe5\xaa\xe2\xfbm" if fields.len() == 1 => {
            PlanType::CallableArguments(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"acl\xde\xf5G" if fields.len() == 1 => {
            PlanType::Future(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\x89\xd2qrhj" if fields.len() == 0 => PlanType::NativeStr,
        b"\x1f\x04\x02YL\xb8" if fields.len() == 1 => {
            PlanType::NativeSlice(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xa1\xf0\x0b\xce\x97\xfb" if fields.len() == 1 => {
            PlanType::SliceRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xabf`]\xf9\xcc" if fields.len() == 1 => {
            PlanType::SliceMutRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"0?\xb1)\xac\xe5" if fields.len() == 2 => PlanType::NativeArray(
            read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?,
            number(&fields[1].value)?,
        ),
        b"x?\xbd{\xe2\x8f" if fields.len() == 2 => PlanType::NativeArrayRef(
            read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?,
            number(&fields[1].value)?,
        ),
        b"o\xda\x01L\xed0" if fields.len() == 2 => PlanType::NativeArrayMutRef(
            read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?,
            number(&fields[1].value)?,
        ),
        b"D,\xa28\xaa\xb8" if fields.len() == 0 => PlanType::CChar,
        b"'\xee\xe0\xd7P=" if fields.len() == 2 => PlanType::RawPointer(
            boolean_value(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        b"\x1e\x82j\xeb<\xa2" if fields.len() == 1 => {
            PlanType::Boxed(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        _ => return Err("unknown or incomplete StoredLayoutPlanType alternative".to_owned()),
    })
}

/// Project one `StoredLayoutSizedNumeric` alternative onto its identity and positional payload.
pub(crate) fn wire_sized_numeric(value: &SizedNumeric) -> Result<Wire, String> {
    Ok(match value {
        SizedNumeric::I8 => unit(b"1Fb\x8c\x11\x8c"),
        SizedNumeric::I16 => unit(b"\xb9\xff?nq\xda"),
        SizedNumeric::I32 => unit(b"\xbe-,\xd01}"),
        SizedNumeric::I128 => unit(b"G\xeck\xdaM\xa3"),
        SizedNumeric::U8 => unit(b"4s\xd3\x8a\x12A"),
        SizedNumeric::U16 => unit(b"\xef\x09\xbc\x80\xcdb"),
        SizedNumeric::U32 => unit(b"\x80\x07$\x86\xf2\xea"),
        SizedNumeric::U64 => unit(b"\x84\xb8\x9e\x22ET"),
        SizedNumeric::U128 => unit(b"\x0c\x80\x0d\xc4\xf9I"),
        SizedNumeric::F32 => unit(b"\x82%Z\xeel\xcc"),
        SizedNumeric::F64 => unit(b"\xa4M\x7ft\xc8)"),
        SizedNumeric::ISize => unit(b"\xec|D\x91\xf6\x86"),
        SizedNumeric::USize => unit(b"\xbaL\xadu\xa6Z"),
    })
}

/// Read one `StoredLayoutSizedNumeric` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_sized_numeric(value: &Wire, positions: &WirePositions) -> Result<SizedNumeric, String> {
    let (identity, fields) = wire_variant(value)?;
    read_sized_numeric_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutSizedNumeric` alternative from part 0 of its identity table.
fn read_sized_numeric_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<SizedNumeric, String> {
    let _ = positions;
    Ok(match identity {
        b"1Fb\x8c\x11\x8c" if fields.len() == 0 => SizedNumeric::I8,
        b"\xb9\xff?nq\xda" if fields.len() == 0 => SizedNumeric::I16,
        b"\xbe-,\xd01}" if fields.len() == 0 => SizedNumeric::I32,
        b"G\xeck\xdaM\xa3" if fields.len() == 0 => SizedNumeric::I128,
        b"4s\xd3\x8a\x12A" if fields.len() == 0 => SizedNumeric::U8,
        b"\xef\x09\xbc\x80\xcdb" if fields.len() == 0 => SizedNumeric::U16,
        b"\x80\x07$\x86\xf2\xea" if fields.len() == 0 => SizedNumeric::U32,
        b"\x84\xb8\x9e\x22ET" if fields.len() == 0 => SizedNumeric::U64,
        b"\x0c\x80\x0d\xc4\xf9I" if fields.len() == 0 => SizedNumeric::U128,
        b"\x82%Z\xeel\xcc" if fields.len() == 0 => SizedNumeric::F32,
        b"\xa4M\x7ft\xc8)" if fields.len() == 0 => SizedNumeric::F64,
        b"\xec|D\x91\xf6\x86" if fields.len() == 0 => SizedNumeric::ISize,
        b"\xbaL\xadu\xa6Z" if fields.len() == 0 => SizedNumeric::USize,
        _ => return Err("unknown or incomplete StoredLayoutSizedNumeric alternative".to_owned()),
    })
}

/// Project one `StoredLayoutTupleElement` alternative onto its identity and positional payload.
pub(crate) fn wire_tuple_element(value: &TupleElement) -> Result<Wire, String> {
    Ok(match value {
        TupleElement::Int => unit(b"\x0b_`\x80p\xc6"),
        TupleElement::Float => unit(b"\x1ai<\x00\xc4\x0c"),
        TupleElement::Numeric(field0) => alternative(b"6u-l\xd1G", vec![wire_sized_numeric(field0)?]),
        TupleElement::Bool => unit(b"I\xf4\x11\xf0\xa1\xa7"),
        TupleElement::Unit => unit(b"NTY`\xf1\xbf"),
        TupleElement::String => unit(b"\xb2\xef#\x0e\x7fO"),
        TupleElement::List(field0, field1) => {
            alternative(b"o /T\xa7\xb2", vec![wire_list_leaf(field0)?, Wire::integer(*field1)])
        }
        TupleElement::Dict(field0, field1) => alternative(
            b"\x96\x11\xd3\xd8\x00\xfc",
            vec![wire_list_leaf(field0)?, wire_list_leaf(field1)?],
        ),
        TupleElement::Set(field0) => alternative(b"\xb6\xf6\xf3\xad\x07\xb3", vec![wire_list_leaf(field0)?]),
        TupleElement::Model(field0, field1) => {
            alternative(b"^,aL#\xf0", vec![Wire::integer(*field0), Wire::Text((field1).clone())])
        }
        TupleElement::Enum(field0, field1) => alternative(
            b"T\x10\x8d\xaf\xaa\xc5",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        TupleElement::Tuple(field0) => alternative(b"\xf16\xeex\xc3;", vec![list(field0, wire_tuple_element)?]),
        TupleElement::FunctionPointer(field0) => {
            alternative(b"\xa7\x0a\xca\xf0X`", vec![list(field0, wire_plan_type)?])
        }
        TupleElement::Reference(field0, field1) => alternative(
            b"q\xbf\x90\x93_\xc2",
            vec![Wire::Boolean(*field0), list(field1, wire_plan_type)?],
        ),
        TupleElement::ModelRef(field0, field1) => alternative(
            b"C\xc3U\x17\xae\x15",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        TupleElement::ModelMutRef(field0, field1) => alternative(
            b"\xb0\x02\x8e\xa2$\x98",
            vec![Wire::integer(*field0), Wire::Text((field1).clone())],
        ),
        TupleElement::SliceRef(field0) => alternative(b"\xa1\xf0\x0b\xce\x97\xfb", vec![list(field0, wire_plan_type)?]),
        TupleElement::SliceMutRef(field0) => alternative(b"\xabf`]\xf9\xcc", vec![list(field0, wire_plan_type)?]),
        TupleElement::FrozenStr => unit(b"O\xab\xe1W\xe8\xf9"),
        TupleElement::StaticText => unit(b"\xd5\xdf\x07x\xa5\x0a"),
        TupleElement::StaticBytes => unit(b"_\x93\x11xcg"),
        TupleElement::FrozenBytes => unit(b"\xa5\x1aU\xd1;\xee"),
        TupleElement::FrozenCollection(field0, field1) => alternative(
            b"`ar\x85\xa4\xcb",
            vec![Wire::Text((field0).clone()), list(field1, wire_plan_type)?],
        ),
    })
}

/// Read one `StoredLayoutTupleElement` alternative by its identity, requiring its exact payload arity.
pub(crate) fn read_tuple_element(value: &Wire, positions: &WirePositions) -> Result<TupleElement, String> {
    let (identity, fields) = wire_variant(value)?;
    read_tuple_element_alternative(identity, fields, positions)
}

/// Restore one `StoredLayoutTupleElement` alternative from part 0 of its identity table.
fn read_tuple_element_alternative(
    identity: &[u8],
    fields: &[WireField],
    positions: &WirePositions,
) -> Result<TupleElement, String> {
    let _ = positions;
    Ok(match identity {
        b"\x0b_`\x80p\xc6" if fields.len() == 0 => TupleElement::Int,
        b"\x1ai<\x00\xc4\x0c" if fields.len() == 0 => TupleElement::Float,
        b"6u-l\xd1G" if fields.len() == 1 => TupleElement::Numeric(read_sized_numeric(&fields[0].value, positions)?),
        b"I\xf4\x11\xf0\xa1\xa7" if fields.len() == 0 => TupleElement::Bool,
        b"NTY`\xf1\xbf" if fields.len() == 0 => TupleElement::Unit,
        b"\xb2\xef#\x0e\x7fO" if fields.len() == 0 => TupleElement::String,
        b"o /T\xa7\xb2" if fields.len() == 2 => {
            TupleElement::List(read_list_leaf(&fields[0].value, positions)?, number(&fields[1].value)?)
        }
        b"\x96\x11\xd3\xd8\x00\xfc" if fields.len() == 2 => TupleElement::Dict(
            read_list_leaf(&fields[0].value, positions)?,
            read_list_leaf(&fields[1].value, positions)?,
        ),
        b"\xb6\xf6\xf3\xad\x07\xb3" if fields.len() == 1 => {
            TupleElement::Set(read_list_leaf(&fields[0].value, positions)?)
        }
        b"^,aL#\xf0" if fields.len() == 2 => TupleElement::Model(number(&fields[0].value)?, text(&fields[1].value)?),
        b"T\x10\x8d\xaf\xaa\xc5" if fields.len() == 2 => {
            TupleElement::Enum(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"\xf16\xeex\xc3;" if fields.len() == 1 => TupleElement::Tuple(read_list(&fields[0].value, |item0| {
            read_tuple_element(item0, positions)
        })?),
        b"\xa7\x0a\xca\xf0X`" if fields.len() == 1 => {
            TupleElement::FunctionPointer(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"q\xbf\x90\x93_\xc2" if fields.len() == 2 => TupleElement::Reference(
            boolean_value(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        b"C\xc3U\x17\xae\x15" if fields.len() == 2 => {
            TupleElement::ModelRef(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"\xb0\x02\x8e\xa2$\x98" if fields.len() == 2 => {
            TupleElement::ModelMutRef(number(&fields[0].value)?, text(&fields[1].value)?)
        }
        b"\xa1\xf0\x0b\xce\x97\xfb" if fields.len() == 1 => {
            TupleElement::SliceRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"\xabf`]\xf9\xcc" if fields.len() == 1 => {
            TupleElement::SliceMutRef(read_list(&fields[0].value, |item0| read_plan_type(item0, positions))?)
        }
        b"O\xab\xe1W\xe8\xf9" if fields.len() == 0 => TupleElement::FrozenStr,
        b"\xd5\xdf\x07x\xa5\x0a" if fields.len() == 0 => TupleElement::StaticText,
        b"_\x93\x11xcg" if fields.len() == 0 => TupleElement::StaticBytes,
        b"\xa5\x1aU\xd1;\xee" if fields.len() == 0 => TupleElement::FrozenBytes,
        b"`ar\x85\xa4\xcb" if fields.len() == 2 => TupleElement::FrozenCollection(
            text(&fields[0].value)?,
            read_list(&fields[1].value, |item0| read_plan_type(item0, positions))?,
        ),
        _ => return Err("unknown or incomplete StoredLayoutTupleElement alternative".to_owned()),
    })
}
