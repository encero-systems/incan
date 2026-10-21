//! Contextual closure expression types from rustc's retained Fn-family predicates and Output equalities.

use super::TypeChecker;
use crate::ast::{CallArg, Expr};
use crate::resolved_type_subst::map_resolved_type_leaves;
use crate::symbols::{CallableParam, ResolvedType};
use incan_lang::interop::RustFunctionSig;

impl TypeChecker {
    /// Recover a callable's inputs and result from its declared predicate, rather than inferring them from F's name.
    /// Unbound result variables become holes only for contextual checking; inference retains the original binders.
    pub(in crate::typechecker::check_expr) fn rust_bound_callable_type(
        &self,
        signature: &RustFunctionSig,
        parameter: &str,
        contextual: bool,
    ) -> Option<ResolvedType> {
        let bound = signature.bounds.iter().find(|bound| {
            bound.self_type == parameter
                && matches!(bound.path.rsplit("::").next(), Some("Fn" | "FnMut" | "FnOnce"))
                && bound
                    .associated_type_bindings
                    .iter()
                    .any(|binding| binding.name == "Output")
        })?;
        let inputs = self.resolved_type_from_rust_display(bound.type_arguments.first()?);
        let inputs = match inputs {
            ResolvedType::Tuple(inputs) => inputs,
            ResolvedType::Unit => Vec::new(),
            _ => return None,
        };
        let output = &bound
            .associated_type_bindings
            .iter()
            .find(|binding| binding.name == "Output")?
            .value_path;
        let result = ResolvedType::Function(
            inputs.into_iter().map(CallableParam::positional).collect(),
            Box::new(self.resolved_type_from_rust_display(output)),
        );
        if !contextual {
            return Some(result);
        }
        Some(map_resolved_type_leaves(&result, &|leaf| match leaf {
            ResolvedType::RustPath(path) if signature.type_params.contains(path) => Some(ResolvedType::Unknown),
            _ => None,
        }))
    }

    /// Recheck deferred closure expressions once a Rust method's owner and callable predicates have been selected.
    pub(in crate::typechecker::check_expr) fn contextualize_rust_bound_closures(
        &mut self,
        signature: &RustFunctionSig,
        args: &[CallArg],
        types: &[ResolvedType],
    ) -> Vec<ResolvedType> {
        let parameters = if Self::rust_signature_has_receiver(signature) {
            &signature.params[1..]
        } else {
            &signature.params
        };
        args.iter()
            .zip(types)
            .enumerate()
            .map(|(index, (argument, ty))| {
                let expression = Self::call_arg_expr(argument);
                if !matches!(expression.node, Expr::Closure(_, _)) {
                    return ty.clone();
                }
                let parameter = match argument {
                    CallArg::Positional(_) => parameters.get(index),
                    CallArg::Named(name, _) => parameters
                        .iter()
                        .find(|parameter| parameter.name.as_ref() == Some(&name.node)),
                    _ => None,
                };
                let Some(expected) = parameter
                    .and_then(|parameter| self.rust_bound_callable_type(signature, &parameter.type_display, true))
                else {
                    return ty.clone();
                };
                self.check_expr_with_expected(expression, Some(&expected))
            })
            .collect()
    }
}
