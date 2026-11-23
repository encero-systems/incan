//! Model declaration checking.

use super::*;

impl TypeChecker {
    /// Validate a model declaration and defaults, returning its exact lexical inputs before member checking begins.
    pub(super) fn prepare_model_declaration(&mut self, model: &ModelDecl) -> (crate::symbols::PreparedScope, bool) {
        self.symbols.enter_scope(ScopeKind::Model);
        self.refuse_unbounded_model_members(model);

        // ---- Declared derives ----
        self.validate_decorators_rejecting_user_defined(&model.decorators, "model");
        self.validate_made_private_decorators(&model.decorators, model.visibility, &format!("model '{}'", model.name));
        self.reject_registry_description_decorators(&model.decorators, "model");
        // Validate @derive decorators
        self.validate_derives(&model.decorators);
        self.validate_rust_derives(&model.decorators, "model", false, &model.traits);
        let derives = self.extract_derive_names(&model.decorators);
        // Adopted serde derives and checked Rust derives join the model's explicit selection.
        let mut declared = self.adopted_serde_derives(&model.traits);
        if self.append_native_rust_derives(&model.name, &mut declared) {
            self.type_info
                .declarations
                .rust_native_derive_types
                .insert(model.name.clone());
        }
        let native_derives = self.native_derive_selection(&derives, declared);
        let body_ir_derives = self.body_ir_native_derive_selection(&native_derives);
        self.type_info
            .declarations
            .body_ir_model_derives
            .insert(model.name.clone(), body_ir_derives);
        self.type_info
            .declarations
            .model_derives
            .insert(model.name.clone(), native_derives);
        self.validate_descriptor_model_shape(model, &derives);
        let has_validate = derives
            .iter()
            .any(|d| derives::from_str(d.as_str()) == Some(DeriveId::Validate));

        // ---- Generic binders and trait adoptions ----
        for param in &model.type_params {
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
            &model.type_params,
            CallableMarkerOwner::Nominal {
                kind: "model",
                name: &model.name,
            },
        );

        // Check traits exist and are satisfied (models can adopt storage-free traits, RFC 000).
        // Note: do this after defining type params so `@requires(field: T)` can resolve `T`.
        let resolved_trait_adoptions = self.check_model_trait_adoptions(model, &derives);
        let active_model_fields = model
            .fields
            .iter()
            .filter(|field| self.member_binding_is_active(field.span))
            .collect::<Vec<_>>();
        self.validate_model_record_identities(model);
        self.refuse_awaitable_adoptions(&model.name, &resolved_trait_adoptions);
        self.refuse_pub_on_trait_implementations(&model.name, &model.traits, &model.methods);

        // ---- Field metadata and checked defaults ----
        let mut method_names = HashSet::new();
        if let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) {
            method_names.extend(info.methods.keys().cloned());
        }
        method_names.extend(self.collect_trait_method_names(&model.traits));
        self.validate_field_metadata(&model.name, &model.fields, &method_names);
        for field in &active_model_fields {
            if field.node.visibility == Visibility::Public
                && let Some(span) = field.node.metadata.made_private_key_span
            {
                self.errors.push(errors::invalid_made_private_annotation(
                    &format!("field '{}.{}'", model.name, field.node.name),
                    "the field is public",
                    span,
                ));
            }
        }
        if let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name).cloned() {
            let method_spans = Self::method_decl_spans_by_name(&model.methods);
            self.validate_multi_instantiation_trait_surface(
                &model.name,
                &resolved_trait_adoptions,
                &info.method_overloads,
                &method_spans,
            );
        }

        self.check_model_fields(model, active_model_fields);

        (self.symbols.suspend_scope(), has_validate)
    }

    /// Check model members only after its declaration producer has returned final field and default inputs. Keep
    /// post-member derive validation in its legacy position without enclosing a body in the declaration producer.
    pub(super) fn check_model(&mut self, model: &ModelDecl, span: Span) {
        let index = self.body_checks.len();
        let (scope, has_validate) = self.check_declaration_owner(span, |checker| {
            checker.validate_protected_type_param_bindings(&model.type_params, span);
            checker.prepare_model_declaration(model)
        });
        self.symbols.resume_scope(scope);
        self.check_model_members(model, index);
        self.continue_declaration_owner(index, |checker| {
            checker.complete_model_declaration(model, has_validate);
            checker.symbols.exit_scope();
            checker.record_declared_display_route(&model.name, span);
        });
    }

    /// Check each model member in source order against the same prepared nominal scope used by its declaration.
    pub(super) fn check_model_members(&mut self, model: &ModelDecl, declaration: usize) {
        // Check methods
        let active_model_methods = model
            .methods
            .iter()
            .filter(|method| self.member_binding_is_active(method.span))
            .collect::<Vec<_>>();
        for method in active_model_methods {
            self.check_method_with_owner_type_params(&method.node, method.span, &model.name, &model.type_params);
            self.record_registered_model_check(model, &method.node, method.span);
        }
        self.continue_declaration_owner(declaration, |checker| {
            checker.check_method_partials(&model.name, &model.method_partials)
        });
        let active_model_properties = model
            .properties
            .iter()
            .filter(|property| self.member_binding_is_active(property.span))
            .collect::<Vec<_>>();
        for property in active_model_properties {
            self.check_property(property, &model.name, &model.type_params);
        }
    }

    /// Validate and retain one compiler-owned `@validate` registration after ordinary method checking has resolved
    /// its exact callable, target field and failure type. Invalid registrations never become lowering authority.
    fn record_registered_model_check(&mut self, model: &ModelDecl, method: &MethodDecl, method_span: Span) {
        let registrations = method
            .decorators
            .iter()
            .filter(|decorator| decorators::from_segments(&decorator.node.path.segments) == Some(DecoratorId::Validate))
            .collect::<Vec<_>>();
        if registrations.is_empty() {
            return;
        }
        let registration = registrations[0];
        let refuse = |checker: &mut Self, reason: &str| {
            checker.errors.push(errors::invalid_registered_validation(
                &model.name,
                &method.name,
                reason,
                registration.span,
            ));
        };
        if registrations.len() != 1 {
            refuse(self, "a method may register exactly one final check");
            return;
        }
        if !method.type_params.is_empty() || method.result_source.is_some() {
            refuse(
                self,
                "a registered check cannot be generic or declare a borrowed result source",
            );
            return;
        }
        let Some(binding) = self
            .type_info
            .declarations
            .method_bindings_by_span
            .get(&method_span)
            .cloned()
        else {
            refuse(self, "the checked method signature is unavailable");
            return;
        };
        if binding.is_async {
            refuse(self, "a registered check must be synchronous");
            return;
        }
        let Some(callable) = binding.identity.clone() else {
            refuse(self, "the registered check has no canonical callable identity");
            return;
        };
        let (Some(ok), Some(error_type)) = (
            binding.return_type.result_ok_type(),
            binding.return_type.result_err_type(),
        ) else {
            refuse(self, "the return type must be Result[None, E]");
            return;
        };
        if ok != &ResolvedType::Unit {
            refuse(
                self,
                "the successful result must be None and cannot replace the final value",
            );
            return;
        }
        let error_name = match error_type {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => Some(name.as_str()),
            _ => None,
        };
        if !error_name.is_some_and(|name| {
            name == "ValidationError" || self.type_implements_trait(name, builtin_traits::as_str(TraitId::Error))
        }) {
            refuse(self, "the failure type E must implement Error");
            return;
        }
        let target_field = match registration.node.args.as_slice() {
            [] if !registration.node.is_call => {
                if method.receiver != Some(Receiver::Immutable) || !binding.params.is_empty() {
                    refuse(
                        self,
                        "a whole-model check must have the signature def check(self) -> Result[None, E]",
                    );
                    return;
                }
                None
            }
            [DecoratorArg::Positional(target)] => {
                let Expr::Ident(field_name) = &target.node else {
                    refuse(self, "a field check target must be one declared field name");
                    return;
                };
                if method.receiver.is_some() || binding.params.len() != 1 {
                    refuse(
                        self,
                        "a field check must receive exactly one explicitly typed field value and no self",
                    );
                    return;
                }
                let parameter = &binding.params[0];
                if parameter.kind != ParamKind::Normal || parameter.has_default {
                    refuse(self, "a field check parameter must be one required ordinary parameter");
                    return;
                }
                if !model.fields.iter().any(|field| field.node.name == *field_name) {
                    refuse(self, "the registered field target does not exist on this model");
                    return;
                }
                let Some((field_type, identity)) = self.lookup_symbol(field_name).and_then(|symbol| {
                    let SymbolKind::Field(info) = &symbol.kind else {
                        return None;
                    };
                    Some((info.ty.clone(), info.identity.clone()?))
                }) else {
                    refuse(
                        self,
                        "the registered field target has no checked type and canonical identity",
                    );
                    return;
                };
                if parameter.ty != field_type {
                    refuse(self, "the check parameter type must exactly match the final field type");
                    return;
                }
                Some(identity)
            }
            _ => {
                refuse(self, "use bare @validate or one positional field name");
                return;
            }
        };
        self.type_info
            .declarations
            .registered_model_checks
            .entry(model.name.clone())
            .or_default()
            .push(super::type_info::RegisteredModelCheckInfo {
                callable,
                callable_span: method_span,
                target_field,
                error_type: error_type.clone(),
                registration_span: registration.span,
            });
        if let Some(symbol_id) = self.symbols.lookup(&model.name)
            && let Some(Symbol {
                kind: SymbolKind::Type(TypeInfo::Model(info)),
                ..
            }) = self.symbols.get_mut(symbol_id)
        {
            info.has_registered_validation = true;
        }
    }

    /// Validate provisional final-check registrations after every declaration body has contributed its call facts.
    /// Signature checking retains candidates early so model-level conflicts remain deterministic, but only this late
    /// pass grants lowering authority. Unsafe candidates are diagnosed and removed before Body IR can observe them.
    pub(in crate::typechecker) fn validate_registered_model_check_safety(&mut self) {
        let registrations = std::mem::take(&mut self.type_info.declarations.registered_model_checks);
        for (model_name, checks) in registrations {
            let mut accepted = Vec::new();
            for check in checks {
                if let Some(reason) = self.registered_check_construction_safety_refusal(
                    &check.callable.declaration_name,
                    check.callable_span,
                ) {
                    self.errors.push(errors::invalid_registered_validation(
                        &model_name,
                        &check.callable.declaration_name,
                        &reason,
                        check.registration_span,
                    ));
                } else {
                    accepted.push(check);
                }
            }
            let has_registered_validation = !accepted.is_empty();
            if has_registered_validation {
                self.type_info
                    .declarations
                    .registered_model_checks
                    .insert(model_name.clone(), accepted);
            }
            if let Some(symbol_id) = self.symbols.lookup(&model_name)
                && let Some(Symbol {
                    kind: SymbolKind::Type(TypeInfo::Model(info)),
                    ..
                }) = self.symbols.get_mut(symbol_id)
            {
                info.has_registered_validation = has_registered_validation;
            }
        }
    }

    /// Conservatively verify one registered check's resolved local call closure before granting lowering authority.
    /// Cycles are admitted as one active component; every edge leaving it must be a checked local function. Imported,
    /// native, asynchronous, stateful and open-method boundaries remain unverified until package metadata or a
    /// compiler-owned registry explicitly certifies them.
    fn registered_check_construction_safety_refusal(&self, root: &str, span: Span) -> Option<String> {
        let mut active = HashSet::new();
        let mut accepted = HashSet::new();
        let mut chain = vec![root.to_owned()];
        self.callable_construction_safety_refusal(span, &mut active, &mut accepted, &mut chain)
    }

    /// Infer the versioned public construction-safety summary for one authored callable.
    pub(crate) fn callable_is_construction_safe(&self, root: &str, span: Span) -> bool {
        self.registered_check_construction_safety_refusal(root, span).is_none()
    }

    /// Walk exact checked function-call edges for one callable, refusing the first unsafe operation with its chain.
    fn callable_construction_safety_refusal(
        &self,
        declaration: Span,
        active: &mut HashSet<Span>,
        accepted: &mut HashSet<Span>,
        chain: &mut Vec<String>,
    ) -> Option<String> {
        if accepted.contains(&declaration) || !active.insert(declaration) {
            return None;
        }
        let render = |operation: &str, chain: &[String]| {
            format!(
                "construction-safe verification reaches {operation}; call chain: {}",
                chain.join(" -> ")
            )
        };
        if self.type_info.c_abi.raw_calls.iter().any(|call| {
            call.owner
                .as_ref()
                .is_some_and(|owner| owner.declaration_span == declaration)
        }) {
            active.remove(&declaration);
            return Some(render("a C or host effect", chain));
        }
        if self
            .type_info
            .calls
            .body_ir_async_call_targets
            .keys()
            .any(|span| Self::construction_span_contains(declaration, *span))
        {
            active.remove(&declaration);
            return Some(render("asynchronous work", chain));
        }
        if self
            .type_info
            .calls
            .resolved_builtin_calls
            .keys()
            .any(|span| Self::construction_span_contains(declaration, *span))
        {
            active.remove(&declaration);
            return Some(render("an uncertified builtin operation", chain));
        }
        if self
            .type_info
            .calls
            .resolved_method_calls
            .keys()
            .any(|span| Self::construction_span_contains(declaration, *span))
        {
            active.remove(&declaration);
            return Some(render("an open or unverified method dispatch", chain));
        }
        if self
            .type_info
            .references
            .resolved_identities
            .iter()
            .any(|(span, identity)| {
                Self::construction_span_contains(declaration, *span)
                    && (identity.kind == incan_semantics_core::SemanticSourceTargetKind::Static
                        || matches!(identity.origin, incan_semantics_core::SymbolOrigin::RustCrate(_)))
            })
        {
            active.remove(&declaration);
            return Some(render("mutable static state or an uncertified native operation", chain));
        }
        let calls = self
            .type_info
            .references
            .resolved_identities
            .iter()
            .filter(|(span, identity)| {
                Self::construction_span_contains(declaration, **span)
                    && identity.kind == incan_semantics_core::SemanticSourceTargetKind::Function
                    && self.type_info.references.resolved_identities.iter().any(|(callee_span, callee)| {
                        callee == *identity
                            && callee_span != *span
                            && Self::construction_span_contains(**span, *callee_span)
                    })
            })
            .map(|(_, identity)| identity.clone())
            .collect::<Vec<_>>();
        for target in calls {
            match &target.origin {
                incan_semantics_core::SymbolOrigin::Module(_) => {
                    if self
                        .local_function_decls
                        .get(&target.declaration_name)
                        .is_some_and(FunctionDecl::is_async)
                    {
                        active.remove(&declaration);
                        return Some(render("an asynchronous local helper", chain));
                    }
                    chain.push(target.declaration_name.clone());
                    if let Some(reason) =
                        self.callable_construction_safety_refusal(target.declaration_span, active, accepted, chain)
                    {
                        active.remove(&declaration);
                        return Some(reason);
                    }
                    chain.pop();
                }
                incan_semantics_core::SymbolOrigin::Builtin => {}
                incan_semantics_core::SymbolOrigin::Package { .. } => {
                    if !self.imported_callable_is_construction_safe(&target) {
                        chain.push(target.declaration_name.clone());
                        let reason = render(
                            "an imported callable without checked construction-safety metadata",
                            chain,
                        );
                        chain.pop();
                        active.remove(&declaration);
                        return Some(reason);
                    }
                }
                incan_semantics_core::SymbolOrigin::RustCrate(_) => {
                    chain.push(target.declaration_name.clone());
                    let reason = render("an uncertified Rust-backed callable", chain);
                    chain.pop();
                    active.remove(&declaration);
                    return Some(reason);
                }
            }
        }
        active.remove(&declaration);
        accepted.insert(declaration);
        None
    }

    /// Test inclusive-exclusive containment while keeping generated expansions in their own identity domain.
    fn construction_span_contains(owner: Span, nested: Span) -> bool {
        owner.expansion == nested.expansion && owner.start <= nested.start && nested.end <= owner.end
    }

    /// Accept only a v3 checked API summary for the exact package module and free-function identity.
    fn imported_callable_is_construction_safe(&self, identity: &incan_semantics_core::CanonicalSymbolId) -> bool {
        let incan_semantics_core::SymbolOrigin::Package { library, module_path } = &identity.origin else {
            return false;
        };
        let Some(api) = self
            .loaves
            .package_manifest(library)
            .and_then(|manifest| manifest.contract_metadata.api.as_ref())
            .filter(|api| api.schema_version >= crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION)
        else {
            return false;
        };
        api.modules
            .iter()
            .find(|module| &module.module_path == module_path)
            .into_iter()
            .flat_map(|module| &module.declarations)
            .any(|declaration| {
                matches!(
                    declaration,
                    crate::api_metadata::ApiDeclaration::Function(function)
                        if function.name == identity.declaration_name && function.construction_safe == Some(true)
                )
            })
    }

    /// Complete declaration-only derive checks after model member validation, without revisiting its fields or
    /// defaults.
    pub(super) fn complete_model_declaration(&mut self, model: &ModelDecl, has_validate: bool) {
        if has_validate
            && let Some(check) = self
                .type_info
                .declarations
                .registered_model_checks
                .get(&model.name)
                .and_then(|checks| checks.first())
        {
            self.errors.push(errors::invalid_registered_validation(
                &model.name,
                "<model>",
                "registered final checks use canonical Result[Self, ValidationError] construction and cannot be combined with the legacy Validate derive",
                check.registration_span,
            ));
        }
        if has_validate {
            self.check_validate_derive_model(model);
        }
        let member_spans = model
            .fields
            .iter()
            .map(|field| (field.node.name.as_str(), field.span))
            .collect();
        self.check_derive_contract(
            DerivedKind::Model,
            &model.name,
            &model.decorators,
            &member_spans,
            &model.methods,
        );
    }

    /// Check explicit model trait conformance after its binders exist, then retain implied derive methods and
    /// adoption facts. Awaitable and multi-instantiation surface checks remain in the model coordinator.
    pub(super) fn check_model_trait_adoptions(
        &mut self,
        model: &ModelDecl,
        derives: &[String],
    ) -> Vec<ResolvedTraitAdoption> {
        let mut resolved_trait_adoptions = Vec::new();
        for trait_ref in &model.traits {
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
                self.check_trait_conformance_model(
                    model,
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
        let derive_adoption_span = Self::first_derive_span(&model.decorators);
        for adoption in self.collect_derive_trait_adoption_infos(derives) {
            let Some(resolved) = self.resolve_collected_trait_adoption(&adoption, derive_adoption_span) else {
                continue;
            };
            self.record_body_ir_derived_methods(&model.name, &resolved);
            resolved_trait_adoptions.push(resolved);
        }
        self.expand_body_ir_model_derives(&model.name, derives);
        self.record_body_ir_trait_adoptions(&model.name, &resolved_trait_adoptions);
        resolved_trait_adoptions
    }

    /// Consume the active field inventory in source order, publishing each field before checking its default.
    /// Stored trait, recursion, automatic derive and JSON-form checks retain the existing checked field contracts.
    pub(super) fn check_model_fields(&mut self, model: &ModelDecl, fields: Vec<&Spanned<FieldDecl>>) {
        let mut json_members = Vec::new();
        for field in fields {
            let ty = self.resolve_type_checked(&field.node.ty);
            self.refuse_stored_trait_field("model", &model.name, &field.node, &ty);
            self.validate_direct_recursive_model_field(&model.name, &ty, field.span);
            self.refuse_member_without_automatic_derives(
                "model",
                &model.name,
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
                        crate::symbols::members_are_type_private(false, model.visibility),
                    ),
                    owner: Some(model.name.clone()),
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
        let adoptions = self.nominal_trait_adoptions(&model.name);
        self.refuse_members_without_json_form("model", &model.name, &adoptions, &json_members);
        self.record_adopted_json_derives(&model.name, &adoptions);
    }

    /// Reject model fields whose resolved type contains the model itself without an indirection boundary.
    pub(super) fn validate_direct_recursive_model_field(
        &mut self,
        model_name: &str,
        field_ty: &ResolvedType,
        span: Span,
    ) {
        let mut visiting = HashSet::new();
        if self.type_contains_direct_recursive_model(field_ty, model_name, true, &mut visiting) {
            self.errors.push(CompileError::type_error(
                format!(
                    "Model '{model_name}' has a direct recursive field type '{field_ty}'. Use an indirection such as List[...] for recursive payloads."
                ),
                span,
            ));
        }
    }

    /// Reject a class field that holds its own class with no `Option` or collection in between: no value of such a
    /// class can be built, and its layout has no finite size.
    pub(super) fn validate_direct_recursive_class_field(
        &mut self,
        class_name: &str,
        field: &str,
        field_ty: &ResolvedType,
        span: Span,
    ) {
        let mut visiting = HashSet::new();
        if self.type_contains_direct_recursive_model(field_ty, class_name, false, &mut visiting) {
            self.errors
                .push(errors::direct_recursive_class_field(class_name, field, span));
        }
    }

    /// Return whether a type contains the target model through only inline Rust-layout positions. With
    /// `options_inline` false an `Option` is an indirection: a class field that reaches its own class through one is
    /// stored behind a box.
    pub(super) fn type_contains_direct_recursive_model(
        &self,
        ty: &ResolvedType,
        model_name: &str,
        options_inline: bool,
        visiting: &mut HashSet<String>,
    ) -> bool {
        match ty {
            ResolvedType::Named(name) => {
                self.nominal_type_contains_direct_recursive_model(name, &[], model_name, options_inline, visiting)
            }
            ResolvedType::Generic(name, args) if name == UNION_TYPE_NAME => args
                .iter()
                .any(|arg| self.type_contains_direct_recursive_model(arg, model_name, options_inline, visiting)),
            ResolvedType::Generic(name, args) => match collection_type_id(name.as_str()) {
                Some(
                    CollectionTypeId::List
                    | CollectionTypeId::Dict
                    | CollectionTypeId::Set
                    | CollectionTypeId::FrozenList
                    | CollectionTypeId::FrozenDict
                    | CollectionTypeId::FrozenSet
                    | CollectionTypeId::Generator,
                ) => false,
                Some(CollectionTypeId::Option) if !options_inline => false,
                Some(CollectionTypeId::Tuple | CollectionTypeId::Option | CollectionTypeId::Result) => args
                    .iter()
                    .any(|arg| self.type_contains_direct_recursive_model(arg, model_name, options_inline, visiting)),
                None => {
                    self.nominal_type_contains_direct_recursive_model(name, args, model_name, options_inline, visiting)
                }
            },
            ResolvedType::Tuple(items) => items
                .iter()
                .any(|item| self.type_contains_direct_recursive_model(item, model_name, options_inline, visiting)),
            ResolvedType::Ref(_)
            | ResolvedType::RefMut(_)
            | ResolvedType::TypeToken(_)
            | ResolvedType::Trait(_)
            | ResolvedType::FrozenList(_)
            | ResolvedType::FrozenDict(_, _)
            | ResolvedType::FrozenSet(_)
            | ResolvedType::Function(_, _) => false,
            ResolvedType::Never
            | ResolvedType::Int
            | ResolvedType::Float
            | ResolvedType::Numeric(_)
            | ResolvedType::Bool
            | ResolvedType::Str
            | ResolvedType::Bytes
            | ResolvedType::FrozenStr
            | ResolvedType::FrozenBytes
            | ResolvedType::Unit
            | ResolvedType::TypeVar(_)
            | ResolvedType::SelfType
            | ResolvedType::RustPath(_)
            | ResolvedType::CallSiteInfer
            | ResolvedType::Unknown => false,
        }
    }

    /// Follow known nominal field types to find direct recursive model layouts.
    pub(super) fn nominal_type_contains_direct_recursive_model(
        &self,
        type_name: &str,
        type_args: &[ResolvedType],
        model_name: &str,
        options_inline: bool,
        visiting: &mut HashSet<String>,
    ) -> bool {
        if type_name == model_name {
            return true;
        }

        let visit_key = if type_args.is_empty() {
            type_name.to_string()
        } else {
            format!(
                "{}[{}]",
                type_name,
                type_args.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            )
        };
        if !visiting.insert(visit_key.clone()) {
            return false;
        }

        let result = match self.lookup_semantic_type_info(type_name) {
            Some(TypeInfo::Model(info)) => {
                let subst = type_param_subst_map(&info.type_params, type_args);
                info.fields.values().any(|field| {
                    let field_ty = substitute_resolved_type(&field.ty, &subst);
                    let field_ty = self.expand_type_aliases(field_ty);
                    self.type_contains_direct_recursive_model(&field_ty, model_name, options_inline, visiting)
                })
            }
            Some(TypeInfo::Class(info)) => {
                let subst = type_param_subst_map(&info.type_params, type_args);
                info.fields.values().any(|field| {
                    let field_ty = substitute_resolved_type(&field.ty, &subst);
                    let field_ty = self.expand_type_aliases(field_ty);
                    self.type_contains_direct_recursive_model(&field_ty, model_name, options_inline, visiting)
                })
            }
            Some(TypeInfo::Newtype(info)) => {
                let subst = type_param_subst_map(&info.type_params, type_args);
                let underlying = substitute_resolved_type(&info.underlying, &subst);
                let underlying = self.expand_type_aliases(underlying);
                self.type_contains_direct_recursive_model(&underlying, model_name, options_inline, visiting)
            }
            Some(TypeInfo::Enum(_) | TypeInfo::Builtin | TypeInfo::TypeAlias) | None => false,
        };

        visiting.remove(&visit_key);
        result
    }

    /// Validate a model declaration that derives validation support.
    pub(super) fn check_validate_derive_model(&mut self, model: &ModelDecl) {
        // Validate that validate() exists and has the expected signature.
        let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) else {
            return;
        };

        let Some(validate) = info.methods.get("validate") else {
            self.errors.push(errors::validate_derive_missing_validate_method(
                &model.name,
                Span::default(),
            ));
            return;
        };

        let expected = "def validate(self) -> Result[Self, E]";
        let found_sig = self.method_sig_string_named("validate", validate);

        // Receiver must exist and be immutable.
        if validate.receiver != Some(Receiver::Immutable) || validate.is_async || !validate.params.is_empty() {
            self.errors.push(errors::validate_derive_invalid_validate_signature(
                &model.name,
                expected,
                &found_sig,
                Span::default(),
            ));
            return;
        }

        // Return type must be Result[Self, E] (allow Result[ModelName, E] too).
        let ok_matches_self = |ok: &ResolvedType| {
            matches!(ok, ResolvedType::SelfType)
                || matches!(ok, ResolvedType::Named(n) if n == &model.name)
                || matches!(ok, ResolvedType::TypeVar(n) if n == &model.name)
        };

        if validate.return_type.is_result() {
            let ok_ty = validate
                .return_type
                .result_ok_type()
                .cloned()
                .unwrap_or(ResolvedType::Unknown);
            if !ok_matches_self(&ok_ty) {
                self.errors.push(errors::validate_derive_invalid_validate_signature(
                    &model.name,
                    expected,
                    &found_sig,
                    Span::default(),
                ));
            }
        } else {
            self.errors.push(errors::validate_derive_invalid_validate_signature(
                &model.name,
                expected,
                &found_sig,
                Span::default(),
            ));
        }
    }

    /// Validate that a model satisfies required fields and methods for one instantiated trait adoption.
    pub(super) fn check_trait_conformance_model(
        &mut self,
        model: &ModelDecl,
        trait_info: TraitInfo,
        trait_name: &str,
        trait_args: Option<&[ResolvedType]>,
        adoption_span: Span,
    ) {
        // Check required fields (including types)
        for (field_name, field_ty) in &trait_info.requires {
            let found = model.fields.iter().find(|f| &f.node.name == field_name);
            match found {
                None => {
                    self.errors
                        .push(errors::missing_field(&model.name, field_name, adoption_span));
                }
                Some(f) => {
                    let actual_ty = self.resolve_type_checked(&f.node.ty);
                    if !self.types_compatible(&actual_ty, field_ty) {
                        self.errors.push(errors::type_mismatch(
                            &field_ty.to_string(),
                            &actual_ty.to_string(),
                            f.node.ty.span,
                        ));
                    }
                }
            }
        }

        // Required methods: direct trait + transitive supertraits (RFC 042).
        let model_info = self
            .symbols
            .lookup(&model.name)
            .and_then(|id| self.symbols.get(id))
            .and_then(|sym| match &sym.kind {
                SymbolKind::Type(TypeInfo::Model(info)) => Some(info.clone()),
                _ => None,
            });

        if let Some(mi) = model_info {
            self.enforce_trait_abstract_methods(
                &model.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &mi.method_overloads,
            );
            self.enforce_trait_default_method_overrides(
                &model.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &mi.method_overloads,
            );
            self.enforce_trait_abstract_properties(
                &model.name,
                trait_name,
                &trait_info,
                trait_args,
                adoption_span,
                &mi.properties,
            );
        } else {
            for (method_name, method_info) in &trait_info.methods {
                if !method_info.has_body {
                    let found = model.methods.iter().any(|m| &m.node.name == method_name);
                    if !found {
                        self.errors
                            .push(errors::missing_trait_method(trait_name, method_name, adoption_span));
                    }
                }
            }
        }
    }
}
