//! Retain complete checked generic call arguments for Body IR without changing legacy inference records.

use std::collections::{HashMap, HashSet};

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Span, Spanned};
use crate::resolved_type_subst::substitute_resolved_type;
use crate::symbols::{FunctionInfo, ResolvedType, TypeBoundInfo};
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
            .filter(|(_, ty)| concrete(&normalized(ty, &names, &self.type_info)))
            .map(|(name, ty)| (name.clone(), normalized(ty, &names, &self.type_info)))
            .collect();
        let mut native_binders = HashSet::new();
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
            let expected = normalized(&parameter.ty, &names, &self.type_info);
            let actual = self.body_ir_argument_type(expression, actual);
            let actual = normalized(&actual, &[], &self.type_info);
            if let ResolvedType::TypeVar(name) = &expected {
                let first_actual = native_binders.insert(name.clone());
                if first_actual
                    && matches!(&actual, ResolvedType::Generic(base, outputs) if base == "Awaitable" && outputs.len() == 1)
                {
                    // Replace only a legacy eventual-output seed, preserving every preceding native argument's
                    // evidence.
                    bindings.remove(name);
                }
            }
            self.infer_type_param_bindings(&expected, &actual, &mut bindings);
        }
        self.retain_body_ir_future_outputs(info, &names, &mut bindings);
        let arguments: Option<Vec<_>> = names
            .iter()
            .map(|name| bindings.get(name).filter(|ty| concrete(ty)).cloned())
            .collect();
        if let Some(arguments) = arguments {
            self.type_info
                .calls
                .body_ir_call_type_arguments
                .insert((span.start, span.end), arguments);
            let result = substitute_resolved_type(&normalized(&info.return_type, &names, &self.type_info), &bindings);
            let future_bridge = info
                .type_param_bound_details
                .values()
                .flatten()
                .any(|bound| self.body_ir_runtime_future_bound(bound));
            let runtime_result = matches!(&info.return_type, ResolvedType::RustPath(path) if crate::body_ir::runtime_nominals::project_resolved_path(path, &self.type_info).is_some());
            if concrete(&result) && (future_bridge || runtime_result) {
                self.type_info
                    .calls
                    .inferred_call_result_types
                    .insert((span.start, span.end), result);
            }
        }
    }

    /// Retain a checked async call as a future argument while leaving legacy's eventual-output expression type alone.
    fn body_ir_argument_type(&self, expression: &Spanned<Expr>, actual: &ResolvedType) -> ResolvedType {
        if matches!(actual, ResolvedType::Generic(name, args) if name == "Awaitable" && args.len() == 1) {
            return actual.clone();
        }
        let callee_span = match &expression.node {
            Expr::Call(callee, _, _) => callee.span,
            Expr::MethodCall(_, _, _, _) => expression.span,
            Expr::Paren(inner) => return self.body_ir_argument_type(inner, actual),
            _ => return actual.clone(),
        };
        let source_async = self.type_info.resolved_identity(callee_span).is_some_and(|identity| {
            self.type_info
                .declarations
                .function_bindings
                .values()
                .any(|binding| binding.is_async && binding.identity.as_ref() == Some(identity))
        });
        if source_async
            || self
                .type_info
                .rust
                .async_call_realizations
                .contains(&(expression.span.start, expression.span.end))
        {
            ResolvedType::Generic("Awaitable".into(), vec![actual.clone()])
        } else {
            actual.clone()
        }
    }

    /// Close only the exact SDK RuntimeFuture bridge's output from already checked future carriers in a cloned binding
    /// set.
    fn retain_body_ir_future_outputs(
        &self,
        info: &FunctionInfo,
        names: &[String],
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        for name in names {
            let Some(actual) = bindings.get(name).cloned() else {
                continue;
            };
            let Some(bounds) = info.type_param_bound_details.get(name) else {
                continue;
            };
            for bound in bounds {
                if !self.body_ir_runtime_future_bound(bound) || bound.type_args.len() != 1 {
                    continue;
                }
                let Some(output) = self.await_output_type_from_type(&actual) else {
                    continue;
                };
                if !concrete(&output) {
                    continue;
                }
                self.infer_type_param_bindings(
                    &normalized(&bound.type_args[0], names, &self.type_info),
                    &output,
                    bindings,
                );
            }
        }
    }

    /// Match the bridge by its resolved Rust owner, keeping equal-spelled application traits out of native inference.
    fn body_ir_runtime_future_bound(&self, bound: &TypeBoundInfo) -> bool {
        const OWNER: &str = "incan_std_async::task::RuntimeFuture";
        if bound.name.trim_start_matches("::") == OWNER {
            return true;
        }
        self.import_binding_path(&bound.name)
            .is_some_and(|path| path.join("::").trim_start_matches("rust::") == OWNER)
    }
}

/// Normalize checked tuple aliases and only the callee's declared named binders for structural unification.
fn normalized(ty: &ResolvedType, binders: &[String], facts: &crate::typechecker::TypeCheckInfo) -> ResolvedType {
    match ty {
        ResolvedType::Named(name) if binders.contains(name) => ResolvedType::TypeVar(name.clone()),
        ResolvedType::RustPath(name) if binders.contains(name) => ResolvedType::TypeVar(name.clone()),
        ResolvedType::RustPath(path) => crate::body_ir::runtime_nominals::project_resolved_path(path, facts)
            .map(|carrier| normalized(&carrier, binders, facts))
            .unwrap_or_else(|| ty.clone()),
        ResolvedType::Generic(name, elements) => {
            let elements = elements
                .iter()
                .map(|element| normalized(element, binders, facts))
                .collect();
            if name == "Tuple" {
                ResolvedType::Tuple(elements)
            } else {
                ResolvedType::Generic(name.clone(), elements)
            }
        }
        ResolvedType::Tuple(elements) => ResolvedType::Tuple(
            elements
                .iter()
                .map(|element| normalized(element, binders, facts))
                .collect(),
        ),
        ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(normalized(inner, binders, facts))),
        ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(normalized(inner, binders, facts))),
        ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(normalized(inner, binders, facts))),
        ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(normalized(inner, binders, facts))),
        ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
            Box::new(normalized(key, binders, facts)),
            Box::new(normalized(value, binders, facts)),
        ),
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
