//! Retained Rust trait candidates, trait method selection, and retained trait call resolution.

use super::*;

impl TypeChecker {
    /// Keep impl and method binders in distinct scopes when both declarations use the same source name.
    /// Renaming only the method signature preserves native argument order and leaves imported metadata unchanged.
    pub(super) fn disjoint_rust_method_binders(
        signature: &RustFunctionSig,
        implementation: &RustImplementedTrait,
        trait_parameters: &[String],
    ) -> Option<RustFunctionSig> {
        let mut substitutions = HashMap::new();
        for (index, parameter) in signature.type_params.iter().enumerate() {
            if !implementation.type_params.contains(parameter) {
                continue;
            }
            let mut name = format!("__incan_method_{index}");
            while implementation.type_params.contains(&name)
                || signature.type_params.contains(&name)
                || trait_parameters.contains(&name)
            {
                name.push('_');
            }
            substitutions.insert(parameter.as_str(), name);
        }
        if substitutions.is_empty() {
            return None;
        }
        let mut scoped = Self::specialize_rust_signature(signature, &substitutions, "Self");
        scoped.type_params = signature
            .type_params
            .iter()
            .map(|name| substitutions.get(name.as_str()).unwrap_or(name).clone())
            .collect();
        Some(scoped)
    }

    /// Resolve one trait method only when its self template, inputs, and expected output close every impl binder.
    /// A method's own result parameters may stay open for the local's first typed use, recorded explicitly so an
    /// equally named source binder cannot supply an accidental binding.
    /// Impl predicates remain native obligations: this selects an exact trait reference, and the adapter verifies
    /// that rustc can satisfy it before emitting MIR. A method bound whose closed associated equality contradicts the
    /// value its subject supplies excludes the reference; other competing equally ranked references remain ambiguous.
    pub(super) fn retained_trait_candidate(
        &self,
        implementation: &RustImplementedTrait,
        call: &RustPathMethodCall<'_>,
        owner: &ResolvedType,
        info: &RustTypeInfo,
    ) -> Option<TraitCandidate> {
        let (metadata, original) = self.retained_trait_method(implementation, call)?;
        let RustItemKind::Trait(trait_info) = &metadata.kind else {
            return None;
        };
        let trait_path = &metadata.canonical_path;
        let original = &original;
        let mut variables = implementation.type_params.clone();
        variables.extend(original.type_params.iter().cloned());
        let open_owner = self.rust_owner_is_open(owner, info, call.rust_path);
        let mut bindings = if open_owner {
            let mut defaults = HashMap::new();
            self.insert_rust_impl_default_bindings(implementation, &mut defaults);
            defaults
        } else {
            self.rust_impl_self_bindings(implementation, owner)?
        };
        if !call.type_args.is_empty() {
            let explicit = self.type_info.calls.call_site_monomorph_type_args.get(&call.span)?;
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
        let mut storage = self.rust_storage_owner(owner, info, call.rust_path);
        // An open owner stands for the impl's own Self template until the call's arguments close its binders.
        let inference_owner = if open_owner {
            implementation.self_type.clone()
        } else {
            Self::rust_owner_display(&storage, call.rust_path)
        };
        let signature = Self::specialize_rust_signature(original, &substitutions, &inference_owner);
        variables.extend(Self::rust_signature_const_parameters(&signature));
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
        self.infer_rust_bound_bindings(&implementation.bounds, &variables, &mut bindings);
        if open_owner {
            let template = self.native_rust_signature_type(&implementation.self_type, "");
            let closed = self.instantiate_rust_signature_type(&template, &variables, &bindings);
            storage = self.rust_storage_owner(&closed, info, call.rust_path);
        }
        if !self.rust_associated_equalities_hold(&signature, &variables, &bindings) {
            return None;
        }
        let owner_display = Self::rust_owner_display(&storage, call.rust_path);
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
            arguments.push(
                bindings
                    .get(parameter)
                    .cloned()
                    .unwrap_or_else(|| ResolvedType::TypeVar(parameter.clone())),
            );
        }
        if variables.iter().any(|name| {
            !original.type_params.contains(name)
                && bindings
                    .get(name)
                    .is_none_or(|ty| matches!(ty, ResolvedType::Unknown | ResolvedType::CallSiteInfer))
        }) {
            return None;
        }
        Some(TraitCandidate {
            path: format!("{trait_path}::{}", call.method),
            signature,
            arguments,
            self_type: storage,
            borrow_cost,
            returns_self: original.return_type.trim() == "Self",
            open_parameters: self.open_rust_trait_method_parameters(&original.type_params, &bindings),
        })
    }

    /// Keep unresolved method binders explicit, so an unbound `S` never resolves by matching a source binder's name.
    pub(super) fn open_rust_trait_method_parameters(
        &self,
        parameters: &[String],
        bindings: &HashMap<String, ResolvedType>,
    ) -> Vec<String> {
        let source = self.native_call_type_parameters();
        parameters
            .iter()
            .filter(|name| {
                bindings.get(*name).is_none_or(|argument| {
                    matches!(argument, ResolvedType::Unknown | ResolvedType::CallSiteInfer)
                        || Self::mentions_foreign_type_variable(argument, &source)
                })
            })
            .cloned()
            .collect()
    }

    /// Whether each associated equality a candidate's method bounds state agrees with the value its bound subject
    /// supplies: `Extend<&T>` requires `I: IntoIterator<Item = &T>`, which a vector of owned items contradicts. An
    /// equality that is still open or whose value no impl supplies decides nothing; rustc verifies the rest.
    pub(super) fn rust_associated_equalities_hold(
        &self,
        signature: &RustFunctionSig,
        variables: &[String],
        bindings: &HashMap<String, ResolvedType>,
    ) -> bool {
        let closed = |display: &str| {
            let template = self.native_rust_signature_type(display, "");
            let ty = self.instantiate_rust_signature_type(&template, variables, bindings);
            (!Self::contains_unbound_rust_type_var(&ty)).then_some(ty)
        };
        signature.bounds.iter().all(|bound| {
            let Some(subject) = closed(&bound.self_type) else {
                return true;
            };
            bound.associated_type_bindings.iter().all(|binding| {
                let Some(expected) = closed(&binding.value_path) else {
                    return true;
                };
                self.rust_binding_associated_type(&subject, &bound.path, &binding.name)
                    .is_none_or(|actual| self.rust_trait_carriers_match(&expected, &actual))
            })
        })
    }

    /// The trait declaration behind one impl, when `call.method` is a synchronous safe member of a trait the program
    /// can name (prelude or imported). Its method binders are renamed apart from the impl's own binders.
    pub(super) fn retained_trait_method(
        &self,
        implementation: &RustImplementedTrait,
        call: &RustPathMethodCall<'_>,
    ) -> Option<(Arc<RustItemMetadata>, RustFunctionSig)> {
        let metadata = self.rust_item_metadata_for_path(&implementation.path)?;
        let RustItemKind::Trait(trait_info) = &metadata.kind else {
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
        let scoped = Self::disjoint_rust_method_binders(original, implementation, &trait_info.type_params);
        let original = scoped.unwrap_or_else(|| original.clone());
        Some((Arc::clone(&metadata), original))
    }

    /// The Rust spelling of a trait candidate's Self owner; an owned string keeps the call's written owner path.
    pub(super) fn rust_owner_display(storage: &ResolvedType, path: &str) -> String {
        if *storage == ResolvedType::Str {
            path.to_owned()
        } else {
            render_resolved_type_as_rust_arg(storage)
        }
    }

    /// Match a closed native input signature using exact carriers and admitted input adjustments. Exact carriers
    /// rank before automatic borrows or fitting integer contexts for unsuffixed literals. Final validation records
    /// each selected argument's contextual type; this ranking grants no new conversion or trait implementation.
    pub(super) fn rust_trait_input_borrow_cost(
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
        for ((parameter, argument), expression) in ordinary.iter().zip(call.arg_types).zip(call.args) {
            let native = self.rust_argument_native_type(Self::call_arg_expr(expression));
            let argument = native.as_ref().unwrap_or(argument);
            let target = self.native_rust_signature_type(&parameter.type_display, call.rust_path);
            let expression = Self::call_arg_expr(expression);
            if self.rust_trait_carriers_match(&target, argument) {
                continue;
            }
            if contextual_integer_literal_fits(expression, &target) {
                cost += 1;
                continue;
            }
            let (target, exclusive) = match &target {
                ResolvedType::Ref(inner) => {
                    cost += 1;
                    (inner.as_ref(), Some(false))
                }
                ResolvedType::RefMut(inner) => {
                    cost += 1;
                    (inner.as_ref(), Some(true))
                }
                _ => (&target, None),
            };
            // A loan argument offers its referent's view as an owner does: `&Vec<u8>` reaches `&[u8]` like `Vec<u8>`.
            // A shared parameter views either loan; an exclusive parameter views only an exclusive one.
            let lent = match (exclusive, argument) {
                (Some(false), ResolvedType::Ref(referent) | ResolvedType::RefMut(referent))
                | (Some(true), ResolvedType::RefMut(referent)) => Some(referent.as_ref()),
                _ => None,
            };
            let view = parameter.type_display.trim().starts_with('&')
                && std::iter::once(argument)
                    .chain(lent)
                    .any(|offered| self.rust_borrowed_view_matches(target, offered));
            if !view && !self.rust_trait_carriers_match(target, argument) {
                return None;
            }
        }
        Some(cost)
    }

    /// Select the unique best trait callee after inherent methods, retaining its exact trait path and Self argument.
    pub(in crate::typechecker::check_expr::access) fn resolve_retained_rust_trait_call(
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
            self.type_info
                .expr_type(call.receiver_span)
                .and_then(|receiver| self.rusttype_native_target(receiver))
                .unwrap_or_else(|| self.resolved_type_from_rust_display(call.rust_path))
        };
        self.resolve_retained_rust_trait_call_on_owner(call, info, &owner, value_receiver, None)
    }

    /// Resolve a trait call on an already selected native owner, including a checked dereference target. A qualified
    /// call restricts candidates to its named trait; ordinary value lookup considers all imported and prelude traits.
    pub(in crate::typechecker::check_expr::access) fn resolve_retained_rust_trait_call_on_owner(
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
                .insert(call.span, explicit);
        }
        let mut candidates = self.retained_rust_trait_candidates(&call, info, owner, trait_path, false);
        if crate::trace_flags::trace_rust_facts() {
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
            self.record_tied_rust_trait_callees(call.span, &candidates);
            return None;
        }
        let selected = candidates.remove(0);
        let has_receiver = Self::rust_signature_has_receiver(&selected.signature);
        if has_receiver != value_receiver {
            return None;
        }
        if let Some(result) = self.watch_open_rust_trait_result(&call, &selected) {
            return Some(result);
        }
        if has_receiver
            && selected.signature.params.first().is_some_and(|parameter| {
                Self::rust_display_borrow_kind(&parameter.type_display).is_some_and(|(mutable, _)| mutable)
            })
        {
            self.validate_exclusive_rust_receiver(call.receiver_expr, call.span);
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
        let returns_self = selected.returns_self;
        self.retain_selected_rust_trait_call(&call, selected)?;
        if returns_self && let Some(rusttype) = self.rusttype_associated_receiver(&call) {
            return Some(rusttype);
        }
        Some(self.native_borrowed_source_result(call.span, result))
    }

    /// Watch a uniquely selected method's own open result parameters using the associated-call first-use rule.
    /// Impl binders are already fixed by candidate selection. No executable callee is retained until a typed use
    /// rechecks this initializer with a closed destination; an unused or unconstrained result is refused normally.
    pub(super) fn watch_open_rust_trait_result(
        &mut self,
        call: &RustPathMethodCall<'_>,
        selected: &TraitCandidate,
    ) -> Option<ResolvedType> {
        let parameters = &selected.signature.type_params;
        let open = &selected.open_parameters;
        if open.is_empty() {
            return None;
        }
        let result = self.rust_inference_type(
            &self.native_rust_signature_type(&selected.signature.return_type, call.rust_path),
            parameters,
        );
        if !Self::native_result_carries_parameters(&result, open) {
            self.errors.push(crate::diagnostics::errors::rust_callee_not_selected(
                &format!("rust::{}.{}", call.rust_path, call.method),
                crate::diagnostics::errors::NativeCalleeGap::OpenTypeParameters(open),
                call.span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.open_rust_generic_calls.insert(
            call.span,
            crate::typechecker::OpenRustGenericCall {
                owner: call.rust_path.to_owned(),
                method: call.method.to_owned(),
                type_params: open.clone(),
                owner_parameters: false,
                native: Some(result.clone()),
                native_item: None,
            },
        );
        Some(result)
    }

    /// Retain the equally ranked trait callees a call could not choose between, each as its trait with the trait's
    /// arguments, for the refusal of a call that no later step selects. The first tie a call records is kept.
    pub(super) fn record_tied_rust_trait_callees(&mut self, span: crate::ast::Span, candidates: &[TraitCandidate]) {
        let Some(best) = candidates.first().map(|candidate| candidate.borrow_cost) else {
            return;
        };
        let mut tied: Vec<String> = Vec::new();
        for candidate in candidates.iter().take_while(|candidate| candidate.borrow_cost == best) {
            // Arguments hold Self, then the trait's arguments, then the method's own type arguments.
            let trait_arguments = candidate
                .arguments
                .len()
                .saturating_sub(1 + candidate.signature.type_params.len());
            let arguments = candidate
                .arguments
                .iter()
                .skip(1)
                .take(trait_arguments)
                .map(render_resolved_type_as_rust_arg)
                .collect::<Vec<_>>();
            let display = match candidate.path.rsplit_once("::") {
                Some((trait_path, method)) if !arguments.is_empty() => {
                    format!("{trait_path}<{}>::{method}", arguments.join(", "))
                }
                _ => candidate.path.clone(),
            };
            if !tied.contains(&display) {
                tied.push(display);
            }
        }
        self.tied_rust_trait_callees.entry(span).or_insert(tied);
    }

    /// The rusttype an associated function is called on (`Name.from(...)` over a Rust String), which keeps that
    /// identity for a result the Rust signature declares as `Self`.
    pub(in crate::typechecker::check_expr::access) fn rusttype_associated_receiver(
        &self,
        call: &RustPathMethodCall<'_>,
    ) -> Option<ResolvedType> {
        if !matches!(
            self.type_info.ident_kind(call.receiver_span),
            Some(IdentKind::TypeName | IdentKind::RustImport)
        ) {
            return None;
        }
        let receiver = self.type_info.expr_type(call.receiver_span)?;
        self.rusttype_native_target(receiver)?;
        Some(receiver.clone())
    }

    /// Match native value and reference impl templates before adding an automatic receiver borrow. A consuming
    /// method on an exclusive reference takes that reference; a borrowing method may directly reborrow its referent.
    /// A referent reached through a shared reference is never lent exclusively, so its `&mut self` methods are no
    /// candidates there: `Read for &UnixStream` serves a shared socket, `Read for UnixStream` does not.
    /// Iteration filters callable bounds before selecting its item; ordinary native calls leave full obligations to
    /// rustc, including source collection views whose native implementations are not nominal checker owners.
    pub(super) fn retained_rust_trait_candidates(
        &self,
        call: &RustPathMethodCall<'_>,
        info: &RustTypeInfo,
        source: &ResolvedType,
        trait_path: Option<&str>,
        filter_callable_bounds: bool,
    ) -> Vec<TraitCandidate> {
        let mut candidates = Vec::new();
        let selected = trait_path.map(|path| self.rust_trait_identity(path));
        let imported = self.rust_imported_trait_templates(info, trait_path);
        let mut current = source;
        let mut behind_shared_reference = false;
        loop {
            let owner = self.rust_trait_owner(current, info, call.rust_path);
            for (implementation, external) in info
                .implemented_traits
                .iter()
                .map(|implementation| (implementation, false))
                .chain(imported.iter().map(|implementation| (implementation, true)))
            {
                if selected
                    .as_ref()
                    .is_some_and(|path| *path != self.rust_trait_identity(&implementation.path))
                {
                    continue;
                }
                if (external || filter_callable_bounds) && !self.rust_impl_may_apply(implementation, &owner) {
                    continue;
                }
                let Some(mut candidate) = self.retained_trait_candidate(implementation, call, &owner, info) else {
                    continue;
                };
                if Self::rust_signature_has_receiver(&candidate.signature) {
                    let Some(receiver) =
                        self.checked_rust_receiver_parameter(&candidate.signature, &candidate.self_type)
                    else {
                        continue;
                    };
                    if behind_shared_reference && matches!(receiver.ty, ResolvedType::RefMut(_)) {
                        continue;
                    }
                    if !self.rust_trait_carriers_match(&receiver.ty, source) {
                        candidate.borrow_cost += 1;
                    }
                }
                candidates.push(candidate);
            }
            match current {
                ResolvedType::Ref(inner) => {
                    behind_shared_reference = true;
                    current = inner;
                }
                ResolvedType::RefMut(inner) => current = inner,
                _ => break,
            }
        }
        candidates
    }

    /// Return the one identity two Rust trait paths are compared by: the definition path the extractor retained, else
    /// the canonical path, else `path` itself, normalized through `ancestral_rust_path`. A facade re-export and the
    /// trait's definition therefore agree. Every trait comparison applies this to both of its sides.
    pub(in crate::typechecker) fn rust_trait_identity(&self, path: &str) -> String {
        let metadata = self.rust_item_metadata_for_path(path);
        let definition = metadata
            .as_deref()
            .map(|metadata| metadata.definition_path.as_deref().unwrap_or(&metadata.canonical_path))
            .unwrap_or(path);
        incan_lang::interop::ancestral_rust_path(definition)
    }

    /// Read extension-trait templates from their own ABI when the implementor's older ABI cannot contain that trait.
    /// Existing owner facts retain priority; importing a trait never rewrites the cached native type's answer.
    pub(super) fn rust_imported_trait_templates(
        &self,
        info: &RustTypeInfo,
        selected: Option<&str>,
    ) -> Vec<RustImplementedTrait> {
        let mut templates = Vec::new();
        let selected = selected.map(|path| self.rust_trait_identity(path));
        for import in self.type_info.rust.trait_imports.values() {
            let identity = self.rust_trait_identity(import.definition_path.as_deref().unwrap_or(&import.trait_path));
            if selected.as_ref().is_some_and(|selected| *selected != identity)
                || info
                    .implemented_traits
                    .iter()
                    .any(|implementation| self.rust_trait_identity(&implementation.path) == identity)
            {
                continue;
            }
            let Some(metadata) = self.rust_item_metadata_for_path(&import.trait_path) else {
                continue;
            };
            let RustItemKind::Trait(trait_info) = &metadata.kind else {
                continue;
            };
            templates.extend(trait_info.implementations.iter().cloned());
        }
        templates
    }

    /// Eliminate an extension or explicitly selected protocol candidate when a callable trait condition has no matching
    /// native Self template. This is eligibility filtering, not proof: marker predicates, nested conditions and all
    /// complete native obligations remain with rustc before MIR. In particular, a guard must reach its checked
    /// Deref target instead of winning an inapplicable reader or writer extension merely because its impl binds an
    /// arbitrary T.
    pub(in crate::typechecker::check_expr) fn rust_impl_may_apply(
        &self,
        implementation: &RustImplementedTrait,
        owner: &ResolvedType,
    ) -> bool {
        let Some(bindings) = self.rust_impl_self_bindings(implementation, owner) else {
            return false;
        };
        for bound in &implementation.bounds {
            let Some(metadata) = self.rust_item_metadata_for_path(&bound.path) else {
                return false;
            };
            let required_path = metadata.canonical_path.clone();
            let RustItemKind::Trait(required) = &metadata.kind else {
                return false;
            };
            if !required
                .items
                .iter()
                .any(|item| matches!(item, RustTraitAssoc::Function { .. }))
            {
                continue;
            }
            let mut actual = self.instantiate_rust_signature_type(
                &self.native_rust_signature_type(&bound.self_type, ""),
                &implementation.type_params,
                &bindings,
            );
            if bound.self_type == implementation.self_type {
                actual = self.rust_native_self_binding(owner, actual);
            }
            if self.active_type_param_name(&actual).is_some() {
                if !self.type_satisfies_explicit_bound(&actual, &bound.path) {
                    return false;
                }
                continue;
            }
            let Some(path) = self.rust_bound_owner_item_path(&actual) else {
                return false;
            };
            let Some(metadata) = self.rust_item_metadata_for_path(&path) else {
                return false;
            };
            if matches!(metadata.kind, RustItemKind::Trait(_)) {
                if self.rust_trait_identity(&metadata.canonical_path) != self.rust_trait_identity(&required_path) {
                    return false;
                }
                continue;
            }
            let RustItemKind::Type(info) = &metadata.kind else {
                return false;
            };
            let actual = self.rust_closed_owner(&actual, &info, &path);
            let identity = self.rust_trait_identity(&bound.path);
            if !info
                .implemented_traits
                .iter()
                .chain(required.implementations.iter())
                .any(|candidate| {
                    self.rust_trait_identity(&candidate.path) == identity
                        && self.rust_impl_self_bindings(candidate, &actual).is_some()
                })
            {
                return false;
            }
        }
        true
    }

    /// Recover an inspected native Self only when source inference lost its metadata owner. Existing native
    /// identities keep their inference representation, preserving ordinary receiver and associated-output selection.
    pub(in crate::typechecker::check_expr) fn rust_native_self_binding(
        &self,
        owner: &ResolvedType,
        inferred: ResolvedType,
    ) -> ResolvedType {
        if self.rust_bound_owner_item_path(&inferred).is_none() && self.rust_bound_owner_item_path(owner).is_some() {
            owner.clone()
        } else {
            inferred
        }
    }

    /// Locate primitive and nominal metadata for a closed trait obligation without changing ordinary method priority.
    pub(in crate::typechecker::check_expr::access) fn rust_bound_owner_item_path(
        &self,
        ty: &ResolvedType,
    ) -> Option<String> {
        match ty {
            ResolvedType::Int => Some("i64".to_owned()),
            ResolvedType::Float => Some("f64".to_owned()),
            ResolvedType::Bool => Some("bool".to_owned()),
            ResolvedType::Numeric(id) => Some(numerics::as_str(*id).to_owned()),
            _ => self.rust_canonical_path_for_receiver_type(ty),
        }
    }

    /// Fill metadata defaults inside a native reference without erasing that reference's identity as trait Self.
    pub(in crate::typechecker::check_expr::access) fn rust_trait_owner(
        &self,
        owner: &ResolvedType,
        info: &RustTypeInfo,
        path: &str,
    ) -> ResolvedType {
        match owner {
            ResolvedType::Ref(inner) => ResolvedType::Ref(Box::new(self.rust_trait_owner(inner, info, path))),
            ResolvedType::RefMut(inner) => ResolvedType::RefMut(Box::new(self.rust_trait_owner(inner, info, path))),
            _ => self.rust_closed_owner(owner, info, path),
        }
    }

    /// Project a closed trait reference into the same checked external-call record as inherent and free callees.
    pub(super) fn retain_selected_rust_trait_call(
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
                self.checked_rust_receiver_parameter(&selected.signature, &selected.self_type)?,
            );
        }
        self.type_info.rust.checked_calls.insert(
            call.span,
            crate::typechecker::type_info::RustCheckedCall {
                storage: None,
                source_type_parameters: self.native_call_type_parameters(),
                signature_loans: Self::native_signature_loan_identities(&selected.signature),
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
