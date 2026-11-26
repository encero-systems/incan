//! Structural admission for Rust interop artifacts.

use super::*;
use crate::typechecker::type_info::{RustCheckedCall, RustCheckedSignature, RustIterationHooks};
use std::collections::{BTreeMap, HashMap};

/// Project checked Rust calls at their exact source call sites.
pub(super) fn project_checked_calls(
    positions: &Positions,
    values: &HashMap<Span, RustCheckedCall>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, value) in values {
        let mut row = vec![
            "artifact-rust".into(),
            positions.nodes.anchor(*span)?.path,
            "checked_calls".into(),
        ];
        rust_call_fields(positions, value, &mut row, 0)?;
        fields.push(row);
    }
    Ok(())
}

/// Restore one checked Rust call without re-running metadata selection.
pub(super) fn restore_checked_calls(
    positions: &Positions,
    target: &mut HashMap<Span, RustCheckedCall>,
    span: Span,
    values: &mut std::slice::Iter<'_, String>,
) -> Result<(), NodeBindingError> {
    let value = read_rust_call(positions, values, 0)?;
    if target.insert(span, value).is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Project both native iteration calls at the exact iterable expression that selected them.
pub(super) fn project_iteration_hooks(
    positions: &Positions,
    values: &HashMap<Span, RustIterationHooks>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, value) in values {
        let mut row = vec![
            "artifact-rust".into(),
            positions.nodes.anchor(*span)?.path,
            "iteration_hooks".into(),
        ];
        rust_call_fields(positions, &value.iter, &mut row, 0)?;
        rust_call_fields(positions, &value.next, &mut row, 0)?;
        fields.push(row);
    }
    Ok(())
}

/// Restore both checked native iteration calls without repeating trait selection.
pub(super) fn restore_iteration_hooks(
    positions: &Positions,
    target: &mut HashMap<Span, RustIterationHooks>,
    span: Span,
    values: &mut std::slice::Iter<'_, String>,
) -> Result<(), NodeBindingError> {
    let hooks = RustIterationHooks {
        iter: read_rust_call(positions, values, 0)?,
        next: read_rust_call(positions, values, 0)?,
    };
    if target.insert(span, hooks).is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

/// Encode a recursively adjusted Rust call using bounded structural values.
pub(super) fn rust_call_fields(
    positions: &Positions,
    call: &RustCheckedCall,
    fields: &mut Vec<String>,
    depth: usize,
) -> Result<(), NodeBindingError> {
    if depth > 16 {
        return Err(NodeBindingError::Unsupported("Rust call adjustment nesting"));
    }
    match &call.storage {
        Some(storage) => {
            fields.push("some".into());
            signature_fields(positions, storage, fields)?;
        }
        None => fields.push("none".into()),
    }
    string_list_fields(&call.source_type_parameters, fields);
    fields.push(call.signature_loans.len().to_string());
    for loans in &call.signature_loans {
        string_list_fields(loans, fields);
    }
    fields.push(call.receiver_adjustments.len().to_string());
    for adjustment in &call.receiver_adjustments {
        rust_call_fields(positions, adjustment, fields, depth + 1)?;
    }
    fields.extend([call.path.clone(), call.type_arguments.len().to_string()]);
    for ty in &call.type_arguments {
        type_fields(ty, fields)?;
    }
    parameter_fields(&call.parameters, fields)?;
    type_fields(&call.return_type, fields)?;
    match &call.result_item_path {
        Some(path) => fields.extend(["some".into(), path.clone()]),
        None => fields.push("none".into()),
    }
    fields.push(call.has_receiver.to_string());
    Ok(())
}

fn signature_fields(
    positions: &Positions,
    signature: &RustCheckedSignature,
    fields: &mut Vec<String>,
) -> Result<(), NodeBindingError> {
    fields.push(signature.parameters.len().to_string());
    for ty in &signature.parameters {
        native_types::native_type_fields(positions, ty, fields)?;
    }
    native_types::native_type_fields(positions, &signature.return_type, fields)?;
    fields.push(signature.type_arguments.len().to_string());
    for ty in &signature.type_arguments {
        native_types::native_type_fields(positions, ty, fields)?;
    }
    fields.push(signature.identities.len().to_string());
    for (name, identity) in &signature.identities {
        fields.extend([name.clone(), positions.type_identity(identity)?]);
    }
    Ok(())
}

fn string_list_fields(values: &[String], fields: &mut Vec<String>) {
    fields.push(values.len().to_string());
    fields.extend(values.iter().cloned());
}

/// Decode one recursively adjusted Rust call.
pub(super) fn read_rust_call(
    positions: &Positions,
    values: &mut std::slice::Iter<'_, String>,
    depth: usize,
) -> Result<RustCheckedCall, NodeBindingError> {
    if depth > 16 {
        return Err(NodeBindingError::Unsupported("Rust call adjustment nesting"));
    }
    let storage = match next(values)? {
        "some" => Some(read_signature(positions, values)?),
        "none" => None,
        _ => return Err(NodeBindingError::MissingNode),
    };
    let source_type_parameters = read_string_list(values)?;
    let loan_count = read_number(values)?;
    let signature_loans = (0..loan_count)
        .map(|_| read_string_list(values))
        .collect::<Result<Vec<_>, _>>()?;
    let adjustment_count = read_number(values)?;
    let receiver_adjustments = (0..adjustment_count)
        .map(|_| read_rust_call(positions, values, depth + 1))
        .collect::<Result<Vec<_>, _>>()?;
    let path = next(values)?.to_owned();
    if path.is_empty() {
        return Err(NodeBindingError::MissingNode);
    }
    let type_count = read_number(values)?;
    let type_arguments = (0..type_count)
        .map(|_| read_type(values))
        .collect::<Result<Vec<_>, _>>()?;
    let parameters = read_parameters(values)?;
    let return_type = read_type(values)?;
    let result_item_path = match next(values)? {
        "some" => Some(next(values)?.to_owned()),
        "none" => None,
        _ => return Err(NodeBindingError::MissingNode),
    };
    Ok(RustCheckedCall {
        storage,
        source_type_parameters,
        signature_loans,
        receiver_adjustments,
        path,
        type_arguments,
        parameters,
        return_type,
        result_item_path,
        has_receiver: read_bool(values)?,
    })
}

fn read_signature(
    positions: &Positions,
    values: &mut std::slice::Iter<'_, String>,
) -> Result<RustCheckedSignature, NodeBindingError> {
    let parameter_count = read_number(values)?;
    let parameters = (0..parameter_count)
        .map(|_| native_types::read_native_type(positions, values))
        .collect::<Result<Vec<_>, _>>()?;
    let return_type = native_types::read_native_type(positions, values)?;
    let argument_count = read_number(values)?;
    let type_arguments = (0..argument_count)
        .map(|_| native_types::read_native_type(positions, values))
        .collect::<Result<Vec<_>, _>>()?;
    let identity_count = read_number(values)?;
    let mut identities = BTreeMap::new();
    for _ in 0..identity_count {
        let name = next(values)?.to_owned();
        if name.is_empty()
            || identities
                .insert(name, positions.canonical_type(next(values)?)?)
                .is_some()
        {
            return Err(NodeBindingError::MissingNode);
        }
    }
    Ok(RustCheckedSignature {
        parameters,
        return_type,
        type_arguments,
        identities,
    })
}

fn read_string_list(values: &mut std::slice::Iter<'_, String>) -> Result<Vec<String>, NodeBindingError> {
    let count = read_number(values)?;
    if count > values.len() {
        return Err(NodeBindingError::MissingNode);
    }
    (0..count).map(|_| Ok(next(values)?.to_owned())).collect()
}

fn read_bool(values: &mut std::slice::Iter<'_, String>) -> Result<bool, NodeBindingError> {
    match next(values)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(NodeBindingError::MissingNode),
    }
}
