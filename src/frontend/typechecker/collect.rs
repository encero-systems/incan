//! First-pass collection: register types, functions, and imports into the symbol table.

use std::collections::{HashMap, HashSet};

use crate::frontend::ast::*;
use crate::frontend::diagnostics::CompileError;
use crate::frontend::diagnostics::errors;
use crate::frontend::resolved_type_subst::{substitute_resolved_type, type_param_subst_map};
use crate::frontend::symbols::*;
use crate::frontend::typechecker::helpers::freeze_const_type;
use incan_core::lang::decorators::{self as core_decorators, DecoratorId};

use super::{FunctionBindingInfo, TypeChecker};

mod decl_helpers;
pub(super) mod decorators;
mod stdlib_imports;

use self::decl_helpers::{
    collect_fields, collect_method_aliases, collect_method_overloads, collect_methods_from_overloads,
    collect_properties, inject_validate_methods, owner_resolved_type, resolve_declared_type, type_param_name_set,
};

type InheritedMembers = (
    HashMap<String, FieldInfo>,
    HashMap<String, PropertyInfo>,
    HashMap<String, MethodInfo>,
    HashMap<String, Vec<MethodInfo>>,
);

type PartialCallableSignature = (
    Vec<CallableParam>,
    ResolvedType,
    bool,
    Vec<String>,
    HashMap<String, Vec<String>>,
    HashMap<String, Vec<TypeBoundInfo>>,
);

/// Extract constrained-primitive predicates from a newtype underlying annotation.
fn newtype_constraints(ty: &Type) -> Vec<NewtypePrimitiveConstraint> {
    let Type::ConstrainedPrimitive(_, constraints) = ty else {
        return Vec::new();
    };
    constraints
        .iter()
        .map(|constraint| NewtypePrimitiveConstraint {
            key: constraint.node.key,
            value: constraint.node.value.value,
            repr: constraint.node.value.repr.clone(),
        })
        .collect()
}

/// Return whether a newtype declaration permits RFC 017 implicit coercion.
fn newtype_allows_implicit_coercion(decorators: &[Spanned<Decorator>]) -> bool {
    !decorators.iter().any(|decorator| {
        core_decorators::from_segments(&decorator.node.path.segments) == Some(DecoratorId::NoImplicitCoercion)
    })
}

/// Convert parsed value enum backing syntax into symbol-table metadata.
fn value_enum_backing(value_type: ValueEnumType) -> ValueEnumBacking {
    match value_type {
        ValueEnumType::Str => ValueEnumBacking::Str,
        ValueEnumType::Int => ValueEnumBacking::Int,
    }
}

/// Convert parsed value enum raw literals into symbol-table metadata.
fn value_enum_value_from_literal(value: &ValueEnumLiteral) -> ValueEnumValue {
    match value {
        ValueEnumLiteral::Str(value) => ValueEnumValue::Str(value.clone()),
        ValueEnumLiteral::Int(value) => ValueEnumValue::Int(value.value),
    }
}

impl TypeChecker {
    // ========================================================================
    // First pass: collect declarations
    // ========================================================================

    /// Register a declaration in the symbol table (first pass).
    ///
    /// Dispatches to `collect_import`, `collect_model`, etc. to populate the [`SymbolTable`] with type, function,
    /// and trait definitions. Bodies are **not** validated here; that happens in
    /// [`check_declaration`](Self::check_declaration) in the second pass.
    pub(crate) fn collect_declaration(&mut self, decl: &Spanned<Declaration>) {
        match &decl.node {
            Declaration::Import(import) => self.collect_import(import, decl.span),
            Declaration::Const(konst) => {
                if konst.name == "__derives__" {
                    return;
                }
                self.validate_root_namespace(&konst.name, decl.span);
                self.collect_const(konst, decl.span);
            }
            Declaration::Static(static_decl) => {
                self.validate_root_namespace(&static_decl.name, decl.span);
                self.collect_static(static_decl, decl.span);
            }
            Declaration::Model(model) => {
                self.validate_root_namespace(&model.name, decl.span);
                self.collect_model(model, decl.span);
            }
            Declaration::Class(class) => {
                self.validate_root_namespace(&class.name, decl.span);
                self.collect_class(class, decl.span);
            }
            Declaration::Trait(tr) => {
                self.validate_root_namespace(&tr.name, decl.span);
                self.collect_trait(tr, decl.span);
            }
            Declaration::Alias(alias) => {
                self.validate_root_namespace(&alias.name, decl.span);
                self.collect_alias(alias, decl.span);
            }
            Declaration::Partial(partial) => {
                self.validate_root_namespace(&partial.name, decl.span);
                self.collect_partial(partial, decl.span);
            }
            Declaration::TypeAlias(a) => {
                self.validate_root_namespace(&a.name, decl.span);
                // Register the alias name as a known type so other declarations can reference it.
                self.symbols.define(Symbol {
                    name: a.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::TypeAlias),
                    span: decl.span,
                    scope: 0,
                });
                self.register_type_alias_target(a);
            }
            Declaration::Newtype(nt) => {
                self.validate_root_namespace(&nt.name, decl.span);
                self.collect_newtype(nt, decl.span);
            }
            Declaration::Enum(en) => {
                self.validate_root_namespace(&en.name, decl.span);
                self.collect_enum(en, decl.span);
            }
            Declaration::Function(func) => {
                self.validate_root_namespace(&func.name, decl.span);
                self.collect_function(func, decl.span);
            }
            Declaration::Docstring(_) | Declaration::TestModule(_) => {} // Docstrings/tests don't need root collection
        }
    }

    /// Register a module-level alias after concrete symbols have been collected.
    fn collect_alias(&mut self, alias: &AliasDecl, span: Span) {
        let target_name = alias.target.segments.join(".");
        let Some(kind) = self.alias_target_symbol_kind(&alias.target.segments) else {
            self.errors.push(CompileError::type_error(
                format!("Alias '{}' targets unknown symbol '{}'", alias.name, target_name),
                span,
            ));
            return;
        };

        let kind = match kind {
            SymbolKind::Function(info) => SymbolKind::Function(info),
            SymbolKind::Type(info) => SymbolKind::Type(info),
            SymbolKind::Trait(info) => SymbolKind::Trait(info),
            other => {
                self.errors.push(CompileError::type_error(
                    format!("Alias '{}' targets unsupported symbol '{}'", alias.name, target_name),
                    span,
                ));
                tracing::debug!(?other, alias = %alias.name, target = %target_name, "unsupported alias target");
                return;
            }
        };

        self.symbols.define(Symbol {
            name: alias.name.clone(),
            kind,
            span,
            scope: 0,
        });
    }

    /// Resolve an alias target path to the semantic symbol kind it projects.
    ///
    /// Single-segment targets use ordinary module-scope lookup. Qualified targets must begin with an imported module
    /// binding and are resolved through stdlib or `pub::` library metadata so `lib.name` cannot accidentally fall back
    /// to an unrelated local `name`.
    fn alias_target_symbol_kind(&mut self, segments: &[String]) -> Option<SymbolKind> {
        match segments {
            [name] => self.lookup_symbol(name).map(|symbol| symbol.kind.clone()),
            [module_name, rest @ ..] => {
                let member = rest.last()?;
                let module_path = {
                    let symbol = self.lookup_symbol(module_name)?;
                    let SymbolKind::Module(info) = &symbol.kind else {
                        return None;
                    };
                    if info.is_python {
                        return None;
                    }
                    let mut module_path = info.path.clone();
                    module_path.extend_from_slice(&rest[..rest.len().saturating_sub(1)]);
                    module_path
                };
                if let Some(info) = self.stdlib_cache.lookup_function(&module_path, member) {
                    return Some(SymbolKind::Function(info));
                }
                if let Some(info) = self.stdlib_cache.lookup_type(&module_path, member) {
                    return Some(SymbolKind::Type(info));
                }
                if let Some(info) = self.stdlib_cache.lookup_trait(&module_path, member) {
                    return Some(SymbolKind::Trait(info));
                }
                if module_path.len() == 2 && module_path.first().is_some_and(|seg| seg == "pub") {
                    return self.lookup_pub_library_symbol_member(&module_path[1], member);
                }
                None
            }
            [] => None,
        }
    }

    /// Register a module-level partial as a projected callable symbol.
    fn collect_partial(&mut self, partial: &PartialDecl, span: Span) {
        let target_name = partial.target.segments.join(".");
        let Some(kind) = self.alias_target_symbol_kind(&partial.target.segments) else {
            self.errors.push(CompileError::type_error(
                format!("Partial '{}' targets unknown callable '{}'", partial.name, target_name),
                span,
            ));
            return;
        };
        let Some((params, return_type, is_async, type_params, type_param_bounds, type_param_bound_details)) =
            Self::partial_callable_signature_from_kind(&partial.target.segments, kind)
        else {
            self.errors.push(CompileError::type_error(
                format!(
                    "Partial '{}' targets unsupported symbol '{}'; expected a function, constructor, alias, or partial",
                    partial.name, target_name
                ),
                span,
            ));
            return;
        };

        let Some(params) = self.project_partial_params(&partial.name, &target_name, params, &partial.args, span) else {
            return;
        };

        self.symbols.define(Symbol {
            name: partial.name.clone(),
            kind: SymbolKind::Function(FunctionInfo {
                params,
                return_type,
                is_async,
                type_params,
                type_param_bounds,
                type_param_bound_details,
            }),
            span,
            scope: 0,
        });
    }

    /// Resolve the callable surface that a top-level partial declaration projects from an already-resolved symbol.
    fn partial_callable_signature_from_kind(segments: &[String], kind: SymbolKind) -> Option<PartialCallableSignature> {
        match kind {
            SymbolKind::Function(info) => Some((
                info.params,
                info.return_type,
                info.is_async,
                info.type_params,
                info.type_param_bounds,
                info.type_param_bound_details,
            )),
            SymbolKind::Type(TypeInfo::Model(info)) => Some((
                Self::constructor_params_from_fields(&info.fields),
                ResolvedType::Named(segments.last()?.clone()),
                false,
                info.type_params,
                HashMap::new(),
                HashMap::new(),
            )),
            SymbolKind::Type(TypeInfo::Class(info)) => Some((
                Self::constructor_params_from_fields(&info.fields),
                ResolvedType::Named(segments.last()?.clone()),
                false,
                info.type_params,
                HashMap::new(),
                HashMap::new(),
            )),
            SymbolKind::Type(TypeInfo::Newtype(info)) => Some((
                vec![CallableParam::named("value", info.underlying, ParamKind::Normal)],
                ResolvedType::Named(segments.last()?.clone()),
                false,
                info.type_params,
                HashMap::new(),
                HashMap::new(),
            )),
            _ => None,
        }
    }

    /// Convert collected field metadata into constructor callable parameters.
    fn constructor_params_from_fields(fields: &HashMap<String, FieldInfo>) -> Vec<CallableParam> {
        let mut params: Vec<_> = fields
            .iter()
            .map(|(name, info)| {
                CallableParam::named_with_default(name.clone(), info.ty.clone(), ParamKind::Normal, info.has_default)
            })
            .collect();
        params.sort_by(|a, b| a.name().cmp(&b.name()));
        params
    }

    /// Apply partial preset keywords to callable parameters by marking matching parameters defaulted.
    pub(crate) fn project_partial_params(
        &mut self,
        partial_name: &str,
        target_name: &str,
        mut params: Vec<CallableParam>,
        args: &[PartialArg],
        span: Span,
    ) -> Option<Vec<CallableParam>> {
        if args.is_empty() {
            self.errors.push(CompileError::type_error(
                format!(
                    "Partial '{partial_name}' for '{target_name}' must preset at least one keyword; use an alias for a no-op projection"
                ),
                span,
            ));
            return None;
        }

        for param in &params {
            if param.kind != ParamKind::Normal {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Partial '{partial_name}' cannot target callable '{target_name}' because parameter '{}' is a rest parameter",
                        param.name().unwrap_or("<anonymous>")
                    ),
                    span,
                ));
                return None;
            }
        }

        let mut seen = HashSet::new();
        let mut ok = true;
        for arg in args {
            if !seen.insert(arg.name.as_str()) {
                self.errors.push(CompileError::type_error(
                    format!("Partial '{partial_name}' repeats preset keyword '{}'", arg.name),
                    arg.value.span,
                ));
                ok = false;
                continue;
            }
            let Some(param) = params.iter_mut().find(|param| param.name() == Some(arg.name.as_str())) else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Partial '{partial_name}' presets unknown parameter '{}' on target '{target_name}'",
                        arg.name
                    ),
                    arg.value.span,
                ));
                ok = false;
                continue;
            };
            if param.kind != ParamKind::Normal {
                self.errors.push(CompileError::type_error(
                    format!(
                        "Partial '{partial_name}' cannot preset rest parameter '{}' on target '{target_name}'",
                        arg.name
                    ),
                    arg.value.span,
                ));
                ok = false;
                continue;
            }
            param.has_default = true;
        }

        ok.then_some(params)
    }

    /// Register a module-level const binding (first pass).
    ///
    /// Note: the initializer is validated in the second pass.
    fn collect_const(&mut self, konst: &ConstDecl, span: Span) {
        // Remember for const-eval (cycle detection / evaluation).
        self.const_decls.insert(konst.name.clone(), (konst.clone(), span));

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

        // Define as an immutable variable-like symbol for name resolution.
        self.symbols.define(Symbol {
            name: konst.name.clone(),
            kind: SymbolKind::Variable(VariableInfo {
                ty,
                is_mutable: false,
                is_used: false,
            }),
            span,
            scope: 0,
        });
    }

    /// Register a module-level static binding (first pass).
    fn collect_static(&mut self, static_decl: &StaticDecl, span: Span) {
        let decl_index = self.static_decls.len();
        self.static_decl_positions.insert(static_decl.name.clone(), decl_index);
        self.static_decls.push((static_decl.clone(), span));

        let ty = self.resolve_type_checked(&static_decl.ty);
        self.type_info.declarations.static_bindings.insert(
            static_decl.name.clone(),
            crate::frontend::typechecker::StaticBindingInfo { is_imported: false },
        );

        self.symbols.define(Symbol {
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
    }

    /// Register a model declaration with its fields, methods, and derived traits.
    fn collect_model(&mut self, model: &ModelDecl, span: Span) {
        let fields = collect_fields(&model.fields, self, &model.name, &model.type_params);
        let properties = collect_properties(&model.properties, self, Some(&model.name), &model.type_params);
        let mut method_overloads =
            collect_method_overloads(&model.methods, self, Some(&model.name), &model.type_params);
        let mut methods = collect_methods_from_overloads(&method_overloads);

        let derives = self.extract_derive_names(&model.decorators);
        let field_order: Vec<Ident> = model.fields.iter().map(|f| f.node.name.clone()).collect();
        inject_validate_methods(
            &mut methods,
            &mut method_overloads,
            &model.name,
            &fields,
            &field_order,
            &derives,
        );
        let method_aliases = collect_method_aliases(&model.method_aliases, &mut methods, &mut method_overloads);
        self.collect_method_partials(&model.name, &model.method_partials, &mut methods, &mut method_overloads);
        let mut trait_adoptions =
            self.collect_trait_adoption_infos(&model.traits, Some(&model.name), &model.type_params);
        trait_adoptions.extend(self.collect_derive_trait_adoption_infos(&derives));

        self.symbols.define(Symbol {
            name: model.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Model(ModelInfo {
                type_params: model.type_params.iter().map(|tp| tp.name.clone()).collect(),
                traits: model.traits.iter().map(|t| t.node.name.clone()).collect(),
                trait_adoptions,
                derives,
                fields,
                properties,
                methods,
                method_overloads,
                method_aliases,
            })),
            span,
            scope: 0,
        });
    }

    /// Register a class declaration, inheriting from parent if present.
    fn collect_class(&mut self, class: &ClassDecl, span: Span) {
        let (mut fields, mut properties, mut methods, mut method_overloads) = self.inherit_from_parent(&class.extends);

        // Add own fields (can override inherited ones)
        fields.extend(collect_fields(&class.fields, self, &class.name, &class.type_params));
        properties.extend(collect_properties(
            &class.properties,
            self,
            Some(&class.name),
            &class.type_params,
        ));

        // Add own methods (can override inherited ones)
        let own_method_overloads =
            collect_method_overloads(&class.methods, self, Some(&class.name), &class.type_params);
        methods.extend(collect_methods_from_overloads(&own_method_overloads));
        method_overloads.extend(own_method_overloads);

        let derives = self.extract_derive_names(&class.decorators);
        let method_aliases = collect_method_aliases(&class.method_aliases, &mut methods, &mut method_overloads);
        self.collect_method_partials(&class.name, &class.method_partials, &mut methods, &mut method_overloads);
        let mut trait_adoptions =
            self.collect_trait_adoption_infos(&class.traits, Some(&class.name), &class.type_params);
        trait_adoptions.extend(self.collect_derive_trait_adoption_infos(&derives));

        self.symbols.define(Symbol {
            name: class.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Class(ClassInfo {
                type_params: class.type_params.iter().map(|tp| tp.name.clone()).collect(),
                extends: class.extends.clone(),
                traits: class.traits.iter().map(|t| t.node.name.clone()).collect(),
                trait_adoptions,
                derives,
                fields,
                properties,
                methods,
                method_overloads,
                method_aliases,
            })),
            span,
            scope: 0,
        });
    }

    /// Inherit fields and methods from a parent class if present.
    fn inherit_from_parent(&self, extends: &Option<String>) -> InheritedMembers {
        let Some(parent_name) = extends else {
            return (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
        };
        let Some(TypeInfo::Class(parent_info)) = self.lookup_type_info(parent_name) else {
            return (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
        };
        (
            parent_info.fields.clone(),
            parent_info.properties.clone(),
            parent_info.methods.clone(),
            parent_info.method_overloads.clone(),
        )
    }

    /// Resolve direct trait adoptions, retaining generic trait arguments for later dispatch checks.
    fn collect_trait_adoption_infos(
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
                }
            })
            .collect()
    }

    /// Return the source module that owns a trait bound, including direct imports and module-qualified spellings.
    pub(crate) fn trait_bound_module_path(&self, name: &str) -> Option<Vec<String>> {
        if let Some(path) = self.import_aliases.get(name) {
            return path
                .len()
                .checked_sub(1)
                .map(|end| path[..end].to_vec())
                .filter(|segments| !segments.is_empty());
        }
        let (module_name, _trait_name) = name.rsplit_once('.')?;
        self.module_path_for_imported_name(module_name)
    }

    /// Return the defining source trait name for imported or module-qualified trait bounds.
    pub(crate) fn trait_bound_source_name(&self, name: &str) -> Option<String> {
        if let Some(path) = self.import_aliases.get(name) {
            return path.last().cloned();
        }
        let (_module_name, trait_name) = name.rsplit_once('.')?;
        Some(trait_name.to_string())
    }

    /// Resolve a trait bound name, installing hidden symbols for module-qualified imported traits.
    pub(crate) fn resolve_trait_bound_name(&mut self, name: &str, span: Span) -> String {
        if name.contains('.')
            && let Some((canonical, info)) = self.resolve_qualified_trait(name)
        {
            self.define_hidden_trait_symbol(&canonical, info, span);
            return canonical;
        }
        name.to_string()
    }

    /// Collect synthetic trait adoptions introduced by RFC 024 `@derive(...)` arguments.
    pub(crate) fn collect_derive_trait_adoption_infos(&mut self, derives: &[String]) -> Vec<TypeBoundInfo> {
        let mut out = Vec::new();
        for derive_name in derives {
            if incan_core::lang::derives::from_str(derive_name).is_some() {
                continue;
            }
            if let Some(module_path) = self.module_path_for_imported_name(derive_name)
                && let Some(traits) = self.lookup_derivable_traits(&module_path)
            {
                for trait_name in traits {
                    let Some(info) = self.lookup_imported_module_trait(&module_path, &trait_name) else {
                        continue;
                    };
                    let canonical = format!("{}.{}", derive_name, trait_name);
                    self.define_hidden_trait_symbol(&canonical, info, Span::default());
                    if !out.iter().any(|existing: &TypeBoundInfo| existing.name == canonical) {
                        out.push(TypeBoundInfo {
                            name: canonical,
                            source_name: Some(trait_name.clone()),
                            type_args: Vec::new(),
                            module_path: None,
                        });
                    }
                }
                continue;
            }
            let resolved = self
                .import_aliases
                .get(derive_name)
                .cloned()
                .unwrap_or_else(|| vec![derive_name.to_string()]);
            if resolved.len() >= 2 {
                let module_segments = &resolved[..resolved.len() - 1];
                let trait_name = &resolved[resolved.len() - 1];
                if self.imported_trait_is_derivable(module_segments, trait_name)
                    && let Some(info) = self.lookup_imported_module_trait(module_segments, trait_name)
                {
                    self.define_hidden_trait_symbol(derive_name, info, Span::default());
                    if !out.iter().any(|existing: &TypeBoundInfo| existing.name == *derive_name) {
                        out.push(TypeBoundInfo {
                            name: derive_name.clone(),
                            source_name: Some(trait_name.clone()),
                            type_args: Vec::new(),
                            module_path: None,
                        });
                    }
                } else if self.lookup_trait_info(derive_name).is_some() {
                    out.push(TypeBoundInfo {
                        name: derive_name.clone(),
                        source_name: None,
                        type_args: Vec::new(),
                        module_path: None,
                    });
                }
            }
        }
        out
    }

    /// Resolve a module-qualified trait name through the imported-module metadata table.
    pub(crate) fn resolve_qualified_trait(&mut self, name: &str) -> Option<(String, TraitInfo)> {
        let (module_name, trait_name) = name.rsplit_once('.')?;
        let module_path = self.module_path_for_imported_name(module_name)?;
        let info = self.lookup_imported_module_trait(&module_path, trait_name)?;
        Some((name.to_string(), info))
    }

    /// Look up a module's RFC 024 `__derives__` trait list from stdlib or imported dependency metadata.
    pub(crate) fn lookup_derivable_traits(&mut self, module_path: &[String]) -> Option<Vec<String>> {
        if let Some(traits) = self.stdlib_cache.lookup_derivable_traits(module_path) {
            return Some(traits);
        }
        self.dependency_derivable_modules
            .get(&module_path.join("."))
            .cloned()
            .filter(|traits| !traits.is_empty())
    }

    /// Look up a trait declared by an imported module, falling back to the current scope for direct imports.
    pub(crate) fn lookup_imported_module_trait(
        &mut self,
        module_path: &[String],
        trait_name: &str,
    ) -> Option<TraitInfo> {
        if let Some(info) = self.stdlib_cache.lookup_trait(module_path, trait_name) {
            return Some(info);
        }
        if let Some(info) = self
            .dependency_module_traits
            .get(&format!("{}.{}", module_path.join("."), trait_name))
        {
            return Some(info.clone());
        }
        self.lookup_trait_info(trait_name).cloned()
    }

    /// Return whether a module-qualified trait may be adopted through `@derive(...)`.
    pub(crate) fn imported_trait_is_derivable(&mut self, module_path: &[String], trait_name: &str) -> bool {
        if self
            .stdlib_cache
            .lookup_trait_meta(module_path, trait_name)
            .is_some_and(|meta| meta.rust_module_path.is_some() || !meta.rust_derive_paths.is_empty())
        {
            return true;
        }
        self.lookup_imported_module_trait(module_path, trait_name).is_some()
            && self
                .lookup_derivable_traits(module_path)
                .is_some_and(|traits| traits.iter().any(|name| name == trait_name))
    }

    /// Resolve an imported name or alias to a module path.
    pub(crate) fn module_path_for_imported_name(&self, name: &str) -> Option<Vec<String>> {
        if name.contains('.') {
            return Some(name.split('.').map(str::to_string).collect());
        }
        if let Some(symbol) = self.lookup_symbol(name)
            && let SymbolKind::Module(info) = &symbol.kind
        {
            return Some(info.path.clone());
        }
        self.import_aliases.get(name).cloned()
    }

    /// Define a compiler-internal trait symbol used for qualified imported trait references.
    pub(crate) fn define_hidden_trait_symbol(&mut self, name: &str, info: TraitInfo, span: Span) {
        if self.symbols.lookup(name).is_some() {
            return;
        }
        self.symbols.define(Symbol {
            name: name.to_string(),
            kind: SymbolKind::Trait(info),
            span,
            scope: 0,
        });
    }

    /// Register a trait declaration with its method signatures, supertraits, and requirements.
    fn collect_trait(&mut self, tr: &TraitDecl, span: Span) {
        let mut method_overloads = collect_method_overloads(&tr.methods, self, None, &tr.type_params);
        let mut methods = collect_methods_from_overloads(&method_overloads);
        let properties = collect_properties(&tr.properties, self, None, &tr.type_params);
        let method_aliases = collect_method_aliases(&tr.method_aliases, &mut methods, &mut method_overloads);
        self.collect_method_partials(&tr.name, &tr.method_partials, &mut methods, &mut method_overloads);
        let requires = self.extract_requires(&tr.decorators);
        if !tr.traits.is_empty() {
            self.pending_trait_supertraits
                .push((tr.name.clone(), tr.traits.clone()));
        }

        self.symbols.define(Symbol {
            name: tr.name.clone(),
            kind: SymbolKind::Trait(TraitInfo {
                type_params: tr.type_params.iter().map(|tp| tp.name.clone()).collect(),
                supertraits: Vec::new(),
                methods,
                method_aliases,
                properties,
                requires,
            }),
            span,
            scope: 0,
        });
    }

    /// Resolve one `with` supertrait bound to `(trait_name, type_arguments)` after validation (RFC 042).
    fn resolve_trait_supertrait_bound(&mut self, bound: &Spanned<TraitBound>) -> Option<(String, Vec<ResolvedType>)> {
        let trait_name = self.resolve_trait_bound_name(&bound.node.name, bound.span);
        let args = bound
            .node
            .type_args
            .iter()
            .map(|arg| self.resolve_type_checked(arg))
            .collect::<Vec<_>>();
        let trait_info = if let Some(info) = self.lookup_trait_info(&trait_name).cloned() {
            info
        } else if let Some((hidden_name, info)) = self.resolve_imported_trait_bound_symbol(&bound.node.name, bound.span)
        {
            return self.validate_supertrait_bound(hidden_name, args, info, bound.span);
        } else {
            self.errors
                .push(errors::supertrait_bound_not_trait(&trait_name, bound.span));
            return None;
        };
        self.validate_supertrait_bound(trait_name, args, trait_info, bound.span)
    }

    /// Define a hidden trait symbol for an imported supertrait bound that has not been collected as a declaration.
    fn resolve_imported_trait_bound_symbol(&mut self, name: &str, span: Span) -> Option<(String, TraitInfo)> {
        let module_path = self.trait_bound_module_path(name)?;
        let trait_name = self
            .import_aliases
            .get(name)
            .and_then(|path| path.last())
            .cloned()
            .unwrap_or_else(|| name.rsplit('.').next().unwrap_or(name).to_string());
        let info = self.lookup_imported_module_trait(&module_path, &trait_name)?;
        let symbol_name = name.to_string();
        self.define_hidden_trait_symbol(&symbol_name, info.clone(), span);
        Some((symbol_name, info))
    }

    /// Validate a resolved supertrait bound against the target trait arity.
    fn validate_supertrait_bound(
        &mut self,
        trait_name: String,
        args: Vec<ResolvedType>,
        trait_info: TraitInfo,
        span: Span,
    ) -> Option<(String, Vec<ResolvedType>)> {
        let expected_arity = trait_info.type_params.len();
        if args.len() != expected_arity {
            self.errors.push(errors::supertrait_bound_arity_mismatch(
                &trait_name,
                expected_arity,
                args.len(),
                span,
            ));
            return None;
        }
        Some((trait_name, args))
    }

    /// Resolve queued trait `with` bounds now that all types and traits exist in the symbol table (RFC 042).
    pub(crate) fn resolve_pending_trait_supertraits(&mut self) {
        let pending = std::mem::take(&mut self.pending_trait_supertraits);
        for (trait_name, bounds) in pending {
            let mut supertraits: Vec<(String, Vec<ResolvedType>)> = Vec::new();
            for bound in &bounds {
                if let Some(entry) = self.resolve_trait_supertrait_bound(bound) {
                    supertraits.push(entry);
                }
            }
            let Some(sym_id) = self.symbols.lookup(&trait_name) else {
                continue;
            };
            let Some(sym) = self.symbols.get_mut(sym_id) else {
                continue;
            };
            let SymbolKind::Trait(info) = &mut sym.kind else {
                continue;
            };
            info.supertraits = supertraits;
        }
    }

    /// After all declarations are collected: detect supertrait cycles and fill `supertrait_closure`.
    pub(crate) fn finalize_supertrait_graph(&mut self) {
        self.supertrait_closure.clear();
        let edges = self.supertrait_name_adjacency();
        let trait_names: Vec<String> = self
            .symbols
            .all_symbols()
            .iter()
            .filter_map(|sym| matches!(sym.kind, SymbolKind::Trait(_)).then_some(sym.name.clone()))
            .collect();
        if let Some(cycle) = find_supertrait_cycle_path(&edges) {
            let span = cycle
                .first()
                .and_then(|name| self.lookup_symbol(name))
                .map(|sym| sym.span)
                .unwrap_or_default();
            self.errors.push(errors::supertrait_cycle(&cycle, span));
            for name in trait_names {
                self.supertrait_closure.insert(name, Vec::new());
            }
            return;
        }
        for name in trait_names {
            let closure = self.expand_supertraits_transitively(&name);
            self.supertrait_closure.insert(name, closure);
        }
    }

    fn supertrait_name_adjacency(&self) -> HashMap<String, Vec<String>> {
        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                let names: Vec<String> = info.supertraits.iter().map(|(n, _)| n.clone()).collect();
                map.insert(sym.name.clone(), names);
            }
        }
        map
    }

    /// Transitive supertraits of `trait_name`, with type arguments substituted along each edge.
    fn expand_supertraits_transitively(&self, trait_name: &str) -> Vec<(String, Vec<ResolvedType>)> {
        let mut result: Vec<(String, Vec<ResolvedType>)> = Vec::new();
        let mut seen = HashSet::new();
        let mut work: Vec<(String, Vec<ResolvedType>)> = Vec::new();
        let Some(root) = self.lookup_trait_info(trait_name) else {
            return result;
        };
        work.extend(root.supertraits.clone());
        while let Some((sup_name, sup_args)) = work.pop() {
            let key = format!(
                "{sup_name}<{}>",
                sup_args.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",")
            );
            if !seen.insert(key) {
                continue;
            }
            result.push((sup_name.clone(), sup_args.clone()));
            let Some(sup_info) = self.lookup_trait_info(&sup_name) else {
                continue;
            };
            let subst = type_param_subst_map(&sup_info.type_params, &sup_args);
            for (ss_name, ss_args) in &sup_info.supertraits {
                let mapped: Vec<ResolvedType> = ss_args.iter().map(|t| substitute_resolved_type(t, &subst)).collect();
                work.push((ss_name.clone(), mapped));
            }
        }
        result
    }

    /// Merge `@requires` fields from transitive supertraits into each trait symbol (RFC 042).
    ///
    /// Uses each trait's **explicit** `@requires` from a snapshot taken before merging, so order does not matter.
    /// Incompatible requirements for the same field name emit [`errors::supertrait_requires_conflict`].
    pub(crate) fn merge_supertrait_requires_into_traits(&mut self) {
        let mut explicit: HashMap<String, Vec<(String, ResolvedType)>> = HashMap::new();
        for sym in self.symbols.all_symbols() {
            if let SymbolKind::Trait(info) = &sym.kind {
                explicit.insert(sym.name.clone(), info.requires.clone());
            }
        }

        let trait_names: Vec<String> = explicit.keys().cloned().collect();
        let mut updates: Vec<(String, Vec<(String, ResolvedType)>)> = Vec::new();

        for tname in trait_names {
            let span = self
                .symbols
                .lookup(&tname)
                .and_then(|id| self.symbols.get(id))
                .map(|s| s.span)
                .unwrap_or_default();

            let mut merged: HashMap<String, ResolvedType> =
                explicit.get(&tname).cloned().unwrap_or_default().into_iter().collect();

            let closure = self.supertrait_closure.get(tname.as_str()).cloned().unwrap_or_default();

            for (sup_name, sup_args) in closure {
                let Some(sup_info) = self.lookup_trait_info(&sup_name) else {
                    continue;
                };
                let sup_req = explicit.get(&sup_name).cloned().unwrap_or_default();
                let subst = type_param_subst_map(&sup_info.type_params, &sup_args);
                for (field, ty) in sup_req {
                    let inst = substitute_resolved_type(&ty, &subst);
                    match merged.get(&field) {
                        None => {
                            merged.insert(field, inst);
                        }
                        Some(existing) => {
                            if existing == &inst {
                                continue;
                            }
                            if self.types_compatible(existing, &inst) || self.types_compatible(&inst, existing) {
                                continue;
                            }
                            self.errors.push(errors::supertrait_requires_conflict(
                                &tname,
                                &field,
                                &existing.to_string(),
                                &inst.to_string(),
                                span,
                            ));
                        }
                    }
                }
            }

            let mut req_vec: Vec<(String, ResolvedType)> = merged.into_iter().collect();
            req_vec.sort_by(|a, b| a.0.cmp(&b.0));
            updates.push((tname, req_vec));
        }

        for (tname, requires) in updates {
            if let Some(id) = self.symbols.lookup(&tname)
                && let Some(sym) = self.symbols.get_mut(id)
                && let SymbolKind::Trait(info) = &mut sym.kind
            {
                info.requires = requires;
            }
        }
    }

    /// Register a newtype declaration with its underlying type and methods.
    fn collect_newtype(&mut self, nt: &NewtypeDecl, span: Span) {
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
        let trait_adoptions = self.collect_trait_adoption_infos(&nt.traits, Some(&nt.name), &nt.type_params);

        // Define a placeholder symbol FIRST so methods can reference the newtype name
        self.symbols.define(Symbol {
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
                method_aliases: HashMap::new(),
                methods: HashMap::new(), // Empty for now
                method_overloads: HashMap::new(),
            })),
            span,
            scope: 0,
        });

        // Now collect methods - they can reference the newtype name
        let mut method_overloads = collect_method_overloads(&nt.methods, self, Some(&nt.name), &nt.type_params);
        let mut methods = collect_methods_from_overloads(&method_overloads);
        let method_aliases = collect_method_aliases(&nt.method_aliases, &mut methods, &mut method_overloads);
        self.collect_method_partials(&nt.name, &nt.method_partials, &mut methods, &mut method_overloads);

        // Update the symbol with the collected methods
        if let Some(sym_id) = self.symbols.lookup(&nt.name)
            && let Some(sym) = self.symbols.get_mut(sym_id)
            && let SymbolKind::Type(TypeInfo::Newtype(info)) = &mut sym.kind
        {
            info.methods = methods;
            info.method_aliases = method_aliases;
            info.method_overloads = method_overloads;
        }
    }

    /// Register same-type method partials as projected method metadata on the owning type surface.
    fn collect_method_partials(
        &mut self,
        owner_name: &str,
        partials: &[Spanned<MethodPartialDecl>],
        methods: &mut HashMap<String, MethodInfo>,
        overloads: &mut HashMap<String, Vec<MethodInfo>>,
    ) {
        for partial in partials {
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
    fn rebinding_target_method_name(target: &Expr) -> Option<String> {
        match target {
            Expr::Ident(name) => Some(name.clone()),
            Expr::Field(_, member) => Some(member.clone()),
            _ => None,
        }
    }

    /// Register an enum declaration and define symbols for each variant.
    fn collect_enum(&mut self, en: &EnumDecl, span: Span) {
        let variants: Vec<_> = en.variants.iter().map(|v| v.node.name.clone()).collect();
        let variant_fields: HashMap<_, _> = en
            .variants
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
        let variant_aliases: HashMap<_, _> = en
            .variant_aliases
            .iter()
            .map(|alias| (alias.node.name.clone(), alias.node.target.clone()))
            .collect();
        let derives = self.extract_derive_names(&en.decorators);
        let value_enum = en.value_type.as_ref().map(|value_type| ValueEnumInfo {
            value_type: value_enum_backing(value_type.node),
            values: en
                .variants
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
        self.symbols.define(Symbol {
            name: en.name.clone(),
            kind: SymbolKind::Type(TypeInfo::Enum(EnumInfo {
                type_params: en.type_params.iter().map(|tp| tp.name.clone()).collect(),
                traits: en.traits.iter().map(|t| t.node.name.clone()).collect(),
                trait_adoptions,
                variants: variants.clone(),
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

        let method_overloads = collect_method_overloads(&en.methods, self, Some(&en.name), &en.type_params);
        let methods = collect_methods_from_overloads(&method_overloads);
        if let Some(sym_id) = self.symbols.lookup(&en.name)
            && let Some(sym) = self.symbols.get_mut(sym_id)
            && let SymbolKind::Type(TypeInfo::Enum(info)) = &mut sym.kind
        {
            info.methods = methods;
            info.method_overloads = method_overloads;
        }

        // Also define each variant as a symbol
        for variant in &en.variants {
            let fields = variant_fields.get(&variant.node.name).cloned().unwrap_or_default();
            self.symbols.define_preserving_existing_binding(Symbol {
                name: variant.node.name.clone(),
                kind: SymbolKind::Variant(VariantInfo {
                    enum_name: en.name.clone(),
                    fields,
                }),
                span: variant.span,
                scope: 0,
            });
        }
        for alias in &en.variant_aliases {
            if let Some(target_variant) = en
                .variants
                .iter()
                .find(|variant| variant.node.name == alias.node.target)
            {
                let fields: Vec<_> = target_variant
                    .node
                    .fields
                    .iter()
                    .map(|f| self.resolve_type_checked(f))
                    .collect();
                self.symbols.define_preserving_existing_binding(Symbol {
                    name: alias.node.name.clone(),
                    kind: SymbolKind::Variant(VariantInfo {
                        enum_name: en.name.clone(),
                        fields,
                    }),
                    span: alias.span,
                    scope: 0,
                });
            }
        }
    }

    /// Register a top-level function declaration.
    fn collect_function(&mut self, func: &FunctionDecl, span: Span) {
        // Local declaration shadows any imported marker binding with the same name.
        self.testing_marker_import_bindings.remove(&func.name);
        self.local_function_decls.insert(func.name.clone(), func.clone());
        let type_params: Vec<String> = func.type_params.iter().map(|tp| tp.name.clone()).collect();
        let type_param_bounds: HashMap<String, Vec<String>> = func
            .type_params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds
                        .iter()
                        .map(|bound| self.resolve_trait_bound_name(&bound.name, Span::default()))
                        .collect(),
                )
            })
            .collect();
        let type_param_bound_details: HashMap<String, Vec<TypeBoundInfo>> = func
            .type_params
            .iter()
            .map(|tp| {
                (
                    tp.name.clone(),
                    tp.bounds
                        .iter()
                        .map(|bound| TypeBoundInfo {
                            name: self.resolve_trait_bound_name(&bound.name, Span::default()),
                            source_name: self.trait_bound_source_name(&bound.name),
                            type_args: bound
                                .type_args
                                .iter()
                                .map(|type_arg| self.resolve_type_checked(type_arg))
                                .collect(),
                            module_path: self.trait_bound_module_path(&bound.name),
                        })
                        .collect(),
                )
            })
            .collect();

        let params: Vec<_> = func
            .params
            .iter()
            .map(|p| {
                CallableParam::named_with_default(
                    p.node.name.clone(),
                    self.resolve_type_checked(&p.node.ty),
                    p.node.kind,
                    p.node.default.is_some(),
                )
            })
            .collect();
        let return_type = self.resolve_type_checked(&func.return_type);
        self.type_info.declarations.function_bindings.insert(
            func.name.clone(),
            FunctionBindingInfo {
                params: params.clone(),
                return_type: return_type.clone(),
            },
        );

        self.symbols.define(Symbol {
            name: func.name.clone(),
            kind: SymbolKind::Function(FunctionInfo {
                params,
                return_type,
                is_async: func.is_async(),
                type_params,
                type_param_bounds,
                type_param_bound_details,
            }),
            span,
            scope: 0,
        });
    }
}

/// Returns one simple cycle (trait names), if the directed supertrait graph has a cycle.
fn find_supertrait_cycle_path(edges: &HashMap<String, Vec<String>>) -> Option<Vec<String>> {
    let mut nodes: HashSet<String> = HashSet::new();
    for (k, vs) in edges {
        nodes.insert(k.clone());
        for v in vs {
            nodes.insert(v.clone());
        }
    }
    let mut color: HashMap<String, u8> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    for start in nodes {
        if color.get(&start).copied().unwrap_or(0) != 0 {
            continue;
        }
        stack.clear();
        if let Some(cycle) = dfs_supertrait_cycle(&start, edges, &mut color, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

fn dfs_supertrait_cycle(
    n: &str,
    edges: &HashMap<String, Vec<String>>,
    color: &mut HashMap<String, u8>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    match color.get(n).copied().unwrap_or(0) {
        1 => {
            let idx = stack.iter().position(|x| x == n)?;
            Some(stack[idx..].to_vec())
        }
        2 => None,
        _ => {
            color.insert(n.to_string(), 1);
            stack.push(n.to_string());
            for succ in edges.get(n).into_iter().flatten() {
                if let Some(c) = dfs_supertrait_cycle(succ, edges, color, stack) {
                    return Some(c);
                }
            }
            stack.pop();
            color.insert(n.to_string(), 2);
            None
        }
    }
}
