//! Class declaration checking and native class trait adoption.

use super::*;

impl TypeChecker {
    /// Validate a class declaration and field defaults without entering any member statement producer.
    pub(super) fn prepare_class_declaration(
        &mut self,
        class: &ClassDecl,
    ) -> (crate::symbols::PreparedScope, Vec<ResolvedTraitAdoption>) {
        self.symbols.enter_scope(ScopeKind::Class);
        self.refuse_unbounded_class_members(class);

        // ---- Generic scope and derives ----
        for param in &class.type_params {
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
        self.refuse_unsupported_callable_markers(
            &class.type_params,
            CallableMarkerOwner::Nominal {
                kind: "class",
                name: &class.name,
            },
        );

        let derives = self.check_class_derives(class);
        self.validate_made_private_decorators(&class.decorators, class.visibility, &format!("class '{}'", class.name));

        // Check base class exists
        if let Some(base) = &class.extends
            && self.symbols.lookup(base).is_none()
        {
            self.errors.push(errors::unknown_symbol(base, Span::default()));
        }

        // ---- Trait and field validation ----
        let resolved_trait_adoptions = self.check_class_trait_adoptions(class, &derives);
        let active_class_fields = class
            .fields
            .iter()
            .filter(|field| self.member_binding_is_active(field.span))
            .collect::<Vec<_>>();
        self.refuse_awaitable_adoptions(&class.name, &resolved_trait_adoptions);
        self.refuse_pub_on_trait_implementations(&class.name, &class.traits, &class.methods);

        // RFC 021: Field aliases are NOT supported on class declarations.
        // Reject any field metadata on class fields.
        for field in &active_class_fields {
            if field.node.metadata.alias.is_some() {
                self.errors.push(errors::alias_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    field.span,
                ));
            }
            if field.node.metadata.description.is_some() {
                self.errors.push(errors::description_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    field.span,
                ));
            }
            if let Some(span) = field.node.metadata.renamed_from_key_span {
                self.errors.push(errors::renamed_from_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    span,
                ));
            }
            if let Some(span) = field.node.metadata.canonical_key_span {
                self.errors.push(errors::canonical_not_supported_on_class(
                    &class.name,
                    &field.node.name,
                    span,
                ));
            }
            if field.node.visibility == Visibility::Public
                && let Some(span) = field.node.metadata.made_private_key_span
            {
                self.errors.push(errors::invalid_made_private_annotation(
                    &format!("field '{}.{}'", class.name, field.node.name),
                    "the field is public",
                    span,
                ));
            }
        }

        self.check_class_fields(class, active_class_fields);

        (self.symbols.suspend_scope(), resolved_trait_adoptions)
    }

    /// Check class members after its field/default producer returns, then complete its declaration-only surface
    /// checks at their original traversal point. No class declaration evaluation encloses a statement producer.
    pub(super) fn check_class(&mut self, class: &ClassDecl, span: Span) {
        let index = self.body_checks.len();
        let (scope, adoptions) = self.check_declaration_owner(span, |checker| {
            checker.validate_protected_type_param_bindings(&class.type_params, span);
            checker.prepare_class_declaration(class)
        });
        self.symbols.resume_scope(scope);
        self.check_class_members(class, index);
        self.continue_declaration_owner(index, |checker| {
            checker.complete_class_declaration(class, &adoptions);
            checker.symbols.exit_scope();
            checker.record_declared_display_route(&class.name, span);
        });
    }

    /// Check each class member in source order using its prepared nominal scope and canonical field identities.
    pub(super) fn check_class_members(&mut self, class: &ClassDecl, declaration: usize) {
        // Check methods
        let active_class_methods = class
            .methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .collect::<Vec<_>>();
        for method in active_class_methods {
            self.check_method_with_owner_type_params(&method.node, method.span, &class.name, &class.type_params);
        }
        self.continue_declaration_owner(declaration, |checker| {
            checker.check_method_partials(&class.name, &class.method_partials)
        });
        let active_class_properties = class
            .properties
            .iter()
            .filter(|property| self.member_binding_is_active(property.span))
            .collect::<Vec<_>>();
        for property in active_class_properties {
            self.check_property(property, &class.name, &class.type_params);
        }
    }

    /// Complete class trait-surface and derive checks after its independently checked members.
    pub(super) fn complete_class_declaration(
        &mut self,
        class: &ClassDecl,
        resolved_trait_adoptions: &[ResolvedTraitAdoption],
    ) {
        if let Some(TypeInfo::Class(info)) = self.lookup_type_info(&class.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&class.methods);
            self.validate_multi_instantiation_trait_surface(
                &class.name,
                resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }
        let member_spans = class
            .fields
            .iter()
            .map(|field| (field.node.name.as_str(), field.span))
            .collect();
        self.check_derive_contract(
            DerivedKind::Class,
            &class.name,
            &class.decorators,
            &member_spans,
            &class.methods,
        );
    }

    /// Validate class decorators and retain explicit/native derive selection, excluding automatic Debug where its
    /// checked class shape requires it. Adoption-implied serde derives remain a separate later selection.
    pub(super) fn check_class_derives(&mut self, class: &ClassDecl) -> Vec<String> {
        self.validate_decorators_rejecting_user_defined(&class.decorators, "class");
        self.validate_c_binding_class(class);
        self.reject_registry_description_decorators(&class.decorators, "class");
        self.validate_derives(&class.decorators);
        self.validate_descriptor_derive_attachment(&class.decorators, "class");
        self.validate_rust_derives(&class.decorators, "class", false, &class.traits);
        let derives = self.extract_derive_names(&class.decorators);
        let mut declared = Vec::new();
        if self.append_native_rust_derives(&class.name, &mut declared) {
            self.type_info
                .declarations
                .rust_native_derive_types
                .insert(class.name.clone());
        }
        let native_derives = self.native_derive_selection(&derives, declared);
        let mut native_derives = self.body_ir_native_derive_selection(&native_derives);
        if let Some(TypeInfo::Class(info)) = self.lookup_semantic_type_info(&class.name)
            && self.class_omits_automatic_debug(&class.name, info)
        {
            native_derives.retain(|derive| derive != derives::as_str(incan_lang::lang::derives::DeriveId::Debug));
        }
        self.type_info
            .declarations
            .class_derives
            .insert(class.name.clone(), native_derives);
        derives
    }

    /// Check explicit class trait conformance before implied derives and retain adoption-implied serde selection
    /// separately from explicit class derives. Later member-surface checks reuse the same resolved inventory.
    pub(super) fn check_class_trait_adoptions(
        &mut self,
        class: &ClassDecl,
        derives: &[String],
    ) -> Vec<ResolvedTraitAdoption> {
        let mut resolved_trait_adoptions = Vec::new();
        let mut native_adoptions = Vec::new();
        for trait_ref in &class.traits {
            let trait_name = trait_ref.node.name.as_str();
            if let Some((trait_path, trait_metadata)) = self.imported_rust_trait_candidate(trait_name) {
                native_adoptions.extend(self.check_native_class_adoption(
                    class,
                    &trait_ref.node,
                    trait_ref.span,
                    &trait_path,
                    trait_metadata.as_deref(),
                ));
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
                self.check_trait_conformance(
                    class,
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
        let derive_adoption_span = Self::first_derive_span(&class.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            resolved_trait_adoptions.push(resolved);
        }
        let adoption_derives = self.adopted_serde_derives(&class.traits);
        self.record_body_ir_trait_adoptions(&class.name, &resolved_trait_adoptions);
        if !native_adoptions.is_empty() {
            self.type_info
                .declarations
                .body_ir_trait_adoptions
                .entry(class.name.clone())
                .or_default()
                .extend(native_adoptions);
        }
        if !adoption_derives.is_empty() {
            self.type_info
                .declarations
                .class_adoption_derives
                .insert(class.name.clone(), adoption_derives);
        }
        resolved_trait_adoptions
    }

    /// Record a concrete class's request to implement an inspected Rust trait, with every trait method slot it fills.
    ///
    /// This is not a conformance proof: the native compiler checks the implementation against the trait's signatures,
    /// required methods, defaults and supertraits. Only a concrete class and a concrete trait that declares nothing but
    /// methods qualify.
    pub(super) fn check_native_class_adoption(
        &mut self,
        class: &ClassDecl,
        bound: &TraitBound,
        span: Span,
        path: &str,
        metadata: Option<&RustItemMetadata>,
    ) -> Option<super::type_info::BodyIrCheckedTraitAdoption> {
        let concrete = match metadata.map(|metadata| &metadata.kind) {
            Some(RustItemKind::Trait(info)) => (class.type_params.is_empty()
                && bound.type_args.is_empty()
                && info.type_params.is_empty()
                && info
                    .items
                    .iter()
                    .all(|item| matches!(item, RustTraitAssoc::Function { .. })))
            .then_some(info),
            _ => None,
        };
        let identity = |name: &str| {
            self.symbols
                .lookup(name)
                .and_then(|id| self.symbols.identity_of(id))
                .cloned()
        };
        let (Some(info), Some(mut trait_identity), Some(owner)) =
            (concrete, identity(&bound.name), identity(&class.name))
        else {
            self.errors
                .push(errors::class_rust_trait_adoption_unsupported(&class.name, path, span));
            return None;
        };
        // The inspected metadata establishes a trait; the imported path still names it.
        trait_identity.kind = SemanticSourceTargetKind::Trait;
        trait_identity.namespace = incan_semantics_core::SymbolNamespace::OrdinaryLexical;
        trait_identity.scope_discriminant = None;
        let mut slots = Vec::new();
        for item in &info.items {
            let RustTraitAssoc::Function { name, .. } = item else {
                continue;
            };
            let slot = Self::synthetic_member_identity(&trait_identity, name, SemanticSourceTargetKind::Method);
            self.bind_native_trait_methods(class, bound, path, &owner, &slot)?;
            slots.push(slot);
        }
        Some(super::type_info::BodyIrCheckedTraitAdoption {
            trait_identity,
            arguments: Vec::new(),
            methods: Some(slots),
            fields: Vec::new(),
            span,
        })
    }

    /// Fill one Rust trait method slot with the class method of the same name, by the method's canonical identity.
    /// A method aimed at another trait does not fill it; an unfilled slot keeps the trait's default.
    pub(super) fn bind_native_trait_methods(
        &mut self,
        class: &ClassDecl,
        bound: &TraitBound,
        path: &str,
        owner: &CanonicalSymbolId,
        slot: &CanonicalSymbolId,
    ) -> Option<()> {
        for method in &class.methods {
            if method.node.name != slot.declaration_name
                || method
                    .node
                    .trait_target
                    .as_ref()
                    .is_some_and(|target| target.node.name != bound.name)
            {
                continue;
            }
            let implementation = self
                .type_info
                .declarations
                .member_declaration_identities
                .get(&method.span)
                .cloned();
            let Some(implementation) = implementation.filter(|_| {
                method.node.type_params.is_empty() && method.node.receiver.is_some() && method.node.body.is_some()
            }) else {
                self.errors.push(errors::class_rust_trait_method_unsupported(
                    &class.name,
                    &method.node.name,
                    path,
                    method.span,
                ));
                return None;
            };
            self.type_info.declarations.native_trait_implementations.push(
                incan_semantics_core::body_ir::TraitImplementation {
                    owner: owner.clone(),
                    method: slot.clone(),
                    implementation,
                    slot_arguments: Vec::new(),
                    type_arguments: Vec::new(),
                },
            );
            self.publish_rust_trait_method(&class.name, &method.node.name);
        }
        Some(())
    }

    /// Consume active class fields in source order, retaining checked types and defaults before JSON-form checks.
    /// Class field metadata refusals run first in the coordinator and do not suppress these independent checks.
    pub(super) fn check_class_fields(&mut self, class: &ClassDecl, fields: Vec<&Spanned<FieldDecl>>) {
        let mut json_members = Vec::new();
        for field in fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            self.refuse_stored_trait_field("class", &class.name, &field.node, &ty);
            self.validate_direct_recursive_class_field(&class.name, &field.node.name, &ty, field.span);
            self.refuse_member_without_automatic_derives(
                "class",
                &class.name,
                DerivedMember::Field(&field.node.name),
                &ty,
                field.span,
            );
            json_members.push((DerivedMember::Field(&field.node.name), ty.clone(), field.span));
            self.symbols.define(Symbol {
                name: field.node.name.clone(),
                kind: SymbolKind::Field(FieldInfo {
                    identity: Some(self.symbols.member_declaration_identity(
                        &field.node.name,
                        SemanticSourceTargetKind::Field,
                        field.span,
                    )),
                    record_identity: crate::symbols::field_record_identity(&field.node),
                    record_canonical: crate::symbols::field_record_canonical(&field.node),
                    renamed_from: crate::symbols::field_renamed_from(&field.node),
                    renamed_from_lifecycle: crate::symbols::field_renamed_from_lifecycle(&field.node),
                    default_is_empty: crate::symbols::field_default_is_empty(&field.node),
                    surface_type_name: Some(crate::symbols::field_surface_type_name(&field.node.ty.node, &ty)),
                    ty,
                    visibility: field.node.visibility,
                    is_type_private: crate::symbols::field_is_type_private(
                        &field.node,
                        crate::symbols::members_are_type_private(true, class.visibility),
                    ),
                    owner: Some(class.name.clone()),
                    has_default: field.node.default.is_some(),
                    alias: field.node.metadata.alias.clone(),
                    description: field.node.metadata.description.clone(),
                    made_private: field
                        .node
                        .metadata
                        .made_private
                        .as_ref()
                        .map(crate::symbols::MadePrivateInfo::from_field),
                }),
                span: field.span,
                scope: 0,
            });
            if let Some(default) = &field.node.default {
                let field_ty = self.resolve_type_checked(&field.node.ty);
                self.check_work.field_defaults += 1;
                let default_ty = self.check_expr_with_expected(default, Some(&field_ty));
                if !self.types_compatible(&default_ty, &field_ty)
                    && !self.record_validated_newtype_coercion_if_possible(&default_ty, &field_ty, default.span)
                {
                    self.errors.push(errors::type_mismatch(
                        &field_ty.to_string(),
                        &default_ty.to_string(),
                        default.span,
                    ));
                }
            }
        }
        let adoptions = self.nominal_trait_adoptions(&class.name);
        self.refuse_members_without_json_form("class", &class.name, &adoptions, &json_members);
    }
}
