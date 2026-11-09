//! Collection of consts, statics, models, classes, newtypes, method partials and enums.

use super::*;

impl TypeChecker {
    /// Register a module-level const binding (first pass).
    ///
    /// Note: the initializer is validated in the second pass.
    pub(super) fn collect_const(&mut self, konst: &ConstDecl, span: Span) {
        // Best-effort type from annotation; refined during const-eval in second pass.
        let ty = konst
            .ty
            .as_ref()
            .map(|t| {
                // `const` implies deep immutability; map common container annotations to frozen equivalents.
                let resolved = self.resolve_type_checked(t);
                freeze_const_type(resolved)
            })
            .unwrap_or(ResolvedType::Unknown);

        // Define as an immutable variable-like symbol for name resolution. The identity category is stated
        // explicitly: a `const` is not a body-local binding even though the table represents both as variables.
        let symbol_id = self.symbols.define_with_target_kind(
            Symbol {
                name: konst.name.clone(),
                kind: SymbolKind::Variable(VariableInfo {
                    ty,
                    is_mutable: false,
                    is_used: false,
                }),
                span,
                scope: 0,
            },
            SemanticSourceTargetKind::Const,
        );
        if self.symbols.is_active_lookup_binding(symbol_id) {
            // Remember only the active declaration for const-eval (cycle detection / evaluation).
            self.const_decls.insert(konst.name.clone(), (konst.clone(), span));
        }
    }

    /// Register a module-level static binding (first pass).
    pub(super) fn collect_static(&mut self, static_decl: &StaticDecl, span: Span) {
        let ty = self.resolve_type_checked(&static_decl.ty);
        let symbol_id = self.symbols.define(Symbol {
            name: static_decl.name.clone(),
            kind: SymbolKind::Static(StaticInfo {
                ty,
                is_public: matches!(static_decl.visibility, Visibility::Public),
                is_imported: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        let decl_index = self.static_decls.len();
        self.static_decl_positions.insert(static_decl.name.clone(), decl_index);
        self.static_decls.push((static_decl.clone(), span));
    }

    /// Register a model declaration with its fields, methods, and derived traits.
    pub(super) fn collect_model(&mut self, model: &ModelDecl, span: Span) {
        let members_private = crate::symbols::members_are_type_private(false, model.visibility);
        let fields = collect_fields(&model.fields, self, &model.name, &model.type_params, members_private);
        let properties = collect_properties(&model.properties, self, Some(&model.name), &model.type_params);
        let mut method_overloads = collect_method_overloads(
            &model.methods,
            self,
            Some(&model.name),
            &model.type_params,
            members_private,
        );
        let mut methods = collect_methods_from_overloads(&method_overloads);

        let derives = self.extract_derive_names(&model.decorators);
        let owner = self
            .symbols
            .module_declaration_identity(&model.name, SemanticSourceTargetKind::Model, span);
        let field_order: Vec<Ident> = model
            .fields
            .iter()
            .filter(|field| self.member_binding_is_active(field.span))
            .map(|field| field.node.name.clone())
            .collect();
        if let Some(constructor) = inject_validate_methods(
            &mut methods,
            &mut method_overloads,
            &owner,
            &fields,
            &field_order,
            &derives,
        ) {
            self.type_info
                .record_compiler_generated_member_identity(constructor, owner);
        }
        let method_aliases = collect_method_aliases(&model.method_aliases, self, &mut methods, &mut method_overloads);
        self.collect_method_partials(&model.name, &model.method_partials, &mut methods, &mut method_overloads);
        let mut trait_adoptions =
            self.collect_trait_adoption_infos(&model.traits, Some(&model.name), &model.type_params);
        trait_adoptions.extend(self.collect_derive_trait_adoption_infos(&derives));

        let symbol_id = self.symbols.define(Symbol {
            name: model.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
                traits: model.traits.iter().map(|t| t.node.name.clone()).collect(),
                trait_adoptions,
                derives,
                fields,
                field_order,
                properties,
                methods,
                method_overloads,
                method_aliases,
            })),
            span,
            scope: 0,
        });
        if self.symbols.is_active_lookup_binding(symbol_id) {
            self.record_local_rust_derive_paths(&model.name, &model.decorators);
        }
    }

    /// Register a class declaration, inheriting from parent if present.
    pub(super) fn collect_class(&mut self, class: &ClassDecl, span: Span) {
        let (
            mut fields,
            mut field_defaults,
            mut field_default_metadata,
            mut field_provider_libraries,
            mut field_order,
            mut properties,
            mut methods,
            mut method_overloads,
        ) = self.inherit_from_parent(&class.extends);

        // Add own fields (can override inherited ones)
        let members_private = crate::symbols::members_are_type_private(true, class.visibility);
        fields.extend(collect_fields(
            &class.fields,
            self,
            &class.name,
            &class.type_params,
            members_private,
        ));
        for field in class
            .fields
            .iter()
            .filter(|field| self.member_binding_is_active(field.span))
        {
            if !field_order.iter().any(|name| name == &field.node.name) {
                field_order.push(field.node.name.clone());
            }
            if let Some(default) = &field.node.default {
                field_defaults.insert(field.node.name.clone(), default.clone());
                field_default_metadata.remove(&field.node.name);
            } else {
                field_defaults.remove(&field.node.name);
                field_default_metadata.remove(&field.node.name);
            }
            field_provider_libraries.remove(&field.node.name);
        }
        properties.extend(collect_properties(
            &class.properties,
            self,
            Some(&class.name),
            &class.type_params,
        ));

        // Add own methods (can override inherited ones)
        let own_method_overloads = collect_method_overloads(
            &class.methods,
            self,
            Some(&class.name),
            &class.type_params,
            members_private,
        );
        methods.extend(collect_methods_from_overloads(&own_method_overloads));
        method_overloads.extend(own_method_overloads);

        let derives = self.extract_derive_names(&class.decorators);
        let method_aliases = collect_method_aliases(&class.method_aliases, self, &mut methods, &mut method_overloads);
        self.collect_method_partials(&class.name, &class.method_partials, &mut methods, &mut method_overloads);
        let mut trait_adoptions =
            self.collect_trait_adoption_infos(&class.traits, Some(&class.name), &class.type_params);
        trait_adoptions.extend(self.collect_derive_trait_adoption_infos(&derives));

        let symbol_id = self.symbols.define(Symbol {
            name: class.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
                extends: class.extends.clone(),
                traits: class.traits.iter().map(|t| t.node.name.clone()).collect(),
                trait_adoptions,
                derives,
                fields,
                field_defaults: Box::new(field_defaults),
                field_default_metadata: Box::new(field_default_metadata),
                field_provider_libraries: Box::new(field_provider_libraries),
                field_order,
                properties,
                methods,
                method_overloads,
                method_aliases,
            })),
            span,
            scope: 0,
        });
        if self.symbols.is_active_lookup_binding(symbol_id) {
            self.record_local_rust_derive_paths(&class.name, &class.decorators);
        }
    }

    /// Inherit fields and methods from a parent class if present.
    pub(super) fn inherit_from_parent(&self, extends: &Option<String>) -> InheritedMembers {
        let Some(parent_name) = extends else {
            return (
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                Vec::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
            );
        };
        let Some(TypeInfo::Class(parent_info)) = self.lookup_type_info(parent_name) else {
            return (
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                Vec::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
            );
        };
        (
            parent_info.fields.clone(),
            parent_info.field_defaults.as_ref().clone(),
            parent_info.field_default_metadata.as_ref().clone(),
            parent_info.field_provider_libraries.as_ref().clone(),
            parent_info.field_order.clone(),
            parent_info.properties.clone(),
            parent_info.methods.clone(),
            parent_info.method_overloads.clone(),
        )
    }

    /// Resolve direct trait adoptions, retaining generic trait arguments for later dispatch checks.
    pub(super) fn collect_trait_adoption_infos(
        &mut self,
        traits: &[Spanned<TraitBound>],
        owner_name: Option<&str>,
        owner_type_params: &[TypeParam],
    ) -> Vec<TypeBoundInfo> {
        let active_type_params = type_param_name_set(owner_type_params, &[]);
        let owner_self_ty = owner_name.map(|name| owner_resolved_type(name, owner_type_params));
        traits
            .iter()
            .map(|trait_ref| {
                let module_path = self.trait_bound_module_path(&trait_ref.node.name);
                TypeBoundInfo {
                    name: self.resolve_trait_bound_name(&trait_ref.node.name, trait_ref.span),
                    source_name: self.trait_bound_source_name(&trait_ref.node.name),
                    type_args: trait_ref
                        .node
                        .type_args
                        .iter()
                        .map(|arg| {
                            resolve_declared_type(self, arg, &active_type_params, owner_name, owner_self_ty.as_ref())
                        })
                        .collect(),
                    module_path,
                    implementation_type_params: Vec::new(),
                    inferred: false,
                }
            })
            .collect()
    }

    /// Register a newtype declaration with its underlying type and methods.
    pub(super) fn collect_newtype(&mut self, nt: &NewtypeDecl, span: Span) {
        let resolved_underlying = self.resolve_type_checked(&nt.underlying);
        let underlying = if nt.is_rusttype {
            self.rust_path_for_rusttype_underlying(&resolved_underlying)
                .map(ResolvedType::RustPath)
                .unwrap_or_else(|| resolved_underlying.clone())
        } else {
            resolved_underlying.clone()
        };
        let method_rebindings = nt
            .rebindings
            .iter()
            .filter_map(|rebinding| {
                Self::rebinding_target_method_name(&rebinding.node.target.node)
                    .map(|target| (rebinding.node.name.clone(), target))
            })
            .collect();
        let derives = self.extract_derive_names(&nt.decorators);
        let mut trait_adoptions = self.collect_trait_adoption_infos(&nt.traits, Some(&nt.name), &nt.type_params);
        trait_adoptions.extend(self.collect_derive_trait_adoption_infos(&derives));

        // Define a placeholder symbol FIRST so methods can reference the newtype name
        let symbol_id = self.symbols.define(Symbol {
            name: nt.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Newtype(NewtypeInfo {
                type_params: nt.type_params.iter().map(|tp| tp.name.clone()).collect(),
                is_rusttype: nt.is_rusttype,
                has_interop: !nt.interop_edges.is_empty(),
                underlying: underlying.clone(),
                constraints: newtype_constraints(&nt.underlying.node),
                implicit_coercion_enabled: newtype_allows_implicit_coercion(&nt.decorators),
                method_rebindings,
                traits: nt.traits.iter().map(|trait_ref| trait_ref.node.name.clone()).collect(),
                trait_adoptions,
                derives,
                method_aliases: HashMap::new(),
                methods: HashMap::new(), // Empty for now
                method_overloads: HashMap::new(),
            })),
            span,
            scope: 0,
        });
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        self.record_local_rust_derive_paths(&nt.name, &nt.decorators);

        // Now collect methods - they can reference the newtype name
        let members_private = crate::symbols::members_are_type_private(false, nt.visibility);
        let mut method_overloads =
            collect_method_overloads(&nt.methods, self, Some(&nt.name), &nt.type_params, members_private);
        let mut methods = collect_methods_from_overloads(&method_overloads);
        let method_aliases = collect_method_aliases(&nt.method_aliases, self, &mut methods, &mut method_overloads);
        self.collect_method_partials(&nt.name, &nt.method_partials, &mut methods, &mut method_overloads);

        // Update the symbol with the collected methods
        if let Some(sym) = self.symbols.get_mut(symbol_id)
            && let SymbolKind::Type(TypeInfo::Newtype(info)) = &mut sym.kind
        {
            info.methods = methods;
            info.method_aliases = method_aliases;
            info.method_overloads = method_overloads;
        }
    }

    /// Register same-type method partials as projected method metadata on the owning type surface.
    pub(super) fn collect_method_partials(
        &mut self,
        owner_name: &str,
        partials: &[Spanned<MethodPartialDecl>],
        methods: &mut HashMap<String, MethodInfo>,
        overloads: &mut HashMap<String, Vec<MethodInfo>>,
    ) {
        for partial in partials {
            if !self.member_binding_is_active(partial.span) {
                continue;
            }
            let target = partial.node.target.as_str();
            let Some(target_info) = methods.get(target).cloned() else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Method partial '{}.{}' targets unknown method '{}'",
                        owner_name, partial.node.name, partial.node.target
                    ),
                    partial.span,
                ));
                continue;
            };
            let Some(params) = self.project_partial_params(
                &format!("{owner_name}.{}", partial.node.name),
                &format!("{owner_name}.{target}"),
                target_info.params.clone(),
                &partial.node.args,
                partial.span,
            ) else {
                continue;
            };
            let mut projected = target_info;
            projected.params = params;
            projected.has_body = true;
            projected.alias_of = Some(target.to_string());
            methods.insert(partial.node.name.clone(), projected.clone());
            overloads.insert(partial.node.name.clone(), vec![projected]);
        }
    }

    /// Extract the effective target method name for a `alias = target` rebinding declaration.
    ///
    /// We accept both:
    /// - `alias = method_name`
    /// - `alias = TypeOrValue.method_name` (last segment is the target method)
    pub(super) fn rebinding_target_method_name(target: &Expr) -> Option<String> {
        match target {
            Expr::Ident(name) => Some(name.clone()),
            Expr::Field(_, member) => Some(member.clone()),
            _ => None,
        }
    }

    /// Register an enum declaration and define symbols for each variant.
    pub(super) fn collect_enum(&mut self, en: &EnumDecl, span: Span) {
        let active_variants = en
            .variants
            .iter()
            .filter(|variant| self.member_binding_is_active(variant.span))
            .collect::<Vec<_>>();
        let active_variant_aliases = en
            .variant_aliases
            .iter()
            .filter(|alias| self.member_binding_is_active(alias.span))
            .collect::<Vec<_>>();
        let variants = active_variants
            .iter()
            .map(|variant| variant.node.name.clone())
            .collect::<Vec<_>>();
        let variant_fields: HashMap<_, _> = active_variants
            .iter()
            .map(|variant| {
                let fields = variant
                    .node
                    .fields
                    .iter()
                    .map(|field| self.resolve_type_checked(field))
                    .collect();
                (variant.node.name.clone(), fields)
            })
            .collect();
        let variant_aliases: HashMap<_, _> = active_variant_aliases
            .iter()
            .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
            .collect();
        let mut variant_identities = active_variants
            .iter()
            .map(|variant| {
                (
                    variant.node.name.clone(),
                    self.symbols.member_declaration_identity(
                        &variant.node.name,
                        SemanticSourceTargetKind::Variant,
                        variant.span,
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        for (alias, target) in &variant_aliases {
            if let Some(identity) = variant_identities.get(target).cloned() {
                variant_identities.insert(alias.clone(), identity);
            }
        }
        let derives = self.extract_derive_names(&en.decorators);
        let value_enum = en.value_type.as_ref().map(|value_type| ValueEnumInfo {
            value_type: value_enum_backing(value_type.node),
            values: active_variants
                .iter()
                .filter_map(|variant| {
                    variant
                        .node
                        .value
                        .as_ref()
                        .map(|value| (variant.node.name.clone(), value_enum_value_from_literal(&value.node)))
                })
                .collect(),
        });

        let mut trait_adoptions = self.collect_trait_adoption_infos(&en.traits, Some(&en.name), &en.type_params);
        trait_adoptions.extend(self.collect_derive_trait_adoption_infos(&derives));
        let symbol_id = self.symbols.define(Symbol {
            name: en.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
                traits: en.traits.iter().map(|t| t.node.name.clone()).collect(),
                trait_adoptions,
                variants: variants.clone(),
                variant_identities: variant_identities.clone(),
                variant_fields: variant_fields.clone(),
                variant_aliases: variant_aliases.clone(),
                value_enum,
                derives,
                methods: HashMap::new(),
                method_overloads: HashMap::new(),
            })),
            span,
            scope: 0,
        });
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        self.record_local_rust_derive_paths(&en.name, &en.decorators);

        let members_private = crate::symbols::members_are_type_private(false, en.visibility);
        let method_overloads =
            collect_method_overloads(&en.methods, self, Some(&en.name), &en.type_params, members_private);
        let methods = collect_methods_from_overloads(&method_overloads);
        if let Some(sym) = self.symbols.get_mut(symbol_id)
            && let SymbolKind::Type(TypeInfo::Enum(info)) = &mut sym.kind
        {
            info.methods = methods;
            info.method_overloads = method_overloads;
        }

        self.define_enum_variant_symbols(
            en,
            &active_variants,
            &active_variant_aliases,
            &variant_fields,
            &variant_identities,
        );
    }

    /// Publish active enum variants before their aliases, preserving existing bindings and each target's canonical
    /// identity. Alias payload types retain their existing checked resolution after method registration.
    pub(super) fn define_enum_variant_symbols(
        &mut self,
        en: &EnumDecl,
        active_variants: &[&Spanned<VariantDecl>],
        active_variant_aliases: &[&Spanned<EnumVariantAliasDecl>],
        variant_fields: &HashMap<String, Vec<ResolvedType>>,
        variant_identities: &HashMap<String, CanonicalSymbolId>,
    ) {
        for variant in active_variants {
            let fields = variant_fields.get(&variant.node.name).cloned().unwrap_or_default();
            let variant_id = self.symbols.define_preserving_existing_binding(Symbol {
                name: variant.node.name.clone(),
                kind: SymbolKind::Variant(VariantInfo {
                    identity: Some(self.symbols.member_declaration_identity(
                        &variant.node.name,
                        SemanticSourceTargetKind::Variant,
                        variant.span,
                    )),
                    enum_name: en.name.clone(),
                    fields,
                }),
                span: variant.span,
                scope: 0,
            });
            debug_assert_eq!(
                self.symbols.identity_of(variant_id),
                variant_identities.get(&variant.node.name)
            );
        }
        for alias in active_variant_aliases {
            if let Some(target_variant) = en
                .variants
                .iter()
                .find(|variant| self.member_binding_is_active(variant.span) && variant.node.name == alias.node.target)
            {
                let fields: Vec<_> = target_variant
                    .node
                    .fields
                    .iter()
                    .map(|f| self.resolve_type_checked(f))
                    .collect();
                let target_identity = variant_identities.get(&alias.node.target).cloned();
                self.symbols.define_alias_preserving_existing_binding(
                    Symbol {
                        name: alias.node.name.clone(),
                        kind: SymbolKind::Variant(VariantInfo {
                            identity: target_identity.clone(),
                            enum_name: en.name.clone(),
                            fields,
                        }),
                        span: alias.span,
                        scope: 0,
                    },
                    target_identity,
                );
            }
        }
    }
}
