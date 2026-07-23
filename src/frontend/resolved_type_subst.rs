//! Substitute [`ResolvedType`] and [`MethodInfo`] under a type-parameter map.
//!
//! Used by the typechecker (supertrait closure, conformance) and by IR lowering (trait impl expansion) so both stages
//! agree on how generic trait parameters are threaded through the hierarchy (RFC 042).

use std::collections::HashMap;

use crate::frontend::symbols::{CallableParam, MethodInfo, PropertyInfo, ResolvedType, TypeBoundInfo};

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
        type_param_bound_details: info
            .type_param_bound_details
            .iter()
            .map(|(type_param, bounds)| {
                (
                    type_param.clone(),
                    bounds
                        .iter()
                        .map(|bound| TypeBoundInfo {
                            name: bound.name.clone(),
                            source_name: bound.source_name.clone(),
                            type_args: bound
                                .type_args
                                .iter()
                                .map(|ty| substitute_resolved_type(ty, map))
                                .collect(),
                            module_path: bound.module_path.clone(),
                        })
                        .collect(),
                )
            })
            .collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_substitution_rewrites_generic_bound_arguments() {
        let mut type_param_bound_details = HashMap::new();
        type_param_bound_details.insert(
            "Mapper".to_string(),
            vec![TypeBoundInfo {
                name: "Callable1".to_string(),
                source_name: Some("Callable1".to_string()),
                type_args: vec![
                    ResolvedType::TypeVar("E".to_string()),
                    ResolvedType::TypeVar("F".to_string()),
                ],
                module_path: Some(vec!["std".to_string(), "traits".to_string(), "callable".to_string()]),
            }],
        );
        let method = MethodInfo {
            type_params: vec!["F".to_string(), "Mapper".to_string()],
            type_param_bounds: HashMap::new(),
            type_param_bound_details,
            trait_target: None,
            receiver: None,
            params: Vec::new(),
            return_type: ResolvedType::TypeVar("F".to_string()),
            is_async: false,
            has_body: true,
            alias_of: None,
        };
        let substitutions = HashMap::from([("E".to_string(), ResolvedType::Named("IoError".to_string()))]);

        let substituted = substitute_method_info(&method, &substitutions);
        assert_eq!(
            substituted.type_param_bound_details["Mapper"][0].type_args,
            vec![
                ResolvedType::Named("IoError".to_string()),
                ResolvedType::TypeVar("F".to_string())
            ]
        );
    }
}
