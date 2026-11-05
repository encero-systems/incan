//! Method call checking entry points, receiver mutation checks, and builtin method argument expectations.

use super::*;

impl TypeChecker {
    /// Validate the RFC 006 `Generator[T].map(fn)` helper and return the mapped element type.
    pub(super) fn generator_map_return_type(
        &mut self,
        elem: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> ResolvedType {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one callable argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return ResolvedType::Unknown;
        }

        let Some(arg_ty) = arg_types.first() else {
            return ResolvedType::Unknown;
        };
        let ResolvedType::Function(params, ret) = arg_ty else {
            self.errors.push(errors::type_mismatch(
                &format!("({elem}) -> _"),
                &arg_ty.to_string(),
                span,
            ));
            return ResolvedType::Unknown;
        };
        let [param] = params.as_slice() else {
            self.errors.push(errors::type_mismatch(
                "one-parameter callable",
                &format!("{}-parameter callable", params.len()),
                span,
            ));
            return ResolvedType::Unknown;
        };
        if !self.types_compatible(elem, &param.ty) {
            self.errors
                .push(errors::type_mismatch(&param.ty.to_string(), &elem.to_string(), span));
        }
        ret.as_ref().clone()
    }

    /// Validate the RFC 006 `Generator[T].filter(predicate)` helper.
    pub(super) fn validate_generator_filter_arg(
        &mut self,
        elem: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one callable argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return;
        }

        let Some(arg_ty) = arg_types.first() else {
            return;
        };
        let ResolvedType::Function(params, ret) = arg_ty else {
            self.errors.push(errors::type_mismatch(
                &format!("({elem}) -> bool"),
                &arg_ty.to_string(),
                span,
            ));
            return;
        };
        let [param] = params.as_slice() else {
            self.errors.push(errors::type_mismatch(
                "one-parameter callable",
                &format!("{}-parameter callable", params.len()),
                span,
            ));
            return;
        };
        if !self.types_compatible(elem, &param.ty) {
            self.errors
                .push(errors::type_mismatch(&param.ty.to_string(), &elem.to_string(), span));
        }
        if !self.types_compatible(ret, &ResolvedType::Bool) {
            self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
        }
    }

    /// Validate the RFC 006 `Generator[T].take(count)` helper.
    pub(super) fn validate_generator_take_arg(&mut self, args: &[CallArg], arg_types: &[ResolvedType], span: Span) {
        if args.len() != 1 {
            self.errors.push(errors::type_mismatch(
                "one int argument",
                &format!("{} argument(s)", args.len()),
                span,
            ));
            return;
        }
        if let Some(arg_ty) = arg_types.first()
            && !self.types_compatible(arg_ty, &ResolvedType::Int)
        {
            self.errors
                .push(errors::type_mismatch("int", &arg_ty.to_string(), span));
        }
    }

    /// Type-check a method call (`base.method(args...)`) and return the method's return type. Native boxed variants
    /// validate their semantic payload before the retained Box construction restores exact Rust storage.
    pub(in crate::typechecker::check_expr) fn check_method_call(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> ResolvedType {
        self.check_method_call_with_expected(base, method, type_args, args, span, None)
    }

    /// Refuse a call that changes a place rooted at `self` inside a plain-`self` method (#1723).
    ///
    /// `self.items.pop()` in a `def pop(self)` body writes through a receiver the generated code borrows shared, so
    /// the build would refuse it; reporting it here names the receiver to declare instead. The receiver chain is
    /// followed the way assignments are (`Self::self_rooted_place`), and the method is judged by its declarations
    /// ([`Self::method_receiver_change`]): only a call every candidate of which changes the receiver is refused. A
    /// method the checker cannot classify is left alone, as is a receiver that is not rooted at `self`, which
    /// [`Self::refuse_mutating_call_through_read_only_binding`] judges instead.
    pub(super) fn reject_mutating_call_through_immutable_self(
        &mut self,
        base: &Spanned<Expr>,
        base_ty: &ResolvedType,
        method: &str,
        span: Span,
    ) {
        if self.current_immutable_self_method.is_none() {
            return;
        }
        let Some(place) = Self::self_rooted_place(base) else {
            return;
        };
        if self.method_receiver_change(base_ty, method, span) == ReceiverChange::Changes {
            self.reject_write_through_immutable_self(&place, SelfMutation::MutatingCall { callee: method }, span);
        }
    }

    /// Refuse a call that changes a place rooted at a local declared without `mut` or a parameter not marked `mut`
    /// (#1561).
    ///
    /// A binding is immutable unless declared `mut`, and so is what it holds, so a call that changes its receiver goes
    /// through the same refusal as a field or element write ([`Self::refuse_write_through_read_only_binding`]). The
    /// method is judged by its declarations ([`Self::method_receiver_change`]): only a call every candidate of which
    /// changes the receiver is refused, so a method the checker cannot classify is left alone. A generator's methods
    /// advance it but take it by value, which a binding declared without `mut` hands over, so they are left alone too.
    pub(super) fn refuse_mutating_call_through_read_only_binding(
        &mut self,
        base: &Spanned<Expr>,
        base_ty: &ResolvedType,
        method: &str,
        span: Span,
    ) {
        let generator = matches!(base_ty, ResolvedType::Generic(name, _)
            if collection_type_id(name.as_str()) == Some(CollectionTypeId::Generator));
        if !generator && self.method_receiver_change(base_ty, method, span) == ReceiverChange::Changes {
            self.refuse_write_through_read_only_binding(base, span);
        }
    }

    /// Record the call at `span` when the receiver's source type declares `method` only with `mut self`.
    ///
    /// Only the type's own declarations answer, after resolving a method alias; a method a trait provides reaches
    /// lowering through its trait dispatch fact, which carries the receiver itself. Builtin receivers and unknown
    /// methods are not recorded.
    pub(super) fn record_mutable_receiver_method_call(&mut self, base_ty: &ResolvedType, method: &str, span: Span) {
        let type_name = match base_ty {
            ResolvedType::Named(name) => name,
            ResolvedType::Generic(name, _) if collection_type_id(name.as_str()).is_none() => name,
            _ => return,
        };
        let Some((declared, _)) = self.declared_method_receivers_and_adoptions(type_name, method) else {
            return;
        };
        if !declared.is_empty() && declared.iter().all(|receiver| *receiver == Some(Receiver::Mutable)) {
            self.type_info.calls.mutable_receiver_method_calls.insert(span);
        }
    }

    /// Classify a checked member-call receiver without recovering ownership from its spelling.
    ///
    /// The identifier resolution fact is authoritative for ordinary and parenthesized type expressions. A
    /// `TypeToken[T]` value is also type-owned, which covers checked classmethod receiver values. Everything else is
    /// an instance receiver, even when its source spelling happens to begin with an uppercase letter.
    pub(super) fn checked_member_receiver_surface(
        &self,
        base: &Spanned<Expr>,
        base_ty: &ResolvedType,
    ) -> MemberBindingSurface {
        if matches!(base_ty, ResolvedType::TypeToken(_))
            || matches!(
                self.type_info.ident_kind(base.span),
                Some(IdentKind::TypeName | IdentKind::Trait)
            )
        {
            return MemberBindingSurface::Type;
        }
        match &base.node {
            Expr::Paren(inner) | Expr::Index(inner, _) => self.checked_member_receiver_surface(inner, base_ty),
            _ => MemberBindingSurface::Instance,
        }
    }

    /// Type-check the consuming read of one compiler-managed C output slot.
    pub(super) fn check_c_abi_output_slot_take(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if method != "take" {
            return None;
        }
        let Expr::Ident(local_name) = &base.node else {
            return None;
        };
        let slot_identity = self
            .symbols
            .lookup(local_name)
            .and_then(|symbol_id| self.symbols.get(symbol_id))
            .and_then(|symbol| match &symbol.kind {
                SymbolKind::Variable(variable) => match &variable.ty {
                    ResolvedType::Named(identity)
                        if incan_lang::lang::c_abi::parse_output_slot_type_identity(identity).is_some() =>
                    {
                        Some(identity.clone())
                    }
                    _ => None,
                },
                _ => None,
            });
        let slot = slot_identity.as_deref().and_then(|identity| {
            self.type_info
                .c_abi
                .output_slots
                .iter()
                .find(|slot| slot.identity == identity)
                .cloned()
        });
        let Some(slot) = slot else {
            if self.pending_c_abi_output_slots.contains_key(local_name) {
                self.errors.push(CompileError::type_error(
                    format!("C output slot `{local_name}` must be passed to a checked raw C call before take()"),
                    span,
                ));
                return Some(ResolvedType::Unknown);
            }
            return None;
        };
        if self.closure_depth > slot.closure_depth {
            self.errors.push(CompileError::type_error(
                "C output slots cannot be captured outside their creating frame".to_string(),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if !type_args.is_empty() || !args.is_empty() {
            self.errors.push(CompileError::type_error(
                "C output slot take() accepts no type or value arguments".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let Some(slot_identity) = slot_identity else {
            return Some(ResolvedType::Unknown);
        };
        if self.consumed_c_abi_output_slots.contains_key(&slot_identity) {
            self.errors.push(CompileError::type_error(
                format!("C output slot `{local_name}` may be consumed by take() only once"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if !self.available_c_abi_output_slots.contains(&slot_identity) {
            self.errors.push(CompileError::type_error(
                format!(
                    "C output slot `{local_name}` is not readable on this path; guard take() with a declared {}.{} outcome",
                    slot.binding, slot.symbol
                ),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        // The closed receiver is a permitted slot read; retain its ordinary canonical binding facts too.
        let receiver_span = base.span;
        let inserted = self.c_abi_raw_argument_spans.insert(receiver_span);
        self.check_expr(base);
        if inserted {
            self.c_abi_raw_argument_spans.remove(&receiver_span);
        }
        self.available_c_abi_output_slots.remove(&slot_identity);
        self.consumed_c_abi_output_slots.insert(slot_identity, span);
        self.type_info
            .c_abi
            .bridge_calls
            .push(crate::typechecker::type_info::CAbiBridgeCall {
                span,
                operation: incan_semantics_core::body_ir::CheckedBridgeOperation::TakeOutputSlot,
                slot_identity: Some(slot.identity.clone()),
            });
        Some(Self::c_raw_call_type(&slot.binding, &slot.value))
    }

    /// Resolve a declared enum variant using explicit receiver arguments first, then a same-declaration destination.
    /// Imported aliases and module-qualified names use retained identities rather than matching display spellings.
    ///
    /// A variant payload is declared in the enum's own vocabulary — `Items(list[Elem])` — and that vocabulary is bound
    /// nowhere at a construction site. A model constructor can recover from this by falling back to the supplied
    /// value's own checked type, because the site inferred one. A variant payload has nothing to fall back to:
    /// `Holder.Items([])` checks the literal as `Unknown`, and only the destination says what `Elem` is here. So the
    /// declared payload must be substituted through the enum's arguments instead.
    ///
    /// The destination is the *immediate* expected type, never the enclosing function's return type read directly.
    /// That distinction is what keeps this sound: a function returning `Holder[Picked]` may legitimately build a
    /// `Holder[str]` for a local, and there the local's own annotation is the expectation. Only a same-declaration,
    /// arity-matching instantiation is usable — anything else would substitute one declaration's parameters with
    /// another's arguments — so every other shape yields `None` and leaves the unsubstituted behavior in place.
    ///
    /// See #1516, and #1507 for the model half of the same defect.
    pub(super) fn enum_variant_construction_instantiation(
        &mut self,
        base_ty: &ResolvedType,
        method: &str,
        expected_return_ty: Option<&ResolvedType>,
        span: Span,
    ) -> Option<EnumVariantInstantiation> {
        let (enum_name, explicit_arguments) = match base_ty {
            ResolvedType::Named(name) => (name, None),
            ResolvedType::Generic(name, arguments) => (name, Some(arguments)),
            _ => return None,
        };
        let Some(TypeInfo::Enum(enum_info)) = self.lookup_semantic_type_info(enum_name) else {
            return None;
        };
        if !enum_info.variants.iter().any(|variant| variant == method)
            && !enum_info.variant_aliases.contains_key(method)
        {
            return None;
        }
        let arguments = if let Some(arguments) = explicit_arguments {
            if arguments.len() != enum_info.type_params.len() {
                let written = arguments.iter().map(ToString::to_string).collect::<Vec<_>>();
                let error = errors::explicit_type_arg_arity(enum_name, &enum_info.type_params, &written, span);
                self.errors.push(error);
                return None;
            }
            arguments
        } else {
            if enum_info.type_params.is_empty() {
                return None;
            }
            let ResolvedType::Generic(expected_name, arguments) = expected_return_ty? else {
                return None;
            };
            if !self.source_type_names_same_declaration(expected_name, enum_name)
                || arguments.len() != enum_info.type_params.len()
            {
                return None;
            }
            arguments
        };
        // An alias shares its target's payload, and `variant_fields` is keyed by the canonical name only.
        let canonical_variant = enum_info
            .variant_aliases
            .get(method)
            .map(String::as_str)
            .unwrap_or(method);
        let substitutions = type_param_subst_map(&enum_info.type_params, arguments);
        let payloads = enum_info
            .variant_fields
            .get(canonical_variant)
            .map(|declared| {
                declared
                    .iter()
                    .map(|payload| substitute_resolved_type(payload, &substitutions))
                    .collect()
            })
            .unwrap_or_default();
        Some(EnumVariantInstantiation {
            arguments: arguments.clone(),
            payloads,
        })
    }

    /// Type-check `receiver.method(...)` once the receiver, method name, explicit type arguments, and value arguments
    /// have been identified, against an optional type for the destination the result is being produced for.
    ///
    /// This is the shared arrival point for far more than ordinary user methods: C ABI constructors, `Result` and
    /// `Option` combinators, string and collection surfaces, iterator protocol methods, Rust-receiver calls resolved
    /// through inspected metadata, and `Enum.Variant(...)` construction all resolve here. Two consequences shape the
    /// body. The receiver type is established first, because nearly every later decision keys off it; and the
    /// arguments are checked exactly once, part-way through, so any contextual expectation an argument needs — a
    /// closure's parameter types, an enum variant's substituted payload — must be resolved before that point rather
    /// than corrected afterwards.
    ///
    /// `expected_return_ty` is the destination type, not the enclosing function's return type. Passing the immediate
    /// destination is what lets a construction be checked against the instantiation it is actually built for.
    pub(in crate::typechecker::check_expr) fn check_method_call_with_expected(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        self.type_info.traits.inferred_method_identities.remove(&span);
        let call = MethodCallSite {
            base,
            method,
            type_args,
            args,
            span,
            expected_return_ty,
        };

        // ---- Receiver and declaration-selected forms ----
        if let Some(result) = self.check_compiler_known_method_call(&call) {
            return result;
        }
        let (base_ty, receiver_surface) = self.check_method_receiver(&call);

        // If the receiver type is Unknown, be permissive and do not error on methods.
        if matches!(base_ty, ResolvedType::Unknown) {
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }
        self.check_method_receiver_use(&call, &base_ty);

        if let Some(result) = self.check_registry_method_call(&call, &base_ty) {
            return result;
        }

        if Self::is_borrowed_list_copy_call(&call, &base_ty)
            && let ResolvedType::Ref(inner) | ResolvedType::RefMut(inner) = base_ty
        {
            return *inner;
        }

        if let Some(result) = self.check_module_method_call(&call) {
            return result;
        }

        if let Some(result) = self.resolve_source_method_before_arguments(&call, &base_ty, receiver_surface) {
            return result;
        }

        // ---- Arguments and native method resolution ----
        let argument_context = self.prepare_method_argument_context(&call, &base_ty);
        let arg_types = self.check_method_argument_types(&call, &base_ty, &argument_context);
        let MethodArgumentContext {
            rust_receiver_path,
            enum_variant_construction,
            enum_variant_parameters,
            ..
        } = argument_context;

        if let Some(result) = self.check_receiver_protocol_call(&call, &base_ty, &arg_types) {
            return result;
        }

        if let Some(result) = self.check_native_method_receiver(&call, &arg_types, rust_receiver_path) {
            return result;
        }

        if let Some(ret) = self.check_numeric_resize_method(&base_ty, method, type_args, args, span, expected_return_ty)
        {
            return ret;
        }
        if let Some(ret) = self.check_integer_overflow_helper(&base_ty, method, type_args, args, &arg_types, span) {
            return ret;
        }
        // ---- Declared and builtin members ----
        if let Some(result) = self.check_trait_self_method_call(&call, &base_ty, &arg_types) {
            return result;
        }
        if let Some(result) = self.check_nominal_reflection_method_call(&call, &base_ty) {
            return result;
        }
        if let Some(result) = self.check_enum_method_call(
            &call,
            &base_ty,
            &arg_types,
            enum_variant_construction,
            enum_variant_parameters,
        ) {
            return result;
        }

        if let Some(result) = self.check_builtin_receiver_method_call(&call, &base_ty, &arg_types) {
            return result;
        }

        if let Some(ret) = self.clone_reference_value(&base_ty, method, type_args, args, span) {
            return ret;
        }
        if let Some(ret) =
            self.resolve_union_clone_trait_method_call(&base_ty, method, type_args, args, &arg_types, span)
        {
            return ret;
        }

        if let Some(result) = self.resolve_receiver_trait_method(&call, &base_ty, receiver_surface, &arg_types) {
            return result;
        }

        if let Some(result) = self.resolve_nominal_receiver_method(&call, &base_ty, receiver_surface, &arg_types) {
            return result;
        }

        self.check_method_fallback(&call, &base_ty, &arg_types)
    }
}
