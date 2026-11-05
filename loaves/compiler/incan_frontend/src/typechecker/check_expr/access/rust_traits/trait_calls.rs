//! Native element conversion, Rust trait associated and declared method calls, and iteration hooks.

use super::*;
use crate::typechecker::check_expr::access::MethodCallSite;

impl TypeChecker {
    /// Select an implicit method call through exactly one imported Rust trait bound on an active type parameter.
    ///
    /// The bound is the authority for the call; metadata supplies the declared native signature and the existing
    /// declared-trait path retains its executable result. Competing trait declarations remain unresolved rather than
    /// choosing by spelling, and concrete implementation discovery stays rustc's responsibility.
    pub(in crate::typechecker::check_expr::access) fn resolve_bound_rust_trait_method(
        &mut self,
        call: &MethodCallSite<'_>,
        receiver: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let name = self.generic_placeholder_name(receiver)?;
        let bounds = self.innermost_type_param_bounds(name)?.to_vec();
        let mut selected = None;
        for bound in bounds {
            let path = if bound.name.contains("::") {
                Some(bound.name.trim_start_matches("::").to_owned())
            } else {
                self.rust_canonical_path_for_receiver_type(&ResolvedType::Named(bound.name.clone()))
            };
            let Some(path) = path else { continue };
            let Some(metadata) = self.rust_item_metadata_for_path(&path) else {
                continue;
            };
            let RustItemKind::Trait(info) = &metadata.kind else {
                continue;
            };
            let declares = info
                .items
                .iter()
                .any(|item| matches!(item, RustTraitAssoc::Function { name, .. } if name == call.method));
            if !declares {
                continue;
            }
            if selected.as_ref().is_some_and(|(previous, _)| previous != &path) {
                return None;
            }
            selected = Some((path, metadata));
        }
        let (path, metadata) = selected?;
        let RustItemKind::Trait(info) = &metadata.kind else {
            return None;
        };
        self.resolve_rust_declared_trait_method(
            RustPathMethodCall {
                rust_path: &path,
                method: call.method,
                receiver_metadata: None,
                receiver_binding: None,
                receiver_expr: call.base,
                type_args: call.type_args,
                args: call.args,
                arg_types,
                receiver_span: call.base.span,
                expected_return_ty: call.expected_return_ty,
                span: call.span,
            },
            info,
        )
    }

    /// Select an owned per-element From conversion using the same metadata candidates as explicit native calls.
    /// The typed placeholder supplies no executable expression or source identity; only the checked element type
    /// participates in inference, and exact input/output carriers must match before a call is retained.
    pub(in crate::typechecker::check_expr) fn native_element_conversion(
        &self,
        source: &ResolvedType,
        target: &ResolvedType,
    ) -> Option<crate::typechecker::RustCheckedCall> {
        // ---- Native conversion context ----
        let path = self.rust_canonical_path_for_receiver_type(target)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let placeholder = Spanned::new(
            Expr::Literal(crate::ast::Literal::String(String::new())),
            crate::ast::Span::new(0, 0),
        );
        let arguments = [crate::ast::CallArg::Positional(placeholder.clone())];
        let types = [source.clone()];
        let call = RustPathMethodCall {
            rust_path: &path,
            method: "from",
            receiver_metadata: Some(&metadata),
            receiver_binding: None,
            receiver_expr: &placeholder,
            type_args: &[],
            args: &arguments,
            arg_types: &types,
            receiver_span: placeholder.span,
            expected_return_ty: Some(target),
            span: placeholder.span,
        };

        // ---- Exact and unambiguous From implementation ----
        let identity = self.rust_trait_identity("std::convert::From");
        let mut selected: Option<crate::typechecker::RustCheckedCall> = None;
        for implementation in &info.implemented_traits {
            if self.rust_trait_identity(&implementation.path) != identity {
                continue;
            }
            let Some(candidate) = self.retained_trait_candidate(implementation, &call, target, info) else {
                continue;
            };
            if Self::rust_signature_has_receiver(&candidate.signature) || candidate.signature.params.len() != 1 {
                continue;
            }
            let parameter = self.native_rust_signature_type(&candidate.signature.params[0].type_display, &path);
            let result = self.native_rust_signature_type(&candidate.signature.return_type, &path);
            if !self.rust_trait_carriers_match(&parameter, source) || !self.rust_trait_carriers_match(&result, target) {
                continue;
            }
            let retained = crate::typechecker::RustCheckedCall {
                storage: None,
                source_type_parameters: self.native_call_type_parameters(),
                receiver_adjustments: Vec::new(),
                path: candidate.path,
                type_arguments: candidate.arguments,
                parameters: vec![crate::symbols::CallableParam::positional(parameter)],
                return_type: result,
                result_item_path: Some(path.clone()),
                has_receiver: false,
            };
            if selected.as_ref().is_some_and(|previous| previous != &retained) {
                return None;
            }
            selected = Some(retained);
        }
        selected
    }

    /// Infer a receiver-free trait function's Self from its destination and its declared method binders from inputs.
    /// Close associated projections before validation, retaining source declaration types structurally rather than
    /// replacing an inferred model with a parsed Rust display. Rustc verifies the selected native trait obligations.
    pub(in crate::typechecker::check_expr::access) fn resolve_rust_trait_associated_call(
        &mut self,
        call: &RustTraitQualifiedCall<'_>,
        original: &RustFunctionSig,
    ) -> Option<ResolvedType> {
        if original.is_async || original.is_unsafe || !call.type_args.is_empty() {
            return None;
        }
        let mut variables = vec!["Self".to_owned()];
        variables.extend(call.trait_info.type_params.iter().cloned());
        variables.extend(original.type_params.iter().cloned());
        let bindings = self.infer_rust_signature_bindings(
            original,
            "",
            &variables,
            call.args,
            call.arg_types,
            call.expected_return_ty,
        );
        let arguments = variables
            .iter()
            .map(|name| bindings.get(name).cloned())
            .collect::<Option<Vec<_>>>()?;
        if arguments
            .iter()
            .any(|argument| matches!(argument, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
        {
            return None;
        }
        let owner = bindings.get("Self")?;
        let displays = bindings
            .iter()
            .map(|(name, value)| (name.as_str(), render_resolved_type_as_rust_arg(value)))
            .collect();
        let signature = self.rust_signature_projections(Self::specialize_rust_signature(
            original,
            &displays,
            &render_resolved_type_as_rust_arg(owner),
        ));
        let errors_before = self.errors.len();
        let path = format!("{}::{}", call.rust_path, call.method);
        self.validate_rust_method_call(&path, &signature, call.args, call.arg_types, false, call.span);
        let mut result = self.instantiate_rust_signature_type(
            &self.native_rust_signature_type(&original.return_type, ""),
            &variables,
            &bindings,
        );
        if original.return_type.contains(" as ") {
            let failed = Cell::new(false);
            result = map_resolved_type_leaves(&result, &|leaf| {
                let ResolvedType::RustPath(display) = leaf else {
                    return None;
                };
                if !display.contains(" as ") {
                    return None;
                }
                let display =
                    Self::substitute_rust_owner_display(display, &displays, &render_resolved_type_as_rust_arg(owner));
                let value = self.rust_projected_type(&display, 0);
                if value.is_none() {
                    failed.set(true);
                }
                value
            });
            if failed.get() {
                return None;
            }
        }
        if self.errors.len() != errors_before {
            return Some(result);
        }
        let mut parameters = self
            .type_info
            .call_site_callable_params(call.span)
            .unwrap_or(&[])
            .to_vec();
        for (parameter, declared) in parameters.iter_mut().zip(&original.params) {
            parameter.ty = self.instantiate_rust_signature_type(
                &self.native_rust_signature_type(&declared.type_display, ""),
                &variables,
                &bindings,
            );
        }
        self.type_info.rust.checked_calls.insert(
            call.span,
            crate::typechecker::type_info::RustCheckedCall {
                storage: None,
                source_type_parameters: self.native_call_type_parameters(),
                receiver_adjustments: Vec::new(),
                path,
                type_arguments: arguments,
                parameters,
                return_type: result.clone(),
                result_item_path: self.rust_result_item_path(&signature.return_type, call.rust_path),
                has_receiver: false,
            },
        );
        Some(result)
    }

    /// Select a declared native trait method for a concrete receiver, dynamic receiver or bounded parameter.
    /// The explicit trait path establishes the declaration; checked arguments and destination close its binders,
    /// while native compilation verifies the exact instantiated signature and obligations before constructing MIR.
    pub(in crate::typechecker::check_expr::access) fn resolve_rust_declared_trait_method(
        &mut self,
        call: RustPathMethodCall<'_>,
        info: &RustTraitInfo,
    ) -> Option<ResolvedType> {
        let original = info.items.iter().find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == call.method => Some(signature),
            _ => None,
        })?;
        if !Self::rust_signature_has_receiver(original) || original.is_async || original.is_unsafe {
            return None;
        }
        let source = self
            .rust_native_value_type(call.receiver_expr)
            .or_else(|| self.type_info.expr_type(call.receiver_span).cloned())?;
        let owner = match source {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => *inner,
            other => other,
        };
        let owner = match self.active_type_param_name(&owner) {
            Some(name) => ResolvedType::TypeVar(name.to_owned()),
            None => owner,
        };
        if !call.type_args.is_empty() {
            return None;
        }
        let mut known_arguments = self.rust_owner_type_args(&owner, call.rust_path).unwrap_or_default();
        let associated_count = info
            .items
            .iter()
            .filter(|item| matches!(item, RustTraitAssoc::TypeAlias { .. }))
            .count();
        if known_arguments.len() > info.type_params.len() + associated_count {
            return None;
        }
        known_arguments.truncate(info.type_params.len());
        let mut variables = info.type_params.clone();
        variables.extend(original.type_params.iter().cloned());
        let mut bindings = info
            .type_params
            .iter()
            .zip(known_arguments)
            .map(|(name, argument)| (name.clone(), argument))
            .collect::<HashMap<_, _>>();
        let inferred = self.infer_rust_signature_bindings(
            original,
            call.rust_path,
            &variables,
            call.args,
            call.arg_types,
            call.expected_return_ty,
        );
        for (name, value) in inferred {
            if let Some(previous) = bindings.get(&name) {
                if !self.rust_trait_carriers_match(previous, &value) {
                    return None;
                }
            } else {
                bindings.insert(name, value);
            }
        }
        let trait_arguments = info
            .type_params
            .iter()
            .map(|name| bindings.get(name).cloned())
            .collect::<Option<Vec<_>>>()?;
        if trait_arguments
            .iter()
            .any(|argument| matches!(argument, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
        {
            return None;
        }
        let displays = bindings
            .iter()
            .map(|(name, value)| (name.as_str(), render_resolved_type_as_rust_arg(value)))
            .collect();
        let signature = self.rust_signature_projections(Self::specialize_rust_signature(
            original,
            &displays,
            &render_resolved_type_as_rust_arg(&owner),
        ));
        self.rust_trait_input_borrow_cost(&signature, &call)?;
        let mut native_arguments = vec![owner.clone()];
        native_arguments.extend(trait_arguments);
        for parameter in &original.type_params {
            native_arguments.push(bindings.get(parameter)?.clone());
        }
        if signature.params.first().is_some_and(|parameter| {
            Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
        }) {
            self.validate_exclusive_rust_receiver(call.receiver_expr, call.span);
        }
        let selected = TraitCandidate {
            path: format!("{}::{}", call.rust_path, call.method),
            signature,
            arguments: native_arguments,
            self_type: owner,
            borrow_cost: 0,
            returns_self: false,
            open_parameters: Vec::new(),
        };
        let errors_before = self.errors.len();
        let result = self.validate_rust_method_call(
            &selected.path,
            &selected.signature,
            call.args,
            call.arg_types,
            false,
            call.span,
        );
        if self.errors.len() == errors_before {
            self.retain_selected_rust_trait_call(&call, selected)?;
        }
        Some(result)
    }

    /// Retain an explicit native trait call on a concrete receiver or a parameter with an innermost native bound.
    /// Concrete implementations are verified by rustc; a parameter still requires its declared authority. Both use
    /// the declared-trait selector, preserving source identities in Self and its result.
    pub(in crate::typechecker::check_expr::access) fn retain_rust_source_trait_call(
        &mut self,
        call: &RustTraitQualifiedCall<'_>,
        receiver: &Spanned<Expr>,
        source: &ResolvedType,
    ) -> Option<ResolvedType> {
        let owner = match source {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => inner.as_ref(),
            other => other,
        };
        let admitted = if let Some(name) = self.active_type_param_name(owner) {
            let identity = self.rust_trait_identity(call.rust_path);
            self.innermost_type_param_bounds(name)?.iter().any(|bound| {
                if !bound.type_args.is_empty() {
                    return false;
                }
                let path = if let Some(path) = bound.name.strip_prefix("::") {
                    path
                } else {
                    let Some(symbol) = self.lookup_symbol(&bound.name) else {
                        return false;
                    };
                    let crate::symbols::SymbolKind::RustItem(item) = &symbol.kind else {
                        return false;
                    };
                    &item.path
                };
                self.rust_item_metadata_for_path(path).is_some_and(|metadata| {
                    matches!(metadata.kind, RustItemKind::Trait(_)) && self.rust_trait_identity(path) == identity
                })
            })
        } else {
            matches!(
                owner,
                ResolvedType::Int | ResolvedType::Float | ResolvedType::Numeric(_) | ResolvedType::Bool
            ) || matches!(owner, ResolvedType::Named(name) | ResolvedType::Generic(name, _)
                    if self.source_nominal_carrier_key(name).is_some())
        };
        if !admitted {
            return None;
        }
        self.resolve_rust_declared_trait_method(
            RustPathMethodCall {
                rust_path: call.rust_path,
                method: call.method,
                receiver_metadata: None,
                receiver_binding: None,
                receiver_expr: receiver,
                type_args: call.type_args,
                args: &call.args[1..],
                arg_types: &call.arg_types[1..],
                receiver_span: receiver.span,
                expected_return_ty: call.expected_return_ty,
                span: call.span,
            },
            call.trait_info,
        )
    }

    /// Select the native iteration protocol once, keeping the iterable call's facts intact. Its poll borrows the
    /// newly owned iterator storage, so the written source binding does not need to be mutable.
    pub(in crate::typechecker) fn retain_rust_iteration_hooks(
        &mut self,
        expression: &Spanned<Expr>,
        source: &ResolvedType,
    ) -> Option<ResolvedType> {
        let owner = self
            .rust_native_value_type(expression)
            .unwrap_or_else(|| source.clone());
        let iter = self.rust_iteration_call(expression, &owner, "core::iter::IntoIterator", "into_iter")?;
        let next = self.rust_iteration_call(expression, &iter.return_type, "core::iter::Iterator", "next")?;
        let item = next.return_type.option_inner_type()?.clone();
        self.type_info.rust.iteration_hooks.insert(
            expression.span,
            crate::typechecker::type_info::RustIterationHooks { iter, next },
        );
        Some(item)
    }

    /// Use the ordinary impl-template selector for one zero-argument native protocol call. No method spelling alone
    /// authorizes iteration: both a unique closed trait reference and its metadata signature are required.
    pub(super) fn rust_iteration_call(
        &self,
        expression: &Spanned<Expr>,
        owner: &ResolvedType,
        trait_path: &str,
        method: &str,
    ) -> Option<crate::typechecker::type_info::RustCheckedCall> {
        // Selection scans every implementation of the protocol trait, so each owner is selected once per checker.
        // The source type parameters in scope are part of the key: an owner naming one of them can select
        // differently in another declaration.
        let key = (
            format!("{owner:?}"),
            trait_path.to_owned(),
            method.to_owned(),
            format!("{:?}", self.native_call_type_parameters()),
        );
        if let Some(selected) = self.native_queries.native_iteration_calls.borrow().get(&key) {
            return selected.clone();
        }
        let selected = self.select_rust_iteration_call(expression, owner, trait_path, method);
        self.native_queries
            .native_iteration_calls
            .borrow_mut()
            .insert(key, selected.clone());
        selected
    }

    /// Select one native protocol call for `rust_iteration_call`, which caches the selection per owner.
    pub(super) fn select_rust_iteration_call(
        &self,
        expression: &Spanned<Expr>,
        owner: &ResolvedType,
        trait_path: &str,
        method: &str,
    ) -> Option<crate::typechecker::type_info::RustCheckedCall> {
        let path = self.rust_canonical_path_for_receiver_type(owner)?;
        let metadata = self.rust_item_metadata_for_path(&path)?;
        let RustItemKind::Type(info) = &metadata.kind else {
            return None;
        };
        let call = RustPathMethodCall {
            rust_path: &path,
            method,
            receiver_metadata: Some(&metadata),
            receiver_binding: None,
            receiver_expr: expression,
            type_args: &[],
            args: &[],
            arg_types: &[],
            receiver_span: expression.span,
            expected_return_ty: None,
            span: expression.span,
        };
        let mut candidates = self.retained_rust_trait_candidates(&call, info, owner, Some(trait_path), true);
        if crate::trace_flags::trace_rust_facts() {
            eprintln!(
                "native iteration: {owner:?}, {trait_path}::{method}, candidates {:?}",
                candidates
                    .iter()
                    .map(|candidate| (
                        &candidate.arguments,
                        candidate.borrow_cost,
                        &candidate.signature.return_type
                    ))
                    .collect::<Vec<_>>()
            );
        }
        candidates.sort_by_key(|candidate| candidate.borrow_cost);
        let selected = candidates.first()?;
        if candidates.iter().skip(1).any(|other| {
            other.borrow_cost == selected.borrow_cost
                && (other.path != selected.path || other.arguments != selected.arguments)
        }) {
            return None;
        }
        let selected = candidates.remove(0);
        if selected.signature.params.len() != 1 || !Self::rust_signature_has_receiver(&selected.signature) {
            return None;
        }
        Some(crate::typechecker::type_info::RustCheckedCall {
            storage: None,
            source_type_parameters: self.native_call_type_parameters(),
            receiver_adjustments: Vec::new(),
            path: selected.path,
            type_arguments: selected.arguments,
            parameters: vec![self.checked_rust_receiver_parameter(&selected.signature, &selected.self_type)?],
            return_type: self.native_rust_signature_type(&selected.signature.return_type, &path),
            result_item_path: self.rust_result_item_path(&selected.signature.return_type, &path),
            has_receiver: true,
        })
    }

    /// Retain a qualified method on a source type parameter from its declared native trait bound. The bound fixes
    /// the trait arguments, while the active source instance later closes Self; no concrete impl is guessed here.
    /// An explicit loan selects its referent's bound only when the native receiver is the same kind of Self loan.
    pub(in crate::typechecker::check_expr::access) fn retain_bound_rust_trait_call(
        &mut self,
        call: &RustTraitQualifiedCall<'_>,
        receiver: &Spanned<Expr>,
        source: &ResolvedType,
    ) -> Option<ResolvedType> {
        let original = call.trait_info.items.iter().find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == call.method => Some(signature),
            _ => None,
        })?;
        if original.is_async || original.is_unsafe || !original.type_params.is_empty() || !call.type_args.is_empty() {
            return None;
        }
        let borrowed_self = original.params.first().and_then(|parameter| {
            let display = Self::rust_display_without_lifetimes(&parameter.type_display);
            Self::rust_display_borrow_kind(&display)
                .and_then(|(mutable, inner)| matches!(inner.trim(), "Self" | "self").then_some(mutable))
        });
        let referent = match (source, borrowed_self) {
            (ResolvedType::Ref(inner), Some(false)) | (ResolvedType::RefMut(inner), Some(true)) => inner.as_ref(),
            _ => source,
        };
        let name = self.generic_placeholder_name(referent)?;
        let source = ResolvedType::TypeVar(name.to_owned());
        let identity = self.rust_trait_identity(call.rust_path);
        let mut selected = None;
        for bound in self.innermost_type_param_bounds(name)? {
            let path = if bound.name.contains("::") {
                Some(bound.name.trim_start_matches("::").to_owned())
            } else {
                self.rust_canonical_path_for_receiver_type(&ResolvedType::Named(bound.name.clone()))
            };
            if path.is_none_or(|path| self.rust_trait_identity(&path) != identity) {
                continue;
            }
            if bound.type_args.len() != call.trait_info.type_params.len()
                || selected.as_ref().is_some_and(|previous| previous != &bound.type_args)
            {
                return None;
            }
            selected = Some(bound.type_args.clone());
        }
        let arguments = selected?;
        let substitutions = call
            .trait_info
            .type_params
            .iter()
            .zip(&arguments)
            .map(|(name, ty)| (name.as_str(), render_resolved_type_as_rust_arg(ty)))
            .collect();
        let signature =
            Self::specialize_rust_signature(original, &substitutions, &render_resolved_type_as_rust_arg(&source));
        let lookup = RustPathMethodCall {
            rust_path: call.rust_path,
            method: call.method,
            receiver_metadata: None,
            receiver_binding: None,
            receiver_expr: receiver,
            type_args: call.type_args,
            args: &call.args[1..],
            arg_types: &call.arg_types[1..],
            receiver_span: receiver.span,
            expected_return_ty: None,
            span: call.span,
        };
        let before = self.errors.len();
        let result = self.validate_rust_method_call(
            call.rust_path,
            &signature,
            lookup.args,
            lookup.arg_types,
            false,
            call.span,
        );
        if self.errors.len() != before {
            return Some(result);
        }
        let mut native_arguments = vec![source.clone()];
        native_arguments.extend(arguments);
        self.retain_selected_rust_trait_call(
            &lookup,
            TraitCandidate {
                path: format!("{}::{}", call.rust_path, call.method),
                signature,
                arguments: native_arguments,
                self_type: source,
                borrow_cost: 0,
                returns_self: false,
                open_parameters: Vec::new(),
            },
        )?;
        self.type_info.rust.checked_calls.get_mut(&call.span)?.has_receiver = false;
        Some(result)
    }
}
