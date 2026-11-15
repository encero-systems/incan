//! Structural primitives for persisted native storage types.

use super::*;
use incan_semantics_core::{
    IncanCallableParam, IncanCallableParamKind, IncanInputAccess, IncanPrimitiveType, IncanType, RuntimeBuiltinType,
};

/// Encode one native storage type without relying on its diagnostic spelling or serde layout.
pub(super) fn native_type_fields(
    positions: &Positions,
    ty: &IncanType,
    fields: &mut Vec<String>,
) -> Result<(), NodeBindingError> {
    native_type_fields_at(positions, ty, fields, 0)
}

fn native_type_fields_at(
    positions: &Positions,
    ty: &IncanType,
    fields: &mut Vec<String>,
    depth: usize,
) -> Result<(), NodeBindingError> {
    if depth > 32 {
        return Err(NodeBindingError::Unsupported("native type nesting"));
    }
    match ty {
        IncanType::Never => fields.push("never".into()),
        IncanType::Primitive(value) => primitive_fields(value, fields),
        IncanType::Named(name) => fields.extend(["named".into(), name.clone()]),
        IncanType::Generic { base, args } => {
            fields.extend(["generic".into(), base.clone(), args.len().to_string()]);
            for value in args {
                native_type_fields_at(positions, value, fields, depth + 1)?;
            }
        }
        IncanType::Decimal { precision, scale } => {
            fields.extend(["decimal".into(), precision.to_string(), scale.to_string()]);
        }
        IncanType::Function { params, return_type } => {
            fields.extend(["function".into(), params.len().to_string()]);
            for param in params {
                native_parameter_fields(positions, param, fields, depth + 1)?;
            }
            native_type_fields_at(positions, return_type, fields, depth + 1)?;
        }
        IncanType::TypeToken(value) => nested(positions, "type-token", value, fields, depth)?,
        IncanType::Tuple(values) => {
            fields.extend(["tuple".into(), values.len().to_string()]);
            for value in values {
                native_type_fields_at(positions, value, fields, depth + 1)?;
            }
        }
        IncanType::TypeVar(name) => fields.extend(["type-var".into(), name.clone()]),
        IncanType::SelfType => fields.push("self-type".into()),
        IncanType::Ref(value) => nested(positions, "ref", value, fields, depth)?,
        IncanType::RefMut(value) => nested(positions, "ref-mut", value, fields, depth)?,
        IncanType::RustInteropPath(path) => fields.extend(["rust-path".into(), path.clone()]),
        IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError) => {
            fields.extend(["native-builtin".into(), "validation-error".into()]);
        }
        IncanType::Unsupported(reason) => fields.extend(["unsupported".into(), reason.clone()]),
        IncanType::Infer => fields.push("infer".into()),
        IncanType::Unknown => fields.push("unknown".into()),
        IncanType::NativeArray { element, length } => {
            fields.extend(["native-array".into(), length.to_string()]);
            native_type_fields_at(positions, element, fields, depth + 1)?;
        }
        IncanType::NativeTraitObject { path, send, sync } => {
            fields.extend([
                "native-trait-object".into(),
                path.clone(),
                send.to_string(),
                sync.to_string(),
            ]);
        }
        IncanType::NativePointer { mutable, pointee } => {
            fields.extend(["native-pointer".into(), mutable.to_string()]);
            native_type_fields_at(positions, pointee, fields, depth + 1)?;
        }
        IncanType::NativeCChar => fields.push("native-c-char".into()),
        IncanType::Trait(identity) => fields.extend(["trait".into(), positions.type_identity(identity)?]),
    }
    Ok(())
}

fn nested(
    positions: &Positions,
    tag: &str,
    value: &IncanType,
    fields: &mut Vec<String>,
    depth: usize,
) -> Result<(), NodeBindingError> {
    fields.push(tag.into());
    native_type_fields_at(positions, value, fields, depth + 1)
}

fn primitive_fields(value: &IncanPrimitiveType, fields: &mut Vec<String>) {
    let tag = match value {
        IncanPrimitiveType::Int => "int",
        IncanPrimitiveType::Float => "float",
        IncanPrimitiveType::Numeric(id) => {
            fields.extend([
                "primitive".into(),
                "numeric".into(),
                incan_lang::lang::types::numerics::as_str(*id).into(),
            ]);
            return;
        }
        IncanPrimitiveType::Bool => "bool",
        IncanPrimitiveType::Str => "str",
        IncanPrimitiveType::Bytes => "bytes",
        IncanPrimitiveType::FrozenStr => "frozen-str",
        IncanPrimitiveType::FrozenBytes => "frozen-bytes",
        IncanPrimitiveType::Unit => "unit",
    };
    fields.extend(["primitive".into(), tag.into()]);
}

fn native_parameter_fields(
    positions: &Positions,
    param: &IncanCallableParam,
    fields: &mut Vec<String>,
    depth: usize,
) -> Result<(), NodeBindingError> {
    match &param.name {
        Some(name) => fields.extend(["some".into(), name.clone()]),
        None => fields.push("none".into()),
    }
    fields.push(
        match param.kind {
            IncanCallableParamKind::Normal => "normal",
            IncanCallableParamKind::RestPositional => "rest",
            IncanCallableParamKind::RestKeyword => "keywords",
        }
        .into(),
    );
    fields.extend([
        param.has_default.to_string(),
        param.is_partial_preset.to_string(),
        param.is_mut.to_string(),
        param.native_owned.to_string(),
        match param.input_access {
            IncanInputAccess::Inferred => "inferred",
            IncanInputAccess::Shared => "shared",
            IncanInputAccess::Exclusive => "exclusive",
            IncanInputAccess::Owned => "owned",
            IncanInputAccess::InferredShared => "inferred-shared",
        }
        .into(),
    ]);
    native_type_fields_at(positions, &param.ty, fields, depth)
}

/// Decode one native storage type from the closed structural grammar.
pub(super) fn read_native_type(
    positions: &Positions,
    fields: &mut std::slice::Iter<'_, String>,
) -> Result<IncanType, NodeBindingError> {
    read_native_type_at(positions, fields, 0)
}

fn read_native_type_at(
    positions: &Positions,
    fields: &mut std::slice::Iter<'_, String>,
    depth: usize,
) -> Result<IncanType, NodeBindingError> {
    if depth > 32 {
        return Err(NodeBindingError::Unsupported("native type nesting"));
    }
    Ok(match next(fields)? {
        "never" => IncanType::Never,
        "primitive" => IncanType::Primitive(read_primitive(fields)?),
        "named" => IncanType::Named(nonempty(fields)?.into()),
        "generic" => {
            let base = nonempty(fields)?.to_owned();
            let count = read_number(fields)?;
            IncanType::Generic {
                base,
                args: read_native_types(positions, fields, count, depth)?,
            }
        }
        "decimal" => IncanType::Decimal {
            precision: read_u8(fields)?,
            scale: read_u8(fields)?,
        },
        "function" => {
            let count = read_number(fields)?;
            let mut params = Vec::with_capacity(count);
            for _ in 0..count {
                params.push(read_native_parameter(positions, fields, depth + 1)?);
            }
            IncanType::Function {
                params,
                return_type: Box::new(read_native_type_at(positions, fields, depth + 1)?),
            }
        }
        "type-token" => IncanType::TypeToken(Box::new(read_native_type_at(positions, fields, depth + 1)?)),
        "tuple" => {
            let count = read_number(fields)?;
            IncanType::Tuple(read_native_types(positions, fields, count, depth)?)
        }
        "type-var" => IncanType::TypeVar(nonempty(fields)?.into()),
        "self-type" => IncanType::SelfType,
        "ref" => IncanType::Ref(Box::new(read_native_type_at(positions, fields, depth + 1)?)),
        "ref-mut" => IncanType::RefMut(Box::new(read_native_type_at(positions, fields, depth + 1)?)),
        "rust-path" => IncanType::RustInteropPath(nonempty(fields)?.into()),
        "native-builtin" if next(fields)? == "validation-error" => {
            IncanType::NativeBuiltin(RuntimeBuiltinType::ValidationError)
        }
        "unsupported" => IncanType::Unsupported(next(fields)?.into()),
        "infer" => IncanType::Infer,
        "unknown" => IncanType::Unknown,
        "native-array" => IncanType::NativeArray {
            length: read_u64(fields)?,
            element: Box::new(read_native_type_at(positions, fields, depth + 1)?),
        },
        "native-trait-object" => IncanType::NativeTraitObject {
            path: nonempty(fields)?.into(),
            send: read_bool(fields)?,
            sync: read_bool(fields)?,
        },
        "native-pointer" => IncanType::NativePointer {
            mutable: read_bool(fields)?,
            pointee: Box::new(read_native_type_at(positions, fields, depth + 1)?),
        },
        "native-c-char" => IncanType::NativeCChar,
        "trait" => IncanType::Trait(positions.canonical_type(next(fields)?)?),
        _ => return Err(NodeBindingError::MissingNode),
    })
}

fn read_native_types(
    positions: &Positions,
    fields: &mut std::slice::Iter<'_, String>,
    count: usize,
    depth: usize,
) -> Result<Vec<IncanType>, NodeBindingError> {
    if count > fields.len() {
        return Err(NodeBindingError::MissingNode);
    }
    (0..count)
        .map(|_| read_native_type_at(positions, fields, depth + 1))
        .collect()
}

fn read_primitive(fields: &mut std::slice::Iter<'_, String>) -> Result<IncanPrimitiveType, NodeBindingError> {
    Ok(match next(fields)? {
        "int" => IncanPrimitiveType::Int,
        "float" => IncanPrimitiveType::Float,
        "numeric" => {
            let name = next(fields)?;
            let id = incan_lang::lang::types::numerics::from_str(name)
                .filter(|id| incan_lang::lang::types::numerics::as_str(*id) == name)
                .ok_or(NodeBindingError::MissingNode)?;
            IncanPrimitiveType::Numeric(id)
        }
        "bool" => IncanPrimitiveType::Bool,
        "str" => IncanPrimitiveType::Str,
        "bytes" => IncanPrimitiveType::Bytes,
        "frozen-str" => IncanPrimitiveType::FrozenStr,
        "frozen-bytes" => IncanPrimitiveType::FrozenBytes,
        "unit" => IncanPrimitiveType::Unit,
        _ => return Err(NodeBindingError::MissingNode),
    })
}

fn read_native_parameter(
    positions: &Positions,
    fields: &mut std::slice::Iter<'_, String>,
    depth: usize,
) -> Result<IncanCallableParam, NodeBindingError> {
    let name = match next(fields)? {
        "some" => Some(nonempty(fields)?.into()),
        "none" => None,
        _ => return Err(NodeBindingError::MissingNode),
    };
    let kind = match next(fields)? {
        "normal" => IncanCallableParamKind::Normal,
        "rest" => IncanCallableParamKind::RestPositional,
        "keywords" => IncanCallableParamKind::RestKeyword,
        _ => return Err(NodeBindingError::MissingNode),
    };
    let has_default = read_bool(fields)?;
    let is_partial_preset = read_bool(fields)?;
    let is_mut = read_bool(fields)?;
    let native_owned = read_bool(fields)?;
    let input_access = match next(fields)? {
        "inferred" => IncanInputAccess::Inferred,
        "shared" => IncanInputAccess::Shared,
        "exclusive" => IncanInputAccess::Exclusive,
        "owned" => IncanInputAccess::Owned,
        "inferred-shared" => IncanInputAccess::InferredShared,
        _ => return Err(NodeBindingError::MissingNode),
    };
    Ok(IncanCallableParam {
        name,
        ty: read_native_type_at(positions, fields, depth)?,
        kind,
        has_default,
        is_partial_preset,
        is_mut,
        native_owned,
        input_access,
    })
}

fn read_bool(fields: &mut std::slice::Iter<'_, String>) -> Result<bool, NodeBindingError> {
    match next(fields)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(NodeBindingError::MissingNode),
    }
}

fn nonempty<'a>(fields: &mut std::slice::Iter<'a, String>) -> Result<&'a str, NodeBindingError> {
    let value = next(fields)?;
    if value.is_empty() {
        Err(NodeBindingError::MissingNode)
    } else {
        Ok(value)
    }
}

fn read_u8(fields: &mut std::slice::Iter<'_, String>) -> Result<u8, NodeBindingError> {
    let value = next(fields)?;
    let number = value.parse::<u8>().map_err(|_| NodeBindingError::MissingNode)?;
    (number.to_string() == value)
        .then_some(number)
        .ok_or(NodeBindingError::MissingNode)
}

fn read_u64(fields: &mut std::slice::Iter<'_, String>) -> Result<u64, NodeBindingError> {
    let value = next(fields)?;
    let number = value.parse::<u64>().map_err(|_| NodeBindingError::MissingNode)?;
    (number.to_string() == value)
        .then_some(number)
        .ok_or(NodeBindingError::MissingNode)
}
