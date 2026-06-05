//! Substitute [`ResolvedType`] and [`MethodInfo`] under a type-parameter map.
//!
//! Used by the typechecker (supertrait closure, conformance) and by IR lowering (trait impl expansion) so both stages
//! agree on how generic trait parameters are threaded through the hierarchy (RFC 042).

use std::collections::HashMap;

use crate::frontend::symbols::{CallableParam, MethodInfo, PropertyInfo, ResolvedType};

/// Build a substitution map from declared type parameter names to concrete (or still-generic) arguments.
///
/// `params` and `args` must have the same length; callers typically enforce arity before calling.
pub(crate) fn type_param_subst_map(params: &[String], args: &[ResolvedType]) -> HashMap<String, ResolvedType> {
    params
        .iter()
        .zip(args.iter())
        .map(|(p, a)| (p.clone(), a.clone()))
        .collect()
}

/// Like [`type_param_subst_map`], but omits entries where the explicit argument is [`ResolvedType::CallSiteInfer`]
/// (`_` at the call site, RFC 054) so those parameters stay as [`ResolvedType::TypeVar`] until inference fills them.
pub(crate) fn type_param_subst_map_call_site(
    params: &[String],
    args: &[ResolvedType],
) -> HashMap<String, ResolvedType> {
    params
        .iter()
        .zip(args.iter())
        .filter_map(|(p, a)| {
            if matches!(a, ResolvedType::CallSiteInfer) {
                None
            } else {
                Some((p.clone(), a.clone()))
            }
        })
        .collect()
}

/// Apply `map` throughout `ty`, replacing [`ResolvedType::TypeVar`] leaves when a binding exists.
pub(crate) fn substitute_resolved_type(ty: &ResolvedType, map: &HashMap<String, ResolvedType>) -> ResolvedType {
    match ty {
        ResolvedType::TypeVar(name) => map.get(name).cloned().unwrap_or_else(|| ty.clone()),
        ResolvedType::Generic(name, args) => ResolvedType::Generic(
            name.clone(),
            args.iter().map(|a| substitute_resolved_type(a, map)).collect(),
        ),
        ResolvedType::Function(params, ret) => ResolvedType::Function(
            params
                .iter()
                .map(|p| CallableParam {
                    name: p.name.clone(),
                    ty: substitute_resolved_type(&p.ty, map),
                    kind: p.kind,
                    has_default: p.has_default,
                })
                .collect(),
            Box::new(substitute_resolved_type(ret, map)),
        ),
        ResolvedType::TypeToken(inner) => ResolvedType::TypeToken(Box::new(substitute_resolved_type(inner, map))),
        ResolvedType::Tuple(elems) => {
            ResolvedType::Tuple(elems.iter().map(|e| substitute_resolved_type(e, map)).collect())
        }
        ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(substitute_resolved_type(inner, map))),
        ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
            Box::new(substitute_resolved_type(key, map)),
            Box::new(substitute_resolved_type(value, map)),
        ),
        ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(substitute_resolved_type(inner, map))),
        ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(substitute_resolved_type(inner, map))),
        ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(substitute_resolved_type(inner, map))),
        ResolvedType::CallSiteInfer => ty.clone(),
        _ => ty.clone(),
    }
}

/// Substitute a computed property return type using `map`.
pub(crate) fn substitute_property_info(info: &PropertyInfo, map: &HashMap<String, ResolvedType>) -> PropertyInfo {
    PropertyInfo {
        return_type: substitute_resolved_type(&info.return_type, map),
        visibility: info.visibility,
        owner: info.owner.clone(),
        has_body: info.has_body,
    }
}

/// Substitute every parameter and return type in a [`MethodInfo`] using `map`.
pub(crate) fn substitute_method_info(info: &MethodInfo, map: &HashMap<String, ResolvedType>) -> MethodInfo {
    MethodInfo {
        type_params: info.type_params.clone(),
        type_param_bounds: info.type_param_bounds.clone(),
        type_param_bound_details: info.type_param_bound_details.clone(),
        trait_target: info.trait_target.clone(),
        receiver: info.receiver,
        params: info
            .params
            .iter()
            .map(|p| CallableParam {
                name: p.name.clone(),
                ty: substitute_resolved_type(&p.ty, map),
                kind: p.kind,
                has_default: p.has_default,
            })
            .collect(),
        return_type: substitute_resolved_type(&info.return_type, map),
        is_async: info.is_async,
        has_body: info.has_body,
        alias_of: info.alias_of.clone(),
    }
}
