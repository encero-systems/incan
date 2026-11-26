//! Rust value item paths, native value types, impl self bindings, and storage owners.

use super::*;

impl TypeChecker {
    /// Keep a metadata-selected result item's identity before source scalar and collection projection, normalizing
    /// token-printer spacing before the exact metadata lookup.
    pub(in crate::typechecker::check_expr) fn rust_result_item_path(
        &self,
        display: &str,
        owner: &str,
    ) -> Option<String> {
        let display = self.rust_display_for_owner_path(display, owner);
        let display = Self::rust_display_without_lifetimes(&display);
        let display = Self::compact_rust_display(&display);
        let referent = Self::rust_display_borrow_kind(&display).map_or(display.as_str(), |(_, inner)| inner);
        let base = Self::rust_generic_base_and_args(referent).map_or(referent, |(base, _)| base);
        let native = incan_lang::interop::ancestral_rust_path(base);
        let path = if native.contains("::") {
            native
        } else {
            self.unique_imported_rust_item_path_for_display_tail(base)?
        };
        let metadata = self.rust_item_metadata_for_path(&path)?;
        matches!(metadata.kind, RustItemKind::Type(_)).then(|| metadata.canonical_path.clone())
    }

    /// Read a native value's retained item from its field, call, propagated success payload or canonical binding,
    /// never from a method spelling. Propagation selects the payload's owner rather than the Result container.
    pub(in crate::typechecker::check_expr) fn rust_value_item_path(
        &self,
        expression: &Spanned<Expr>,
    ) -> Option<String> {
        if let Some(field) = self.type_info.rust.field_values.get(&expression.span) {
            return field.item_path.clone();
        }
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_value_item_path(inner);
        }
        if let Expr::Try(inner) = &expression.node {
            if !self.type_info.rust.checked_calls.contains_key(&inner.span)
                && self.rust_value_item_path(inner).is_none()
            {
                return None;
            }
            return self
                .rust_native_value_type(expression)
                .and_then(|native| self.rust_adjusted_owner_path(&native));
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
            .get(&expression.span)?
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

    /// Select a native method receiver before materializing a direct borrowed return as its owned source surface.
    /// The same consumer marker used for native arguments prevents a later return pass from recreating that copy;
    /// assigned source locals retain their already selected storage, and parentheses preserve the inner identity.
    pub(in crate::typechecker::check_expr::access) fn rust_method_receiver_path(
        &mut self,
        expression: &Spanned<Expr>,
    ) -> Option<String> {
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_method_receiver_path(inner);
        }
        let key = expression.span;
        let native = self
            .type_info
            .rust
            .checked_calls
            .get(&key)
            .map(|call| call.return_type.clone())
            .or_else(|| self.rust_native_value_type(expression))?;
        let path = self.rust_canonical_path_for_receiver_type(&native)?;
        self.type_info.rust.native_return_consumers.insert(key);
        self.type_info.rust.return_coercions.remove(&key);
        Some(path)
    }

    /// Recover the exact native carrier of a public Rust field, selected call, checked source view, or local alias for
    /// generic inference, including the success edge of a Result. A source view's flattened public type must not
    /// replace its already retained executable borrow.
    pub(in crate::typechecker::check_expr) fn rust_native_value_type(
        &self,
        expression: &Spanned<Expr>,
    ) -> Option<ResolvedType> {
        if let Some(field) = self.type_info.rust.field_values.get(&expression.span) {
            return Some(field.native_type.clone());
        }
        if let Some(conversion) = self.type_info.rust.return_coercions.get(&expression.span) {
            return Some(conversion.target_type.clone());
        }
        if let Expr::Paren(inner) = &expression.node {
            return self.rust_native_value_type(inner);
        }
        if let Expr::Try(inner) = &expression.node {
            return self
                .rust_native_value_type(inner)
                .and_then(|result| result.result_ok_type().cloned());
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
        let key = expression.span;
        self.type_info
            .rust
            .checked_calls
            .get(&key)
            .map(|call| call.return_type.clone())
            .or_else(|| self.type_info.calls.body_ir_call_result_types.get(&key).cloned())
            .or_else(|| {
                let retained = self.type_info.expressions.body_ir_expr_types.get(&key)?;
                let ResolvedType::Generic(owner, _) = retained else {
                    return None;
                };
                self.rust_item_metadata_for_path(owner)
                    .is_some_and(|metadata| matches!(metadata.kind, RustItemKind::Type(_)))
                    .then(|| retained.clone())
            })
    }

    /// Compare canonical native carriers for trait selection. The source compatibility relation deliberately admits
    /// opaque Rust paths, so it cannot distinguish competing trait instantiations such as From<char> and From<String>.
    pub(in crate::typechecker) fn rust_trait_carriers_match(&self, left: &ResolvedType, right: &ResolvedType) -> bool {
        self.rust_trait_comparison_type(left) == self.rust_trait_comparison_type(right)
    }

    /// Normalize native identities for comparison without rewriting the public paths retained for executable
    /// storage. Constructor aliases and leaf aliases share retained defining paths and standard reexport identities;
    /// a checked native nominal keeps its identity when storage freezes its path into a named carrier.
    pub(super) fn rust_trait_comparison_type(&self, ty: &ResolvedType) -> ResolvedType {
        let ty = self.rust_inference_type(ty, &[]);
        map_resolved_type_leaves(&ty, &|leaf| {
            if self.active_type_param_name(leaf).is_some() {
                return None;
            }
            if let ResolvedType::Generic(path, arguments) = leaf
                && path.contains("::")
                && arguments.is_empty()
                && self.source_trait_carrier_key(path).is_none()
            {
                return Some(ResolvedType::RustPath(self.rust_native_type_identity_path(path)));
            }
            if let ResolvedType::Function(parameters, result) = leaf {
                return Some(ResolvedType::Function(
                    parameters
                        .iter()
                        .map(|parameter| {
                            let mut parameter = parameter.clone();
                            parameter.name = None;
                            parameter.ty = self.rust_trait_comparison_type(&parameter.ty);
                            parameter
                        })
                        .collect(),
                    Box::new(self.rust_trait_comparison_type(result)),
                ));
            }
            if let ResolvedType::Generic(base, arguments) = leaf {
                if collections::from_str(base) == Some(CollectionTypeId::Tuple) {
                    return Some(ResolvedType::Tuple(
                        arguments
                            .iter()
                            .map(|argument| self.rust_trait_comparison_type(argument))
                            .collect(),
                    ));
                }
                let base = self.source_trait_carrier_key(base).unwrap_or_else(|| {
                    if base.contains("::") {
                        self.rust_native_type_identity_path(base)
                    } else {
                        base.clone()
                    }
                });
                return Some(ResolvedType::Generic(
                    base,
                    arguments
                        .iter()
                        .map(|argument| self.rust_trait_comparison_type(argument))
                        .collect(),
                ));
            }
            if let ResolvedType::Named(name) = leaf
                && let Some(key) = self.source_trait_carrier_key(name)
            {
                return Some(ResolvedType::Named(key));
            }
            let path = match leaf {
                ResolvedType::RustPath(path) => path,
                ResolvedType::Named(path)
                    if self
                        .rust_item_metadata_for_path(path)
                        .is_some_and(|metadata| matches!(metadata.kind, RustItemKind::Type(_))) =>
                {
                    path
                }
                _ => return None,
            };
            if incan_lang::interop::rust_display_is_owned_string(path) {
                return Some(ResolvedType::Str);
            }
            // A spelled `dyn Trait` and the native object signature types retain share one identity.
            if path.starts_with("dyn ") {
                let object = self.native_rust_signature_type(path, "");
                if &object != leaf {
                    return Some(self.rust_trait_comparison_type(&object));
                }
            }
            Some(ResolvedType::RustPath(self.rust_native_type_identity_path(path)))
        })
    }

    /// Normalize source nominals owned by this compilation through their checked identity, including library
    /// declarations whose origin already names the publishing package. Foreign package aliases keep their native
    /// defining-path comparison and selected artifact identity.
    pub(super) fn source_trait_carrier_key(&self, name: &str) -> Option<String> {
        let identity = self.symbols.module_type_identity(name)?;
        let owned = match &identity.origin {
            incan_semantics_core::SymbolOrigin::Module(_) => true,
            incan_semantics_core::SymbolOrigin::Package { library, .. } => {
                self.symbols.package_identity() == Some(library.as_str())
            }
            _ => false,
        };
        if !owned {
            return None;
        }
        self.source_nominal_carrier_key(name)
    }

    /// Compare public type reexports through their retained defining path within the same bound crate. Distinct
    /// extern aliases remain separate domains; standard reexports retain the existing ancestry normalization.
    pub(in crate::typechecker) fn rust_native_type_identity_path(&self, path: &str) -> String {
        let metadata = self.rust_item_metadata_for_path(path);
        let definition = metadata.as_ref().and_then(|metadata| {
            if !matches!(metadata.kind, RustItemKind::Type(_)) {
                return None;
            }
            let definition = metadata.definition_path.as_deref()?;
            (definition.split("::").next() == path.split("::").next()).then_some(definition)
        });
        incan_lang::interop::ancestral_rust_path(definition.unwrap_or(path))
    }

    /// Bind one impl's declared Self template to a concrete native owner before member, projection, or ownership
    /// admission. A matching Self template is a candidate only; native predicate obligations remain unproven here.
    pub(in crate::typechecker) fn rust_impl_self_bindings(
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
        self.insert_rust_impl_default_bindings(implementation, &mut bindings);
        let closed = substitute_resolved_type(&template, &bindings);
        if !self.rust_trait_carriers_match(&closed, &actual) {
            return None;
        }
        self.rust_callable_impl_bindings(implementation, &mut bindings);
        Some(bindings)
    }

    /// Close an impl binder constrained by a retained native callable Output equality. Iterator adapters commonly
    /// bind their Item through `F: FnMut(...) -> B`, so matching Self alone cannot discover B. The actual checked
    /// function signature supplies that result; rustc still verifies every native trait predicate before execution.
    pub(super) fn rust_callable_impl_bindings(
        &self,
        implementation: &RustImplementedTrait,
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        for _ in 0..implementation.type_params.len() {
            for bound in &implementation.bounds {
                if incan_lang::interop::rust_callable_trait(&bound.path).is_none() {
                    continue;
                }
                let owner = self.native_rust_signature_type(&bound.self_type, "");
                let owner = self.rust_inference_type(&owner, &implementation.type_params);
                let ResolvedType::Function(_, result) = substitute_resolved_type(&owner, bindings) else {
                    continue;
                };
                for equality in &bound.associated_type_bindings {
                    if equality.name != "Output" {
                        continue;
                    }
                    let output = self.native_rust_signature_type(&equality.value_path, "");
                    let output = self.rust_inference_type(&output, &implementation.type_params);
                    self.infer_type_param_bindings(&output, &result, bindings);
                }
            }
        }
    }

    /// Bind the impl binders that stand at a defaulted position of its Self template (`A` in `Vec<T, A>`) to the
    /// declaration's default, unless a binding already exists.
    pub(super) fn insert_rust_impl_default_bindings(
        &self,
        implementation: &RustImplementedTrait,
        bindings: &mut HashMap<String, ResolvedType>,
    ) {
        let Some((base, arguments)) = Self::rust_generic_base_and_args(&implementation.self_type) else {
            return;
        };
        let Some(metadata) = self.rust_item_metadata_for_path(base) else {
            return;
        };
        let RustItemKind::Type(info) = &metadata.kind else {
            return;
        };
        let mut earlier = Vec::new();
        for (index, argument) in arguments
            .iter()
            .filter(|argument| !Self::rust_generic_arg_is_lifetime(argument))
            .enumerate()
        {
            if implementation.type_params.iter().any(|name| name == argument)
                && !bindings.contains_key(*argument)
                && let Some(default) = self.owner_default(info, index, &earlier, base)
            {
                bindings.insert((*argument).to_owned(), default);
            }
            let closed = bindings
                .get(*argument)
                .cloned()
                .unwrap_or_else(|| self.native_rust_signature_type(argument, base));
            earlier.push(closed);
        }
    }

    /// Whether `owner` names a generic native type without its arguments, as in `Vec.from(...)`: each impl then
    /// supplies its own Self template and the call's arguments or expected result close the binders.
    pub(super) fn rust_owner_is_open(&self, owner: &ResolvedType, info: &RustTypeInfo, path: &str) -> bool {
        !info.type_params.is_empty()
            && self
                .rust_owner_type_args(owner, path)
                .is_some_and(|arguments| arguments.is_empty())
    }

    /// Close omitted native owner defaults once, before matching either impl members or associated type templates.
    /// Native slices retain the signature parser's structural owner instead of acquiring nominal generic syntax.
    pub(in crate::typechecker::check_expr::access) fn rust_closed_owner(
        &self,
        ty: &ResolvedType,
        info: &RustTypeInfo,
        path: &str,
    ) -> ResolvedType {
        if matches!(ty, ResolvedType::Generic(base, _) if base == "[]") {
            return ty.clone();
        }
        let mut owner = match ty {
            ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) => *inner.clone(),
            other => other.clone(),
        };
        if path == "[]" {
            return self.native_rust_signature_type(&render_resolved_type_as_rust_arg(&owner), "");
        }
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
            let display = format!("{path}<{arguments}>");
            // Keep existing source Option/Result and collection projections while restoring frozen native owners,
            // whose inferred method calls still need their exact impl identity and tuple arguments.
            owner = self.native_frozen_owner(&display, path, self.resolved_type_from_rust_display(&display));
        }
        owner
    }

    /// Keep inference owners in the shortest form rustc displays at constructors. The accepting checker separately
    /// freezes complete native storage defaults, including public field templates, before publishing its facts.
    pub(in crate::typechecker::check_expr) fn rust_storage_owner(
        &self,
        owner: &ResolvedType,
        info: &RustTypeInfo,
        path: &str,
    ) -> ResolvedType {
        match owner {
            ResolvedType::Ref(inner) => {
                return ResolvedType::Ref(Box::new(self.rust_storage_owner(inner, info, path)));
            }
            ResolvedType::RefMut(inner) => {
                return ResolvedType::RefMut(Box::new(self.rust_storage_owner(inner, info, path)));
            }
            _ => {}
        }
        // A transparent rusttype contributes its backing Rust owner to a selected native signature. Keeping the
        // source alias here would make a forwarded shared receiver borrow a nominal wrapper that has no MIR frame.
        let owner = self.rusttype_native_target(owner).unwrap_or_else(|| owner.clone());
        let Some(arguments) = self.rust_owner_type_args(&owner, path) else {
            return owner;
        };
        let original_arity = arguments.len();
        let arguments = self.rust_storage_owner_arguments(arguments, info, path);
        if arguments.len() == original_arity {
            return owner;
        }
        if arguments.is_empty() {
            if info.type_params.is_empty() {
                return owner;
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
            let Some(default) = self.owner_default(info, index, &arguments[..index], path) else {
                break;
            };
            if !self.rust_trait_carriers_match(&arguments[index], &default) {
                break;
            }
            arguments.pop();
        }
        arguments
    }

    /// Recognize only exact owned-text and vector views admitted at native borrowed parameters. Element equality
    /// uses native carrier comparison, so this does not widen trait selection to permissive opaque path compatibility.
    pub(super) fn rust_borrowed_view_matches(&self, target: &ResolvedType, argument: &ResolvedType) -> bool {
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
}
