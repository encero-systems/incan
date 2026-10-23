//! Select retained trait callees after inherent lookup, using the extractor's impl templates and signatures.

use std::collections::HashMap;

use super::{RustPathMethodCall, RustTraitQualifiedCall, TypeChecker};
use crate::ast::{Expr, Spanned};
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type};
use crate::symbols::ResolvedType;
use crate::typechecker::helpers::render_resolved_type_as_rust_arg;
use crate::typechecker::{IdentKind, RustArgCoercionKind};
use incan_lang::interop::{RustFunctionSig, RustImplementedTrait, RustItemKind, RustTraitAssoc, RustTypeInfo};

/// A closed trait reference and the signature it contributes to one method call.
struct TraitCandidate {
    path: String,
    signature: RustFunctionSig,
    arguments: Vec<ResolvedType>,
    self_type: ResolvedType,
    borrow_cost: usize,
}

impl TypeChecker {
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
            (expression.span.start, expression.span.end),
            crate::typechecker::type_info::RustIterationHooks { iter, next },
        );
        Some(item)
    }

    /// Use the ordinary impl-template selector for one zero-argument native protocol call. No method spelling alone
    /// authorizes iteration: both a unique closed trait reference and its metadata signature are required.
    fn rust_iteration_call(
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
        let mut candidates = self.retained_rust_trait_candidates(&call, info, owner, Some(trait_path));
        if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
            eprintln!(
                "native iteration: {owner:?}, {trait_path}::{method}, candidates {}",
                candidates.len()
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
            receiver_adjustments: Vec::new(),
            path: selected.path,
            type_arguments: selected.arguments,
            parameters: vec![Self::checked_rust_receiver_parameter(
                &selected.signature,
                &selected.self_type,
            )?],
            return_type: self.native_rust_signature_type(&selected.signature.return_type, &path),
            result_item_path: self.rust_result_item_path(&selected.signature.return_type, &path),
            has_receiver: true,
        })
    }

    /// Retain a qualified method on a source type parameter from its declared native trait bound. The bound fixes
    /// the trait arguments, while the active source instance later closes Self; no concrete impl is guessed here.
    pub(super) fn retain_bound_rust_trait_call(
        &mut self,
        call: &RustTraitQualifiedCall<'_>,
        receiver: &Spanned<Expr>,
        source: &ResolvedType,
    ) -> Option<ResolvedType> {
        let name = self.generic_placeholder_name(source)?;
        let source = ResolvedType::TypeVar(name.to_owned());
        let original = call.trait_info.items.iter().find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == call.method => Some(signature),
            _ => None,
        })?;
        if original.is_async || original.is_unsafe || !original.type_params.is_empty() || !call.type_args.is_empty() {
            return None;
        }
        let mut selected = None;
        for frame in self.current_type_param_bound_details.iter().rev() {
            for bound in frame.get(name).into_iter().flatten() {
                let path = if bound.name.contains("::") {
                    Some(bound.name.trim_start_matches("::").to_owned())
                } else {
                    self.rust_canonical_path_for_receiver_type(&ResolvedType::Named(bound.name.clone()))
                };
                if path.as_deref().is_none_or(|path| {
                    incan_lang::interop::ancestral_rust_path(path)
                        != incan_lang::interop::ancestral_rust_path(call.rust_path)
                }) {
                    continue;
                }
                if bound.type_args.len() != call.trait_info.type_params.len()
                    || selected.as_ref().is_some_and(|previous| previous != &bound.type_args)
                {
                    return None;
                }
                selected = Some(bound.type_args.clone());
            }
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
            },
        )?;
        self.type_info
            .rust
            .checked_calls
            .get_mut(&(call.span.start, call.span.end))?
            .has_receiver = false;
        Some(result)
    }

    /// Keep a metadata-selected result item's identity before source scalar and collection projection.
    pub(in crate::typechecker::check_expr) fn rust_result_item_path(
        &self,
        display: &str,
        owner: &str,
    ) -> Option<String> {
        let display = self.rust_display_for_owner_path(display, owner);
        let display = Self::rust_display_without_lifetimes(&display);
        let referent = Self::rust_display_borrow_kind(&display).map_or(display.as_str(), |(_, inner)| inner);
        let base = Self::rust_generic_base_and_args(referent).map_or(referent, |(base, _)| base);
        let native = incan_lang::interop::ancestral_rust_path(base);
        let path = if native.contains("::") {
            native
        } else {
            self.unique_imported_rust_item_path_for_display_tail(base)?
        };
        let metadata = self.rust_item_metadata_for_path(&path)?;
        matches!(metadata.kind, RustItemKind::Type(_)).then_some(metadata.canonical_path)
    }

    /// Read a native value's retained item from its public field, call or canonical local binding, never from a method
    /// spelling.
    pub(in crate::typechecker::check_expr) fn rust_value_item_path(
        &self,
        expression: &Spanned<Expr>,
    ) -> Option<String> {
        if let Some(field) = self
            .type_info
            .rust
            .field_values
            .get(&(expression.span.start, expression.span.end))
        {
            return field.item_path.clone();
        }
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_value_item_path(inner);
        }
        if let Expr::Ident(name) = &expression.node {
            let symbol = self.symbols.lookup(name)?;
            return self
                .type_info
                .rust
                .value_item_paths
                .get(self.symbols.identity_of(symbol)?)
                .cloned();
        }
        self.type_info
            .rust
            .checked_calls
            .get(&(expression.span.start, expression.span.end))?
            .result_item_path
            .clone()
    }

    /// Preserve native result provenance through a new local or alias without changing its source type or storage.
    pub(in crate::typechecker) fn retain_rust_value_binding(&mut self, name: &str, expression: &Spanned<Expr>) {
        let path = self.rust_value_item_path(expression);
        let native = self.rust_native_value_type(expression);
        let Some(symbol) = self.symbols.lookup(name) else {
            return;
        };
        let Some(identity) = self.symbols.identity_of(symbol) else {
            return;
        };
        if let Some(path) = path {
            self.type_info.rust.value_item_paths.insert(identity.clone(), path);
        }
        if let Some(native) = native {
            self.type_info.rust.value_native_types.insert(identity.clone(), native);
        }
    }

    /// Recover the exact native carrier of a public Rust field, selected call or local alias for generic inference.
    pub(in crate::typechecker::check_expr) fn rust_native_value_type(
        &self,
        expression: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        if let Some(field) = self
            .type_info
            .rust
            .field_values
            .get(&(expression.span.start, expression.span.end))
        {
            return Some(field.native_type.clone());
        }
        if let Some(conversion) = self
            .type_info
            .rust
            .return_coercions
            .get(&(expression.span.start, expression.span.end))
        {
            return Some(conversion.target_type.clone());
        }
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_native_value_type(inner);
        }
        if let Expr::Ident(name) = &expression.node {
            let symbol = self.symbols.lookup(name)?;
            return self
                .type_info
                .rust
                .value_native_types
                .get(self.symbols.identity_of(symbol)?)
                .cloned();
        }
        Some(
            self.type_info
                .rust
                .checked_calls
                .get(&(expression.span.start, expression.span.end))?
                .return_type
                .clone(),
        )
    }

    /// Compare canonical native carriers for trait selection. The source compatibility relation deliberately admits
    /// opaque Rust paths, so it cannot distinguish competing trait instantiations such as From<char> and From<String>.
    pub(in crate::typechecker) fn rust_trait_carriers_match(&self, left: &ResolvedType, right: &ResolvedType) -> bool {
        self.rust_trait_comparison_type(left) == self.rust_trait_comparison_type(right)
    }

    /// Normalize native identities for comparison without rewriting the public paths retained for executable
    /// storage. Constructor aliases and leaf aliases use the same standard re-export identity table.
    fn rust_trait_comparison_type(&self, ty: &ResolvedType) -> ResolvedType {
        let ty = self.rust_inference_type(ty, &[]);
        map_resolved_type_leaves(&ty, &|leaf| {
            if let ResolvedType::Generic(base, arguments) = leaf {
                let base = if base.contains("::") {
                    incan_lang::interop::ancestral_rust_path(base)
                } else {
                    base.clone()
                };
                return Some(ResolvedType::Generic(
                    base,
                    arguments
                        .iter()
                        .map(|argument| self.rust_trait_comparison_type(argument))
                        .collect(),
                ));
            }
            let ResolvedType::RustPath(path) = leaf else {
                return None;
            };
            if incan_lang::interop::rust_display_is_owned_string(path) {
                return Some(ResolvedType::Str);
            }
            Some(ResolvedType::RustPath(incan_lang::interop::ancestral_rust_path(path)))
        })
    }

    /// Bind one impl's declared Self template to a concrete native owner before inspecting its members or projections.
    pub(super) fn rust_impl_self_bindings(
        &self,
        implementation: &RustImplementedTrait,
        owner: &ResolvedType,
    ) -> Option<HashMap<String, ResolvedType>> {
        let template = self.rust_inference_type(
            &self.native_rust_signature_type(&implementation.self_type, ""),
            &implementation.type_params,
        );
        let actual = self.rust_inference_type(owner, &[]);
        let mut bindings = HashMap::new();
        self.infer_type_param_bindings(&template, &actual, &mut bindings);
        if let Some((base, arguments)) = Self::rust_generic_base_and_args(&implementation.self_type)
            && let Some(metadata) = self.rust_item_metadata_for_path(base)
            && let RustItemKind::Type(info) = metadata.kind
        {
            for (index, argument) in arguments
                .iter()
                .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
                .enumerate()
            {
                if implementation.type_params.iter().any(|name| name == argument)
                    && !bindings.contains_key(*argument)
                    && let Some(default) = info.type_param_defaults.get(index).and_then(Option::as_deref)
                {
                    bindings.insert((*argument).to_owned(), self.native_rust_signature_type(default, base));
                }
            }
        }
        let closed = substitute_resolved_type(&template, &bindings);
        self.rust_trait_carriers_match(&closed, &actual).then_some(bindings)
    }

    /// Close omitted native owner defaults once, before matching either impl members or associated type templates.
    pub(super) fn rust_closed_owner(&self, ty: &ResolvedType, info: &RustTypeInfo, path: &str) -> ResolvedType {
        if matches!(ty, ResolvedType::Generic(base, _) if base == "[]") {
            return ty.clone();
        }
        let mut owner = match ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => *inner.clone(),
            other => other.clone(),
        };
        if let Some(mut arguments) = self.rust_owner_type_args(&owner, path)
            && self.complete_rust_owner_arguments(info, &mut arguments)
        {
            if arguments.is_empty() {
                return owner;
            }
            let arguments = arguments
                .iter()
                .map(render_resolved_type_as_rust_arg)
                .collect::<Vec<_>>()
                .join(", ");
            owner = self.resolved_type_from_rust_display(&format!("{path}<{arguments}>"));
        }
        owner
    }

    /// Keep checker inference owners in the shortest form rustc displays at constructors. Body IR completes native
    /// storage defaults separately from retained declaration facts, including defaults exposed by public field
    /// templates. Already-short owners retain structured source arguments rather than round-tripping their aliases
    /// and model identities through a display.
    pub(super) fn rust_storage_owner(&self, owner: &ResolvedType, info: &RustTypeInfo, path: &str) -> ResolvedType {
        match owner {
            ResolvedType::Ref(inner) => return ResolvedType::Ref(Box::new(self.rust_storage_owner(inner, info, path))),
            ResolvedType::RefMut(inner) => {
                return ResolvedType::RefMut(Box::new(self.rust_storage_owner(inner, info, path)));
            }
            _ => {}
        }
        let Some(arguments) = self.rust_owner_type_args(owner, path) else {
            return owner.clone();
        };
        let original_arity = arguments.len();
        let arguments = self.rust_storage_owner_arguments(arguments, info, path);
        if arguments.len() == original_arity {
            return owner.clone();
        }
        if arguments.is_empty() {
            if info.type_params.is_empty() {
                return owner.clone();
            }
            return self.resolved_type_from_rust_display(path);
        }
        let arguments = arguments
            .iter()
            .map(render_resolved_type_as_rust_arg)
            .collect::<Vec<_>>()
            .join(", ");
        self.resolved_type_from_rust_display(&format!("{path}<{arguments}>"))
    }

    /// Remove only trailing arguments equal to the native declaration's own defaults. Both storage identity and
    /// structural inference use this boundary, including native owners nested inside another generic owner.
    pub(in crate::typechecker) fn rust_storage_owner_arguments(
        &self,
        mut arguments: Vec<ResolvedType>,
        info: &RustTypeInfo,
        path: &str,
    ) -> Vec<ResolvedType> {
        while let Some(index) = arguments.len().checked_sub(1) {
            let Some(default) = info.type_param_defaults.get(index).and_then(Option::as_deref) else {
                break;
            };
            let displays = arguments[..index]
                .iter()
                .map(render_resolved_type_as_rust_arg)
                .collect::<Vec<_>>();
            let displays = displays.iter().map(String::as_str).collect::<Vec<_>>();
            let default = Self::substitute_rust_display_params(default, &info.type_params[..index], &displays);
            if !self.rust_trait_carriers_match(&arguments[index], &self.native_rust_signature_type(&default, path)) {
                break;
            }
            arguments.pop();
        }
        arguments
    }

    /// Recognize only exact owned-text and vector views admitted at native borrowed parameters. Element equality
    /// uses native carrier comparison, so this does not widen trait selection to permissive opaque path compatibility.
    fn rust_borrowed_view_matches(&self, target: &ResolvedType, argument: &ResolvedType) -> bool {
        if target == &ResolvedType::RustPath("str".into()) && argument == &ResolvedType::Str {
            return true;
        }
        let ResolvedType::Generic(base, elements) = target else {
            return false;
        };
        let ResolvedType::Generic(owner, arguments) = self.rust_inference_type(argument, &[]) else {
            return false;
        };
        base == "[]"
            && owner == "List"
            && elements.len() == 1
            && arguments.len() == 1
            && self.rust_trait_carriers_match(&elements[0], &arguments[0])
    }

    /// Resolve one trait method only when its self template, inputs, and expected output close every native binder.
    /// Impl predicates remain native obligations: this selects an exact trait reference, and the adapter verifies
    /// that rustc can satisfy it before emitting MIR. Competing equally ranked trait references remain ambiguous.
    fn retained_trait_candidate(
        &self,
        implementation: &RustImplementedTrait,
        call: &RustPathMethodCall<'_>,
        owner: &ResolvedType,
        info: &RustTypeInfo,
    ) -> Option<TraitCandidate> {
        let metadata = self.rust_item_metadata_for_path(&implementation.path)?;
        let RustItemKind::Trait(trait_info) = metadata.kind else {
            return None;
        };
        if !trait_info.is_in_prelude
            && !self.type_info.rust.trait_imports.values().any(|import| {
                import.trait_path == implementation.path
                    || import.definition_path.as_deref() == Some(implementation.path.as_str())
            })
        {
            return None;
        }
        let original = trait_info.items.iter().find_map(|item| match item {
            RustTraitAssoc::Function { name, signature } if name == call.method => Some(signature),
            _ => None,
        })?;
        if original.is_async || original.is_unsafe {
            return None;
        }
        let mut variables = implementation.type_params.clone();
        variables.extend(original.type_params.iter().cloned());
        let mut bindings = self.rust_impl_self_bindings(implementation, owner)?;
        if !call.type_args.is_empty() {
            let explicit = self
                .type_info
                .calls
                .call_site_monomorph_type_args
                .get(&(call.span.start, call.span.end))?;
            if explicit.len() != original.type_params.len() {
                return None;
            }
            bindings.extend(original.type_params.iter().cloned().zip(explicit.iter().cloned()));
        }
        let substitutions = trait_info
            .type_params
            .iter()
            .zip(&implementation.type_arguments)
            .map(|(name, argument)| (name.as_str(), argument.clone()))
            .collect();
        let storage = self.rust_storage_owner(owner, info, call.rust_path);
        let owner_display = if storage == ResolvedType::Str {
            call.rust_path.to_owned()
        } else {
            render_resolved_type_as_rust_arg(&storage)
        };
        let signature = Self::specialize_rust_signature(original, &substitutions, &owner_display);
        let inferred = self.infer_rust_signature_bindings(
            &signature,
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
        let displays = bindings
            .iter()
            .map(|(name, value)| (name.as_str(), render_resolved_type_as_rust_arg(value)))
            .collect::<HashMap<_, _>>();
        let mut closed = displays.clone();
        for (name, template) in &substitutions {
            closed.insert(
                *name,
                Self::substitute_rust_owner_display(template, &displays, &owner_display),
            );
        }
        // Apply the final bindings to the original signature once: Self's inserted owner may contain caller binders
        // with the same spelling as an impl parameter, and must not be substituted again as an open template.
        let signature =
            self.rust_signature_projections(Self::specialize_rust_signature(original, &closed, &owner_display));
        let borrow_cost = self.rust_trait_input_borrow_cost(&signature, call)?;
        let mut arguments = vec![storage.clone()];
        for display in &implementation.type_arguments {
            let template = self.native_rust_signature_type(display, call.rust_path);
            arguments.push(self.instantiate_rust_signature_type(&template, &variables, &bindings));
        }
        for parameter in &original.type_params {
            arguments.push(bindings.get(parameter)?.clone());
        }
        if variables.iter().any(|name| {
            bindings
                .get(name)
                .is_none_or(|ty| matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
        }) {
            return None;
        }
        Some(TraitCandidate {
            path: format!("{}::{}", metadata.canonical_path, call.method),
            signature,
            arguments,
            self_type: storage,
            borrow_cost,
        })
    }

    /// Match a closed native input signature using exact carriers and the already admitted borrowed views. The cost
    /// ranks receiver-compatible candidates; it grants no new conversion or trait implementation.
    fn rust_trait_input_borrow_cost(
        &self,
        signature: &RustFunctionSig,
        call: &RustPathMethodCall<'_>,
    ) -> Option<usize> {
        let ordinary = if Self::rust_signature_has_receiver(signature) {
            &signature.params[1..]
        } else {
            &signature.params
        };
        if ordinary.len() != call.arg_types.len() {
            return None;
        }
        let mut cost = 0;
        for (parameter, argument) in ordinary.iter().zip(call.arg_types) {
            let target = self.native_rust_signature_type(&parameter.type_display, call.rust_path);
            let target = match &target {
                ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => {
                    cost += 1;
                    inner.as_ref()
                }
                _ => &target,
            };
            let view =
                parameter.type_display.trim().starts_with('&') && self.rust_borrowed_view_matches(target, argument);
            if !view && !self.rust_trait_carriers_match(target, argument) {
                return None;
            }
        }
        Some(cost)
    }

    /// Select the unique best trait callee after inherent methods, retaining its exact trait path and Self argument.
    pub(super) fn resolve_retained_rust_trait_call(
        &mut self,
        call: RustPathMethodCall<'_>,
        info: &RustTypeInfo,
    ) -> Option<ResolvedType> {
        let value_receiver = !matches!(
            self.type_info.ident_kind(call.receiver_span),
            Some(IdentKind::TypeName | IdentKind::RustImport)
        );
        let owner = if value_receiver {
            self.type_info.expr_type(call.receiver_span)?.clone()
        } else {
            self.resolved_type_from_rust_display(call.rust_path)
        };
        self.resolve_retained_rust_trait_call_on_owner(call, info, &owner, value_receiver, None)
    }

    /// Resolve a trait call on an already selected native owner, including a checked dereference target. A qualified
    /// call restricts candidates to its named trait; ordinary value lookup considers all imported and prelude traits.
    pub(super) fn resolve_retained_rust_trait_call_on_owner(
        &mut self,
        call: RustPathMethodCall<'_>,
        info: &RustTypeInfo,
        owner: &ResolvedType,
        value_receiver: bool,
        trait_path: Option<&str>,
    ) -> Option<ResolvedType> {
        if !call.type_args.is_empty() {
            let explicit = call
                .type_args
                .iter()
                .map(|argument| self.resolve_type_checked(argument))
                .collect();
            self.type_info
                .calls
                .call_site_monomorph_type_args
                .insert((call.span.start, call.span.end), explicit);
        }
        let mut candidates = self.retained_rust_trait_candidates(&call, info, owner, trait_path);
        if std::env::var_os("INCAN_TRACE_RUST_FACTS").is_some() {
            eprintln!(
                "rust trait call: {}.{} owner {owner:?}, {} impls, candidates {:?}",
                call.rust_path,
                call.method,
                info.implemented_traits.len(),
                candidates
                    .iter()
                    .map(|candidate| (&candidate.path, &candidate.arguments))
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
        let has_receiver = Self::rust_signature_has_receiver(&selected.signature);
        if has_receiver != value_receiver {
            return None;
        }
        if has_receiver
            && selected.signature.params.first().is_some_and(|parameter| {
                Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
            })
        {
            self.refuse_write_through_read_only_binding(call.receiver_expr, call.span);
            self.validate_rust_borrow_mutability(RustArgCoercionKind::Borrow { mutable: true }, call.receiver_expr);
        }
        let errors_before = self.errors.len();
        let result = self.validate_rust_method_call(
            &selected.path,
            &selected.signature,
            call.args,
            call.arg_types,
            false,
            call.span,
        );
        if self.errors.len() != errors_before {
            return Some(result);
        }
        self.retain_selected_rust_trait_call(&call, selected)?;
        Some(result)
    }

    /// Match native value and reference impl templates before adding an automatic receiver borrow. A consuming
    /// method on an exclusive reference takes that reference; a borrowing method may directly reborrow its referent.
    fn retained_rust_trait_candidates(
        &self,
        call: &RustPathMethodCall<'_>,
        info: &RustTypeInfo,
        source: &ResolvedType,
        trait_path: Option<&str>,
    ) -> Vec<TraitCandidate> {
        let mut candidates = Vec::new();
        let imported = self.rust_imported_trait_templates(info, trait_path);
        let mut current = source;
        loop {
            let owner = self.rust_trait_owner(current, info, call.rust_path);
            for (implementation, external) in info
                .implemented_traits
                .iter()
                .map(|implementation| (implementation, false))
                .chain(imported.iter().map(|implementation| (implementation, true)))
            {
                if trait_path.is_some_and(|path| {
                    incan_lang::interop::ancestral_rust_path(path)
                        != incan_lang::interop::ancestral_rust_path(&implementation.path)
                }) {
                    continue;
                }
                if external && !self.rust_imported_impl_may_apply(implementation, &owner) {
                    continue;
                }
                let Some(mut candidate) = self.retained_trait_candidate(implementation, call, &owner, info) else {
                    continue;
                };
                if Self::rust_signature_has_receiver(&candidate.signature) {
                    let Some(receiver) =
                        Self::checked_rust_receiver_parameter(&candidate.signature, &candidate.self_type)
                    else {
                        continue;
                    };
                    if !self.rust_trait_carriers_match(&receiver.ty, source) {
                        candidate.borrow_cost += 1;
                    }
                }
                candidates.push(candidate);
            }
            match current {
                ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => current = inner,
                _ => break,
            }
        }
        candidates
    }

    /// Read extension-trait templates from their own ABI when the implementor's older ABI cannot contain that trait.
    /// Existing owner facts retain priority; importing a trait never rewrites the cached native type's answer.
    fn rust_imported_trait_templates(&self, info: &RustTypeInfo, selected: Option<&str>) -> Vec<RustImplementedTrait> {
        let mut templates = Vec::new();
        for import in self.type_info.rust.trait_imports.values() {
            let identity = incan_lang::interop::ancestral_rust_path(&import.trait_path);
            if selected.is_some_and(|selected| incan_lang::interop::ancestral_rust_path(selected) != identity)
                || info
                    .implemented_traits
                    .iter()
                    .any(|implementation| incan_lang::interop::ancestral_rust_path(&implementation.path) == identity)
            {
                continue;
            }
            let Some(metadata) = self.rust_item_metadata_for_path(&import.trait_path) else {
                continue;
            };
            let RustItemKind::Trait(trait_info) = metadata.kind else {
                continue;
            };
            templates.extend(trait_info.implementations);
        }
        templates
    }

    /// Eliminate an extension blanket candidate when a callable trait condition has no matching native Self
    /// template. This is eligibility filtering, not proof: marker predicates, nested conditions and all complete
    /// native obligations remain with rustc before MIR. In particular, a guard must reach its checked Deref target
    /// instead of winning an inapplicable reader or writer extension merely because its impl binds an arbitrary T.
    fn rust_imported_impl_may_apply(&self, implementation: &RustImplementedTrait, owner: &ResolvedType) -> bool {
        let Some(bindings) = self.rust_impl_self_bindings(implementation, owner) else {
            return false;
        };
        for bound in &implementation.bounds {
            let Some(metadata) = self.rust_item_metadata_for_path(&bound.path) else {
                return false;
            };
            let RustItemKind::Trait(required) = metadata.kind else {
                return false;
            };
            if !required
                .items
                .iter()
                .any(|item| matches!(item, RustTraitAssoc::Function { .. }))
            {
                continue;
            }
            let actual = self.instantiate_rust_signature_type(
                &self.native_rust_signature_type(&bound.self_type, ""),
                &implementation.type_params,
                &bindings,
            );
            let Some(path) = self.rust_canonical_path_for_receiver_type(&actual) else {
                return false;
            };
            let Some(metadata) = self.rust_item_metadata_for_path(&path) else {
                return false;
            };
            let RustItemKind::Type(info) = metadata.kind else {
                return false;
            };
            let actual = self.rust_closed_owner(&actual, &info, &path);
            let identity = incan_lang::interop::ancestral_rust_path(&bound.path);
            if !info
                .implemented_traits
                .iter()
                .chain(required.implementations.iter())
                .any(|candidate| {
                    incan_lang::interop::ancestral_rust_path(&candidate.path) == identity
                        && self.rust_impl_self_bindings(candidate, &actual).is_some()
                })
            {
                return false;
            }
        }
        true
    }

    /// Fill metadata defaults inside a native reference without erasing that reference's identity as trait Self.
    fn rust_trait_owner(&self, owner: &ResolvedType, info: &RustTypeInfo, path: &str) -> ResolvedType {
        match owner {
            ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(self.rust_trait_owner(inner, info, path))),
            ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(self.rust_trait_owner(inner, info, path))),
            _ => self.rust_closed_owner(owner, info, path),
        }
    }

    /// Project a closed trait reference into the same checked external-call record as inherent and free callees.
    fn retain_selected_rust_trait_call(
        &mut self,
        call: &RustPathMethodCall<'_>,
        selected: TraitCandidate,
    ) -> Option<()> {
        let has_receiver = Self::rust_signature_has_receiver(&selected.signature);
        let mut parameters = self
            .type_info
            .call_site_callable_params(call.span)
            .unwrap_or(&[])
            .to_vec();
        let ordinary = if has_receiver {
            &selected.signature.params[1..]
        } else {
            &selected.signature.params
        };
        for (parameter, declared) in parameters.iter_mut().zip(ordinary) {
            parameter.ty = self.native_rust_signature_type(&declared.type_display, call.rust_path);
        }
        if has_receiver {
            parameters.insert(
                0,
                Self::checked_rust_receiver_parameter(&selected.signature, &selected.self_type)?,
            );
        }
        self.type_info.rust.checked_calls.insert(
            (call.span.start, call.span.end),
            crate::typechecker::type_info::RustCheckedCall {
                receiver_adjustments: Vec::new(),
                path: selected.path,
                type_arguments: selected.arguments,
                parameters,
                return_type: self.native_rust_signature_type(&selected.signature.return_type, call.rust_path),
                result_item_path: self.rust_result_item_path(&selected.signature.return_type, call.rust_path),
                has_receiver,
            },
        );
        Some(())
    }
}
