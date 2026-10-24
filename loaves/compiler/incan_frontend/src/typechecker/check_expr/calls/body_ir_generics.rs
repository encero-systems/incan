//! Retain checked generic call arguments and results shared by expression checking and Body IR.

use std::collections::{HashMap, HashSet};

use super::TypeChecker;
use crate::ast::{CallArg, Expr, Literal, Span, Spanned};
use crate::resolved_type_subst::substitute_resolved_type;
use crate::symbols::{FunctionInfo, MethodInfo, ResolvedType, TypeBoundInfo, TypeInfo};
use incan_lang::interop::is_rust_capability_bound;
use incan_lang::lang::surface::constructors::ConstructorId;
use incan_lang::lang::testing::{self, TestingAssertHelperId};
use incan_semantics_core::{CanonicalSymbolId, SemanticSourceTargetKind};

impl TypeChecker {
    /// Retain a selected async function call only after its arity, argument, bound and capture checks succeed. The
    /// enclosing module wrapper's earlier diagnostics are included; a missing canonical function grants no contract.
    pub(in crate::typechecker::check_expr) fn retain_body_ir_async_call_target(
        &mut self,
        is_async: bool,
        name: &str,
        span: Span,
        callee_span: Span,
        errors_before_call: usize,
    ) {
        let key = (span.start, span.end);
        self.type_info.calls.body_ir_async_call_targets.remove(&key);
        if !is_async || self.errors.len() != errors_before_call {
            return;
        }
        let Some(identity) = self
            .type_info
            .resolved_call_identity(name, callee_span, None)
            .or_else(|| self.type_info.resolved_import_identity(name))
            .cloned()
        else {
            return;
        };
        if identity.kind == SemanticSourceTargetKind::Function {
            if self.type_info.resolved_identity(callee_span).is_none() {
                self.type_info.record_resolved_identity(callee_span, identity.clone());
            }
            self.type_info.calls.body_ir_async_call_targets.insert(key, identity);
        }
    }

    /// Retain a generic source newtype method's native output before legacy erases an open receiver binder.
    /// The selected declaration and already checked arguments supply the constraint; no legacy binding is changed.
    pub(super) fn retain_body_ir_newtype_method_result(
        &mut self,
        method: &MethodInfo,
        receiver: &ResolvedType,
        args: &[CallArg],
        legacy_bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let ResolvedType::Generic(owner, _) = receiver else {
            return;
        };
        let Some(TypeInfo::Newtype(owner)) = self.lookup_semantic_type_info(owner) else {
            return;
        };
        if owner.is_rusttype || owner.type_params.is_empty() || method.identity.is_none() {
            return;
        }
        let mut names = owner.type_params.clone();
        names.extend(method.type_params.iter().cloned());
        let mut bindings: HashMap<_, _> = legacy_bindings
            .iter()
            .filter(|(_, value)| concrete(value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let (parameters, result) = self.method_types_substituting_call_site_self(method, receiver);
        for (expression, parameter) in Self::arguments_with_parameter_types(&parameters, args) {
            let Some(actual) = self.type_info.expr_type(expression.span) else {
                return;
            };
            let expected = normalized(parameter, &names, self);
            let actual = normalized(actual, &[], self);
            if !concrete(&actual) {
                return;
            }
            self.infer_type_param_bindings(&expected, &actual, &mut bindings);
            let closed = substitute_resolved_type(&expected, &bindings);
            if !self.types_compatible(&closed, &actual) {
                return;
            }
        }
        let output = substitute_resolved_type(&normalized(&result, &names, self), &bindings);
        if !concrete(&output) {
            return;
        }
        self.type_info
            .calls
            .body_ir_call_result_types
            .insert((span.start, span.end), output);
    }

    /// Record a concrete native instantiation from already checked parameter/argument relationships.
    ///
    /// Legacy preserves its own bindings and Rust inference. Body IR additionally normalizes the checked tuple
    /// spellings and declaration-bound named placeholders, then uses the same unifier. Unchecked spreads, conflicting
    /// bindings and open arguments publish no fact. This runs only after the selected ordinary or surface call checks
    /// have succeeded.
    pub(super) fn retain_body_ir_call_arguments(
        &mut self,
        info: &FunctionInfo,
        args: &[CallArg],
        legacy_bindings: &HashMap<String, ResolvedType>,
        span: Span,
        identity: Option<&CanonicalSymbolId>,
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
        let logical_result = substitute_resolved_type(&normalized(&info.return_type, &names, self), legacy_bindings);
        let mut bindings: HashMap<_, _> = legacy_bindings
            .iter()
            .filter(|(_, ty)| concrete(&normalized(ty, &names, self)))
            .map(|(name, ty)| (name.clone(), normalized(ty, &names, self)))
            .collect();
        let mut native_binders = HashSet::new();
        let rest = info
            .params
            .iter()
            .find(|parameter| parameter.kind == crate::ast::ParamKind::RestPositional);
        for ((expression, parameter), argument) in Self::arguments_with_parameters(&info.params, args)
            .into_iter()
            .zip(args)
        {
            let parameter = parameter.or_else(|| matches!(argument, CallArg::Positional(_)).then_some(rest).flatten());
            let Some(parameter) = parameter else {
                return;
            };
            let testing_argument = self.body_ir_testing_argument_type(identity, expression);
            let testing_channel = testing_argument.is_some();
            let key = (expression.span.start, expression.span.end);
            let Some(actual) = self
                .type_info
                .expressions
                .body_ir_expr_types
                .get(&key)
                .or_else(|| self.type_info.calls.body_ir_constructor_types.get(&key))
                .or_else(|| self.type_info.calls.body_ir_call_result_types.get(&key))
                .or_else(|| self.type_info.calls.inferred_constructor_types.get(&key))
                .or_else(|| self.type_info.calls.inferred_call_result_types.get(&key))
                .or_else(|| self.type_info.expr_type(expression.span))
            else {
                return;
            };
            let expected = normalized(&parameter.ty, &names, self);
            let actual = testing_argument.unwrap_or_else(|| self.body_ir_argument_type(expression, actual));
            // A native slice is not the source byte buffer used by legacy inference. Preserve the selected
            // reference at the source instantiation boundary, before argument storage is chosen.
            let native_slice = self.body_ir_native_slice_argument(expression);
            let actual = native_slice.as_ref().unwrap_or(&actual);
            if testing_channel {
                self.type_info
                    .expressions
                    .body_ir_expr_types
                    .insert(key, actual.clone());
            }
            let actual = normalized(actual, &[], self);
            let actual = self.body_ir_adopted_trait_type(&expected, &actual).unwrap_or(actual);
            if let ResolvedType::TypeVar(name) = &expected {
                let first_actual = native_binders.insert(name.clone());
                if first_actual
                    && (native_slice.is_some()
                        || matches!(&actual, ResolvedType::Generic(base, outputs) if base == "Awaitable" && outputs.len() == 1))
                {
                    // Replace only a legacy representation seed, preserving every preceding argument's evidence.
                    bindings.remove(name);
                }
            }
            self.retain_body_ir_argument_bindings(&expected, &actual, &mut bindings);
        }
        self.retain_body_ir_runtime_outputs(info, &names, &mut bindings);
        let arguments: Option<Vec<_>> = names
            .iter()
            .map(|name| bindings.get(name).filter(|ty| concrete(ty)).cloned())
            .collect();
        if let Some(arguments) = arguments {
            self.type_info
                .calls
                .body_ir_call_type_arguments
                .insert((span.start, span.end), arguments);
            let result = substitute_resolved_type(&normalized(&info.return_type, &names, self), &bindings);
            self.retain_body_ir_call_parameters(info, &names, &bindings, span);
            tracing::debug!(target: "incan::invocation", identity = ?identity, declared = ?info.return_type, names = ?names, bindings = ?bindings, result = ?result, "direct-route checked call result");
            if concrete(&result) {
                self.type_info
                    .calls
                    .body_ir_call_result_types
                    .insert((span.start, span.end), result.clone());
            }
            if concrete(&result) {
                // Keep a settled source spelling in the semantic checker; canonical owner keys belong to Body IR.
                let result = if concrete(&logical_result) {
                    logical_result
                } else {
                    result
                };
                self.type_info
                    .calls
                    .inferred_call_result_types
                    .insert((span.start, span.end), result);
            }
        } else {
            self.retain_open_source_call(info, &names, &bindings, span, identity);
        }
    }

    /// Enrich legacy seeds only with the actual argument's already checked nominal owners before unification.
    /// An incompatible owner stays incompatible; the ordinary unifier still refuses conflicting bindings.
    fn retain_body_ir_argument_bindings(
        &self,
        expected: &ResolvedType,
        actual: &ResolvedType,
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        let mut observed = HashMap::new();
        self.infer_type_param_bindings(expected, actual, &mut observed);
        for (name, value) in observed {
            if let Some(existing) = bindings.get_mut(&name) {
                *existing = self.body_ir_close_value_parts(existing, &value);
            }
        }
        self.infer_type_param_bindings(expected, actual, bindings);
    }

    /// Record the already checked instance's parameter storage, so argument binding cannot reopen its closed leaves.
    fn retain_body_ir_call_parameters(
        &mut self,
        info: &FunctionInfo,
        names: &[String],
        bindings: &HashMap<String, ResolvedType>,
        span: Span,
    ) {
        let parameters = info
            .params
            .iter()
            .map(|parameter| crate::symbols::CallableParam {
                ty: substitute_resolved_type(&normalized(&parameter.ty, names, self), bindings),
                ..parameter.clone()
            })
            .collect::<Vec<_>>();
        if parameters.iter().all(|parameter| concrete(&parameter.ty)) {
            self.type_info
                .calls
                .body_ir_call_parameter_types
                .insert((span.start, span.end), parameters);
        }
    }

    /// Retain only canonical testing helpers' syntactically absent channels as Unit, matching legacy emission.
    /// Ordinary calls, source-owned equal spellings and constructors with supplied channels keep their checked facts.
    fn body_ir_testing_argument_type(
        &mut self,
        identity: Option<&CanonicalSymbolId>,
        expression: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        let identity = identity?;
        let mut path = self.standard_declaring_module(identity)?;
        path.push(identity.declaration_name.clone());
        let helper = testing::assert_helper_id_from_std_path(&path)?;
        if helper == TestingAssertHelperId::AssertIsNone && matches!(expression.node, Expr::Literal(Literal::None)) {
            return Some(ResolvedType::Generic("Option".into(), vec![ResolvedType::Unit]));
        }
        let constructor = self.literal_carrier_constructor(expression)?;
        let (expected, present) = match helper {
            TestingAssertHelperId::AssertIsOk => (ConstructorId::Ok, true),
            TestingAssertHelperId::AssertIsErr => (ConstructorId::Err, false),
            _ => return None,
        };
        if constructor != expected {
            return None;
        }
        let args = match &expression.node {
            Expr::Call(_, _, args) | Expr::Constructor(_, args) => args,
            _ => return None,
        };
        let [CallArg::Positional(payload)] = args.as_slice() else {
            return None;
        };
        let payload = self.body_ir_checked_value_type(payload.span)?;
        if !Self::is_settled_type_argument(payload) {
            return None;
        }
        let channels = if present {
            vec![payload.clone(), ResolvedType::Unit]
        } else {
            vec![ResolvedType::Unit, payload.clone()]
        };
        Some(ResolvedType::Generic("Result".into(), channels))
    }

    /// Read an adopter's checked trait arguments, including explicit inherited arguments, before structural binding.
    /// This supplements ordinary inference privately; it neither selects an adopter nor changes legacy trait facts.
    fn body_ir_adopted_trait_type(&self, expected: &ResolvedType, actual: &ResolvedType) -> Option<ResolvedType> {
        let ResolvedType::Generic(required, _) = expected else {
            return None;
        };
        self.lookup_semantic_trait_info(required)?;
        let (owner, arguments) = match actual {
            ResolvedType::Named(owner) => (owner, &[][..]),
            ResolvedType::Generic(owner, arguments) => (owner, arguments.as_slice()),
            _ => return None,
        };
        let (parameters, adoptions) = match self.lookup_semantic_type_info(owner)? {
            TypeInfo::Model(info) => (&info.type_params, &info.trait_adoptions),
            TypeInfo::Class(info) => (&info.type_params, &info.trait_adoptions),
            TypeInfo::Enum(info) => (&info.type_params, &info.trait_adoptions),
            TypeInfo::Newtype(info) => (&info.type_params, &info.trait_adoptions),
            _ => return None,
        };
        let owner_bindings = crate::resolved_type_subst::type_param_subst_map(parameters, arguments);
        let mut selected = None;
        for adoption in adoptions {
            let info = self.lookup_trait_adoption_info(adoption)?;
            let direct = if adoption.type_args.is_empty() {
                arguments
                    .iter()
                    .take(info.type_params.len())
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                adoption
                    .type_args
                    .iter()
                    .map(|ty| substitute_resolved_type(ty, &owner_bindings))
                    .collect()
            };
            if direct.len() != info.type_params.len() || !direct.iter().all(concrete) {
                continue;
            }
            let mut candidates = vec![(adoption.name.clone(), direct.clone())];
            let bindings = crate::resolved_type_subst::type_param_subst_map(&info.type_params, &direct);
            candidates.extend(
                self.semantic_supertrait_closure(&adoption.name)
                    .into_iter()
                    .map(|(name, args)| {
                        (
                            name,
                            args.iter().map(|ty| substitute_resolved_type(ty, &bindings)).collect(),
                        )
                    }),
            );
            for (name, args) in candidates {
                if !self.trait_name_matches(&name, required) {
                    continue;
                }
                let carrier = ResolvedType::Generic(required.clone(), args);
                if selected.as_ref().is_some_and(|previous| previous != &carrier) {
                    return None;
                }
                selected = Some(carrier);
            }
        }
        selected
    }

    /// Read an exact native slice before source byte-buffer projection. The selected call result takes priority over
    /// legacy return coercions, matching the carrier Body IR emits; canonical locals use their retained provenance.
    fn body_ir_native_slice_argument(&self, expression: &Spanned<Expr>) -> Option<ResolvedType> {
        self.type_info
            .rust
            .checked_calls
            .get(&(expression.span.start, expression.span.end))
            .map(|call| call.return_type.clone())
            .or_else(|| self.rust_native_value_type(expression))
            .filter(|ty| {
                matches!(ty, ResolvedType::Ref(inner) | ResolvedType::RefMut(inner)
                if matches!(inner.as_ref(), ResolvedType::Generic(base, _) if base == "[]"))
            })
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
        let checked_async = self
            .type_info
            .calls
            .body_ir_async_call_targets
            .get(&(expression.span.start, expression.span.end))
            .is_some_and(|identity| self.type_info.resolved_identity(callee_span) == Some(identity));
        if source_async
            || checked_async
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

    /// Close exact SDK future/callback outputs from checked carriers in a cloned binding set. Ordinary checking and
    /// native metadata still enforce trait bounds; foreign and source-owned equal spellings publish no bridge fact.
    fn retain_body_ir_runtime_outputs(
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
                let Some(outputs) = self.body_ir_runtime_bridge_arguments(bound, &actual) else {
                    continue;
                };
                if bound.type_args.len() != outputs.len() || !outputs.iter().all(concrete) {
                    continue;
                }
                for (parameter, output) in bound.type_args.iter().zip(outputs) {
                    self.infer_type_param_bindings(&normalized(parameter, names, self), &output, bindings);
                }
            }
        }
    }

    /// Read only the exact SDK bridge's associated arguments from an already checked future or callable shape.
    fn body_ir_runtime_bridge_arguments(
        &self,
        bound: &TypeBoundInfo,
        actual: &ResolvedType,
    ) -> Option<Vec<ResolvedType>> {
        if self.body_ir_runtime_bound_owner(bound, "incan_std_async::task::RuntimeFuture") {
            return self.await_output_type_from_type(actual).map(|output| vec![output]);
        }
        let ResolvedType::Function(parameters, output) = actual else {
            return None;
        };
        if parameters.is_empty() && self.body_ir_runtime_bound_owner(bound, "incan_std_async::task::RuntimeFnOnce") {
            return Some(vec![output.as_ref().clone()]);
        }
        if let [parameter] = parameters.as_slice()
            && parameter.kind == crate::ast::ParamKind::Normal
            && !parameter.is_mut
            && self.body_ir_runtime_bound_owner(bound, "incan_std_async::race::RuntimeRaceCallback")
        {
            return Some(vec![parameter.ty.clone(), output.as_ref().clone()]);
        }
        None
    }

    /// Match a resolved native owner or an explicit Rust import, so an equal-spelled source import cannot authorize it.
    fn body_ir_runtime_bound_owner(&self, bound: &TypeBoundInfo, owner: &str) -> bool {
        if bound.name.trim_start_matches("::") == owner {
            return true;
        }
        self.import_binding_path(&bound.name)
            .and_then(|path| path.strip_prefix(&["rust".to_owned()]))
            .is_some_and(|path| path.join("::") == owner)
    }
}

/// Normalize checked aliases and declared binders in the accepting checker; SDK projection keeps its lexical scope.
pub(super) fn normalized(ty: &ResolvedType, binders: &[String], checker: &TypeChecker) -> ResolvedType {
    match ty {
        ResolvedType::Named(name) if binders.contains(name) => ResolvedType::TypeVar(name.clone()),
        ResolvedType::RustPath(name) if binders.contains(name) => ResolvedType::TypeVar(name.clone()),
        ResolvedType::Generic(name, elements) => {
            let elements = elements
                .iter()
                .map(|element| normalized(element, binders, checker))
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
                .map(|element| normalized(element, binders, checker))
                .collect(),
        ),
        ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(normalized(inner, binders, checker))),
        ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(normalized(inner, binders, checker))),
        ResolvedType::TypeToken(inner) => ResolvedType::TypeToken(Box::new(normalized(inner, binders, checker))),
        ResolvedType::Function(parameters, result) => ResolvedType::Function(
            parameters
                .iter()
                .map(|parameter| crate::symbols::CallableParam {
                    ty: normalized(&parameter.ty, binders, checker),
                    ..parameter.clone()
                })
                .collect(),
            Box::new(normalized(result, binders, checker)),
        ),
        ResolvedType::FrozenList(inner) => ResolvedType::FrozenList(Box::new(normalized(inner, binders, checker))),
        ResolvedType::FrozenSet(inner) => ResolvedType::FrozenSet(Box::new(normalized(inner, binders, checker))),
        ResolvedType::FrozenDict(key, value) => ResolvedType::FrozenDict(
            Box::new(normalized(key, binders, checker)),
            Box::new(normalized(value, binders, checker)),
        ),
        ty => ty.clone(),
    }
}

/// A native fallback requires every structural leaf to be fixed; unresolved or caller-open types stay on the old path.
pub(super) fn concrete(ty: &ResolvedType) -> bool {
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
