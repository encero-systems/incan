//! Enum and value-enum declaration checking.

use super::*;

impl TypeChecker {
    /// Validate enum decorators, value-enum rules, and variant payload field types.
    pub(super) fn check_enum(&mut self, en: &EnumDecl) {
        self.symbols.enter_scope(ScopeKind::Block);
        self.refuse_unbounded_enum_members(en);

        self.validate_decorators_rejecting_user_defined(&en.decorators, "enum");
        self.validate_made_private_decorators(&en.decorators, en.visibility, &format!("enum '{}'", en.name));
        self.reject_registry_description_decorators(&en.decorators, "enum");
        self.validate_derives(&en.decorators);
        self.validate_descriptor_derive_attachment(&en.decorators, "enum");
        self.validate_rust_derives(&en.decorators, "enum", false, &en.traits);
        let derives = self.extract_derive_names(&en.decorators);

        for param in &en.type_params {
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
            &en.type_params,
            CallableMarkerOwner::Nominal {
                kind: "enum",
                name: &en.name,
            },
        );
        // A value enum rejects type parameters outright (`check_value_enum_decl`), so only a payload-carrying enum
        // is asked whether every parameter is stored by some variant.
        if en.value_type.is_none() {
            self.validate_type_params_stored(
                StoredTypeParamOwner::Enum,
                &en.name,
                &en.type_params,
                en.variants
                    .iter()
                    .flat_map(|variant| variant.node.fields.iter().map(|field| &field.node)),
            );
        }

        let resolved_trait_adoptions = self.check_enum_trait_adoptions(en, &derives);

        self.check_value_enum_decl(en);
        self.validate_enum_record_identities(en);
        self.check_enum_variant_aliases(en);
        self.check_enum_payloads(en);

        let active_methods = en
            .methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .collect::<Vec<_>>();
        for method in active_methods {
            self.check_method_with_owner_type_params(&method.node, method.span, &en.name, &en.type_params);
        }
        if let Some(TypeInfo::Enum(info)) = self.lookup_type_info(&en.name).cloned() {
            let mut native_derives = self.json_derive_selections(&derives);
            if self.append_native_rust_derives(&en.name, &mut native_derives) {
                self.type_info
                    .declarations
                    .rust_native_derive_types
                    .insert(en.name.clone());
            }
            // A derive brings the derives it implies, as a model's does: a derived `Ord` needs `PartialOrd` natively.
            Self::append_implied_derives(&mut native_derives);
            for automatic in [DeriveId::Debug, DeriveId::Clone].map(derives::as_str) {
                if !native_derives.iter().any(|name| name == automatic) {
                    native_derives.push(automatic.to_owned());
                }
            }
            if Self::enum_has_automatic_partial_eq(&info)
                && !native_derives
                    .iter()
                    .any(|name| derives::from_str(name) == Some(DeriveId::PartialEq))
            {
                native_derives.push(derives::as_str(DeriveId::PartialEq).to_owned());
            }
            let native_derives = self.body_ir_native_derive_selection(&native_derives);
            self.type_info
                .declarations
                .enum_derives
                .insert(en.name.clone(), native_derives);
            let method_spans = Self::method_decl_spans_by_name(&en.methods);
            self.validate_multi_instantiation_trait_surface(
                &en.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }
        let member_spans = en
            .variants
            .iter()
            .map(|variant| (variant.node.name.as_str(), variant.span))
            .collect();
        self.check_derive_contract(DerivedKind::Enum, &en.name, &en.decorators, &member_spans, &en.methods);

        self.symbols.exit_scope();
    }

    /// Check explicit enum trait conformance before implied derive adoptions and the nominal Awaitable refusal.
    /// The returned inventory retains each resolved adoption for the later multi-instantiation surface check.
    pub(super) fn check_enum_trait_adoptions(
        &mut self,
        en: &EnumDecl,
        derives: &[String],
    ) -> Vec<ResolvedTraitAdoption> {
        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &en.traits {
            let trait_name = trait_ref.node.name.as_str();
            if self.lookup_trait_info(trait_name).is_some() {
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
                self.check_trait_conformance_enum(
                    en,
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
        let derive_adoption_span = Self::first_derive_span(&en.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            resolved_trait_adoptions.push(resolved);
        }
        self.refuse_awaitable_adoptions(&en.name, &resolved_trait_adoptions);
        self.refuse_pub_on_trait_implementations(&en.name, &en.traits, &en.methods);
        resolved_trait_adoptions
    }

    /// Check active variant payloads in source order and retain their span-keyed types before automatic-derive and
    /// JSON-form validation. Inactive variants contribute neither diagnostics nor payload facts.
    pub(super) fn check_enum_payloads(&mut self, en: &EnumDecl) {
        let active_variants = en
            .variants
            .iter()
            .filter(|variant| self.member_binding_is_active(variant.span))
            .collect::<Vec<_>>();
        let mut json_members = Vec::new();
        for variant in active_variants {
            for field_ty in &variant.node.fields {
                let resolved = self.resolve_type_checked(field_ty);
                self.type_info
                    .declarations
                    .enum_payload_types
                    .insert(field_ty.span, resolved.clone());
                if matches!(resolved, ResolvedType::Unknown) {
                    self.errors
                        .push(errors::unknown_symbol(&format!("{:?}", field_ty.node), field_ty.span));
                }
                self.refuse_member_without_automatic_derives(
                    "enum",
                    &en.name,
                    DerivedMember::VariantPayload(&variant.node.name),
                    &resolved,
                    field_ty.span,
                );
                json_members.push((
                    DerivedMember::VariantPayload(&variant.node.name),
                    resolved,
                    field_ty.span,
                ));
            }
        }
        let adoptions = self.nominal_trait_adoptions(&en.name);
        self.refuse_members_without_json_form("enum", &en.name, &adoptions, &json_members);
    }

    /// Validate enum variant aliases against the variant namespace.
    pub(super) fn check_enum_variant_aliases(&mut self, en: &EnumDecl) {
        let variants: HashSet<&str> = en
            .variants
            .iter()
            .filter(|variant| self.member_binding_is_active(variant.span))
            .map(|variant| variant.node.name.as_str())
            .collect();
        let active_aliases = en
            .variant_aliases
            .iter()
            .filter(|alias| self.member_binding_is_active(alias.span))
            .collect::<Vec<_>>();
        for alias in active_aliases {
            if !variants.contains(alias.node.target.as_str()) {
                self.errors.push(errors::unknown_symbol(&alias.node.target, alias.span));
            }
        }
    }

    /// Validate explicit enum trait adoption using the same abstract-method contract as models/classes.
    ///
    /// Enums currently do not expose fields, so `@requires(...)` obligations are reported as missing fields at the
    /// adoption site. Method obligations are checked against the collected enum method map.
    pub(super) fn check_trait_conformance_enum(
        &mut self,
        en: &EnumDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        for (field_name, _) in &trait_info.requires {
            self.errors
                .push(errors::missing_field(&en.name, field_name, adoption_span));
        }

        let enum_info = self
            .symbols
            .lookup(&en.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Enum(info)) => Some(info.clone()),
                _ => None,
            });

        if let Some(info) = enum_info {
            self.enforce_trait_abstract_methods(
                &en.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &info.method_overloads,
            );
            self.enforce_trait_default_method_overrides(
                &en.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &info.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &en.name,
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

    /// Validate the declaration-time invariants that make value enum helpers type safe.
    pub(super) fn check_value_enum_decl(&mut self, en: &EnumDecl) {
        let Some(value_type) = &en.value_type else {
            for variant in &en.variants {
                if variant.node.value.is_some() {
                    self.errors.push(errors::regular_enum_variant_value_not_allowed(
                        &en.name,
                        &variant.node.name,
                        variant.span,
                    ));
                }
            }
            return;
        };

        if !en.type_params.is_empty() {
            self.errors
                .push(errors::value_enum_type_params_not_supported(&en.name, value_type.span));
        }

        let backing = match value_type.node {
            ValueEnumType::Str => ValueEnumBacking::Str,
            ValueEnumType::Int => ValueEnumBacking::Int,
        };
        let mut seen_values: HashMap<ValueEnumValue, (&str, Span)> = HashMap::new();

        for variant in &en.variants {
            if matches!(variant.node.name.as_str(), "value" | "from_value") {
                self.errors.push(errors::value_enum_reserved_generated_name(
                    &en.name,
                    &variant.node.name,
                    variant.span,
                ));
            }

            if !variant.node.fields.is_empty() {
                self.errors.push(errors::value_enum_variant_payload_not_allowed(
                    &en.name,
                    &variant.node.name,
                    variant.span,
                ));
            }

            let Some(raw_value) = &variant.node.value else {
                self.errors.push(errors::value_enum_variant_missing_value(
                    &en.name,
                    &variant.node.name,
                    variant.span,
                ));
                continue;
            };

            let value = match (backing, &raw_value.node) {
                (ValueEnumBacking::Str, ValueEnumLiteral::Str(value)) => ValueEnumValue::Str(value.clone()),
                (ValueEnumBacking::Int, ValueEnumLiteral::Int(value)) => ValueEnumValue::Int(value.value),
                (ValueEnumBacking::Str, ValueEnumLiteral::Int(_)) => {
                    self.errors.push(errors::value_enum_literal_type_mismatch(
                        &en.name,
                        backing.display_name(),
                        "int",
                        raw_value.span,
                    ));
                    continue;
                }
                (ValueEnumBacking::Int, ValueEnumLiteral::Str(_)) => {
                    self.errors.push(errors::value_enum_literal_type_mismatch(
                        &en.name,
                        backing.display_name(),
                        "str",
                        raw_value.span,
                    ));
                    continue;
                }
            };

            if let Some((first_variant, _first_span)) = seen_values.get(&value) {
                self.errors.push(errors::value_enum_duplicate_value(
                    &en.name,
                    &value.display_value(),
                    first_variant,
                    &variant.node.name,
                    raw_value.span,
                ));
            } else {
                seen_values.insert(value, (variant.node.name.as_str(), raw_value.span));
            }
        }
    }
}
