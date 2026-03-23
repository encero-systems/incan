//! Substitute [`ResolvedType`] and [`MethodInfo`] under a type-parameter map.
//!
//! Used by the typechecker (supertrait closure, conformance) and by IR lowering (trait impl expansion) so both stages
//! agree on how generic trait parameters are threaded through the hierarchy (RFC 042).

use std::collections::HashMap;

use crate::frontend::symbols::{MethodInfo, ResolvedType};

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

/// Apply `map` throughout `ty`, replacing [`ResolvedType::TypeVar`] leaves when a binding exists.
pub(crate) fn substitute_resolved_type(ty: &ResolvedType, map: &HashMap<String, ResolvedType>) -> ResolvedType {
    match ty {
        ResolvedType::TypeVar(name) => map.get(name).cloned().unwrap_or_else(|| ty.clone()),
        ResolvedType::Generic(name, args) => ResolvedType::Generic(
            name.clone(),
            args.iter().map(|a| substitute_resolved_type(a, map)).collect(),
        ),
        ResolvedType::Function(params, ret) => ResolvedType::Function(
            params.iter().map(|p| substitute_resolved_type(p, map)).collect(),
            Box::new(substitute_resolved_type(ret, map)),
        ),
        ResolvedType::Tuple(elems) => {
            ResolvedType::Tuple(elems.iter().map(|e| substitute_resolved_type(e, map)).collect())
        }
        ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(substitute_resolved_type(inner, map))),
        _ => ty.clone(),
    }
}

/// Substitute every parameter and return type in a [`MethodInfo`] using `map`.
pub(crate) fn substitute_method_info(info: &MethodInfo, map: &HashMap<String, ResolvedType>) -> MethodInfo {
    MethodInfo {
        receiver: info.receiver,
        params: info
            .params
            .iter()
            .map(|(n, t)| (n.clone(), substitute_resolved_type(t, map)))
            .collect(),
        return_type: substitute_resolved_type(&info.return_type, map),
        is_async: info.is_async,
        has_body: info.has_body,
    }
}
