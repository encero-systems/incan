//! Declaration collection dispatch, aliases and their targets, function bindings, and partials.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    // ========================================================================
    // First pass: collect declarations
    // ========================================================================

    /// Register a declaration in the symbol table (first pass).
    ///
    /// Dispatches to `collect_import`, `collect_model`, etc. to populate the [`SymbolTable`] with type, function,
    /// and trait definitions. Bodies are **not** validated here; that happens in
    /// [`check_declaration`](Self::check_declaration) in the second pass.
    pub fn collect_declaration(&mut self, decl: &Spanned<Declaration>) {
        self.record_check_work("header_visit", "top_level", decl.span);
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
                let symbol_id = self.symbols.define(Symbol {
                    name: a.name.clone(),
                    kind: SymbolKind::Type(TypeInfo::TypeAlias),
                    span: decl.span,
                    scope: 0,
                });
                if self.symbols.is_active_lookup_binding(symbol_id) {
                    self.register_type_alias_target(a);
                }
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
            Declaration::Capability(cap) => {
                self.validate_root_namespace(&cap.name, decl.span);
                self.collect_capability(cap, decl.span);
            }
            Declaration::Docstring(_) | Declaration::TestModule(_) | Declaration::VocabBlock(_) => {}
        }
    }

    /// Register a module-level alias after concrete symbols have been collected.
    pub(super) fn collect_alias(&mut self, alias: &AliasDecl, span: Span) {
        let target_name = alias.target.segments.join(".");
        let target_identity = self.alias_target_identity(&alias.target.segments);
        let target_binding_path = self.alias_target_binding_path(&alias.target.segments);
        let Some(kind) = self.alias_target_symbol_kind(&alias.target.segments) else {
            self.errors.push(CompileError::type_error(
                format!("Alias '{}' targets unknown symbol '{}'", alias.name, target_name),
                span,
            ));
            return;
        };

        let kind = match kind {
            SymbolKind::Function(info) => SymbolKind::Function(info),
            SymbolKind::FunctionOverloads(overloads) => SymbolKind::FunctionOverloads(overloads),
            SymbolKind::Static(info) => SymbolKind::Static(info),
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

        // RFC 083/RFC 120: an alias is a binding to the existing declaration, so it carries the target's
        // identity — never a second identity minted for the alias spelling.
        let symbol = Symbol {
            name: alias.name.clone(),
            kind: kind.clone(),
            span,
            scope: 0,
        };
        let symbol_id = if let Some(binding_path) = target_binding_path {
            self.symbols
                .define_alias_binding_at_path(symbol, target_identity.clone(), binding_path)
        } else {
            self.symbols.define_alias_binding(symbol, target_identity.clone())
        };
        if self.symbols.is_active_lookup_binding(symbol_id) {
            if let Some(identity) = target_identity {
                self.type_info
                    .declarations
                    .resolved_import_identities
                    .insert(alias.name.clone(), identity);
            }
            match &kind {
                SymbolKind::Function(_) => self.record_imported_function_binding(&alias.name, &kind),
                SymbolKind::FunctionOverloads(overloads) => {
                    self.record_function_overload_binding(&alias.name, overloads);
                }
                _ => {}
            }
        }
    }

    /// Resolve the RFC 120 identity an alias binding must carry, when resolution proves one.
    ///
    /// Single-segment targets take the identity of the module-scope symbol they name. Qualified targets resolve
    /// through the same dependency-member path import resolution uses, so the alias lands on the declaring module
    /// even through re-exports. An overload-set or otherwise unproven target yields `None` — the alias binding then
    /// records no identity rather than an arbitrary or minted one.
    pub(super) fn alias_target_identity(&mut self, segments: &[String]) -> Option<CanonicalSymbolId> {
        match segments {
            [name] => {
                let id = self.symbols.lookup(name)?;
                self.symbols.identity_of(id).cloned()
            }
            [_, _, ..] => self.resolve_qualified_name(segments).ok()?.canonical,
            [] => None,
        }
    }

    /// Resolve the checked source import path projected by an alias target.
    pub(super) fn alias_target_binding_path(&self, segments: &[String]) -> Option<Vec<String>> {
        let (leading, rest) = segments.split_first()?;
        let mut path = self.import_binding_path(leading)?.to_vec();
        path.extend_from_slice(rest);
        Some(path)
    }

    /// Record the common metadata for any source binding that resolves to an overload set.
    pub(in crate::typechecker) fn record_function_overload_binding(
        &mut self,
        local_name: &str,
        overloads: &[FunctionOverloadInfo],
    ) {
        self.type_info
            .record_function_overloads(local_name.to_string(), overloads.to_vec());
    }

    /// Preserve the checked callable surface for an imported source binding.
    ///
    /// Local and imported functions both need a declaration-slot surface for later call lowering. The canonical
    /// owner of an import is recorded separately, but its parameter facts must travel with the local binding too:
    /// a caller cannot describe evaluated inputs against an identity alone. Overload bindings retain their existing
    /// dedicated table because a bare source name cannot distinguish their declaration spans.
    pub(in crate::typechecker) fn record_imported_function_binding(&mut self, local_name: &str, kind: &SymbolKind) {
        match kind {
            SymbolKind::Function(info) => {
                let identity = self
                    .type_info
                    .declarations
                    .resolved_import_identities
                    .get(local_name)
                    .cloned();
                self.type_info.declarations.function_bindings.insert(
                    local_name.to_string(),
                    FunctionBindingInfo {
                        is_async: info.is_async,
                        params: info.params.clone(),
                        return_type: info.return_type.clone(),
                        identity,
                    },
                );
            }
            SymbolKind::FunctionOverloads(overloads) => {
                self.record_function_overload_binding(local_name, overloads);
            }
            _ => {}
        }
    }

    /// Resolve an alias target path to the semantic symbol kind it projects.
    ///
    /// Single-segment targets use ordinary module-scope lookup. Qualified targets must begin with an imported module
    /// binding and are resolved through stdlib or `pub::` library metadata so `lib.name` cannot accidentally fall back
    /// to an unrelated local `name`.
    pub(in crate::typechecker) fn alias_target_symbol_kind(&mut self, segments: &[String]) -> Option<SymbolKind> {
        match segments {
            [name] => self.lookup_symbol(name).map(|symbol| symbol.kind.clone()),
            [_, _, ..] => match self.resolve_qualified_name(segments).ok()?.kind {
                ResolvedNamespaceMemberKind::Symbol(kind) => Some(kind),
                ResolvedNamespaceMemberKind::SurfaceType(_) | ResolvedNamespaceMemberKind::Method(_) => None,
            },
            [] => None,
        }
    }

    /// Resolve a member from a source dependency module path recorded in the dependency member cache.
    pub(super) fn source_dependency_member_symbol_kind(
        &self,
        module_path: &[String],
        member: &str,
    ) -> Option<SymbolKind> {
        self.dependency_member_symbol_for_path(
            &ImportPath {
                parent_levels: 0,
                is_absolute: false,
                segments: module_path.to_vec(),
            },
            member,
        )
    }

    /// Register a module-level partial as a projected callable symbol.
    pub(super) fn collect_partial(&mut self, partial: &PartialDecl, span: Span) {
        let target_name = partial.target.segments.join(".");
        let Some(kind) = self.alias_target_symbol_kind(&partial.target.segments) else {
            self.errors.push(CompileError::type_error(
                format!("Partial '{}' targets unknown callable '{}'", partial.name, target_name),
                span,
            ));
            return;
        };
        let target_kind = Self::partial_projection_target_kind(&kind);
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

        let symbol_id = self.symbols.define_with_target_kind(
            Symbol {
                name: partial.name.clone(),
                kind: SymbolKind::Function(FunctionInfo {
                    params: params.clone(),
                    return_type: return_type.clone(),
                    is_async,
                    type_params,
                    type_param_bounds,
                    type_param_bound_details,
                    emitted_name: None,
                }),
                span,
                scope: 0,
            },
            SemanticSourceTargetKind::Partial,
        );
        if !self.symbols.is_active_lookup_binding(symbol_id) {
            return;
        }
        self.type_info.record_partial_projection(PartialProjectionInfo {
            name: partial.name.clone(),
            target_path: partial.target.segments.clone(),
            target_kind,
            presets: partial
                .args
                .iter()
                .map(|arg| PartialProjectionPreset {
                    name: arg.name.clone(),
                    value: arg.value.clone(),
                })
                .collect(),
        });
        let binding = FunctionBindingInfo {
            is_async,
            identity: Some(self.symbols.module_declaration_identity(
                &partial.name,
                SemanticSourceTargetKind::Partial,
                span,
            )),
            params,
            return_type,
        };
        self.type_info
            .declarations
            .function_bindings_by_span
            .insert(span, binding.clone());
        self.type_info
            .declarations
            .function_bindings
            .insert(partial.name.clone(), binding);
    }

    /// Resolve the callable surface that a top-level partial declaration projects from an already-resolved symbol.
    pub(in crate::typechecker) fn partial_callable_signature_from_kind(
        segments: &[String],
        kind: SymbolKind,
    ) -> Option<PartialCallableSignature> {
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
                Self::constructor_params_from_fields(&info.fields, &info.field_order),
                ResolvedType::Named(segments.last()?.clone()),
                false,
                info.type_params,
                HashMap::new(),
                HashMap::new(),
            )),
            SymbolKind::Type(TypeInfo::Class(info)) => Some((
                Self::constructor_params_from_fields(&info.fields, &info.field_order),
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

    /// Classify a resolved partial target for downstream projection consumers.
    pub(super) fn partial_projection_target_kind(kind: &SymbolKind) -> PartialProjectionTargetKind {
        match kind {
            SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_) => PartialProjectionTargetKind::Function,
            SymbolKind::Type(TypeInfo::Model(_)) => PartialProjectionTargetKind::ModelConstructor,
            SymbolKind::Type(TypeInfo::Class(_)) => PartialProjectionTargetKind::ClassConstructor,
            SymbolKind::Type(TypeInfo::Newtype(_)) => PartialProjectionTargetKind::NewtypeConstructor,
            _ => PartialProjectionTargetKind::Unknown,
        }
    }

    /// Convert collected field metadata into constructor callable parameters.
    pub(super) fn constructor_params_from_fields(
        fields: &HashMap<String, FieldInfo>,
        field_order: &[String],
    ) -> Vec<CallableParam> {
        let mut used = HashSet::new();
        let mut params = Vec::with_capacity(fields.len());
        for name in field_order {
            if let Some(info) = fields.get(name) {
                used.insert(name.as_str());
                params.push(CallableParam::named_with_default(
                    name.clone(),
                    info.ty.clone(),
                    ParamKind::Normal,
                    info.has_default,
                ));
            }
        }
        let mut remaining = fields
            .iter()
            .filter(|(name, _)| !used.contains(name.as_str()))
            .collect::<Vec<_>>();
        remaining.sort_by_key(|(name, _)| *name);
        params.extend(remaining.into_iter().map(|(name, info)| {
            CallableParam::named_with_default(name.clone(), info.ty.clone(), ParamKind::Normal, info.has_default)
        }));
        params
    }

    /// Apply partial preset keywords to callable parameters by marking matching parameters defaulted.
    pub fn project_partial_params(
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

    /// Preserve a local partial's full callable surface while marking construction-time presets.
    ///
    /// [`Self::project_partial_params`] marks every preset defaulted for module partial declarations. A local
    /// [`Expr::Partial`] has the same name-overrideable default surface, plus `is_partial_preset` to preserve its
    /// residual positional rule: positional calls bind only non-preset parameters, while a named argument may replace
    /// the captured value for one invocation.
    pub fn local_partial_params(mut params: Vec<CallableParam>, args: &[PartialArg]) -> Vec<CallableParam> {
        let preset_names: HashSet<&str> = args.iter().map(|arg| arg.name.as_str()).collect();
        for param in &mut params {
            param.is_partial_preset = param.name().is_some_and(|name| preset_names.contains(name));
        }
        params
    }
}
