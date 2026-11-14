//! Trait declarations, trait conformance, and imported Rust trait newtype adoption.

use super::*;

impl TypeChecker {
    /// Validate that a class satisfies required fields and methods for one instantiated trait adoption.
    pub(super) fn check_trait_conformance(
        &mut self,
        class: &ClassDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        // Use the effective members view (own + inherited) from the symbol table.
        let class_info = self
            .symbols
            .lookup(&class.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Class(info)) => Some(info.clone()),
                _ => None,
            });

        // Check required fields (presence + type compatibility).
        for (field_name, field_ty) in &trait_info.requires {
            match class_info.as_ref().and_then(|ci| ci.fields.get(field_name)) {
                None => {
                    self.errors
                        .push(errors::missing_field(&class.name, field_name, adoption_span));
                }
                Some(found) => {
                    if !self.types_compatible(&found.ty, field_ty) {
                        self.errors.push(errors::trait_required_field_type_mismatch(
                            trait_name,
                            &class.name,
                            field_name,
                            &field_ty.to_string(),
                            &found.ty.to_string(),
                            adoption_span,
                        ));
                    }
                }
            }
        }

        if let Some(ci) = class_info.as_ref() {
            self.enforce_trait_abstract_methods(
                &class.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &ci.method_overloads,
            );
            self.enforce_trait_default_method_overrides(
                &class.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &ci.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &class.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &ci.properties,
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

    /// Validate a trait declaration, including required fields, abstract properties, defaults, and self-typed method
    /// bodies.
    pub(super) fn check_trait(&mut self, tr: &TraitDecl) {
        self.symbols.enter_scope(ScopeKind::Trait);

        // Trait API annotations are collected before source names can be validated. Re-establish the trait's generic
        // scope and revisit every declaration-only surface during the semantic pass.
        for param in &tr.type_params {
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
        self.validate_type_param_bound_type_names(&tr.type_params);
        self.refuse_unsupported_callable_markers(
            &tr.type_params,
            CallableMarkerOwner::Nominal {
                kind: "trait",
                name: &tr.name,
            },
        );
        for supertrait in &tr.traits {
            for type_arg in &supertrait.node.type_args {
                let _ = self.resolve_type_checked(type_arg);
            }
        }
        for decorator in &tr.decorators {
            if resolve_decorator_id(&decorator.node, &self.symbols) != Some(DecoratorId::Requires) {
                continue;
            }
            for arg in &decorator.node.args {
                if let DecoratorArg::Named(_, DecoratorArgValue::Type(ty)) = arg {
                    let _ = self.resolve_type_checked(ty);
                }
            }
        }

        self.validate_decorators_rejecting_user_defined(&tr.decorators, "trait");
        self.reject_registry_description_decorators(&tr.decorators, "trait");
        let requires_map: HashMap<String, ResolvedType> = self
            .symbols
            .lookup(&tr.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Trait(info) => Some(info.requires.clone()),
                _ => None,
            })
            .unwrap_or_default()
            .into_iter()
            .fold(HashMap::new(), |mut acc, (name, ty)| {
                acc.entry(name).or_insert(ty);
                acc
            });
        let prev_trait_requires = self.current_trait_requires.take();
        let prev_trait_properties = self.current_trait_properties.take();
        let prev_trait_name = self.current_trait_name.take();
        let prev_missing_emitted = self.current_trait_missing_requires_emitted.take();
        self.current_trait_requires = Some(requires_map);
        self.current_trait_properties = self.lookup_trait_info(&tr.name).map(|info| info.properties.clone());
        self.current_trait_name = Some(tr.name.clone());
        self.validate_trait_partial_inherited_overrides(&tr.name, &tr.method_partials);

        let active_trait_methods = tr
            .methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .collect::<Vec<_>>();
        for method in active_trait_methods {
            self.check_trait_method(&method.node, method.span, tr);
        }
        self.check_method_partials(&tr.name, &tr.method_partials);
        let active_trait_properties = tr
            .properties
            .iter()
            .filter(|property| self.member_binding_is_active(property.span))
            .collect::<Vec<_>>();
        for property in active_trait_properties {
            let _ = self.resolve_type_checked(&property.node.return_type);
            if property.node.body.is_some() {
                self.errors.push(errors::trait_property_body_not_supported(
                    &tr.name,
                    &property.node.name,
                    property.span,
                ));
            }
        }

        self.current_trait_requires = prev_trait_requires;
        self.current_trait_properties = prev_trait_properties;
        self.current_trait_name = prev_trait_name;
        self.current_trait_missing_requires_emitted = prev_missing_emitted;
        self.symbols.exit_scope();
    }

    /// Prepare one trait method's metadata/defaults, consume its body once, and retain post-body decoration
    /// at the original traversal point. Abstract methods validate their declaration without entering statements.
    pub(super) fn check_trait_method(&mut self, method: &MethodDecl, span: Span, tr: &TraitDecl) {
        let index = self.body_checks.len();
        let (prepared, previous_seen) = self.check_declaration_owner(span, |checker| {
            checker.validate_decorators_allowing_user_defined(&method.decorators);
            checker.reject_registry_description_decorators(&method.decorators, "trait method");
            let previous = checker.current_trait_missing_requires_emitted.replace(HashSet::new());
            let prepared = checker.prepare_method_declaration(method, span, ResolvedType::SelfType, &tr.type_params);
            (prepared, previous)
        });
        self.check_prepared_method(method, span, prepared);
        #[cfg(test)]
        if method.body.is_some() {
            self.observe_body_local(span);
        }
        self.current_trait_missing_requires_emitted = previous_seen;
        self.continue_declaration_owner(index, |checker| {
            checker.apply_user_defined_method_decorators(method, &tr.name)
        });
    }

    /// Return Rust trait metadata for a direct `rust::...` import used in a `with` clause.
    pub(super) fn imported_rust_trait_candidate(
        &self,
        trait_name: &str,
    ) -> Option<(String, Option<Arc<RustItemMetadata>>)> {
        let sym = self.lookup_symbol(trait_name)?;
        let SymbolKind::RustItem(info) = &sym.kind else {
            return None;
        };
        let metadata = match &info.metadata {
            Some(metadata) if matches!(metadata.kind, RustItemKind::Trait(_)) => Some(metadata.clone()),
            Some(_) => None,
            None => None,
        };
        Some((info.path.clone(), metadata))
    }

    /// Return whether an adopted trait name denotes the RFC 039 `Awaitable` protocol.
    pub(super) fn is_awaitable_trait_name(trait_name: &str) -> bool {
        trait_name == "Awaitable" || trait_name.ends_with(".Awaitable")
    }

    /// Compare an associated item target with an adopted trait bound by name and arity.
    pub(super) fn trait_target_matches_bound(target: &TraitBound, bound: &TraitBound) -> bool {
        target.name == bound.name && target.type_args.len() == bound.type_args.len()
    }

    /// Return whether a rusttype declaration authors members for the imported Rust trait instead of requesting
    /// body-less forwarding.
    pub(super) fn newtype_has_explicit_rust_trait_members(
        nt: &NewtypeDecl,
        trait_bound: &TraitBound,
        trait_metadata: Option<&RustItemMetadata>,
    ) -> bool {
        let method_names: HashSet<String> = trait_metadata
            .and_then(|metadata| match &metadata.kind {
                RustItemKind::Trait(info) => Some(
                    info.items
                        .iter()
                        .filter_map(|item| match item {
                            RustTraitAssoc::Function { name, .. } => Some(name.clone()),
                            RustTraitAssoc::TypeAlias { .. } | RustTraitAssoc::Constant { .. } => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default();

        nt.associated_types.iter().any(|associated_type| {
            Self::trait_target_matches_bound(&associated_type.node.trait_target.node, trait_bound)
        }) || nt.methods.iter().any(|method| {
            method
                .node
                .trait_target
                .as_ref()
                .is_some_and(|target| Self::trait_target_matches_bound(&target.node, trait_bound))
                || method_names.contains(&method.node.name)
        })
    }

    /// Return the path suffix used when comparing re-exported Rust trait paths.
    pub(in crate::typechecker) fn rust_trait_path_suffix(path: &str) -> &str {
        path.split_once("::").map(|(_, suffix)| suffix).unwrap_or(path)
    }

    /// Return whether Rust type metadata proves the backing type implements the requested trait path.
    pub(in crate::typechecker) fn rust_type_metadata_implements_trait(
        type_metadata: &RustItemMetadata,
        trait_path: &str,
        trait_definition_path: Option<&str>,
    ) -> bool {
        let RustItemKind::Type(type_info) = &type_metadata.kind else {
            return false;
        };
        if !type_info.metadata_completeness.has_trait_impls() {
            return false;
        }
        let trait_suffix = Self::rust_trait_path_suffix(trait_path);
        type_info.implemented_traits.iter().any(|implemented| {
            implemented.is_unconditional()
                && (implemented.path == trait_path
                    || Some(implemented.path.as_str()) == trait_definition_path
                    || Self::rust_trait_path_suffix(&implemented.path) == trait_suffix)
        })
    }

    /// Validate imported Rust trait adoption on newtype/rusttype declarations.
    ///
    /// A `rusttype` lowers to a Rust type alias. That means custom `impl ForeignTrait for Alias` output would still be
    /// an impl for the foreign backing type and can violate Rust coherence. Body-less adoption is therefore accepted
    /// only when Rust metadata proves the backing type already implements the trait; lowering then skips the impl.
    pub(super) fn validate_imported_rust_trait_newtype_adoption(
        &mut self,
        nt: &NewtypeDecl,
        trait_bound: &TraitBound,
        trait_span: Span,
        trait_path: &str,
        trait_metadata: Option<&RustItemMetadata>,
        rusttype_path: Option<&str>,
    ) {
        let has_explicit_members = Self::newtype_has_explicit_rust_trait_members(nt, trait_bound, trait_metadata);
        if nt.is_rusttype {
            if has_explicit_members {
                self.errors.push(errors::rusttype_foreign_trait_impl_orphan(
                    &nt.name, trait_path, trait_span,
                ));
                return;
            }
            if !trait_bound.type_args.is_empty() {
                self.errors.push(errors::rusttype_forwarding_generic_trait_blocked(
                    &nt.name, trait_path, trait_span,
                ));
                return;
            }
            let Some(backing_path) = rusttype_path else {
                return;
            };
            let Some(backing_metadata) = self.rust_item_metadata_for_path(backing_path) else {
                self.errors.push(errors::rusttype_forwarding_requires_metadata(
                    &nt.name, trait_path, trait_span,
                ));
                return;
            };
            let trait_definition_path = trait_metadata.and_then(|metadata| metadata.definition_path.as_deref());
            if Self::rust_type_metadata_implements_trait(&backing_metadata, trait_path, trait_definition_path) {
                self.type_info
                    .rust
                    .rusttype_forwarded_trait_adoptions
                    .insert((nt.name.clone(), trait_bound.name.clone()));
            } else {
                self.errors.push(errors::rusttype_forwarding_trait_not_implemented(
                    &nt.name,
                    backing_path,
                    trait_path,
                    trait_span,
                ));
            }
            return;
        }

        let Some(metadata) = trait_metadata else {
            return;
        };
        let RustItemKind::Trait(info) = &metadata.kind else {
            return;
        };
        let declared_associated_types: HashSet<&str> = nt
            .associated_types
            .iter()
            .filter(|associated_type| {
                Self::trait_target_matches_bound(&associated_type.node.trait_target.node, trait_bound)
            })
            .map(|associated_type| associated_type.node.name.as_str())
            .collect();
        for item in &info.items {
            let RustTraitAssoc::TypeAlias { name } = item else {
                continue;
            };
            if !declared_associated_types.contains(name.as_str()) {
                self.errors
                    .push(errors::missing_rust_associated_type(trait_path, name, trait_span));
            }
        }
    }

    /// Refuse a declared type parameter that no part of the declaration's representation mentions.
    ///
    /// `representation` is the set of source annotations that give the declaration its storage: a newtype's
    /// underlying type, an enum's variant payloads. A model or class is not checked here because lowering records
    /// such a parameter as phantom and emission carries it in a marker; a newtype and an enum have no field to add
    /// one to, so the shape is refused at its declaration instead of failing in rustc (#1370).
    pub(super) fn validate_type_params_stored<'a>(
        &mut self,
        owner: StoredTypeParamOwner,
        owner_name: &str,
        type_params: &[TypeParam],
        representation: impl Iterator<Item = &'a Type> + Clone,
    ) {
        for param in type_params {
            if !representation.clone().any(|ty| ty.mentions_name(&param.name)) {
                self.errors.push(errors::type_param_not_stored(
                    owner,
                    owner_name,
                    &param.name,
                    param.span,
                ));
            }
        }
    }
}
