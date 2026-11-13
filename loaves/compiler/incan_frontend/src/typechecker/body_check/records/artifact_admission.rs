//! Handwritten semantic admission for structural artifact leaves that are not blind container serialization.

use super::*;
use std::collections::HashMap;

use crate::typechecker::type_info::{
    BodyIrReexportCallInfo, ConstructorFieldBinding, FixedUnpackPlan, IsInstanceTargetInfo, ResolvedOperatorKind,
};

pub(super) fn operator_kind(kind: ResolvedOperatorKind) -> &'static str {
    match kind {
        ResolvedOperatorKind::Binary => "binary",
        ResolvedOperatorKind::Unary => "unary",
        ResolvedOperatorKind::Index => "index",
        ResolvedOperatorKind::Slice => "slice",
        ResolvedOperatorKind::IndexAssign => "index-assign",
        ResolvedOperatorKind::Truthiness => "truthiness",
        ResolvedOperatorKind::Len => "len",
        ResolvedOperatorKind::Contains => "contains",
        ResolvedOperatorKind::Call => "call",
    }
}

pub(super) fn read_operator_kind(value: &str) -> Result<ResolvedOperatorKind, NodeBindingError> {
    Ok(match value {
        "binary" => ResolvedOperatorKind::Binary,
        "unary" => ResolvedOperatorKind::Unary,
        "index" => ResolvedOperatorKind::Index,
        "slice" => ResolvedOperatorKind::Slice,
        "index-assign" => ResolvedOperatorKind::IndexAssign,
        "truthiness" => ResolvedOperatorKind::Truthiness,
        "len" => ResolvedOperatorKind::Len,
        "contains" => ResolvedOperatorKind::Contains,
        "call" => ResolvedOperatorKind::Call,
        _ => return Err(NodeBindingError::MissingNode),
    })
}

pub(super) fn project_fixed_unpack(value: &FixedUnpackPlan, row: &mut Vec<String>) -> Result<(), NodeBindingError> {
    match value {
        FixedUnpackPlan::Positional(types) => {
            row.extend(["positional".into(), types.len().to_string()]);
            for ty in types {
                type_fields(ty, row)?;
            }
        }
        FixedUnpackPlan::Keyword(names) => {
            row.extend(["keyword".into(), names.len().to_string()]);
            row.extend(names.iter().cloned());
        }
    }
    Ok(())
}

pub(super) fn read_fixed_unpack<'a>(
    values: &mut std::slice::Iter<'a, String>,
) -> Result<FixedUnpackPlan, NodeBindingError> {
    let kind = next(values)?;
    let count = read_number(values)?;
    if count > values.len() {
        return Err(NodeBindingError::MissingNode);
    }
    match kind {
        "positional" => Ok(FixedUnpackPlan::Positional(
            (0..count).map(|_| read_type(values)).collect::<Result<Vec<_>, _>>()?,
        )),
        "keyword" => Ok(FixedUnpackPlan::Keyword(
            (0..count)
                .map(|_| Ok(next(values)?.to_owned()))
                .collect::<Result<Vec<_>, NodeBindingError>>()?,
        )),
        _ => Err(NodeBindingError::MissingNode),
    }
}

pub(super) fn project_body_ir_reexport_calls(
    positions: &Positions,
    values: &HashMap<Span, BodyIrReexportCallInfo>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, value) in values {
        let mut row = vec![
            "artifact-call".into(),
            positions.nodes.anchor(*span)?.path,
            "body_ir_reexport_calls".into(),
            positions.identity(&value.canonical)?,
        ];
        parameter_fields(&value.parameters, &mut row)?;
        fields.push(row);
    }
    Ok(())
}

pub(super) fn restore_body_ir_reexport_calls<'a>(
    positions: &Positions,
    target: &mut HashMap<Span, BodyIrReexportCallInfo>,
    span: Span,
    values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    let value = BodyIrReexportCallInfo {
        canonical: positions.canonical(next(values)?)?,
        parameters: read_parameters(values)?,
    };
    if target.insert(span, value).is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

fn project_constructor_bindings(
    positions: &Positions,
    values: &HashMap<Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
    selector: &str,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, (owner, binding)) in values {
        let mut row = vec![
            "artifact-call".into(),
            positions.nodes.anchor(*span)?.path,
            selector.into(),
            positions.type_identity(owner)?,
        ];
        project_constructor_binding(binding, &mut row);
        fields.push(row);
    }
    Ok(())
}

fn project_constructor_binding(value: &ConstructorFieldBinding, row: &mut Vec<String>) {
    row.extend([value.field_count.to_string(), value.argument_slots.len().to_string()]);
    row.extend(value.argument_slots.iter().map(usize::to_string));
    row.push(value.defaulted_slots.len().to_string());
    row.extend(value.defaulted_slots.iter().map(usize::to_string));
}

fn read_constructor_binding<'a>(
    values: &mut std::slice::Iter<'a, String>,
) -> Result<ConstructorFieldBinding, NodeBindingError> {
    let field_count = read_number(values)?;
    let argument_count = read_number(values)?;
    let argument_slots = (0..argument_count)
        .map(|_| read_number(values))
        .collect::<Result<Vec<_>, _>>()?;
    let default_count = read_number(values)?;
    let defaulted_slots = (0..default_count)
        .map(|_| read_number(values))
        .collect::<Result<Vec<_>, _>>()?;
    let mut occupied = argument_slots.clone();
    occupied.extend(defaulted_slots.iter().copied());
    occupied.sort_unstable();
    if occupied != (0..field_count).collect::<Vec<_>>() || !defaulted_slots.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(ConstructorFieldBinding {
        argument_slots,
        defaulted_slots,
        field_count,
    })
}

macro_rules! constructor_binding_codec {
    ($project:ident, $restore:ident, $selector:literal) => {
        pub(super) fn $project(
            positions: &Positions,
            values: &HashMap<Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
            fields: &mut Fields,
        ) -> Result<(), NodeBindingError> {
            project_constructor_bindings(positions, values, $selector, fields)
        }

        pub(super) fn $restore<'a>(
            positions: &Positions,
            target: &mut HashMap<Span, (CanonicalSymbolId, ConstructorFieldBinding)>,
            span: Span,
            values: &mut std::slice::Iter<'a, String>,
        ) -> Result<(), NodeBindingError> {
            let owner = positions.canonical_type(next(values)?)?;
            let binding = read_constructor_binding(values)?;
            if target.insert(span, (owner, binding)).is_some() {
                return Err(NodeBindingError::MissingNode);
            }
            Ok(())
        }
    };
}

constructor_binding_codec!(
    project_body_ir_native_constructor_bindings,
    restore_body_ir_native_constructor_bindings,
    "body_ir_native_constructor_bindings"
);
constructor_binding_codec!(
    project_body_ir_module_constructor_bindings,
    restore_body_ir_module_constructor_bindings,
    "body_ir_module_constructor_bindings"
);

pub(super) fn project_isinstance_targets(
    positions: &Positions,
    values: &HashMap<Span, IsInstanceTargetInfo>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, value) in values {
        let mut row = vec![
            "artifact-call".into(),
            positions.nodes.anchor(*span)?.path,
            "isinstance_targets".into(),
            positions.nodes.anchor(value.span)?.path,
        ];
        match &value.canonical {
            Some(canonical) => row.extend(["some".into(), positions.type_identity(canonical)?]),
            None => row.push("none".into()),
        }
        type_fields(&value.ty, &mut row)?;
        fields.push(row);
    }
    Ok(())
}

pub(super) fn restore_isinstance_targets<'a>(
    positions: &Positions,
    target: &mut HashMap<Span, IsInstanceTargetInfo>,
    span: Span,
    values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    let target_span = positions.span(next(values)?)?;
    let canonical = match next(values)? {
        "some" => Some(positions.canonical_type(next(values)?)?),
        "none" => None,
        _ => return Err(NodeBindingError::MissingNode),
    };
    let value = IsInstanceTargetInfo {
        ty: read_type(values)?,
        canonical,
        span: target_span,
    };
    if target.insert(span, value).is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

macro_rules! refused_manual_codec {
    ($project:ident, $restore:ident, $ty:ty, $message:literal) => {
        pub(super) fn $project(
            _positions: &Positions,
            values: &HashMap<Span, $ty>,
            _fields: &mut Fields,
        ) -> Result<(), NodeBindingError> {
            if values.is_empty() {
                Ok(())
            } else {
                Err(NodeBindingError::Unsupported($message))
            }
        }

        pub(super) fn $restore<'a>(
            _positions: &Positions,
            _target: &mut HashMap<Span, $ty>,
            _span: Span,
            _values: &mut std::slice::Iter<'a, String>,
        ) -> Result<(), NodeBindingError> {
            Err(NodeBindingError::Unsupported($message))
        }
    };
}

refused_manual_codec!(
    project_resolved_method_calls,
    restore_resolved_method_calls,
    crate::typechecker::type_info::ResolvedMethodCall,
    "CallArtifacts.resolved_method_calls"
);

pub(super) fn project_compiler_generated_member_identities(
    _positions: &Positions,
    values: &HashMap<CanonicalSymbolId, CanonicalSymbolId>,
    _fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    if values.is_empty() {
        Ok(())
    } else {
        Err(NodeBindingError::Unsupported(
            "CallArtifacts.compiler_generated_member_identities",
        ))
    }
}

pub(super) fn restore_compiler_generated_member_identities<'a>(
    _positions: &Positions,
    _target: &mut HashMap<CanonicalSymbolId, CanonicalSymbolId>,
    _span: Span,
    _values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    Err(NodeBindingError::Unsupported(
        "CallArtifacts.compiler_generated_member_identities",
    ))
}

pub(super) fn project_error_message_display_types(
    _positions: &Positions,
    values: &HashMap<String, crate::typechecker::type_info::ErrorMessageDisplay>,
    _fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    if values.is_empty() {
        Ok(())
    } else {
        Err(NodeBindingError::Unsupported(
            "CallArtifacts.error_message_display_types",
        ))
    }
}

pub(super) fn restore_error_message_display_types<'a>(
    _positions: &Positions,
    _target: &mut HashMap<String, crate::typechecker::type_info::ErrorMessageDisplay>,
    _span: Span,
    _values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    Err(NodeBindingError::Unsupported(
        "CallArtifacts.error_message_display_types",
    ))
}
