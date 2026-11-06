//! Nominal and newtype receiver methods, method fallbacks, and receiver protocol calls.

use super::*;

impl TypeChecker {
    /// Resolve generic and nongeneric source receivers through the same declaration method tables.
    ///
    /// Named receivers without a visible type remain permissive when their registry surface is open; generic receivers
    /// without a declaration continue to their bound/external fallback. Only a nongeneric enum receives the existing
    /// generated message member here.
    pub(super) fn resolve_nominal_receiver_method(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        surface: MemberBindingSurface,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let (name, generic) = match base_ty {
            ResolvedType::Generic(name, _) => (name, true),
            ResolvedType::Named(name) => (name, false),
            _ => return None,
        };
        let Some(info) = self.lookup_semantic_type_info(name).cloned() else {
            let open_surface = surface_types::from_str(name).is_some_and(surface_accepts_unknown_methods);
            return if generic || !open_surface {
                None
            } else {
                Some(ResolvedType::Unknown)
            };
        };
        if let TypeInfo::Newtype(newtype) = &info {
            return self.resolve_nominal_newtype_method(call, base_ty, name, newtype, surface, arg_types);
        }
        if !generic
            && matches!(info, TypeInfo::Enum(_))
            && let Some(id @ enum_helpers::EnumHelperId::Message) = enum_helpers::from_str(call.method)
        {
            self.record_compiler_generated_member_identity(
                name,
                enum_helpers::as_str(id),
                Some(call.base.span),
                call.span,
            );
            return Some(ResolvedType::Str);
        }
        let (methods, overloads, adoptions) = match &info {
            TypeInfo::Model(model) => (
                &model.methods,
                &model.method_overloads,
                self.trait_adoptions_for_type_methods(&model.trait_adoptions, &model.derives),
            ),
            TypeInfo::Class(class) => (
                &class.methods,
                &class.method_overloads,
                self.trait_adoptions_for_type_methods(&class.trait_adoptions, &class.derives),
            ),
            TypeInfo::Enum(enumeration) => (
                &enumeration.methods,
                &enumeration.method_overloads,
                self.trait_adoptions_for_type_methods(&enumeration.trait_adoptions, &enumeration.derives),
            ),
            _ => return None,
        };
        self.resolve_named_method(
            methods,
            Some(overloads),
            Some(&adoptions),
            call.method,
            surface,
            call.type_args,
            call.args,
            arg_types,
            call.span,
            base_ty,
            call.expected_return_ty,
        )
    }

    /// Resolve a newtype's declared method alias before considering its native backing type.
    ///
    /// A declared rusttype method may retain a native-to-source return coercion. Native backing lookup is a fallback
    /// only when no declared method was selected, and uses the same receiver expression and destination expectation.
    pub(super) fn resolve_nominal_newtype_method(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        owner: &str,
        newtype: &NewtypeInfo,
        surface: MemberBindingSurface,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let method = self.resolve_newtype_method_name(newtype, call.method);
        let adoptions = self.newtype_trait_adoptions_for_type_methods(owner, newtype);
        if let Some(result) = self.resolve_named_method(
            &newtype.methods,
            Some(&newtype.method_overloads),
            Some(&adoptions),
            method,
            surface,
            call.type_args,
            call.args,
            arg_types,
            call.span,
            base_ty,
            call.expected_return_ty,
        ) {
            if newtype.is_rusttype {
                self.retain_declared_rusttype_receiver_call(base_ty, &newtype.underlying, method, call.span);
                self.maybe_record_rusttype_return_coercion(newtype, method, &result, call.span);
            }
            return Some(result);
        }
        if newtype.is_rusttype
            && let ResolvedType::RustPath(path) = &newtype.underlying
        {
            return self.resolve_rust_path_method_call(RustPathMethodCall {
                rust_path: path,
                method,
                receiver_metadata: None,
                receiver_binding: None,
                receiver_expr: call.base,
                type_args: call.type_args,
                args: call.args,
                arg_types,
                receiver_span: call.base.span,
                expected_return_ty: call.expected_return_ty,
                span: call.span,
            });
        }
        None
    }

    /// Apply an exact receiver adapter already admitted for this transparent rusttype method to an ordinary call.
    ///
    /// Direct-route rusttypes require every body-less method to back an authored interop edge. Reusing that edge's
    /// inspected call keeps normal method syntax on the native backing type instead of emitting the source stub.
    pub(super) fn retain_declared_rusttype_receiver_call(
        &mut self,
        receiver: &ResolvedType,
        underlying: &ResolvedType,
        method: &str,
        span: Span,
    ) {
        let selected = self
            .type_info
            .rust
            .declared_conversions
            .iter()
            .find(|conversion| {
                (conversion.source == *receiver || conversion.source == *underlying)
                    && conversion.call.has_receiver
                    && conversion.call.path.rsplit("::").next() == Some(method)
            })
            .map(|conversion| conversion.call.clone());
        if let Some(call) = selected {
            self.type_info.rust.checked_calls.insert(span, call);
        }
    }

    /// Apply the remaining bound, backend-magic, external and missing-method rules in their established order.
    pub(super) fn check_method_fallback(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> ResolvedType {
        if let Some(result) = self.resolve_bound_placeholder_method(call, base_ty, arg_types) {
            return result;
        }
        if let Some(id) = magic_methods::from_str(call.method)
            && !matches!(
                id,
                magic_methods::MagicMethodId::ClassName
                    | magic_methods::MagicMethodId::Fields
                    | magic_methods::MagicMethodId::FieldValue
                    | magic_methods::MagicMethodId::FieldItems
            )
        {
            return ResolvedType::Unknown;
        }
        if let ResolvedType::Generic(name, _) = base_ty
            && self.lookup_semantic_type_info(name).is_none()
        {
            return ResolvedType::Unknown;
        }
        if let Some(result) = self.check_unresolved_type_parameter_method(call, base_ty) {
            return result;
        }
        self.check_missing_method(call, base_ty, arg_types)
    }

    /// Resolve a generic placeholder's declared bound before permissive fallback can erase its result type.
    ///
    /// A borrowed placeholder retains protocol payloads such as `Iterator[T].__next__() -> Option[T]` downstream.
    pub(super) fn resolve_bound_placeholder_method(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let receiver = match base_ty {
            ResolvedType::Ref(inner) => inner.as_ref(),
            _ => base_ty,
        };
        if !self.is_generic_placeholder_type(receiver) {
            return None;
        }
        if let Some(result) = self.resolve_bound_rust_trait_method(call, receiver, arg_types) {
            return Some(result);
        }
        let name = self.generic_placeholder_name(receiver).map(str::to_string)?;
        self.resolve_generic_placeholder_method(
            &name,
            call.method,
            call.type_args,
            call.args,
            arg_types,
            call.span,
            base_ty,
            call.expected_return_ty,
        )
    }

    /// Retain generic reflection and inferred Clone facts while enforcing visible rigid type-parameter bounds.
    ///
    /// An invisible external bound remains open. A visible bound must declare non-Clone members; a placeholder outside
    /// an enclosing declaration keeps the existing permissive inferred result.
    pub(super) fn check_unresolved_type_parameter_method(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
    ) -> Option<ResolvedType> {
        if !self.is_generic_placeholder_type(base_ty) {
            return None;
        }
        if let Some(result) = self.generic_reflection_magic_method_return_type(call.method) {
            if let Some(id) = magic_methods::from_str(call.method) {
                self.record_compiler_generated_member_identity(
                    self.generic_placeholder_name(base_ty).unwrap_or_default(),
                    magic_methods::as_str(id),
                    Some(call.base.span),
                    call.span,
                );
            }
            self.validate_reflection_magic_call(call.method, call.type_args, call.args, call.span);
            return Some(result);
        }
        if let Some(parameter) = self.rigid_type_param_name(base_ty)
            && call.method != trait_bounds::rust::CLONE_METHOD
        {
            if self.active_type_param_bounds_are_visible(parameter)
                && !self.active_type_param_bound_declares_member(parameter, call.method)
            {
                self.errors.push(errors::type_parameter_method_not_declared(
                    parameter,
                    call.method,
                    call.span,
                ));
            }
            return Some(ResolvedType::Unknown);
        }
        self.record_body_ir_inferred_clone(base_ty, call.method, call.type_args, call.args, call.span);
        Some(base_ty.clone())
    }

    /// Resolve an unambiguous source method before arguments require a general contextual prepass.
    pub(super) fn resolve_source_method_before_arguments(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        surface: MemberBindingSurface,
    ) -> Option<ResolvedType> {
        let prepass = SourceMethodPrepass {
            method: call.method,
            receiver_surface: surface,
            type_args: call.type_args,
            args: call.args,
            span: call.span,
            receiver_ty: base_ty,
            expected_return_ty: call.expected_return_ty,
            shared_receiver: false,
        };
        let result = self.resolve_unambiguous_source_method_without_arg_prepass(&prepass)?;
        self.type_info
            .inherit_same_trait_method_module(call.base.span, call.span);
        Some(result)
    }

    /// Apply established builtin receiver policies in order after native, trait-Self and enum dispatch declined.
    pub(super) fn check_builtin_receiver_method_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        if let Some(result) = self.runtime_primitive_method_return(base_ty, call.method) {
            return Some(result);
        }
        if let Some(result) = self.check_text_method_call(call, base_ty, arg_types) {
            return Some(result);
        }
        self.check_builtin_collection_method_args(base_ty, call.method, call.args, call.span);
        if let Some(result) = self.check_frozen_collection_method_call(call, base_ty, arg_types) {
            return Some(result);
        }
        if let Some(result) = self.check_option_method_call(call, base_ty, arg_types) {
            return Some(result);
        }
        if let Some(result) = self.check_result_unwrap_method_call(call, base_ty, arg_types) {
            return Some(result);
        }
        if let ResolvedType::Generic(name, arguments) = base_ty
            && collection_type_id(name.as_str()) == Some(CollectionTypeId::Result)
            && arguments.len() == 2
            && Self::result_combinator_name(call.method)
        {
            return Some(self.check_result_combinator_method(
                arguments[0].clone(),
                arguments[1].clone(),
                call.method,
                call.args,
                arg_types,
                call.span,
            ));
        }
        self.check_generic_collection_method_call(call, base_ty, arg_types)
    }

    /// Resolve checked trait receivers before nominal lookup, retaining module context for a selected trait member.
    pub(super) fn resolve_receiver_trait_method(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        surface: MemberBindingSurface,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        let trait_name = match base_ty {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => Some(name.as_str()),
            _ => None,
        };
        if trait_name.is_some_and(|name| self.lookup_semantic_trait_info(name).is_some()) {
            if let Some(result) = self.resolve_trait_receiver_method(
                base_ty,
                call.method,
                surface,
                call.type_args,
                call.args,
                arg_types,
                call.span,
                call.expected_return_ty,
            ) {
                self.type_info
                    .inherit_same_trait_method_module(call.base.span, call.span);
                return Some(result);
            }
            self.errors
                .push(errors::missing_method(&base_ty.to_string(), call.method, call.span));
            return Some(ResolvedType::Unknown);
        }
        self.resolve_unbound_trait_receiver_method(
            call.base.span,
            base_ty,
            call.method,
            surface,
            call.type_args,
            call.args,
            arg_types,
            call.span,
            call.expected_return_ty,
        )
    }

    /// Validate receiver mutation and retain the checked binding and builtin member facts before method selection.
    pub(super) fn check_method_receiver_use(&mut self, call: &MethodCallSite<'_>, base_ty: &ResolvedType) {
        self.reject_mutating_call_through_immutable_self(call.base, base_ty, call.method, call.span);
        self.refuse_mutating_call_through_read_only_binding(call.base, base_ty, call.method, call.span);
        self.note_mut_param_method_call(call.base, base_ty, call.method);
        self.record_mutable_receiver_method_call(base_ty, call.method, call.span);
        if let Some(identity) = Self::compiler_builtin_method_identity(base_ty, call.method) {
            self.note_lazy_result_member(&identity);
            self.type_info.record_resolved_identity(call.span, identity);
        }
    }

    /// Identify the existing `to_vec()` source contract on a borrowed list without copying its resolved type.
    pub(super) fn is_borrowed_list_copy_call(call: &MethodCallSite<'_>, base_ty: &ResolvedType) -> bool {
        call.method == "to_vec"
            && call.args.is_empty()
            && matches!(base_ty, ResolvedType::Ref(inner) | ResolvedType::RefMut(inner)
                if matches!(inner.as_ref(), ResolvedType::Generic(name, _)
                    if collection_type_id(name.as_str()) == Some(CollectionTypeId::List)))
    }

    /// Reject property invocation and resolve list/iterator protocols before native method lookup.
    pub(super) fn check_receiver_protocol_call(
        &mut self,
        call: &MethodCallSite<'_>,
        base_ty: &ResolvedType,
        arg_types: &[ResolvedType],
    ) -> Option<ResolvedType> {
        if self.receiver_has_computed_property(base_ty, call.method, call.span) {
            self.errors
                .push(errors::property_called_as_method(call.method, call.span));
            return Some(ResolvedType::Unknown);
        }
        if let Some(result) =
            self.resolve_list_method_by_argument_count(base_ty, call.method, call.args, arg_types, call.span)
        {
            return Some(result);
        }
        let result =
            self.resolve_iterator_protocol_method_call(base_ty, call.method, call.args, arg_types, call.span)?;
        if self.iterator_protocol_element_type(base_ty).is_some() {
            self.mark_direct_iterator_binding_consumed(call.base, call.method, call.span);
        }
        Some(result)
    }

    /// Resolve `Binding.Enum.Variant` through the checked binding descriptor.
    ///
    /// Binding enums are data-shaped class members, not source enum declarations. This records the selected variant so
    /// target verification can supply its folded numeric value before lowering emits an ordinary Incan integer literal.
    pub(super) fn check_c_binding_enum_constant(
        &mut self,
        base: &Spanned<Expr>,
        field: &str,
        span: Span,
    ) -> Option<ResolvedType> {
        let Expr::Field(binding_expr, enumeration) = &base.node else {
            return None;
        };
        let Expr::Ident(binding) = &binding_expr.node else {
            return None;
        };
        let descriptor = self.type_info.c_abi.bindings.get(binding)?;
        let declaration = descriptor
            .enums
            .iter()
            .find(|candidate| candidate.name == *enumeration)?;
        if !declaration.variants.iter().any(|variant| variant.name == field) {
            self.errors.push(CompileError::type_error(
                format!("C binding enum `{binding}.{enumeration}` does not declare `{field}`"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.type_info.c_abi.enum_accesses.push(CBindingEnumAccess {
            span,
            binding: binding.clone(),
            enumeration: enumeration.clone(),
            variant: field.to_string(),
        });
        Some(ResolvedType::Int)
    }

    /// Resolve methods supplied by Clone for anonymous union wrappers.
    pub(super) fn resolve_union_clone_trait_method_call(
        &mut self,
        receiver_ty: &ResolvedType,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        if !receiver_ty.is_union() {
            return None;
        }

        let adoption = TypeBoundInfo {
            name: core_traits::as_str(TraitId::Clone).to_string(),
            source_name: None,
            type_args: Vec::new(),
            module_path: None,
            implementation_type_params: Vec::new(),
            inferred: false,
        };
        let method_info = self.trait_method_info_resolved_for_adoption(&adoption, method, span)?;
        if !self.is_clone_type(receiver_ty) {
            self.errors.push(CompileError::type_error(
                format!("Union type '{receiver_ty}' cannot use '{method}(...)' because not all variants are cloneable"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        Some(self.check_resolved_generic_method_call(
            method,
            method_info,
            type_args,
            args,
            arg_types,
            span,
            receiver_ty,
            None,
        ))
    }

    /// Return the owner type parameters an associated function's result carries that neither its arguments nor a
    /// declared default can fix, in declaration order; empty when the call is not open.
    ///
    /// `HashMap::new() -> HashMap<K, V, RandomState>` leaves `K` and `V` to the caller's context, while the hasher
    /// parameter has a declared default and is never open; `from_iter(iter: I) where I: IntoIterator<Item = (K,
    /// V)>` names them in a parameter, so the argument fixes them and nothing is open. A result written as `Self`
    /// stands for the owner with every parameter. The displays are compared token by token so a parameter named
    /// `K` does not match inside `Key`.
    pub(in crate::typechecker) fn open_rust_owner_type_args(
        sig: &RustFunctionSig,
        type_info: &incan_lang::interop::RustTypeInfo,
    ) -> Vec<String> {
        let mentions = |display: &str, param: &str| {
            display
                .split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .any(|token| token == param)
        };
        let result_is_self = mentions(sig.return_type.as_str(), "Self");
        let open = type_info
            .type_params
            .iter()
            .enumerate()
            .filter(|(index, param)| {
                let has_default = type_info
                    .type_param_defaults
                    .get(*index)
                    .is_some_and(|default| default.is_some());
                !has_default && (result_is_self || mentions(sig.return_type.as_str(), param))
            })
            .map(|(_, param)| param.clone())
            .collect::<Vec<_>>();
        if open.iter().any(|param| {
            sig.params
                .iter()
                .any(|argument| mentions(argument.type_display.as_str(), param))
        }) {
            return Vec::new();
        }
        open
    }
}
