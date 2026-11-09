//! Compiler-known, registry, module, native, trait-self, reflection and enum method call targets.

use super::*;
use crate::typechecker::ResolvedNamespaceMemberKind;

impl TypeChecker {
    /// Check compiler-known receiver forms before the general receiver and argument passes.
    ///
    /// C ABI forms, explicit builtin namespaces and `list.repeat` retain their existing dispatch order and selected
    /// identities; a matched form completes checking here rather than being checked again by the general path.
    pub(super) fn check_compiler_known_method_call(&mut self, call: &MethodCallSite<'_>) -> Option<ResolvedType> {
        let (base, method, type_args, args, span) = (call.base, call.method, call.type_args, call.args, call.span);
        if let Some(result) = self.check_c_abi_c_string_constructor(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_abi_span_constructor(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_abi_output_slot_constructor(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_binding_symbol_member_call(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_abi_output_slot_take(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_abi_span_method(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_abi_c_string_pointer(base, method, type_args, args, span) {
            return Some(result);
        }
        if let Some(result) = self.check_c_abi_scoped_c_string_copy(base, method, type_args, args, span) {
            return Some(result);
        }
        if Self::is_explicit_builtin_namespace_expr(base) {
            let result = self.check_explicit_builtin_call(method, args, span);
            if let Some(builtin) = incan_lang::lang::builtins::from_str(method)
                && let Some(identity) = self.symbols.builtin_function_identity(builtin)
            {
                self.type_info.record_resolved_identity(span, identity);
            }
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
                return Some(ResolvedType::Unknown);
            }
            return Some(result);
        }
        if self.is_builtin_list_surface_receiver(base)
            && method == collection_helpers::member(BuiltinCollectionHelperId::ListRepeat)
        {
            self.type_info.record_resolved_identity(
                span,
                Self::compiler_builtin_member_identity(
                    incan_lang::lang::types::collections::as_str(CollectionTypeId::List),
                    collection_helpers::member(BuiltinCollectionHelperId::ListRepeat),
                ),
            );
            if !type_args.is_empty() {
                self.errors.push(errors::type_mismatch(
                    "inferred type arguments",
                    "explicit type arguments",
                    span,
                ));
            }
            self.type_info
                .expressions
                .ident_kinds
                .insert(base.span, IdentKind::TypeName);
            return Some(self.check_builtin_list_repeat_call(args, span));
        }
        None
    }

    /// Check a receiver with the destination's `Self` or Result expectation and retain its original binding surface.
    ///
    /// A declared owner's `Self` becomes that owner, while a trait default stays open. The binding surface is selected
    /// before a Result receiver is refined, matching the ordering of the existing method-call check.
    pub(super) fn check_method_receiver(&mut self, call: &MethodCallSite<'_>) -> (ResolvedType, MemberBindingSurface) {
        let receiver_expected = call.expected_return_ty.and_then(|expected| {
            let owner = match expected {
                ResolvedType::Generic(name, _) | ResolvedType::Named(name) => name,
                _ => return None,
            };
            let method_info = self
                .lookup_semantic_trait_info(owner)
                .and_then(|info| info.methods.get(call.method))
                .or_else(|| match self.lookup_semantic_type_info(owner) {
                    Some(TypeInfo::Model(info)) => info.methods.get(call.method),
                    Some(TypeInfo::Class(info)) => info.methods.get(call.method),
                    Some(TypeInfo::Newtype(info)) => info.methods.get(call.method),
                    Some(TypeInfo::Enum(info)) => info.methods.get(call.method),
                    Some(TypeInfo::Builtin | TypeInfo::TypeAlias) | None => None,
                });
            method_info
                .is_some_and(|info| matches!(info.return_type, ResolvedType::SelfType))
                .then_some(expected)
        });
        let mut base_ty = match receiver_expected {
            Some(expected) => self.check_type_receiver_expr_with_expected(call.base, expected),
            None => self.check_type_receiver_expr(call.base),
        };
        if matches!(base_ty, ResolvedType::SelfType)
            && let Some(owner_ty) = self.current_method_owner_type()
        {
            base_ty = owner_ty;
        }
        let surface = self.checked_member_receiver_surface(call.base, &base_ty);
        if let Some(expected) = call.expected_return_ty
            && matches!(&call.base.node, Expr::Call(_, _, _))
            && matches!(
                &base_ty,
                ResolvedType::Generic(name, args)
                    if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
                        && args.iter().any(Self::contains_unbound_rust_type_var)
            )
            && let Some(expected_result) = Self::result_receiver_expected_type(call.method, expected)
        {
            base_ty = self.check_expr_with_expected(call.base, Some(&expected_result));
        }
        (base_ty, surface)
    }

    /// Select callback, native receiver and enum payload context before any method argument is checked.
    ///
    /// Source list/text/Result methods keep their source contract even when a native carrier is available. Enum payload
    /// expectations are selected here so their arguments are checked against the destination exactly once.
    pub(super) fn prepare_method_argument_context(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
    ) -> MethodArgumentContext {
        let contextual_rust_callable = call.expected_return_ty.and_then(|expected| {
            if call.args.len() == 1 {
                self.rust_callable_alias_signature(expected)
            } else {
                None
            }
        });
        let result_callback = match (base_ty, result_methods::from_str(call.method)) {
            (ResolvedType::Generic(name, type_args), Some(method_id))
                if collection_type_id(name.as_str()) == Some(CollectionTypeId::Result) && type_args.len() == 2 =>
            {
                let input = match method_id {
                    ResultMethodId::Map | ResultMethodId::AndThen | ResultMethodId::Inspect => {
                        Some(type_args[0].clone())
                    }
                    ResultMethodId::MapErr | ResultMethodId::OrElse | ResultMethodId::InspectErr => {
                        Some(type_args[1].clone())
                    }
                    _ => None,
                };
                input.map(|input| {
                    let output = Self::result_callback_expected_output(
                        method_id,
                        &type_args[0],
                        &type_args[1],
                        call.expected_return_ty,
                    );
                    (input, output)
                })
            }
            _ => None,
        };
        let source_list_method = matches!(base_ty, ResolvedType::Generic(name, _)
            if collection_type_id(name) == Some(CollectionTypeId::List) && list_methods::from_str(call.method).is_some());
        let source_variant = match base_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => {
                matches!(self.lookup_semantic_type_info(name), Some(TypeInfo::Enum(info))
            if info.variants.iter().any(|variant| variant == call.method)
                || info.variant_aliases.contains_key(call.method))
            }
            _ => false,
        };
        // A checked source variant owns its constructor even when its admitted name contains namespace segments.
        let rust_receiver_path = if source_variant {
            None
        } else {
            self.rust_canonical_path_for_receiver_type(base_ty).or_else(|| {
                let source_builtin_method = (*base_ty == ResolvedType::Str
                    && string_methods::from_str(call.method).is_some())
                    || (*base_ty == ResolvedType::Bytes && bytes_methods::from_str(call.method).is_some())
                    || result_callback.is_some();
                if source_list_method || source_builtin_method {
                    return None;
                }
                self.rust_value_item_path(call.base)
                    .or_else(|| self.rust_method_receiver_path(call.base))
            })
        };
        let enum_variant_construction =
            self.enum_variant_construction_instantiation(base_ty, call.method, call.expected_return_ty, call.span);
        let enum_variant_parameters = match base_ty {
            ResolvedType::Named(enum_name) | ResolvedType::Generic(enum_name, _) => {
                self.checked_enum_variant_parameters(enum_name, call.method)
            }
            _ => None,
        };
        MethodArgumentContext {
            contextual_rust_callable,
            result_callback,
            rust_receiver_path,
            enum_variant_construction,
            enum_variant_parameters,
        }
    }

    /// Check method arguments in source order, retaining deferred closure facts and contextual payload types.
    ///
    /// Fold callbacks see the preceding accumulator type. A native closure with unresolved bounds still records its
    /// body's selected identities while its premature diagnostics are discarded (#1492); its type stays open. Every
    /// expression is marked as a call argument so a nested Rust call can defer inference diagnostics until the
    /// selected method supplies its exact parameter type.
    pub(super) fn check_method_argument_types(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        context: &MethodArgumentContext,
    ) -> Vec<ResolvedType> {
        let defer_rust_closures = context.rust_receiver_path.is_some();
        let mut arg_types = Vec::with_capacity(call.args.len());
        for (index, arg) in call.args.iter().enumerate() {
            let arg_expr = match arg {
                CallArg::Positional(expr)
                | CallArg::Named(_, expr)
                | CallArg::PositionalUnpack(expr)
                | CallArg::KeywordUnpack(expr) => expr,
            };
            let is_closure = matches!(arg_expr.node, Expr::Closure(_, _));
            let variant_payload_ty = context
                .enum_variant_construction
                .as_ref()
                .and_then(|construction| construction.payloads.get(index))
                .or_else(|| {
                    context
                        .enum_variant_parameters
                        .as_ref()
                        .and_then(|parameters| parameters.get(index))
                        .map(|parameter| &parameter.ty)
                })
                .filter(|payload| !matches!(payload, ResolvedType::Unknown));
            self.call_argument_depth += 1;
            let arg_ty = if defer_rust_closures && context.contextual_rust_callable.is_none() && is_closure {
                let diagnostics_before = self.errors.len();
                self.check_expr(arg_expr);
                self.errors.truncate(diagnostics_before);
                ResolvedType::Unknown
            } else if let Some((input_ty, output_ty)) = context.result_callback.as_ref()
                && is_closure
            {
                let expected = ResolvedType::Function(
                    vec![CallableParam::positional(input_ty.clone())],
                    Box::new(output_ty.clone()),
                );
                self.check_expr_with_expected(arg_expr, Some(&expected))
            } else if is_closure
                && let Some(expected) =
                    self.iterator_callback_expectation(base_ty, call.method, index, arg_types.first())
            {
                self.check_expr_with_expected(arg_expr, Some(&expected))
            } else if let Some(payload_ty) = variant_payload_ty {
                self.check_expr_with_expected(arg_expr, Some(payload_ty))
            } else if matches!(arg, CallArg::Positional(_))
                && !is_closure
                && let Some(expected) = Self::builtin_method_argument_expectation(base_ty, call.method, index)
            {
                let arg_ty = self.check_expr_with_expected(arg_expr, Some(&expected));
                self.record_value_destination_if_compatible(arg_expr.span, &arg_ty, &expected);
                arg_ty
            } else {
                self.check_method_arg_with_rust_callable_alias(arg, context.contextual_rust_callable.as_ref())
            };
            self.call_argument_depth -= 1;
            arg_types.push(arg_ty);
        }
        if let Some(slot) = self.iterator_callback_slot(base_ty, call.method) {
            for arg in call.args {
                let (CallArg::Positional(arg_expr)
                | CallArg::Named(_, arg_expr)
                | CallArg::PositionalUnpack(arg_expr)
                | CallArg::KeywordUnpack(arg_expr)) = arg;
                self.refuse_capturing_callable(arg_expr, &slot);
            }
        }
        arg_types
    }

    /// Check declaration-only registry calls and reject compiler-reserved registry members before argument inference.
    pub(super) fn check_registry_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        if let ResolvedType::Generic(name, arguments) = base_ty
            && name == "Registry"
            && call.method == "entry"
        {
            if let Some(identity) = self
                .method_info_for_owner(name, call.method)
                .and_then(|info| info.identity.clone())
            {
                self.type_info.record_resolved_identity(call.span, identity);
            }
            self.check_call_args(call.args);
            if !self.checking_registry_entry_static_initializer {
                self.errors.push(CompileError::type_error(
                    "Registry.entry(...) is declaration-only; use it as a module static RegistryEntry[K, T] initializer with RegistrySubject.current_unit() or RegistrySubject.package()".to_string(),
                    call.span,
                ));
            }
            return Some(ResolvedType::Generic("RegistryEntry".to_string(), arguments.clone()));
        }
        if let ResolvedType::Generic(name, _) = base_ty
            && name == "Registry"
            && matches!(call.method, "_describe" | "_describe_function" | "_describe_method")
        {
            self.check_call_args(call.args);
            self.errors.push(CompileError::type_error(
                format!(
                    "Registry.{}(...) is compiler-reserved and cannot be called from source",
                    call.method
                ),
                call.span,
            ));
            return Some(ResolvedType::Unit);
        }
        if matches!(&call.base.node, Expr::Ident(name) if name == "RegistrySubject")
            && matches!(call.method, "_checked_current_unit" | "_checked_package")
        {
            self.check_call_args(call.args);
            self.errors.push(CompileError::type_error(
                format!(
                    "RegistrySubject.{}(...) is compiler-reserved and cannot be called from source",
                    call.method
                ),
                call.span,
            ));
            return Some(ResolvedType::Named("RegistrySubject".to_string()));
        }
        None
    }

    /// Select an imported module's concrete member, retaining canonical identity and partial projection before checking
    /// it.
    pub(super) fn check_module_method_call(&mut self, call: &MethodCallSite<'_>) -> Option<ResolvedType> {
        let (module_name, module_path) = self.imported_module_for_expr(call.base)?;
        self.record_module_namespace_reference(call.base.span, &module_path);
        if let Some((head, head_span)) = Self::dotted_namespace_head(call.base) {
            let _ = self.check_ident(head, head_span);
        }
        let resolved = match self.resolve_module_namespace_member(&module_path, call.method) {
            Ok(resolved) => resolved,
            Err(failure) => {
                self.report_namespace_resolution_failure(failure, call.span);
                return Some(ResolvedType::Unknown);
            }
        };
        let identity = resolved.canonical.clone();
        let surface_rust_item = resolved.surface_rust_item.clone();
        let kind = match resolved.kind {
            ResolvedNamespaceMemberKind::Symbol(kind) => kind,
            ResolvedNamespaceMemberKind::SurfaceType(surface) => {
                return Some(self.check_module_surface_constructor(
                    surface,
                    identity,
                    surface_rust_item,
                    call.type_args,
                    call.args,
                    call.span,
                    call.span,
                ));
            }
            ResolvedNamespaceMemberKind::Method(_) => return Some(ResolvedType::Unknown),
        };
        let callable = format!("{module_name}.{}", call.method);
        if let Some(library) = resolved.public_library.as_deref()
            && let Some((_, rest)) = self.loaf_module(&module_path)
            && let Some(projection) =
                self.lookup_pub_library_module_partial_projection(library, &rest, call.method, &callable)
        {
            self.type_info.record_partial_projection(projection);
        }
        if let Some(identity) = resolved.canonical {
            self.type_info.record_resolved_identity(call.span, identity);
        }
        Some(self.check_module_method_target(
            call,
            &callable,
            kind,
            resolved.source_module_path.unwrap_or_default(),
            resolved.source_name,
            resolved.public_library,
        ))
    }

    /// Validate the selected imported callable and retain its declared binding signature for downstream Body IR.
    ///
    /// The declaration may not be in the caller's scope, so lowering must consume these selected parameters rather than
    /// re-derive them from the module qualifier and display spelling.
    pub(super) fn check_module_method_target(
        &mut self,
        call: &MethodCallSite<'_>,
        callable: &str,
        kind: SymbolKind,
        source_module_path: Vec<String>,
        source_name: String,
        public_library: Option<String>,
    ) -> ResolvedType {
        match (kind, public_library) {
            (SymbolKind::Function(info), _) => {
                self.record_source_target(call.span, source_module_path, source_name, "function");
                self.type_info
                    .record_call_site_callable_params_exact(call.span, &info.params);
                let errors_before_call = self.errors.len();
                let result = self.validate_stdlib_module_function_call(
                    callable,
                    &info,
                    call.type_args,
                    call.args,
                    call.span,
                    call.expected_return_ty,
                );
                let identity = self.type_info.resolved_identity(call.span).cloned();
                self.check_capturing_call_arguments(identity.as_ref(), callable, &info.params, call.args);
                self.retain_body_ir_async_call_target(
                    info.is_async,
                    call.method,
                    call.span,
                    call.span,
                    errors_before_call,
                );
                result
            }
            (SymbolKind::FunctionOverloads(overloads), _) => {
                self.record_source_target(call.span, source_module_path, source_name, "function");
                self.validate_function_overload_call_with_callee_span(
                    callable,
                    &overloads,
                    call.type_args,
                    call.args,
                    call.span,
                    Some(call.span),
                    call.expected_return_ty,
                )
            }
            (SymbolKind::Type(info @ (TypeInfo::Model(_) | TypeInfo::Class(_) | TypeInfo::Newtype(_))), library) => {
                let canonical = if let Some(library) = library {
                    let root_len = self.loaf_binding_root(&library).len();
                    let mut source_type_path = source_module_path.iter().skip(root_len).cloned().collect::<Vec<_>>();
                    source_type_path.push(source_name.clone());
                    canonical_public_library_type_name(&library, &source_type_path.join("::"))
                } else {
                    source_name.clone()
                };
                let source_kind = Self::source_target_kind_for_type_info(&info).unwrap_or("type");
                self.record_source_target(call.span, source_module_path, source_name, source_kind);
                self.check_public_module_type_constructor_call(
                    PublicModuleConstructorContext {
                        display_name: callable,
                        canonical_name: &canonical,
                        type_info: &info,
                    },
                    call.type_args,
                    call.args,
                    call.span,
                    call.span,
                )
            }
            _ => ResolvedType::Unknown,
        }
    }

    /// Resolve a native array/slice method or an already selected Rust receiver without repeating source dispatch.
    ///
    /// A selected Rust path with unavailable metadata completes with `Unknown`, preserving permissive interop behavior;
    /// only an unselected native receiver falls through to subsequent source and builtin rules.
    pub(super) fn check_native_method_receiver(
        &mut self,
        call: &MethodCallSite<'_>,
        arg_types: &[ResolvedType],
        rust_receiver_path: Option<String>,
    ) -> Option<ResolvedType> {
        let MethodCallSite {
            base,
            method,
            type_args,
            args,
            span,
            expected_return_ty,
        } = *call;
        if let Some(result) = self.resolve_rust_array_slice_method(RustPathMethodCall {
            rust_path: "[]",
            method,
            receiver_metadata: None,
            receiver_binding: None,
            receiver_expr: base,
            type_args,
            args,
            arg_types,
            receiver_span: base.span,
            expected_return_ty,
            span,
        }) {
            return Some(result);
        }
        let path = rust_receiver_path?;
        if let Some(params) = self.rust_variant_callable_params(&path, method) {
            if !type_args.is_empty() {
                self.errors
                    .push(errors::explicit_call_site_type_args_not_supported(span));
            }
            let constructor = format!("{path}::{method}");
            let boxed_payload = self
                .rust_variant_payload_carriers(&path, method)
                .is_some_and(|carriers| carriers.contains(&incan_lang::interop::RustPayloadCarrier::Boxed));
            if !boxed_payload
                && let Some(metadata) = self.rust_item_metadata_for_path(&constructor)
                && let RustItemKind::Function(signature) = &metadata.kind
            {
                let result = self.validate_rust_function_call_with_expected(
                    &metadata.canonical_path,
                    signature,
                    args,
                    span,
                    expected_return_ty,
                );
                self.retain_rust_variant_construction(&path, method, args, &params, span);
                self.retain_rust_variant_owner(&path, args, &params, expected_return_ty, span);
                return Some(result);
            }
            let arg_types = self.check_call_arg_types_for_params(args, &params);
            let mut type_bindings = std::collections::HashMap::new();
            self.validate_callable_arg_bindings(
                format!("rust::{path}.{method}").as_str(),
                &params,
                args,
                &arg_types,
                &mut type_bindings,
                span,
            );
            self.type_info.record_call_site_callable_params_exact(span, &params);
            self.retain_rust_variant_construction(&path, method, args, &params, span);
            self.retain_rust_variant_owner(&path, args, &params, expected_return_ty, span);
            return Some(ResolvedType::RustPath(path));
        }
        Some(
            self.resolve_rust_path_method_call(RustPathMethodCall {
                rust_path: &path,
                method,
                receiver_metadata: self.rust_metadata_for_receiver_expr(base).as_ref(),
                receiver_binding: self.rust_import_binding_for_receiver_expr(base),
                receiver_expr: base,
                type_args,
                args,
                arg_types,
                receiver_span: base.span,
                expected_return_ty,
                span,
            })
            .unwrap_or(ResolvedType::Unknown),
        )
    }

    /// Check a trait default's open `Self` receiver against its declared slot rather than a concrete implementation.
    ///
    /// Standard-library source keeps the checked result open while retaining the slot, signature and declared result
    /// for Body IR. Other source uses the existing generic-method validation (#1561).
    pub(super) fn check_trait_self_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        if !matches!(base_ty, ResolvedType::SelfType) {
            return None;
        }
        if let Some(trait_name) = self.current_trait_name.clone()
            && let Some(info) = self.trait_method_info_resolved(&trait_name, call.method, call.span)
        {
            if let Some(identity) = &info.identity {
                self.type_info
                    .traits
                    .self_method_identities
                    .insert(call.span, identity.clone());
            }
            if self.checks_standard_library_source() {
                self.type_info
                    .traits
                    .self_method_result_types
                    .insert(call.span, info.return_type.clone());
                self.type_info
                    .traits
                    .self_method_params
                    .insert(call.span, info.params.clone());
                return Some(ResolvedType::Unknown);
            }
            return Some(self.check_generic_method_call(
                call.method,
                info,
                call.type_args,
                call.args,
                arg_types,
                call.span,
                base_ty,
                call.expected_return_ty,
            ));
        }
        Some(ResolvedType::Unknown)
    }

    /// Select and validate a generated reflection member on a supported nominal receiver.
    pub(super) fn check_nominal_reflection_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        if !self.nominal_type_supports_reflection_magic(base_ty, call.method) {
            return None;
        }
        let result = self.reflection_magic_method_return_type(base_ty, call.method)?;
        if let Some(owner) = match base_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => Some(name.as_str()),
            _ => None,
        } && let Some(id) = magic_methods::from_str(call.method)
        {
            self.record_compiler_generated_member_identity(
                owner,
                magic_methods::as_str(id),
                Some(call.base.span),
                call.span,
            );
        }
        self.validate_reflection_magic_call(call.method, call.type_args, call.args, call.span);
        Some(result)
    }

    /// Check generated enum members, validate constructor arguments and retain the selected payload signature.
    ///
    /// A constructor preserves explicit or expected type arguments, or infers them from its checked payloads when
    /// neither is written, instead of erasing them to the enum name. Payload expressions are checked once with their
    /// context, then ordinary call binding validates their types and arity. A construction that none of these
    /// determines is refused (#2762).
    pub(super) fn check_enum_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
        construction: Option<EnumVariantInstantiation>,
        parameters: Option<Vec<CallableParam>>,
    ) -> Option<ResolvedType> {
        let base_is_type_name = self.is_enum_type_name_expr(call.base);
        if let ResolvedType::Named(enum_name) = base_ty
            && let Some(TypeInfo::Enum(info)) = self.lookup_semantic_type_info(enum_name)
            && let Some(value_enum) = info.value_enum.clone()
            && let Some(result) = self.check_value_enum_generated_method_call(ValueEnumGeneratedCall {
                enum_name,
                value_enum: &value_enum,
                method: call.method,
                base_is_type_name,
                type_args: call.type_args,
                args: call.args,
                arg_types,
                span: call.span,
            })
        {
            self.record_compiler_generated_member_identity(enum_name, call.method, Some(call.base.span), call.span);
            return Some(result);
        }
        let (enum_name, identity) = match base_ty {
            ResolvedType::Named(enum_name) | ResolvedType::Generic(enum_name, _) => {
                let Some(TypeInfo::Enum(info)) = self.lookup_semantic_type_info(enum_name) else {
                    return None;
                };
                if !info.variants.iter().any(|variant| variant == call.method)
                    && !info.variant_aliases.contains_key(call.method)
                {
                    return None;
                }
                (enum_name, info.variant_identities.get(call.method).cloned())
            }
            _ => return None,
        };
        let construction = construction
            .or_else(|| self.inferred_enum_variant_construction(base_ty, call.method, call.args, arg_types, call.span));
        let parameters = construction
            .as_ref()
            .map(|value| {
                value
                    .payloads
                    .iter()
                    .cloned()
                    .map(CallableParam::positional)
                    .collect::<Vec<_>>()
            })
            .or(parameters);
        let errors_before_payloads = self.errors.len();
        if let Some(parameters) = parameters {
            self.validate_callable_arg_bindings(
                &format!("{enum_name}.{}", call.method),
                &parameters,
                call.args,
                arg_types,
                &mut HashMap::new(),
                call.span,
            );
            self.type_info
                .record_call_site_callable_params_exact(call.span, &parameters);
        }
        if let Some(identity) = identity {
            self.type_info.record_resolved_identity(call.span, identity);
        }
        let refused = (construction.is_none() && self.errors.len() == errors_before_payloads)
            .then(|| self.refuse_open_enum_construction(call, base_ty, arg_types))
            .flatten();
        let result = match (construction, refused) {
            (Some(construction), _) => ResolvedType::Generic(enum_name.clone(), construction.arguments),
            (None, Some(arguments)) => ResolvedType::Generic(enum_name.clone(), arguments),
            (None, None) => base_ty.clone(),
        };
        let qualified = crate::typechecker::collect::dotted_path_segments(&call.base.node)
            .and_then(|segments| self.qualified_nominal_carrier(&segments));
        Some(match (qualified, result) {
            (Some(carrier), ResolvedType::Named(_)) => ResolvedType::Named(carrier),
            (Some(carrier), ResolvedType::Generic(_, arguments)) => ResolvedType::Generic(carrier, arguments),
            (_, result) => result,
        })
    }
}
