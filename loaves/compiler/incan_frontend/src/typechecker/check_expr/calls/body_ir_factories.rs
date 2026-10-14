//! Retain checked static factory owner arguments before legacy's Self projection erases their binders.

use std::collections::HashMap;

use super::TypeChecker;
use super::body_ir_generics::{concrete, normalized};
use crate::ast::{CallArg, Span};
use crate::resolved_type_subst::substitute_resolved_type;
use crate::symbols::{MethodInfo, ResolvedType, TypeInfo};
use crate::typechecker::nominal_type_param_bounds::NominalTypeParamBounds;
use crate::typechecker::stdlib_loader::StdlibAstCache;
use incan_semantics_core::SymbolOrigin;

#[cfg(test)]
mod tests;

impl TypeChecker {
    /// Close a selected source static factory from its original owner signature and already checked context.
    /// Private bindings and Body IR tables keep legacy's open Self result and Rust inference unchanged.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn retain_body_ir_factory_owner(
        &mut self,
        method: &MethodInfo,
        receiver: &ResolvedType,
        args: &[CallArg],
        legacy: &HashMap<String, ResolvedType>,
        expected: Option<&ResolvedType>,
        span: Span,
    ) {
        // ---- Selected static declaration ----
        if method.receiver.is_some()
            || method.is_async
            || !method.type_params.is_empty()
            || method.identity.is_none()
            || self.type_info.resolved_identity(span) != method.identity.as_ref()
        {
            return;
        }
        let Some((owner, names, template)) = self.body_ir_factory_template(receiver) else {
            return;
        };
        let Some(declared) = self.body_ir_factory_declaration(&owner, method) else {
            return;
        };
        let (parameters, result) = self.method_types_substituting_call_site_self(&declared, &template);
        let result = normalized(&result, &names, &self.type_info);

        // ---- Checked argument and destination constraints ----
        let mut bindings: HashMap<_, _> = legacy
            .iter()
            .filter(|(_, ty)| concrete(ty))
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();
        if let Some(expected) = expected {
            self.infer_type_param_bindings(&result, &normalized(expected, &[], &self.type_info), &mut bindings);
        }
        for (expression, parameter) in Self::arguments_with_parameter_types(&parameters, args) {
            let Some(actual) = self.type_info.expr_type(expression.span) else {
                return;
            };
            let actual = normalized(actual, &[], &self.type_info);
            let parameter = normalized(parameter, &names, &self.type_info);
            if !concrete(&actual) {
                return;
            }
            self.infer_type_param_bindings(&parameter, &actual, &mut bindings);
            if !self.types_compatible(&substitute_resolved_type(&parameter, &bindings), &actual) {
                self.refuse_body_ir_factory_result(span);
                return;
            }
        }
        let arguments: Option<Vec<_>> = names
            .iter()
            .map(|name| bindings.get(name).filter(|ty| concrete(ty)).cloned())
            .collect();
        let Some(arguments) = arguments else {
            return;
        };

        // ---- Complete owner bounds and native result ----
        let bounds = self.body_ir_factory_owner_bounds(&owner);
        if bounds.iter().any(|(name, requirements)| {
            bindings.get(name).is_none_or(|actual| {
                requirements
                    .iter()
                    .any(|bound| !self.type_satisfies_explicit_bound_info(actual, bound, &bindings))
            })
        }) {
            return;
        }
        let output = substitute_resolved_type(&result, &bindings);
        if !concrete(&output) || expected.is_some_and(|expected| !self.types_compatible(&output, expected)) {
            if concrete(&output) {
                self.refuse_body_ir_factory_result(span);
            }
            return;
        }
        self.type_info
            .calls
            .body_ir_call_type_arguments
            .insert((span.start, span.end), arguments);
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert((span.start, span.end), output);
    }

    /// Retain a checked factory contradiction once, clearing only supplemental native results. Legacy inference and
    /// signatures stay unchanged; neither nominal member propagation nor destination filling may reopen this call.
    fn refuse_body_ir_factory_result(&mut self, span: Span) {
        let key = (span.start, span.end);
        self.type_info.calls.body_ir_factory_result_conflicts.insert(key);
        self.type_info.calls.body_ir_call_result_types.remove(&key);
        self.type_info.calls.body_ir_call_type_arguments.remove(&key);
        self.type_info.expressions.body_ir_expr_types.remove(&key);
    }

    /// Read the exact selected factory's original owner signature. The ordinary call checker can already have
    /// substituted open receiver arguments into its local method copy; those holes cannot prove compatibility with
    /// a destination. This native-only snapshot keeps the original checked parameter constraints and identity.
    fn body_ir_factory_declaration(&self, owner: &str, selected: &MethodInfo) -> Option<MethodInfo> {
        let identity = selected.identity.as_ref()?;
        let (methods, overloads) = match self.lookup_semantic_type_info(owner)? {
            TypeInfo::Class(info) => (&info.methods, &info.method_overloads),
            TypeInfo::Model(info) => (&info.methods, &info.method_overloads),
            _ => return None,
        };
        methods
            .values()
            .chain(overloads.values().flatten())
            .find(|method| method.identity.as_ref() == Some(identity))
            .cloned()
    }

    /// Recover missing SDK bound provenance only from the exact authenticated owner declaration.
    /// An independent source query keeps the shared checker cache and legacy bound tables unchanged. Bounds absent
    /// from that declaration or carrying different arguments retain their original metadata and grant no new fact.
    fn body_ir_factory_owner_bounds(&self, owner: &str) -> NominalTypeParamBounds {
        let mut bounds = self.declared_nominal_bounds(owner).cloned().unwrap_or_default();
        let Some(identity) = self
            .symbols
            .module_type_identity(owner)
            .or_else(|| self.type_info.declarations.named_type_identities.get(owner))
        else {
            return bounds;
        };

        // ---- Authenticated declaring source ----
        let mut cache = StdlibAstCache::new();
        cache.bind_provider_plan(&self.provider_plan);
        let Some(source) = cache.callable_source_identity(identity) else {
            return bounds;
        };
        let SymbolOrigin::Module(path) = source.origin else {
            return bounds;
        };
        let source_bounds = cache.lookup_type_param_bounds(&path, &source.declaration_name);
        if cache.verify_retained_sources().is_err() {
            return bounds;
        }

        // ---- Missing bound provenance ----
        for (parameter, requirements) in &mut bounds {
            let Some((_, declared)) = source_bounds.iter().find(|(name, _)| name == parameter) else {
                continue;
            };
            for bound in requirements {
                if bound.module_path.is_some() || bound.source_name.is_some() {
                    continue;
                }
                if let Some(proven) = declared.iter().find(|proven| {
                    proven.name == bound.name
                        && proven.type_args == bound.type_args
                        && proven.implementation_type_params == bound.implementation_type_params
                        && proven.inferred == bound.inferred
                }) {
                    *bound = proven.clone();
                }
            }
        }
        bounds
    }

    /// Restore only a source model or class's declared owner binders; concrete receiver arguments remain fixed.
    fn body_ir_factory_template(&self, receiver: &ResolvedType) -> Option<(String, Vec<String>, ResolvedType)> {
        let (owner, actual) = match receiver {
            ResolvedType::Named(owner) => (owner, &[][..]),
            ResolvedType::Generic(owner, arguments) => (owner, arguments.as_slice()),
            _ => return None,
        };
        let names = match self.lookup_semantic_type_info(owner)? {
            TypeInfo::Class(info) => info.type_params.clone(),
            TypeInfo::Model(info) => info.type_params.clone(),
            _ => return None,
        };
        if names.is_empty() || (!actual.is_empty() && actual.len() != names.len()) {
            return None;
        }
        let arguments = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                actual
                    .get(index)
                    .filter(|ty| concrete(ty))
                    .cloned()
                    .unwrap_or_else(|| ResolvedType::TypeVar(name.clone()))
            })
            .collect();
        let template = ResolvedType::Generic(owner.clone(), arguments);
        Some((owner.clone(), names, template))
    }
}
