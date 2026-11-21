//! Handwritten semantic admission for structural artifact leaves that are not blind container serialization.

use super::*;
use std::collections::{BTreeSet, HashMap};

use crate::symbols::{ImplementationTraitBoundInfo, ImplementationTraitBoundOriginInfo, ImplementationTypeParamInfo};
use crate::typechecker::type_info::{
    BodyIrReexportCallInfo, ConstructorFieldBinding, ErrorMessageDisplay, FixedUnpackPlan, IsInstanceTargetInfo,
    ResolvedMethodCall, ResolvedMethodDispatch, ResolvedOperatorKind,
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

/// Encode one checker-selected trait dispatch without source-spelling inference.
pub(super) fn project_method_dispatch(
    value: &ResolvedMethodDispatch,
    row: &mut Vec<String>,
) -> Result<(), NodeBindingError> {
    let ResolvedMethodDispatch::Trait {
        trait_name,
        module_path,
        type_args,
        implementation_type_params,
        receiver_is_mutable,
    } = value;
    row.extend(["trait".into(), trait_name.clone()]);
    match module_path {
        Some(path) => {
            row.extend(["some".into(), path.len().to_string()]);
            row.extend(path.iter().cloned());
        }
        None => row.push("none".into()),
    }
    row.push(type_args.len().to_string());
    for ty in type_args {
        type_fields(ty, row)?;
    }
    project_implementation_params(implementation_type_params, row)?;
    row.push(receiver_is_mutable.to_string());
    Ok(())
}

/// Decode one structurally complete trait dispatch.
pub(super) fn read_method_dispatch<'a>(
    values: &mut std::slice::Iter<'a, String>,
) -> Result<ResolvedMethodDispatch, NodeBindingError> {
    if next(values)? != "trait" {
        return Err(NodeBindingError::MissingNode);
    }
    let trait_name = next(values)?.to_owned();
    let module_path = match next(values)? {
        "some" => {
            let count = read_number(values)?;
            Some(
                (0..count)
                    .map(|_| Ok(next(values)?.to_owned()))
                    .collect::<Result<Vec<_>, NodeBindingError>>()?,
            )
        }
        "none" => None,
        _ => return Err(NodeBindingError::MissingNode),
    };
    let type_count = read_number(values)?;
    let type_args = (0..type_count)
        .map(|_| read_type(values))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResolvedMethodDispatch::Trait {
        trait_name,
        module_path,
        type_args,
        implementation_type_params: read_implementation_params(values)?,
        receiver_is_mutable: read_bool(values)?,
    })
}

/// Encode implementation-only generic bounds carried by dispatch metadata.
fn project_implementation_params(
    params: &[ImplementationTypeParamInfo],
    row: &mut Vec<String>,
) -> Result<(), NodeBindingError> {
    row.push(params.len().to_string());
    for param in params {
        row.extend([param.name.clone(), param.bounds.len().to_string()]);
        for bound in &param.bounds {
            row.extend([bound.trait_path.clone(), bound.type_args.len().to_string()]);
            for ty in &bound.type_args {
                type_fields(ty, row)?;
            }
            row.push(bound.associated_types.len().to_string());
            for (name, ty) in &bound.associated_types {
                row.push(name.clone());
                type_fields(ty, row)?;
            }
            row.push(
                match bound.origin {
                    ImplementationTraitBoundOriginInfo::Standard => "standard",
                    ImplementationTraitBoundOriginInfo::RustCapability => "rust-capability",
                    ImplementationTraitBoundOriginInfo::SourceCallable => "source-callable",
                }
                .into(),
            );
        }
    }
    Ok(())
}

/// Decode implementation-only generic bounds carried by dispatch metadata.
fn read_implementation_params<'a>(
    values: &mut std::slice::Iter<'a, String>,
) -> Result<Vec<ImplementationTypeParamInfo>, NodeBindingError> {
    let count = read_number(values)?;
    (0..count)
        .map(|_| {
            let name = next(values)?.to_owned();
            let bound_count = read_number(values)?;
            let bounds = (0..bound_count)
                .map(|_| read_implementation_bound(values))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ImplementationTypeParamInfo { name, bounds })
        })
        .collect()
}

/// Decode one exact implementation trait requirement.
fn read_implementation_bound<'a>(
    values: &mut std::slice::Iter<'a, String>,
) -> Result<ImplementationTraitBoundInfo, NodeBindingError> {
    let trait_path = next(values)?.to_owned();
    let type_count = read_number(values)?;
    let type_args = (0..type_count)
        .map(|_| read_type(values))
        .collect::<Result<Vec<_>, _>>()?;
    let associated_count = read_number(values)?;
    let associated_types = (0..associated_count)
        .map(|_| Ok((next(values)?.to_owned(), read_type(values)?)))
        .collect::<Result<Vec<_>, NodeBindingError>>()?;
    let origin = match next(values)? {
        "standard" => ImplementationTraitBoundOriginInfo::Standard,
        "rust-capability" => ImplementationTraitBoundOriginInfo::RustCapability,
        "source-callable" => ImplementationTraitBoundOriginInfo::SourceCallable,
        _ => return Err(NodeBindingError::MissingNode),
    };
    Ok(ImplementationTraitBoundInfo {
        trait_path,
        type_args,
        associated_types,
        origin,
    })
}

/// Decode a canonical lowercase boolean.
fn read_bool(values: &mut std::slice::Iter<'_, String>) -> Result<bool, NodeBindingError> {
    match next(values)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(NodeBindingError::MissingNode),
    }
}

/// Project all resolved method calls through their selected trait dispatch.
pub(super) fn project_resolved_method_calls(
    positions: &Positions,
    values: &HashMap<Span, ResolvedMethodCall>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    for (span, value) in values {
        let mut row = vec![
            "artifact-call".into(),
            positions.nodes.anchor(*span)?.path,
            "resolved_method_calls".into(),
            value.method.clone(),
        ];
        project_method_dispatch(&value.dispatch, &mut row)?;
        fields.push(row);
    }
    Ok(())
}

/// Restore one resolved method call from its exact structural dispatch.
pub(super) fn restore_resolved_method_calls<'a>(
    _positions: &Positions,
    target: &mut HashMap<Span, ResolvedMethodCall>,
    span: Span,
    values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    let value = ResolvedMethodCall {
        method: next(values)?.to_owned(),
        dispatch: read_method_dispatch(values)?,
        lookup: None,
    };
    if target.insert(span, value).is_some() {
        return Err(NodeBindingError::MissingNode);
    }
    Ok(())
}

pub(super) fn project_compiler_generated_member_identities(
    positions: &Positions,
    values: &HashMap<CanonicalSymbolId, CanonicalSymbolId>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    if values.is_empty() {
        return Ok(());
    }
    let mut entries = values
        .iter()
        .map(|(member, owner)| {
            Ok((
                stable_member_identity(member, owner, 0)?,
                positions.type_identity(owner)?,
                member.kind.as_str().to_owned(),
                member.declaration_name.clone(),
            ))
        })
        .collect::<Result<Vec<_>, NodeBindingError>>()?;
    entries.sort();
    let mut row = vec![
        "artifact-call".into(),
        positions.owner_path()?,
        "compiler_generated_member_identities".into(),
        entries.len().to_string(),
    ];
    for (identity, owner, kind, name) in entries {
        row.extend([identity, owner, kind, name]);
    }
    fields.push(row);
    Ok(())
}

pub(super) fn restore_compiler_generated_member_identities<'a>(
    positions: &Positions,
    target: &mut HashMap<CanonicalSymbolId, CanonicalSymbolId>,
    span: Span,
    values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    if span != positions.owner {
        return Err(NodeBindingError::WrongOwner);
    }
    let count = read_number(values)?;
    let mut previous: Option<String> = None;
    for _ in 0..count {
        let identity = next(values)?.to_owned();
        if previous.as_ref().is_some_and(|value| value >= &identity) {
            return Err(NodeBindingError::MissingNode);
        }
        previous = Some(identity.clone());
        let owner = positions.canonical_type(next(values)?)?;
        let kind_text = next(values)?;
        let kind = SemanticSourceTargetKind::from_kind_str(kind_text);
        if kind.as_str() != kind_text
            || !matches!(
                kind,
                SemanticSourceTargetKind::Field
                    | SemanticSourceTargetKind::Method
                    | SemanticSourceTargetKind::Property
                    | SemanticSourceTargetKind::Variant
            )
        {
            return Err(NodeBindingError::MissingNode);
        }
        let name = next(values)?;
        let member = owner.generated_member(name, kind);
        if stable_member_identity(&member, &owner, 0)? != identity || target.insert(member, owner).is_some() {
            return Err(NodeBindingError::MissingNode);
        }
    }
    Ok(())
}

pub(super) fn project_error_message_display_types(
    positions: &Positions,
    values: &HashMap<String, ErrorMessageDisplay>,
    fields: &mut Fields,
) -> Result<(), NodeBindingError> {
    if values.is_empty() {
        return Ok(());
    }
    let mut entries = values.iter().collect::<Vec<_>>();
    entries.sort_by_key(|(name, _)| *name);
    let mut row = vec![
        "artifact-call".into(),
        positions.owner_path()?,
        "error_message_display_types".into(),
        entries.len().to_string(),
    ];
    for (name, display) in entries {
        row.push(name.clone());
        match &display.identity {
            Some(identity) => row.extend(["some".into(), positions.identity(identity)?]),
            None => row.push("none".into()),
        }
        match &display.dispatch {
            Some(dispatch) => {
                row.push("some".into());
                project_method_dispatch(dispatch, &mut row)?;
            }
            None => row.push("none".into()),
        }
        row.extend([
            display.receiver_is_trait_self.to_string(),
            display.self_displaying_adopters.len().to_string(),
        ]);
        row.extend(display.self_displaying_adopters.iter().cloned());
    }
    fields.push(row);
    Ok(())
}

pub(super) fn restore_error_message_display_types<'a>(
    positions: &Positions,
    target: &mut HashMap<String, ErrorMessageDisplay>,
    span: Span,
    values: &mut std::slice::Iter<'a, String>,
) -> Result<(), NodeBindingError> {
    if span != positions.owner {
        return Err(NodeBindingError::WrongOwner);
    }
    let count = read_number(values)?;
    let mut previous: Option<String> = None;
    for _ in 0..count {
        let name = next(values)?.to_owned();
        if previous.as_ref().is_some_and(|value| value >= &name) {
            return Err(NodeBindingError::MissingNode);
        }
        previous = Some(name.clone());
        let identity = match next(values)? {
            "some" => Some(positions.canonical(next(values)?)?),
            "none" => None,
            _ => return Err(NodeBindingError::MissingNode),
        };
        let dispatch = match next(values)? {
            "some" => Some(read_method_dispatch(values)?),
            "none" => None,
            _ => return Err(NodeBindingError::MissingNode),
        };
        let receiver_is_trait_self = read_bool(values)?;
        let adopter_count = read_number(values)?;
        let self_displaying_adopters = (0..adopter_count)
            .map(|_| Ok(next(values)?.to_owned()))
            .collect::<Result<BTreeSet<_>, NodeBindingError>>()?;
        if self_displaying_adopters.len() != adopter_count
            || target
                .insert(
                    name,
                    ErrorMessageDisplay {
                        identity,
                        dispatch,
                        receiver_is_trait_self,
                        self_displaying_adopters,
                    },
                )
                .is_some()
        {
            return Err(NodeBindingError::MissingNode);
        }
    }
    Ok(())
}
