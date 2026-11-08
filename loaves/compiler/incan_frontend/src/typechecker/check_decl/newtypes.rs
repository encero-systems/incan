//! Newtype declaration checking.

use super::*;

impl TypeChecker {
    /// Validate a `newtype` or `rusttype` declaration: its parameters, derives, underlying type, adopted traits,
    /// checked construction hooks, and member bodies.
    pub(super) fn check_newtype(&mut self, nt: &NewtypeDecl) {
        self.symbols.enter_scope(ScopeKind::Block);
        self.refuse_unbounded_newtype_members(nt);

        for param in &nt.type_params {
            self.symbols.define_with_target_kind(
                Symbol {
                    name: param.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::Builtin), // Type-var placeholder
                    span: param.span,
                    scope: 0,
                },
                SemanticSourceTargetKind::GenericBinder,
            );
        }
        self.validate_type_param_bound_type_names(&nt.type_params);
        self.refuse_unsupported_callable_markers(
            &nt.type_params,
            CallableMarkerOwner::Nominal {
                kind: "newtype",
                name: &nt.name,
            },
        );

        let derives = self.check_newtype_derives(nt);
        self.validate_made_private_decorators(&nt.decorators, nt.visibility, &format!("newtype '{}'", nt.name));

        // Check underlying type exists
        let underlying = self.resolve_type_checked(&nt.underlying);
        if matches!(underlying, ResolvedType::Unknown) {
            self.errors.push(errors::unknown_symbol(
                &format!("{:?}", nt.underlying.node),
                nt.underlying.span,
            ));
        }
        self.validate_type_params_stored(
            StoredTypeParamOwner::Newtype,
            &nt.name,
            &nt.type_params,
            std::iter::once(&nt.underlying.node),
        );

        let rusttype_path = if nt.is_rusttype {
            self.rust_path_for_rusttype_underlying(&underlying)
        } else {
            None
        };
        if nt.is_rusttype && rusttype_path.is_none() {
            self.errors
                .push(errors::rusttype_requires_rust_backing(&nt.name, nt.underlying.span));
        }
        if nt.is_rusttype
            && let Some(path) = &rusttype_path
        {
            self.type_info
                .rust
                .rusttype_canonical_paths
                .insert(nt.name.clone(), path.clone());
        }
        if !nt.is_rusttype && !nt.interop_edges.is_empty() {
            self.errors
                .push(errors::interop_block_requires_rusttype(&nt.name, nt.underlying.span));
        }
        self.validate_newtype_from_underlying_hook(nt, &underlying);
        if !nt.is_rusttype {
            let adoptions = self.nominal_trait_adoptions(&nt.name);
            self.refuse_members_without_json_form(
                "newtype",
                &nt.name,
                &adoptions,
                &[(DerivedMember::Underlying, underlying.clone(), nt.underlying.span)],
            );
        }

        let resolved_trait_adoptions = self.check_newtype_trait_adoptions(nt, &derives, rusttype_path.as_deref());

        self.check_newtype_member_references(nt);

        self.check_newtype_interop_edges(nt);

        // Check methods (reuse the standard method-checking logic so parameters are in scope).
        let active_newtype_methods = nt
            .methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .collect::<Vec<_>>();
        for method in active_newtype_methods {
            if method.node.body.is_some() {
                self.check_method_with_owner_type_params(&method.node, method.span, &nt.name, &nt.type_params);
            }
        }
        self.check_method_partials(&nt.name, &nt.method_partials);

        if let Some(TypeInfo::Newtype(info)) = self.lookup_type_info(&nt.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&nt.methods);
            self.validate_multi_instantiation_trait_surface(
                &nt.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }
        if !nt.is_rusttype {
            let member_spans = HashMap::from([("", nt.underlying.span)]);
            self.check_derive_contract(
                DerivedKind::Newtype,
                &nt.name,
                &nt.decorators,
                &member_spans,
                &nt.methods,
            );
        }

        self.symbols.exit_scope();
    }

    /// Validate and retain a newtype's explicit, implied, native and adopted JSON derives in their existing order.
    /// Return the source derive names for subsequent trait adoption; native selections stay in checked metadata.
    pub(super) fn check_newtype_derives(&mut self, nt: &NewtypeDecl) -> Vec<String> {
        self.validate_decorators_rejecting_user_defined(&nt.decorators, "newtype");
        self.reject_registry_description_decorators(&nt.decorators, "newtype");
        self.validate_derives(&nt.decorators);
        self.validate_descriptor_derive_attachment(&nt.decorators, "newtype");
        self.validate_rust_derives(&nt.decorators, "newtype", nt.is_rusttype, &nt.traits);
        let derives = self.extract_derive_names(&nt.decorators);
        let mut selected = self.json_derive_selections(&derives);
        Self::append_implied_derives(&mut selected);
        if self.append_native_rust_derives(&nt.name, &mut selected) {
            self.type_info
                .declarations
                .rust_native_derive_types
                .insert(nt.name.clone());
        }
        // Adopted JSON protocols select serde derives after explicit selections.
        let adoptions = match self.lookup_semantic_type_info(&nt.name) {
            Some(TypeInfo::Newtype(info)) => info.trait_adoptions.clone(),
            _ => Vec::new(),
        };
        for derive in self.adopted_json_derives(&adoptions) {
            if !selected.iter().any(|existing| existing == derive) {
                selected.push(derive.to_owned());
            }
        }
        let selected = self.body_ir_native_derive_selection(&selected);
        self.type_info
            .declarations
            .body_ir_newtype_derives
            .insert(nt.name.clone(), selected);
        derives
    }

    /// Check explicit Rust/source trait adoptions before implied derives and publish the accepted source facts.
    /// Rusttype Awaitable bridges are refused before collection; ordinary newtypes keep the nominal refusal afterward.
    pub(super) fn check_newtype_trait_adoptions(
        &mut self,
        nt: &NewtypeDecl,
        derives: &[String],
        rusttype_path: Option<&str>,
    ) -> Vec<ResolvedTraitAdoption> {
        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &nt.traits {
            let trait_name = trait_ref.node.name.as_str();
            if nt.is_rusttype && Self::is_awaitable_trait_name(trait_name) {
                self.errors
                    .push(errors::awaitable_future_bridge_blocked(&nt.name, trait_ref.span));
                continue;
            }
            if let Some((trait_path, trait_metadata)) = self.imported_rust_trait_candidate(trait_name) {
                self.validate_imported_rust_trait_newtype_adoption(
                    nt,
                    &trait_ref.node,
                    trait_ref.span,
                    trait_path.as_str(),
                    trait_metadata.as_deref(),
                    rusttype_path,
                );
            } else if self.lookup_trait_info(trait_name).is_some() {
                let Some((trait_info, trait_args)) = self.resolve_adopted_trait_info(&trait_ref.node, trait_ref.span)
                else {
                    continue;
                };
                resolved_trait_adoptions.push(ResolvedTraitAdoption {
                    name: trait_name.to_string(),
                    info: trait_info.clone(),
                    args: trait_args.clone(),
                    module_path: None,
                    span: trait_ref.span,
                });
                self.check_trait_conformance_newtype(
                    nt,
                    trait_info,
                    trait_name,
                    if trait_args.is_empty() {
                        None
                    } else {
                        Some(trait_args.as_slice())
                    },
                    trait_ref.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(trait_name, trait_ref.span));
            }
        }
        let derive_adoption_span = Self::first_derive_span(&nt.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            resolved_trait_adoptions.push(resolved);
        }
        self.record_body_ir_trait_adoptions(&nt.name, &resolved_trait_adoptions);
        self.refuse_awaitable_adoptions(&nt.name, &resolved_trait_adoptions);
        self.refuse_pub_on_trait_implementations(&nt.name, &nt.traits, &nt.methods);
        resolved_trait_adoptions
    }

    /// Resolve associated type declarations before rebinding expressions. A short-form method rebinding is a
    /// member name rather than a local variable read; only expression-shaped rebinding targets are checked here.
    pub(super) fn check_newtype_member_references(&mut self, nt: &NewtypeDecl) {
        for associated_type in &nt.associated_types {
            let trait_name = associated_type.node.trait_target.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
                let _ = self.resolve_adopted_trait_info(
                    &associated_type.node.trait_target.node,
                    associated_type.node.trait_target.span,
                );
            } else if self.lookup_symbol(trait_name).is_none() {
                self.errors.push(errors::unknown_symbol(
                    trait_name,
                    associated_type.node.trait_target.span,
                ));
            }
            let _ = self.resolve_type_checked(&associated_type.node.ty);
        }
        for rebinding in &nt.rebindings {
            if !matches!(rebinding.node.target.node, Expr::Ident(_)) {
                let _ = self.check_expr(&rebinding.node.target);
            }
        }
    }

    /// Validate interop edges in declaration order, diagnosing duplicates before checking adapter signatures.
    /// Retain an edge only when its adapter introduces no new errors; earlier independent edge errors remain visible.
    /// Boundary equality uses resolved type structure; user-facing type text is only a diagnostic rendering.
    pub(super) fn check_newtype_interop_edges(&mut self, nt: &NewtypeDecl) {
        let rusttype_ty = self.rusttype_decl_resolved_type(nt);
        let mut seen_edges: HashMap<(bool, ResolvedType), (InteropAdapterKind, String, Span)> = HashMap::new();
        for edge in &nt.interop_edges {
            let boundary_ty = self.resolve_type_checked(&edge.node.ty);
            if !matches!(boundary_ty, ResolvedType::Unknown) {
                let key = (
                    matches!(edge.node.direction, InteropDirection::Into),
                    boundary_ty.clone(),
                );
                let adapter_ref = Self::interop_adapter_ref_display(&edge.node.adapter);
                if let Some((prev_kind, prev_adapter, prev_span)) = seen_edges.get(&key) {
                    let key_ty = boundary_ty.to_string();
                    if *prev_kind == edge.node.adapter_kind && prev_adapter == &adapter_ref {
                        self.errors.push(errors::duplicate_interop_edge(
                            &nt.name,
                            if key.0 { "into" } else { "from" },
                            key_ty.as_str(),
                            *prev_span,
                            edge.span,
                        ));
                    } else {
                        self.errors.push(errors::conflicting_interop_edge(
                            &nt.name,
                            if key.0 { "into" } else { "from" },
                            key_ty.as_str(),
                            prev_adapter.as_str(),
                            adapter_ref.as_str(),
                            edge.span,
                        ));
                    }
                } else {
                    seen_edges.insert(key, (edge.node.adapter_kind, adapter_ref, edge.span));
                }
            }
            if let Some(adapter_sig) = self.resolve_interop_adapter_signature(nt, edge) {
                let errors_before = self.errors.len();
                self.validate_interop_adapter_signature(nt, edge, &boundary_ty, &rusttype_ty, &adapter_sig);
                if self.errors.len() == errors_before {
                    self.retain_declared_rusttype_edge(
                        nt,
                        edge,
                        &boundary_ty,
                        &rusttype_ty,
                        adapter_sig.receiver.is_some(),
                    );
                }
            }
        }
    }

    /// Validate explicit newtype/rusttype trait adoption using the same method contract as other nominal types.
    ///
    /// Newtypes do not expose storage fields or properties in the trait surface, so field/property obligations are
    /// rejected at the adoption site. Method obligations are checked against the collected newtype method overload map.
    pub(super) fn check_trait_conformance_newtype(
        &mut self,
        nt: &NewtypeDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        for (field_name, _) in &trait_info.requires {
            self.errors
                .push(errors::missing_field(&nt.name, field_name, adoption_span));
        }

        let newtype_info = self
            .symbols
            .lookup(&nt.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Newtype(info)) => Some(info.clone()),
                _ => None,
            });

        if let Some(info) = newtype_info {
            self.enforce_trait_abstract_methods(
                &nt.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &info.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &nt.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &HashMap::new(),
            );
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    self.errors
                        .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                }
            }
        }
    }

    /// Validate the canonical RFC 017 `from_underlying` hook when a newtype declares one.
    pub(super) fn validate_newtype_from_underlying_hook(&mut self, nt: &NewtypeDecl, underlying: &ResolvedType) {
        let Some(method_decl) = nt
            .methods
            .iter()
            .find(|method| method.node.name == incan_lang::lang::conventions::NEWTYPE_FROM_UNDERLYING_METHOD)
        else {
            return;
        };
        let method_span = method_decl.span;
        let method_info = self.lookup_type_info(&nt.name).and_then(|info| match info {
            TypeInfo::Newtype(newtype) => newtype
                .methods
                .get(incan_lang::lang::conventions::NEWTYPE_FROM_UNDERLYING_METHOD)
                .cloned(),
            _ => None,
        });
        let Some(method_info) = method_info else {
            return;
        };

        if method_info.receiver.is_some() {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                "the hook must be a static method with no self receiver",
                method_span,
            ));
        }
        if method_info.is_async {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                "the hook must be deterministic and cannot be async",
                method_span,
            ));
        }
        if method_info.params.len() != 1
            || method_info
                .params
                .first()
                .is_some_and(|param| param.kind != ParamKind::Normal)
        {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                "the hook must accept exactly one ordinary parameter",
                method_span,
            ));
        } else if let Some(param) = method_info.params.first()
            && !self.types_compatible(&param.ty, underlying)
        {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                &format!(
                    "parameter type must be the newtype underlying type '{underlying}', found '{}'",
                    param.ty
                ),
                method_span,
            ));
        }

        let expected_err = ResolvedType::Named("ValidationError".to_string());
        let valid_return = matches!(
            &method_info.return_type,
            ResolvedType::Generic(name, args)
                if crate::typechecker::helpers::collection_type_id(name.as_str())
                    == Some(incan_lang::lang::types::collections::CollectionTypeId::Result)
                    && args.len() == 2
                    && (
                        matches!(&args[0], ResolvedType::Named(name) if name == &nt.name)
                            || matches!(&args[0], ResolvedType::SelfType)
                    )
                    && args[1] == expected_err
        );
        if !valid_return {
            self.errors.push(errors::invalid_newtype_validation_hook(
                &nt.name,
                &format!(
                    "return type must be Result[{}, ValidationError], found '{}'",
                    nt.name, method_info.return_type
                ),
                method_span,
            ));
        }
    }
}
