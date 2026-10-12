//! Retain complete checked generic call arguments for Body IR without changing legacy inference records.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, Span};
use crate::symbols::{FunctionInfo, ResolvedType};
use incan_lang::interop::is_rust_capability_bound;

impl TypeChecker {
    /// Record a concrete native instantiation from already checked parameter/argument relationships.
    ///
    /// Legacy preserves its own bindings and Rust inference. Body IR additionally normalizes the checked tuple
    /// spellings and declaration-bound named placeholders, then uses the same unifier. Unchecked spreads, conflicting
    /// bindings and open arguments publish no fact. This runs only after the ordinary call checks have succeeded.
    pub(super) fn retain_body_ir_call_arguments(
        &mut self,
        info: &FunctionInfo,
        args: &[CallArg],
        legacy_bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let names = crate::body_ir_parameter_names::parameter_names(info.type_params.iter().map(|name| {
            let bounds = info.type_param_bound_details.get(name);
            (
                name.as_str(),
                bounds.is_some_and(|bounds| !bounds.is_empty()),
                bounds.is_some_and(|bounds| bounds.iter().any(|bound| is_rust_capability_bound(&bound.name))),
            )
        }));
        if names.is_empty()
            || args
                .iter()
                .any(|argument| matches!(argument, CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_)))
        {
            return;
        }
        let mut bindings: HashMap<_, _> = legacy_bindings
            .iter()
            .filter(|(_, ty)| concrete(&normalized(ty, &names)))
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();
        for (expression, parameter) in Self::arguments_with_parameters(&info.params, args) {
            let Some(parameter) = parameter else {
                return;
            };
            let key = (expression.span.start, expression.span.end);
            let Some(actual) = self
                .type_info
                .calls
                .body_ir_constructor_types
                .get(&key)
                .or_else(|| self.type_info.calls.inferred_constructor_types.get(&key))
                .or_else(|| self.type_info.expr_type(expression.span))
            else {
                return;
            };
            let expected = normalized(&parameter.ty, &names);
            let actual = normalized(actual, &[]);
            self.infer_type_param_bindings(&expected, &actual, &mut bindings);
        }
        let arguments: Option<Vec<_>> = names
            .iter()
            .map(|name| bindings.get(name).filter(|ty| concrete(ty)).cloned())
            .collect();
        if let Some(arguments) = arguments {
            self.type_info
                .calls
                .body_ir_call_type_arguments
                .insert((span.start, span.end), arguments);
        }
    }
}

/// Normalize checked tuple aliases and only the callee's declared named binders for structural unification.
fn normalized(ty: &ResolvedType, binders: &[String]) -> ResolvedType {
    match ty {
        ResolvedType::Named(name) if binders.contains(name) => ResolvedType::TypeVar(name.clone()),
        ResolvedType::Generic(name, elements) => {
            let elements = elements.iter().map(|element| normalized(element, binders)).collect();
            if name == "Tuple" {
                ResolvedType::Tuple(elements)
            } else {
                ResolvedType::Generic(name.clone(), elements)
            }
        }
        ResolvedType::Tuple(elements) => {
            ResolvedType::Tuple(elements.iter().map(|element| normalized(element, binders)).collect())
        }
        ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(normalized(inner, binders))),
        ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(normalized(inner, binders))),
        ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(normalized(inner, binders))),
        ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(normalized(inner, binders))),
        ResolvedType::FrozenDict(key, value) => {
            ResolvedType::FrozenDict(Box::new(normalized(key, binders)), Box::new(normalized(value, binders)))
        }
        ty => ty.clone(),
    }
}

/// A native fallback requires every structural leaf to be fixed; unresolved or caller-open types stay on the old path.
fn concrete(ty: &ResolvedType) -> bool {
    match ty {
        ResolvedType::Unknown | ResolvedType::CallSiteInfer | ResolvedType::TypeVar(_) => false,
        ResolvedType::Generic(_, elements) | ResolvedType::Tuple(elements) => elements.iter().all(concrete),
        ResolvedType::Ref(inner)
        | ResolvedType::RefMut(inner)
        | ResolvedType::FrozenList(inner)
        | ResolvedType::FrozenSet(inner)
        | ResolvedType::TypeToken(inner) => concrete(inner),
        ResolvedType::FrozenDict(key, value) => concrete(key) && concrete(value),
        ResolvedType::Function(parameters, result) => {
            parameters.iter().all(|parameter| concrete(&parameter.ty)) && concrete(result)
        }
        _ => true,
    }
}
