//! Primitive field projection for the first checker-record profile. The Incan contract owns framing and bytes.

use super::super::super::canonical_nodes::NodeBindingError;
use crate::ast::ParamKind;
use crate::symbols::{CallableParam, ResolvedType};

/// Project only explicitly supported scalar/collection/callable types, never a native Debug or serde representation.
pub(super) fn type_fields(ty: &ResolvedType, fields: &mut Vec<String>) -> Result<(), NodeBindingError> {
    match ty {
        ResolvedType::Int => fields.push("int".into()),
        ResolvedType::Float => fields.push("float".into()),
        ResolvedType::Numeric(id) => {
            fields.push("numeric".into());
            fields.push(incan_lang::lang::types::numerics::as_str(*id).into());
        }
        ResolvedType::Bool => fields.push("bool".into()),
        ResolvedType::Str => fields.push("str".into()),
        ResolvedType::Bytes => fields.push("bytes".into()),
        ResolvedType::RustPath(path) if !path.is_empty() => fields.extend(["rust-path".into(), path.clone()]),
        ResolvedType::Unit => fields.push("unit".into()),
        ResolvedType::Unknown => fields.push("unknown".into()),
        ResolvedType::Named(name) if !name.is_empty() => fields.extend(["named".into(), name.clone()]),
        ResolvedType::TypeVar(name) if !name.is_empty() => fields.extend(["type-var".into(), name.clone()]),
        ResolvedType::Generic(_, arguments)
            if (ty.is_result() && arguments.len() == 2) || (ty.is_option() && arguments.len() == 1) =>
        {
            fields.push(if ty.is_result() { "result" } else { "option" }.into());
            for argument in arguments {
                type_fields(argument, fields)?;
            }
        }
        ResolvedType::Generic(name, arguments) if name == "List" && arguments.len() == 1 => {
            fields.push("list".into());
            type_fields(&arguments[0], fields)?;
        }
        ResolvedType::Generic(name, arguments)
            if incan_lang::lang::types::ownership::from_str(name).is_some() && arguments.len() == 1 =>
        {
            fields.push("ownership".into());
            fields.push(name.clone());
            type_fields(&arguments[0], fields)?;
        }
        ResolvedType::Generic(name, arguments) if !name.is_empty() => {
            fields.extend(["generic".into(), name.clone(), arguments.len().to_string()]);
            for argument in arguments {
                type_fields(argument, fields)?;
            }
        }
        ResolvedType::Function(parameters, result) => {
            fields.push("callable".into());
            parameter_fields(parameters, fields)?;
            type_fields(result, fields)?;
        }
        _ => return Err(NodeBindingError::Unsupported("checked type")),
    }
    Ok(())
}

/// Consume exactly one source type from the canonical prefix grammar; depth is bounded by the record's token limit.
pub(super) fn read_type(fields: &mut std::slice::Iter<'_, String>) -> Result<ResolvedType, NodeBindingError> {
    read_type_at(fields, 0)
}

/// Bound recursive type decoding at the same structural depth admitted by the Incan schema.
fn read_type_at(fields: &mut std::slice::Iter<'_, String>, depth: usize) -> Result<ResolvedType, NodeBindingError> {
    if depth > 32 {
        return Err(NodeBindingError::Unsupported("type nesting"));
    }
    Ok(match next(fields)? {
        "int" => ResolvedType::Int,
        "float" => ResolvedType::Float,
        "numeric" => {
            let name = next(fields)?;
            let id = incan_lang::lang::types::numerics::from_str(name)
                .filter(|id| incan_lang::lang::types::numerics::as_str(*id) == name)
                .ok_or(NodeBindingError::Unsupported("numeric type"))?;
            ResolvedType::Numeric(id)
        }
        "bool" => ResolvedType::Bool,
        "str" => ResolvedType::Str,
        "bytes" => ResolvedType::Bytes,
        "rust-path" => {
            let path = next(fields)?;
            if path.is_empty() {
                return Err(NodeBindingError::Unsupported("empty Rust path"));
            }
            ResolvedType::RustPath(path.into())
        }
        "unit" => ResolvedType::Unit,
        "unknown" => ResolvedType::Unknown,
        "named" => {
            let name = next(fields)?;
            if name.is_empty() {
                return Err(NodeBindingError::Unsupported("empty named type"));
            }
            ResolvedType::Named(name.into())
        }
        "type-var" => {
            let name = next(fields)?;
            if name.is_empty() {
                return Err(NodeBindingError::Unsupported("empty type binder"));
            }
            ResolvedType::TypeVar(name.into())
        }
        "result" => ResolvedType::Generic(
            "Result".into(),
            vec![read_type_at(fields, depth + 1)?, read_type_at(fields, depth + 1)?],
        ),
        "option" => ResolvedType::Generic("Option".into(), vec![read_type_at(fields, depth + 1)?]),
        "list" => ResolvedType::Generic("List".into(), vec![read_type_at(fields, depth + 1)?]),
        "ownership" => {
            let name = next(fields)?;
            let mode = incan_lang::lang::types::ownership::from_str(name)
                .filter(|mode| mode.as_str() == name)
                .ok_or(NodeBindingError::Unsupported("ownership mode"))?;
            ResolvedType::Generic(mode.as_str().into(), vec![read_type_at(fields, depth + 1)?])
        }
        "generic" => {
            let name = next(fields)?;
            if name.is_empty() {
                return Err(NodeBindingError::Unsupported("empty generic type"));
            }
            let count = next(fields)?
                .parse::<usize>()
                .map_err(|_| NodeBindingError::MissingNode)?;
            if count > fields.len() {
                return Err(NodeBindingError::MissingNode);
            }
            let mut arguments = Vec::with_capacity(count);
            for _ in 0..count {
                arguments.push(read_type_at(fields, depth + 1)?);
            }
            ResolvedType::Generic(name.into(), arguments)
        }
        "callable" => ResolvedType::Function(
            read_parameters_at(fields, depth + 1)?,
            Box::new(read_type_at(fields, depth + 1)?),
        ),
        _ => return Err(NodeBindingError::Unsupported("canonical type tag")),
    })
}

/// Preserve parameter names and every source marker, even though parameter names do not distinguish overloads.
pub(super) fn parameter_fields(parameters: &[CallableParam], fields: &mut Vec<String>) -> Result<(), NodeBindingError> {
    fields.push(parameters.len().to_string());
    for parameter in parameters {
        fields.push(if parameter.name.is_some() { "named" } else { "unnamed" }.into());
        fields.push(parameter.name.clone().unwrap_or_default());
        fields.push(
            match parameter.kind {
                ParamKind::Normal => "normal",
                ParamKind::RestPositional => "rest",
                ParamKind::RestKeyword => "keywords",
            }
            .into(),
        );
        for flag in [parameter.has_default, parameter.is_partial_preset, parameter.is_mut] {
            fields.push(if flag { "true" } else { "false" }.into());
        }
        type_fields(&parameter.ty, fields)?;
    }
    Ok(())
}

/// Reconstruct the source parameter contract without consulting current checker type or call tables.
pub(super) fn read_parameters(
    fields: &mut std::slice::Iter<'_, String>,
) -> Result<Vec<CallableParam>, NodeBindingError> {
    read_parameters_at(fields, 0)
}

/// Preserve the enclosing type's nesting budget when a callable's parameter types are decoded.
fn read_parameters_at(
    fields: &mut std::slice::Iter<'_, String>,
    depth: usize,
) -> Result<Vec<CallableParam>, NodeBindingError> {
    let count = next(fields)?
        .parse::<usize>()
        .map_err(|_| NodeBindingError::MissingNode)?;
    if count > fields.len() {
        return Err(NodeBindingError::MissingNode);
    }
    let mut parameters = Vec::new();
    for _ in 0..count {
        let named = next(fields)?;
        let name = next(fields)?.to_owned();
        let name = match named {
            "named" => Some(name),
            "unnamed" if name.is_empty() => None,
            _ => return Err(NodeBindingError::MissingNode),
        };
        let kind = match next(fields)? {
            "normal" => ParamKind::Normal,
            "rest" => ParamKind::RestPositional,
            "keywords" => ParamKind::RestKeyword,
            _ => return Err(NodeBindingError::MissingNode),
        };
        let has_default = flag(fields)?;
        let is_partial_preset = flag(fields)?;
        let is_mut = flag(fields)?;
        parameters.push(CallableParam {
            name,
            surface_type_name: None,
            kind,
            has_default,
            is_partial_preset,
            is_mut,
            ty: read_type_at(fields, depth + 1)?,
        });
    }
    Ok(parameters)
}

/// Reject malformed booleans instead of treating an unknown spelling as false.
fn flag(fields: &mut std::slice::Iter<'_, String>) -> Result<bool, NodeBindingError> {
    match next(fields)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(NodeBindingError::MissingNode),
    }
}

/// A missing primitive is a malformed reference, never an empty semantic value.
pub(super) fn next<'a>(fields: &mut std::slice::Iter<'a, String>) -> Result<&'a str, NodeBindingError> {
    fields.next().map(String::as_str).ok_or(NodeBindingError::MissingNode)
}
