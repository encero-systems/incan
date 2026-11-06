//! Call checking entry points and direct calls to compiler-known, field-constructor, module, class and builtin callees.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    /// Read a nongeneric variant's declared positional payload signature for contextual checking and both constructor
    /// spellings.
    ///
    /// Generic variants need a concrete instantiation before they can claim declaration-slot types.
    pub(in crate::typechecker::check_expr) fn checked_enum_variant_parameters(
        &self,
        enum_name: &str,
        variant: &str,
    ) -> Option<Vec<CallableParam>> {
        let TypeInfo::Enum(info) = self.lookup_semantic_type_info(enum_name)? else {
            return None;
        };
        if !info.type_params.is_empty() {
            return None;
        }
        let canonical = info.variant_aliases.get(variant).map(String::as_str).unwrap_or(variant);
        info.variant_fields
            .get(canonical)
            .map(|fields| fields.iter().cloned().map(CallableParam::positional).collect())
    }

    /// Record the identity already attached to the active direct callee binding.
    ///
    /// This deliberately reads the symbol table's resolution result instead of deriving an identity from the written
    /// name. Bindings whose declaration could not be proven therefore leave the reference map empty.
    pub(super) fn record_direct_callee_identity(&mut self, name: &str, callee_span: Span) {
        let identity = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
            .cloned();
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(callee_span, identity);
        }
    }

    /// Type-check a call expression after parsing has identified the callee, explicit type arguments, and value
    /// arguments.
    ///
    /// This is the central call coordinator: it preserves constructor and builtin special cases first, then resolves
    /// function values, callable objects, and ordinary value calls through the same argument-binding machinery.
    /// Callable values record their accepted parameter list at the full call span so IR lowering can preserve Rust
    /// borrow boundaries for calls reached through associated-function member access.
    pub(in crate::typechecker::check_expr) fn check_call(
        &mut self,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.check_call_with_expected(callee, type_args, args, span, None)
    }

    /// Type-check a call expression with an optional expected result type.
    ///
    /// Contextual return hints are part of the generic call plan, not a desugaring special case. They let direct
    /// source, vocab-produced AST, and nested call arguments all use the same inference path when a destination
    /// type is known. A type without a construction contract delegates to constructor checking, which refuses the
    /// call instead of admitting an unknown result that cannot cross into Body IR.
    pub(in crate::typechecker::check_expr) fn check_call_with_expected(
        &mut self,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let call = DirectCallSite {
            callee,
            type_args,
            args,
            span,
            expected_return_ty,
        };
        if let Some(result) = self.check_compiler_known_direct_call(&call) {
            return result;
        }
        if let Some(result) = self.check_direct_field_constructor(&call) {
            return result;
        }
        if let Some(result) = self.check_imported_module_direct_call(&call) {
            return result;
        }
        if let Some(result) =
            self.check_source_reexport_overload_call(callee, type_args, args, span, expected_return_ty)
        {
            return result;
        }
        self.check_remaining_direct_call(&call)
    }

    /// Check C ABI and explicit builtin syntax before general receiver evaluation or declaration lookup.
    pub(super) fn check_compiler_known_direct_call(&mut self, call: &DirectCallSite<'_>) -> Option<ResolvedType> {
        let (callee, type_args, args, span) = (call.callee, call.type_args, call.args, call.span);
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_abi_c_string_constructor(base, member, type_args, args, span)
        {
            return Some(result);
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_abi_span_constructor(base, member, type_args, args, span)
        {
            return Some(result);
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_abi_output_slot_constructor(base, member, type_args, args, span)
        {
            return Some(result);
        }
        if let Expr::Field(base, member) = &callee.node
            && let Some(result) = self.check_c_binding_symbol_member_call(base, member, type_args, args, span)
        {
            return Some(result);
        }
        if let Some(name) = Self::explicit_builtin_member_name(callee) {
            let result = self.check_explicit_builtin_call(name, args, span);
            if let Some(builtin) = self.type_info.resolved_builtin_call(span)
                && let Some(identity) = self.symbols.builtin_function_identity(builtin)
            {
                self.type_info.record_resolved_identity(callee.span, identity);
            }
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
                return Some(ResolvedType::Unknown);
            }
            return Some(result);
        }
        None
    }

    /// Resolve field-spelled enum constructors and refuse computed properties before ordinary module calls.
    pub(super) fn check_direct_field_constructor(&mut self, call: &DirectCallSite<'_>) -> Option<ResolvedType> {
        let (callee, type_args, args, span) = (call.callee, call.type_args, call.args, call.span);
        // Special-case: Enum variant constructor syntax `Enum.Variant(...)`. If callee is a field access where the
        // base resolves to a known enum type and the field name matches a variant, treat this as a constructor and
        // return the enum type.
        if let Expr::Field(base, member_name) = &callee.node {
            let base_ty = self.check_type_receiver_expr(base);
            let base_is_enum_type_name = self.is_enum_type_name_expr_for_call(base);
            if let ResolvedType::Named(enum_name) = &base_ty
                && let Some(TypeInfo::Enum(enum_info)) = self.lookup_type_info(enum_name)
                && let Some(value_enum) = enum_info.value_enum.clone()
            {
                if member_name == "from_value" && base_is_enum_type_name {
                    return Some(self.check_value_enum_from_value_call(enum_name, &value_enum, type_args, args, span));
                }
                if member_name == "value" && !base_is_enum_type_name {
                    return Some(self.check_value_enum_value_call(enum_name, &value_enum, type_args, args, span));
                }
            }
            if let ResolvedType::Named(enum_name) = &base_ty
                && let Some(TypeInfo::Enum(enum_info)) = self.lookup_type_info(enum_name)
                && (enum_info.variants.iter().any(|v| v == member_name)
                    || enum_info.variant_aliases.contains_key(member_name))
            {
                let variant_identity = enum_info.variant_identities.get(member_name).cloned();
                let variant_params = self.checked_enum_variant_parameters(enum_name, member_name);
                if !type_args.is_empty() {
                    self.errors
                        .push(errors::explicit_call_site_type_args_not_supported(span));
                }
                match variant_params.as_deref() {
                    Some(parameters) => {
                        self.check_call_arg_types_for_params(args, parameters);
                    }
                    None => self.check_call_args(args),
                }
                // Retain the checked positional payload surface for Body IR's declaration-slot binder.
                if let Some(parameters) = variant_params {
                    self.type_info.record_call_site_callable_params_exact(span, &parameters);
                }
                if let Some(identity) = variant_identity {
                    self.type_info.record_resolved_identity(callee.span, identity);
                }
                return Some(ResolvedType::Named(enum_name.clone()));
            }
            if self.receiver_has_computed_property(&base_ty, member_name, span) {
                self.check_call_args(args);
                self.errors.push(errors::property_called_as_method(member_name, span));
                return Some(ResolvedType::Unknown);
            }
        }
        None
    }

    /// Resolve imported module members and retain their source and canonical identities before checking arguments.
    pub(super) fn check_imported_module_direct_call(&mut self, call: &DirectCallSite<'_>) -> Option<ResolvedType> {
        let (callee, span) = (call.callee, call.span);
        // Imported module function calls whose signatures are known via the stdlib AST cache
        // (for example `math.sqrt(...)`).
        if let Expr::Field(base, method) = &callee.node
            && let Some((module_name, module_path)) = self.imported_module_for_expr(base)
        {
            self.record_module_namespace_reference(base.span, &module_path);
            // Ensure lowering marks the receiver identifier as a module-path binding.
            if let Some((head, head_span)) = Self::dotted_namespace_head(base) {
                let _ = self.check_ident(head, head_span);
            }
            let resolved = match self.resolve_module_namespace_member(&module_path, method) {
                Ok(resolved) => resolved,
                Err(failure) => {
                    self.report_namespace_resolution_failure(failure, span);
                    return Some(ResolvedType::Unknown);
                }
            };
            let identity = resolved.canonical.clone();
            let surface_rust_item = resolved.surface_rust_item.clone();
            let kind = match resolved.kind {
                ResolvedNamespaceMemberKind::SurfaceType(surface) => {
                    return Some(self.check_module_surface_constructor(
                        surface,
                        identity,
                        surface_rust_item,
                        call.type_args,
                        call.args,
                        call.span,
                        call.callee.span,
                    ));
                }
                ResolvedNamespaceMemberKind::Symbol(kind) => kind,
                ResolvedNamespaceMemberKind::Method(_) => return Some(ResolvedType::Unknown),
            };
            let callable = format!("{module_name}.{method}");
            if let Some(library) = resolved.public_library.as_deref()
                && let Some((_, rest)) = self.loaf_module(&module_path)
                && let Some(projection) =
                    self.lookup_pub_library_module_partial_projection(library, &rest, method, &callable)
            {
                self.type_info.record_partial_projection(projection);
            }
            let source_kind = match &kind {
                SymbolKind::Type(type_info) => Self::source_target_kind_for_type_info(type_info).unwrap_or("type"),
                _ => "function",
            };
            if let Some(source_module_path) = resolved.source_module_path.as_ref() {
                self.record_source_target(
                    span,
                    source_module_path.clone(),
                    resolved.source_name.clone(),
                    source_kind,
                );
                self.record_source_target(
                    callee.span,
                    source_module_path.clone(),
                    resolved.source_name.clone(),
                    source_kind,
                );
            }
            if let Some(identity) = resolved.canonical {
                self.type_info.record_resolved_identity(callee.span, identity);
            }
            return Some(self.check_resolved_module_direct_call(
                call,
                &callable,
                kind,
                resolved.source_module_path.unwrap_or_default(),
                resolved.source_name,
                resolved.public_library,
            ));
        }
        None
    }

    /// Check a compiler-owned surface constructor reached through its published standard-library module.
    ///
    /// Surface declarations such as `std.web.Html` deliberately have no source AST entry in the Loaf cache. Their
    /// registry ownership is the same authority used by a direct import, so the qualified form retains that builtin
    /// identity and constructor contract rather than falling through to value-method lookup.
    pub(in crate::typechecker::check_expr) fn check_module_surface_constructor(
        &mut self,
        surface: SurfaceTypeId,
        identity: Option<CanonicalSymbolId>,
        rust_item: Option<RustItemInfo>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        callee_span: Span,
    ) -> ResolvedType {
        let member = surface_types::as_str(surface);
        if surface != SurfaceTypeId::Html {
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
        }
        let parameters = [CallableParam::positional(ResolvedType::Str)];
        self.check_call_arg_types_for_params(args, &parameters);
        self.type_info.record_call_site_callable_params_exact(span, &parameters);
        let identity = identity.or_else(|| self.surface_type_identity(surface));
        let native_path = identity
            .as_ref()
            .and_then(incan_semantics_core::body_ir::native_nominal_item);
        let checked_layout = rust_item.and_then(|info| {
            let metadata = info.metadata.as_deref()?;
            match &metadata.kind {
                RustItemKind::Type(layout) => Some((info.path, layout.clone())),
                _ => None,
            }
        });
        if let Some((path, layout)) = checked_layout {
            self.retain_rust_layout(&path, &layout);
        }
        let result = native_path.map_or_else(|| ResolvedType::Named(member.to_string()), ResolvedType::RustPath);
        if let Some(identity) = identity {
            self.record_body_ir_html_constructor_with_owner(args, span, &identity);
            if callee_span != span {
                self.type_info.record_resolved_identity(callee_span, identity);
            }
        }
        self.record_expr_type(callee_span, result.clone());
        self.type_info
            .expressions
            .ident_kinds
            .insert(callee_span, IdentKind::TypeName);
        result
    }

    /// Check a resolved module function, overload set or public constructor using its retained declaration facts.
    pub(super) fn check_resolved_module_direct_call(
        &mut self,
        call: &DirectCallSite<'_>,
        callable: &str,
        kind: SymbolKind,
        source_module_path: Vec<String>,
        source_name: String,
        public_library: Option<String>,
    ) -> ResolvedType {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
        match (kind, public_library) {
            (SymbolKind::Function(info), _) => {
                let errors_before_call = self.errors.len();
                let result = self.validate_stdlib_module_function_call(
                    callable,
                    &info,
                    type_args,
                    args,
                    span,
                    expected_return_ty,
                );
                let identity = self.type_info.resolved_identity(callee.span).cloned();
                self.check_capturing_call_arguments(identity.as_ref(), callable, &info.params, args);
                self.retain_body_ir_async_call_target(
                    info.is_async,
                    &source_name,
                    span,
                    callee.span,
                    errors_before_call,
                );
                result
            }
            (SymbolKind::FunctionOverloads(overloads), _) => self.validate_function_overload_call_with_callee_span(
                callable,
                &overloads,
                type_args,
                args,
                span,
                Some(callee.span),
                expected_return_ty,
            ),
            (
                SymbolKind::Type(type_info @ (TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))),
                library,
            ) => {
                let canonical_name = if let Some(library) = library {
                    let mut source_type_path = source_module_path.iter().skip(2).cloned().collect::<Vec<_>>();
                    source_type_path.push(source_name.clone());
                    canonical_public_library_type_name(&library, &source_type_path.join("::"))
                } else {
                    source_name
                };
                self.check_public_module_type_constructor_call(
                    PublicModuleConstructorContext {
                        display_name: callable,
                        canonical_name: &canonical_name,
                        type_info: &type_info,
                    },
                    type_args,
                    args,
                    span,
                    callee.span,
                )
            }
            (SymbolKind::RustItem(info), _) => match self.check_declared_rust_direct_call(&info, call) {
                Some(result) => result,
                None => ResolvedType::Unknown,
            },
            _ => ResolvedType::Unknown,
        }
    }

    /// Resolve named and value calls after syntax-specific and imported-module checking has declined.
    pub(super) fn check_remaining_direct_call(&mut self, call: &DirectCallSite<'_>) -> ResolvedType {
        if let Expr::Ident(name) = &call.callee.node {
            if let Some(result) = self.check_class_receiver_direct_call(name, call) {
                return result;
            }
            if let Some(result) = self.check_named_builtin_direct_call(name, call) {
                return result;
            }
            if let Some(result) = self.check_declared_direct_call(name, call) {
                return result;
            }
            if let Some(result) = self.check_legacy_named_constructor_call(name, call) {
                return result;
            }
        }
        if let Some(result) = self.check_decorated_generic_direct_call(call) {
            return result;
        }
        self.check_direct_value_call(call)
    }

    /// Check `cls(...)` only when the active receiver binding proves the classmethod construction context.
    pub(super) fn check_class_receiver_direct_call(
        &mut self,
        name: &str,
        call: &DirectCallSite<'_>,
    ) -> Option<ResolvedType> {
        let (callee, args, span) = (call.callee, call.args, call.span);
        let class_receiver_identity = self
            .symbols
            .lookup(name)
            .and_then(|symbol_id| self.symbols.identity_of(symbol_id))
            .filter(|identity| {
                identity.kind == SemanticSourceTargetKind::Receiver && identity.declaration_name == "cls"
            })
            .cloned();
        if keywords::from_str(name) == Some(KeywordId::Cls)
            && let Some(identity) = class_receiver_identity
            && let (Some(owner_name), Some(self_ty)) = (
                self.current_method_owner.clone(),
                self.current_classmethod_self_ty.clone(),
            )
        {
            let ctor_fields: Option<(std::collections::HashMap<String, FieldInfo>, Vec<String>)> =
                self.lookup_type_info(&owner_name).and_then(|info| match info {
                    TypeInfo::Model(m) => Some((m.fields.clone(), m.field_order.clone())),
                    TypeInfo::Class(c) => Some((c.fields.clone(), c.field_order.clone())),
                    _ => None,
                });
            if let Some((fields, field_order)) = ctor_fields {
                self.type_info.record_resolved_identity(callee.span, identity);
                self.record_expr_type(callee.span, self_ty.clone());
                self.type_info
                    .expressions
                    .ident_kinds
                    .insert(callee.span, IdentKind::TypeName);
                self.check_model_or_class_constructor_call(&owner_name, &owner_name, &fields, &field_order, args, span);
                return Some(self_ty);
            }
        }
        None
    }

    /// Refuse test markers and check named builtins before ordinary declaration dispatch.
    pub(super) fn check_named_builtin_direct_call(
        &mut self,
        name: &str,
        call: &DirectCallSite<'_>,
    ) -> Option<ResolvedType> {
        let (callee, type_args, args, span, expected_return_ty) = (
            call.callee,
            call.type_args,
            call.args,
            call.span,
            call.expected_return_ty,
        );
        let marker_binding_in_scope = self
            .symbols
            .lookup(name)
            .and_then(|id| self.symbols.get(id))
            .is_some_and(|sym| matches!(sym.kind, SymbolKind::Function(_)) && sym.scope == 0);
        if self.testing_marker_import_bindings.contains(name) && marker_binding_in_scope {
            self.check_call_args(args);
            self.errors
                .push(errors::testing_marker_runtime_call_not_supported(name, span));
            return Some(ResolvedType::Unknown);
        }

        if let Some(result) = self.check_builtin_call(name, args, span, expected_return_ty) {
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
                return Some(ResolvedType::Unknown);
            }
            if let Some(builtin) = self.type_info.resolved_builtin_call(span)
                && let Some(identity) = self.symbols.builtin_function_identity(builtin)
            {
                self.type_info.record_resolved_identity(callee.span, identity);
            } else if incan_lang::lang::surface::constructors::from_str(name).is_some()
                || self.active_surface_function_import(name).is_some()
            {
                // Constructor checking has already excluded source shadowing, and an active surface-function
                // import is the binding lookup selects. Retain that binding's proven identity at the callee, so
                // Body IR never recovers it from the import's local name.
                self.record_direct_callee_identity(name, callee.span);
            }
            return Some(result);
        }
        None
    }

    /// Dispatch a snapshot of the resolved declaration without borrowing the symbol table during argument checks.
    pub(super) fn check_declared_direct_call(&mut self, name: &str, call: &DirectCallSite<'_>) -> Option<ResolvedType> {
        let kind = self.lookup_symbol(name)?.kind.clone();
        self.record_declared_direct_call_source(name, &kind, call);
        Some(match kind {
            SymbolKind::Type(info) => self.check_declared_type_direct_call(name, &info, call),
            SymbolKind::Function(info) => self.check_declared_function_direct_call(name, &info, call),
            SymbolKind::FunctionOverloads(overloads) => self.validate_direct_function_overload_call(
                name,
                &overloads,
                call.type_args,
                call.args,
                call.span,
                call.callee.span,
                call.expected_return_ty,
            ),
            SymbolKind::RustItem(info) => return self.check_declared_rust_direct_call(&info, call),
            SymbolKind::Trait(_) => {
                self.check_call_args(call.args);
                self.errors.push(errors::cannot_instantiate_trait(name, call.span));
                ResolvedType::Unknown
            }
            _ => return None,
        })
    }

    /// Retain source targets for declaration-backed calls; function targets also carry checked C ABI contracts.
    pub(super) fn record_declared_direct_call_source(
        &mut self,
        name: &str,
        kind: &SymbolKind,
        call: &DirectCallSite<'_>,
    ) {
        if !matches!(
            kind,
            SymbolKind::Type(_) | SymbolKind::Function(_) | SymbolKind::FunctionOverloads(_)
        ) {
            return;
        }
        let Some(target) = self.source_target_for_symbol(name, kind) else {
            return;
        };
        self.record_source_target(
            call.callee.span,
            target.module_path.clone(),
            target.name.clone(),
            target.kind.clone(),
        );
        if matches!(kind, SymbolKind::Type(_)) {
            self.record_source_target(call.span, target.module_path, target.name, target.kind);
        } else {
            self.record_source_target(
                call.span,
                target.module_path.clone(),
                target.name.clone(),
                target.kind.clone(),
            );
            self.record_c_abi_function_call_target(call.span, target);
        }
    }
}
