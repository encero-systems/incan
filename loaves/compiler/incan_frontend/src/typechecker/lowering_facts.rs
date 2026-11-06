//! Trait, newtype, class-layout and model-field facts recorded for lowering.

use super::*;

impl TypeChecker {
    /// Record each source trait adoption clause's checked type arguments by the clause's span.
    ///
    /// An owner's collected adoptions list its `with` clauses first and in source order, followed by derive-implied
    /// adoptions, so the clauses pair with that prefix. A clause whose paired adoption names a different trait or a
    /// different number of arguments records nothing.
    pub(super) fn record_trait_adoption_arguments_for_lowering(&mut self, program: &Program) {
        let mut recorded = Vec::new();
        for declaration in &program.declarations {
            let (name, clauses) = match &declaration.node {
                Declaration::Model(model) => (&model.name, &model.traits),
                Declaration::Class(class) => (&class.name, &class.traits),
                Declaration::Newtype(newtype) => (&newtype.name, &newtype.traits),
                Declaration::Enum(value) => (&value.name, &value.traits),
                _ => continue,
            };
            let adoptions = match self.lookup_type_info(name) {
                Some(TypeInfo::Model(info)) => &info.trait_adoptions,
                Some(TypeInfo::Class(info)) => &info.trait_adoptions,
                Some(TypeInfo::Newtype(info)) => &info.trait_adoptions,
                Some(TypeInfo::Enum(info)) => &info.trait_adoptions,
                _ => continue,
            };
            for (clause, adoption) in clauses.iter().zip(adoptions) {
                if clause.node.type_args.is_empty()
                    || adoption.name != clause.node.name
                    || adoption.type_args.len() != clause.node.type_args.len()
                {
                    continue;
                }
                recorded.push((clause.span, adoption.type_args.clone()));
            }
        }
        self.type_info.traits.adoption_type_args = recorded.into_iter().collect();
    }

    /// Retain bare source adoptions' implicit arguments for Body IR without changing legacy's explicit snapshot.
    /// Clauses pair with the checked owner's source-ordered adoption prefix, and repeated trait names stay unproven
    /// rather than borrowing another clause's instantiation. The existing semantic helper owns positional mapping and
    /// exact arity; no lowering consumer reconstructs it from parameter spellings.
    pub(super) fn record_body_ir_bare_adoption_arguments(&mut self, program: &Program) {
        let mut recorded = HashMap::new();
        for declaration in &program.declarations {
            let (name, clauses) = match &declaration.node {
                Declaration::Model(model) => (&model.name, &model.traits),
                Declaration::Class(class) => (&class.name, &class.traits),
                Declaration::Newtype(newtype) => (&newtype.name, &newtype.traits),
                Declaration::Enum(value) => (&value.name, &value.traits),
                _ => continue,
            };
            // ---- Declared owner parameter witnesses ----
            let Some((parameters, adoptions)) = self.semantic_type_params_and_adoptions(name) else {
                continue;
            };
            let owner_arguments: Vec<_> = parameters
                .iter()
                .map(|name| ResolvedType::TypeVar(name.clone()))
                .collect();

            // ---- Bare clauses at one checked instantiation ----
            for (clause, adoption) in clauses.iter().zip(adoptions) {
                if !clause.node.type_args.is_empty() || adoption.name != clause.node.name {
                    continue;
                }
                if adoptions
                    .iter()
                    .filter(|candidate| self.trait_name_matches(&candidate.name, &adoption.name))
                    .count()
                    != 1
                {
                    continue;
                }
                let Some(arguments) = self.instantiated_trait_args_for_type(name, &owner_arguments, &adoption.name)
                else {
                    continue;
                };
                if !arguments.is_empty() {
                    recorded.insert(clause.span, arguments);
                }
            }
        }

        // ---- Body IR-only snapshot ----
        self.type_info.traits.body_ir_adoption_type_args = recorded;
    }

    /// Snapshot declaration metadata into [`TypeCheckInfo`] for backend lowering.
    ///
    /// This records all visible trait and method-alias symbols (local and imported), not just declarations in the
    /// current module, so lowering can resolve supertrait graphs, generic trait arity, and same-type method aliases
    /// across module boundaries. Newtype construction retains the selected hook's checked signature and identity
    /// so imported validation calls use the declaring module's contract.
    pub(super) fn record_trait_metadata_for_lowering(&mut self) {
        self.type_info.traits.direct_supertraits.clear();
        self.type_info.traits.type_params.clear();
        self.type_info.traits.method_identities.clear();
        self.type_info.traits.default_method_identities.clear();
        self.type_info.declarations.type_method_rebindings.clear();
        self.type_info.declarations.newtype_construction.clear();
        self.type_info.rust.type_aliases.clear();
        self.type_info.derivations.trait_rust_derive_paths = self.dependency_trait_rust_derive_paths.clone();
        let mut method_rebindings = Vec::new();
        let mut newtype_construction = Vec::new();
        let mut adopted_traits = Vec::new();
        for sym in self.symbols.all_symbols() {
            match &sym.kind {
                SymbolKind::Trait(info) => {
                    self.type_info
                        .traits
                        .direct_supertraits
                        .insert(sym.name.clone(), info.supertraits.clone());
                    self.type_info
                        .traits
                        .type_params
                        .insert(sym.name.clone(), info.type_params.clone());
                    for (method_name, method) in &info.methods {
                        self.type_info.traits.record_method(&sym.name, method_name, method);
                    }
                    method_rebindings.push((sym.name.clone(), info.method_aliases.clone()));
                }
                SymbolKind::Type(TypeInfo::Model(info)) => {
                    method_rebindings.push((sym.name.clone(), info.method_aliases.clone()));
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                SymbolKind::Type(TypeInfo::Class(info)) => {
                    method_rebindings.push((sym.name.clone(), info.method_aliases.clone()));
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                SymbolKind::Type(TypeInfo::Newtype(info)) => {
                    let mut aliases = info.method_rebindings.clone();
                    aliases.extend(info.method_aliases.clone());
                    method_rebindings.push((sym.name.clone(), aliases));
                    if !info.is_rusttype {
                        newtype_construction.push((sym.name.clone(), info.clone()));
                    } else {
                        let alias = ResolvedType::Generic(
                            sym.name.clone(),
                            info.type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
                        );
                        if let Some(template) = self.rusttype_native_target(&alias) {
                            let template = self.rust_storage_resolved(&template, &info.type_params);
                            self.type_info
                                .rust
                                .type_aliases
                                .insert(sym.name.clone(), (info.type_params.clone(), template));
                        }
                    }
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                SymbolKind::Type(TypeInfo::Enum(info)) => {
                    adopted_traits.extend(info.trait_adoptions.clone());
                }
                _ => {}
            }
        }
        self.record_standard_trait_metadata_for_lowering();
        for (qualified_trait_name, info) in &self.dependency_module_traits {
            for (method_name, method) in &info.methods {
                self.type_info
                    .traits
                    .record_method(qualified_trait_name, method_name, method);
            }
        }
        for adoption in adopted_traits {
            let Some(info) = self.lookup_trait_adoption_info(&adoption).cloned() else {
                continue;
            };
            for (method_name, method) in &info.methods {
                self.type_info.traits.record_method(&adoption.name, method_name, method);
            }
        }
        self.record_standard_trait_methods();
        for (type_name, aliases) in method_rebindings {
            if !aliases.is_empty() {
                self.type_info
                    .declarations
                    .type_method_rebindings
                    .insert(type_name, aliases);
            }
        }
        self.record_newtype_construction_metadata(newtype_construction);
    }

    /// Consume the staged newtype snapshots after trait methods and rebindings establish their construction contracts.
    pub(super) fn record_newtype_construction_metadata(&mut self, newtypes: Vec<(String, NewtypeInfo)>) {
        let string_conversion = trait_capabilities::string_try_from();
        for (name, info) in newtypes {
            let concrete_type = if info.type_params.is_empty() {
                ResolvedType::Named(name.clone())
            } else {
                ResolvedType::Generic(
                    name.clone(),
                    info.type_params.iter().cloned().map(ResolvedType::TypeVar).collect(),
                )
            };
            let supports_string_conversion = self
                .temporary_trait_capability_supports_type(string_conversion, &concrete_type)
                .unwrap_or(false);
            let checked_constructor = self.validated_newtype_ctor_name(&name, &info);
            let checked_constructor_binding = checked_constructor
                .as_deref()
                .and_then(|constructor| info.methods.get(constructor))
                .map(|method| Self::newtype_constructor_binding(method, concrete_type));
            let checked_constructor_identity = checked_constructor_binding
                .as_ref()
                .and_then(|binding| binding.identity.clone());
            let automatic_derives = self.newtype_automatic_derive_names(&name, &info);
            // The selection itself is not retained; whether native derives cover every `@rust.derive` is.
            let mut derives = info.derives;
            Self::append_implied_derives(&mut derives);
            if self.append_native_rust_derives(&name, &mut derives) {
                self.type_info
                    .declarations
                    .rust_native_derive_types
                    .insert(name.clone());
            }
            self.type_info.declarations.newtype_construction.insert(
                name,
                crate::typechecker::type_info::NewtypeConstructionInfo {
                    automatic_derives,
                    type_params: info.type_params,
                    underlying: info.underlying,
                    checked_constructor,
                    checked_constructor_identity,
                    checked_constructor_binding,
                    constraints: info.constraints,
                    implicit_coercion_enabled: info.implicit_coercion_enabled,
                    supports_string_conversion,
                },
            );
        }
    }

    /// Retain loaded standard supertraits under their declaring module, without exposing their bare names in source.
    /// An imported Ord's Eq slots must survive even when the consumer imports no Eq or declares its own Eq.
    pub(super) fn record_standard_trait_metadata_for_lowering(&mut self) {
        for (name, info) in &self.standard_traits {
            let Some(module) = info
                .methods
                .values()
                .filter_map(|method| method.identity.as_ref())
                .find_map(|identity| self.standard_declaring_module(identity))
            else {
                continue;
            };
            let key = format!("{}.{name}", module.join("."));
            self.type_info
                .traits
                .direct_supertraits
                .insert(key.clone(), info.supertraits.clone());
            self.type_info
                .traits
                .type_params
                .insert(key.clone(), info.type_params.clone());
            for (method_name, method) in &info.methods {
                self.type_info.traits.record_method(&key, method_name, method);
            }
        }
    }

    /// Record which visible trait slots the standard packages declare, after every slot is recorded.
    ///
    /// Aliases and facade imports keep a slot's provider identity, so the identity's origin decides; a slot of an
    /// ordinary package dependency stays eligible for project trait implementations.
    pub(super) fn record_standard_trait_methods(&mut self) {
        let loaves = &self.loaves;
        self.type_info.traits.standard_method_identities = self
            .type_info
            .traits
            .method_identities
            .values()
            .filter(|identity| match &identity.origin {
                SymbolOrigin::Package { library, .. } => loaves.is_standard_package(library),
                _ => false,
            })
            .cloned()
            .collect();
    }

    /// Snapshot checked class layouts for backend lowering.
    ///
    /// The symbol table already contains inherited fields from source and compiled-library parents. Persisting that
    /// flattened view makes it the semantic authority for lowering and avoids requiring imported classes to have a
    /// synthetic consumer-side AST declaration.
    pub(super) fn record_class_layouts_for_lowering(&mut self, program: &Program) {
        let mut layouts = HashMap::new();
        for declaration in &program.declarations {
            let Declaration::Class(class) = &declaration.node else {
                continue;
            };
            let Some(TypeInfo::Class(info)) = self.lookup_type_info(&class.name).cloned() else {
                continue;
            };
            let mut fields = Vec::with_capacity(info.field_order.len());
            let mut missing_fields = Vec::new();
            let mut unmaterializable_defaults = Vec::new();
            for name in &info.field_order {
                let Some(field) = info.fields.get(name) else {
                    missing_fields.push(name.clone());
                    continue;
                };
                let provider_library = info.field_provider_libraries.get(name).cloned();
                let mut canonical_ty = field.ty.clone();
                self.canonicalize_public_library_nominals(&mut canonical_ty);
                if let Err(message) = crate::rust_type_display::validate_emission_display(&canonical_ty) {
                    self.errors.push(
                        CompileError::type_error(
                            format!(
                                "Checked class '{}' has invalid Rust type metadata for field '{}': {}",
                                class.name, name, message
                            ),
                            declaration.span,
                        )
                        .with_hint("Rebuild the declaring library with a compatible Incan compiler"),
                    );
                }
                if let Some(provider) = provider_library.as_deref()
                    && let Err(message) =
                        crate::rust_type_display::qualify_for_manifest(&canonical_ty, Some(provider), &info.type_params)
                {
                    self.errors.push(
                        CompileError::type_error(
                            format!(
                                "Checked class '{}' cannot republish Rust type metadata for field '{}': {}",
                                class.name, name, message
                            ),
                            declaration.span,
                        )
                        .with_hint("Rebuild the declaring library with a compatible Incan compiler"),
                    );
                }
                let default = self.checked_class_field_default(
                    &class.name,
                    &info,
                    name,
                    provider_library.as_deref(),
                    declaration.span,
                    &mut unmaterializable_defaults,
                );
                if field.has_default && default.is_none() {
                    unmaterializable_defaults.push(name.clone());
                    self.errors.push(CompileError::type_error(
                        format!(
                            "Checked class '{}' has no lowering plan for default field '{}'",
                            class.name, name
                        ),
                        declaration.span,
                    ));
                }
                fields.push(type_info::ClassFieldLayoutInfo {
                    name: name.clone(),
                    ty: canonical_ty,
                    surface_type_name: field.surface_type_name.clone(),
                    visibility: field.visibility,
                    default,
                    alias: field.alias.clone(),
                    description: field.description.clone(),
                });
            }
            unmaterializable_defaults.sort();
            unmaterializable_defaults.dedup();
            layouts.insert(
                class.name.clone(),
                type_info::ClassLayoutInfo {
                    is_public: matches!(class.visibility, Visibility::Public),
                    type_params: info.type_params.clone(),
                    fields,
                    missing_fields,
                    unmaterializable_defaults,
                },
            );
        }
        self.type_info.declarations.class_layouts = layouts;
    }

    /// Resolve a field's local or inherited default, preserving provider failures before layout completeness checks.
    pub(super) fn checked_class_field_default(
        &mut self,
        class_name: &str,
        info: &ClassInfo,
        name: &str,
        provider: Option<&str>,
        span: Span,
        unmaterializable_defaults: &mut Vec<String>,
    ) -> Option<type_info::ClassFieldDefaultInfo> {
        let Some(value) = info.field_default_metadata.get(name) else {
            return info
                .field_defaults
                .get(name)
                .cloned()
                .map(type_info::ClassFieldDefaultInfo::Source);
        };
        if let Some(provider) = provider
            && let Err(message) =
                crate::library_exports::qualify_checked_default_for_manifest(value, provider, &info.type_params)
        {
            unmaterializable_defaults.push(name.to_owned());
            self.errors.push(
                CompileError::type_error(
                    format!(
                        "Checked class '{class_name}' cannot republish default metadata for field '{name}': {message}"
                    ),
                    span,
                )
                .with_hint("Rebuild the declaring library with a compatible Incan compiler"),
            );
        }
        match (provider, value) {
            (Some(library), crate::library_exports::CheckedParamDefault::Unsupported) => {
                unmaterializable_defaults.push(name.to_owned());
                self.errors.push(
                    CompileError::type_error(
                        format!("Class '{class_name}' cannot inherit compiled default for field '{name}' because the provider artifact cannot materialize that expression"), span,
                    ).with_hint(format!("Override '{name}' in '{class_name}' with a local default expression")),
                );
                Some(type_info::ClassFieldDefaultInfo::PublicDependency {
                    library: library.to_owned(),
                    value: value.clone(),
                })
            }
            (Some(library), _) => Some(type_info::ClassFieldDefaultInfo::PublicDependency {
                library: library.to_owned(),
                value: value.clone(),
            }),
            (None, _) => {
                unmaterializable_defaults.push(name.to_owned());
                self.errors.push(
                    CompileError::type_error(
                        format!("Checked class '{class_name}' lost the dependency identity for inherited default field '{name}'"), span,
                    ).with_hint("Re-import the compiled parent through its declared package dependency"),
                );
                None
            }
        }
    }

    /// Snapshot checked model field visibility for backend lowering.
    ///
    /// The parser preserves authored modifiers and applies the compatible public-model default, while the symbol table
    /// is the frontend's effective semantic view. Keeping only that checked view downstream prevents lowering from
    /// independently reconstructing privacy from syntax.
    pub(super) fn record_model_field_visibilities_for_lowering(&mut self, program: &Program) {
        let mut model_field_visibilities = HashMap::new();
        let mut model_field_types = HashMap::new();
        let mut model_type_private_fields = HashSet::new();
        for declaration in &program.declarations {
            let Declaration::Model(model) = &declaration.node else {
                continue;
            };
            let Some(TypeInfo::Model(info)) = self.lookup_type_info(&model.name) else {
                continue;
            };
            for field in &model.fields {
                if let Some(checked) = info.fields.get(&field.node.name) {
                    model_field_types.insert(field.span, checked.ty.clone());
                }
            }
            model_field_visibilities.insert(
                model.name.clone(),
                info.fields
                    .iter()
                    .map(|(name, field)| (name.clone(), field.visibility))
                    .collect(),
            );
            model_type_private_fields.extend(
                info.fields
                    .iter()
                    .filter(|(_, field)| field.is_type_private)
                    .map(|(name, _)| (model.name.clone(), name.clone())),
            );
        }
        self.type_info.declarations.model_field_types = model_field_types;
        self.type_info.declarations.model_field_visibilities = model_field_visibilities;
        self.type_info.declarations.model_type_private_fields = model_type_private_fields;
    }
}
