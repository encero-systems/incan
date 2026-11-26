//! Raw C binding symbol calls: signatures, span and output-slot arguments, and resource arguments.

use super::*;

impl TypeChecker {
    /// Resolve an ordinary `Binding.symbol(...)` expression through a checked C binding descriptor.
    ///
    /// This is deliberately an expression-level semantic hook: parser and class syntax remain ordinary Incan. The
    /// executable subset admits scalar and resource contracts, compiler-managed output positions, and a checked
    /// C string temporary for an exact `const char *` parameter. General pointers and structures remain
    /// declaration-checked until later slices provide their own bounded runtime carriers.
    pub(in crate::typechecker::check_expr) fn check_c_binding_symbol_member_call(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let Expr::Ident(binding_name) = &base.node else {
            return None;
        };
        let descriptor = self.type_info.c_abi.bindings.get(binding_name)?.clone();
        let binding = descriptor.class_name.clone();
        let Some(symbol) = descriptor.symbols.iter().find(|symbol| symbol.name == member).cloned() else {
            self.errors.push(CompileError::type_error(
                format!("C binding `{binding}` does not declare symbol `{member}`"),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        };

        let callable = format!("{binding}.{member}");
        if self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                format!("C binding symbol `{callable}` requires an enclosing `unsafe:` acknowledgment"),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if !type_args.is_empty() {
            self.errors
                .push(errors::explicit_call_site_type_args_not_supported(span));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if !Self::c_raw_call_signature_is_currently_supported(&symbol) {
            self.errors.push(CompileError::type_error(
                format!(
                    "C binding symbol `{callable}` requires native ownership or ABI emission that is not implemented yet"
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }

        let parameters = Self::c_raw_call_parameters(&binding, &symbol);
        let raw_argument_spans = args
            .iter()
            .filter_map(|argument| match argument {
                CallArg::Positional(value) | CallArg::Named(_, value) => Some(value.span),
                _ => None,
            })
            .collect::<Vec<_>>();
        self.c_abi_raw_argument_spans.extend(raw_argument_spans.iter().copied());
        let arg_types = self.check_call_arg_types_for_params(args, &parameters);
        for argument in raw_argument_spans {
            self.c_abi_raw_argument_spans.remove(&argument);
        }
        let mut type_bindings = std::collections::HashMap::new();
        self.validate_checked_c_callable_arg_bindings(
            &callable,
            &parameters,
            args,
            &arg_types,
            &mut type_bindings,
            span,
        );
        if !self.validate_c_raw_span_arguments(&symbol, args, span) {
            return Some(ResolvedType::Unknown);
        }
        let output_slots = self.validate_c_raw_output_slots(&binding, &symbol, &descriptor.resources, args);
        self.validate_c_raw_mutable_resource_borrows(&symbol, args);
        self.record_c_raw_owned_resource_transfers(&symbol, args);
        self.type_info.record_call_site_callable_params(span, &parameters);
        let return_type = Self::c_raw_call_return_type(&binding, &symbol.return_type);
        if matches!(&return_type, ResolvedType::Named(identity) if identity == c_abi::SCOPED_C_STRING_VIEW_TYPE_ID) {
            // The view may only be bound to a local or copied at once; anything else lets it escape (RFC 116).
            self.unbound_scoped_c_string_views.insert(span);
        }
        self.type_info.c_abi.raw_calls.push(CBindingRawCall {
            span,
            owner: self.current_c_abi_raw_call_owner.clone(),
            binding: binding.clone(),
            symbol: member.to_string(),
        });
        if let Some(owner) = self.current_c_abi_raw_call_owner.as_ref()
            && owner.visibility == crate::ast::Visibility::Public
            && self
                .warned_public_c_abi_raw_call_owners
                .insert((owner.name.clone(), owner.declaration_span))
        {
            self.warnings
                .push(errors::public_checked_c_call_requires_private_bridge(
                    &owner.name,
                    owner.declaration_span,
                ));
        }
        if !output_slots.is_empty() {
            self.unbound_c_abi_raw_call_results.insert(
                span,
                CAbiRawCallResult {
                    binding,
                    symbol: member.to_string(),
                    local_name: None,
                    local_symbol_span: None,
                    slots_by_parameter: output_slots,
                },
            );
        }
        Some(return_type)
    }

    /// Project the descriptor's declared parameter order and raw C types into the shared argument binder's surface.
    pub(super) fn c_raw_call_parameters(binding: &str, symbol: &CBindingSymbol) -> Vec<CallableParam> {
        symbol
            .parameters
            .iter()
            .map(|parameter| {
                CallableParam::named(
                    parameter.name.clone(),
                    Self::c_raw_call_type(binding, &parameter.ty),
                    ParamKind::Normal,
                )
            })
            .collect()
    }

    /// Return whether the contained direct-call bridge can preserve every argument and result contract faithfully.
    pub(super) fn c_raw_call_signature_is_currently_supported(symbol: &CBindingSymbol) -> bool {
        symbol.parameters.iter().all(|parameter| {
            matches!(parameter.ty, CBindingType::Scalar(_) | CBindingType::Resource { .. })
                || matches!(
                    &parameter.ty,
                    CBindingType::Pointer {
                        mutable: false,
                        pointee,
                    } if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::CChar))
                )
                || matches!(
                    &parameter.ty,
                    CBindingType::Pointer { pointee, .. }
                        if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::U8 | c_abi::ScalarTypeId::F32))
                            && symbol.buffers.iter().any(|buffer| {
                                buffer.pointer_parameter == parameter.name
                                    && matches!(pointee.as_ref(), CBindingType::Scalar(element) if *element == buffer.element)
                            })
                )
                || matches!(
                    &parameter.ty,
                    CBindingType::Output { value, .. }
                        if matches!(value.as_ref(), CBindingType::Scalar(_)
                            | CBindingType::Resource { access: CResourceAccess::Owned, .. })
                )
        }) && (matches!(
            &symbol.return_type,
            CBindingType::Scalar(_)
                | CBindingType::Void
                | CBindingType::Resource {
                    access: CResourceAccess::Owned,
                    ..
                }
        ) || matches!(
            &symbol.return_type,
            CBindingType::Pointer {
                mutable: false,
                pointee,
            } if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::CChar))
        ) || matches!(
            &symbol.return_type,
            CBindingType::Nullable(value)
                if matches!(value.as_ref(), CBindingType::Resource {
                    access: CResourceAccess::Owned,
                    ..
                })
        ))
    }

    /// Require every declared typed pointer to be passed with the exact count view from the same opaque span.
    ///
    /// Nominal pointer types alone cannot encode this relation: two independent spans would otherwise have the same
    /// pointer type. The checked declaration owns the parameter pairing and this source-level check preserves it
    /// before lowering sees raw arguments.
    pub(super) fn validate_c_raw_span_arguments(
        &mut self,
        symbol: &CBindingSymbol,
        args: &[CallArg],
        span: Span,
    ) -> bool {
        for buffer in &symbol.buffers {
            let Some(pointer_parameter) = symbol
                .parameters
                .iter()
                .find(|parameter| parameter.name == buffer.pointer_parameter)
            else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` has a missing checked span-pointer parameter `{}`",
                        symbol.name, buffer.pointer_parameter
                    ),
                    span,
                ));
                return false;
            };
            let CBindingType::Pointer { mutable, ref pointee } = pointer_parameter.ty else {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` has an invalid checked span-pointer parameter `{}`",
                        symbol.name, buffer.pointer_parameter
                    ),
                    span,
                ));
                return false;
            };
            if !matches!(pointee.as_ref(), CBindingType::Scalar(element) if *element == buffer.element) {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` has a checked span-pointer `{}` whose element contract drifted",
                        symbol.name, buffer.pointer_parameter
                    ),
                    span,
                ));
                return false;
            }
            let (Some(pointer), Some(length)) = (
                Self::c_raw_call_argument(symbol, args, &buffer.pointer_parameter),
                Self::c_raw_call_argument(symbol, args, &buffer.length_parameter),
            ) else {
                // Ordinary argument binding reports omitted or duplicated parameters. Do not replace that diagnostic.
                continue;
            };
            if !Self::c_checked_span_pair_matches(pointer, length, mutable, buffer.element) {
                let (pointer_method, length_method, span_kind) = match (mutable, buffer.element) {
                    (true, c_abi::ScalarTypeId::U8) => {
                        ("as_mut_ptr", "byte_capacity", "mutable caller-owned byte buffer")
                    }
                    (false, c_abi::ScalarTypeId::U8) => ("as_const_ptr", "byte_length", "immutable byte span"),
                    (true, c_abi::ScalarTypeId::F32) => {
                        ("as_mut_ptr", "element_capacity", "mutable caller-owned f32 span")
                    }
                    (false, c_abi::ScalarTypeId::F32) => ("as_const_ptr", "element_count", "immutable f32 span"),
                    _ => ("as_const_ptr", "element_count", "checked C span"),
                };
                self.errors.push(CompileError::type_error(
                    format!(
                        "C binding symbol `{}` requires `{}` and `{}` to come from the same checked {} via `{}()` and `{}()`",
                        symbol.name,
                        buffer.pointer_parameter,
                        buffer.length_parameter,
                        span_kind,
                        pointer_method,
                        length_method,
                    ),
                    span,
                ));
                return false;
            }
        }
        true
    }

    /// Resolve one C parameter to its bound positional or named source argument without accepting unpacking.
    pub(super) fn c_raw_call_argument<'a>(
        symbol: &CBindingSymbol,
        args: &'a [CallArg],
        parameter_name: &str,
    ) -> Option<&'a Spanned<Expr>> {
        if let Some(value) = args.iter().find_map(|argument| match argument {
            CallArg::Named(name, value) if name.node == parameter_name => Some(value),
            _ => None,
        }) {
            return Some(value);
        }
        let position = symbol
            .parameters
            .iter()
            .position(|parameter| parameter.name == parameter_name)?;
        args.iter()
            .filter_map(|argument| match argument {
                CallArg::Positional(value) => Some(value),
                CallArg::Named(_, _) | CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => None,
            })
            .nth(position)
    }

    /// Recognize the only source spelling that may provide one raw typed pointer and its paired checked bound.
    pub(super) fn c_checked_span_pair_matches(
        pointer: &Spanned<Expr>,
        length: &Spanned<Expr>,
        mutable: bool,
        element: c_abi::ScalarTypeId,
    ) -> bool {
        let (pointer_receiver, pointer_method, pointer_type_args, pointer_args) = match &pointer.node {
            Expr::MethodCall(receiver, method, type_args, arguments) => (
                receiver.as_ref(),
                method.as_str(),
                type_args.as_slice(),
                arguments.as_slice(),
            ),
            _ => return false,
        };
        let (length_receiver, length_method, length_type_args, length_args) = match &length.node {
            Expr::MethodCall(receiver, method, type_args, arguments) => (
                receiver.as_ref(),
                method.as_str(),
                type_args.as_slice(),
                arguments.as_slice(),
            ),
            _ => return false,
        };
        let (expected_pointer_method, expected_length_method) = match (mutable, element) {
            (true, c_abi::ScalarTypeId::U8) => ("as_mut_ptr", "byte_capacity"),
            (false, c_abi::ScalarTypeId::U8) => ("as_const_ptr", "byte_length"),
            (true, c_abi::ScalarTypeId::F32) => ("as_mut_ptr", "element_capacity"),
            (false, c_abi::ScalarTypeId::F32) => ("as_const_ptr", "element_count"),
            _ => return false,
        };
        pointer_method == expected_pointer_method
            && length_method == expected_length_method
            && pointer_type_args.is_empty()
            && length_type_args.is_empty()
            && pointer_args.is_empty()
            && length_args.is_empty()
            && Self::c_checked_span_local_name(pointer_receiver)
                .zip(Self::c_checked_span_local_name(length_receiver))
                .is_some_and(|(pointer_local, length_local)| pointer_local == length_local)
    }

    /// Return the local owner of one transparent checked-span receiver expression.
    pub(super) fn c_checked_span_local_name(expr: &Spanned<Expr>) -> Option<&str> {
        match &expr.node {
            Expr::Ident(name) => Some(name),
            Expr::Paren(inner) => Self::c_checked_span_local_name(inner),
            _ => None,
        }
    }

    /// Map the contained C call surface to the semantic carrier used for argument checking and local values.
    pub(in crate::typechecker::check_expr) fn c_raw_call_type(binding: &str, ty: &CBindingType) -> ResolvedType {
        match ty {
            CBindingType::Scalar(scalar) => c_abi::scalar_numeric_type(*scalar)
                .map(ResolvedType::from_numeric_id)
                .unwrap_or(ResolvedType::Int),
            CBindingType::Void => ResolvedType::Unit,
            CBindingType::Resource { resource, .. } => {
                ResolvedType::Named(Self::c_resource_type_identity(binding, resource))
            }
            CBindingType::Nullable(value) => {
                ResolvedType::Generic("Option".to_string(), vec![Self::c_raw_call_type(binding, value)])
            }
            CBindingType::Pointer { mutable, pointee } => Self::c_pointer_type_identity(*mutable, pointee)
                .map(ResolvedType::Named)
                .unwrap_or(ResolvedType::Unknown),
            CBindingType::Struct(_) | CBindingType::Output { .. } => ResolvedType::Unknown,
        }
    }

    /// Map a raw C result to its source-visible checked carrier.
    ///
    /// Returned text pointers are scoped views, not reusable raw input pointers. The distinction keeps them out of
    /// arbitrary later C calls and admits only the bounded owning conversion supplied below.
    pub(super) fn c_raw_call_return_type(binding: &str, ty: &CBindingType) -> ResolvedType {
        match ty {
            CBindingType::Pointer {
                mutable: false,
                pointee,
            } if matches!(pointee.as_ref(), CBindingType::Scalar(c_abi::ScalarTypeId::CChar)) => {
                ResolvedType::Named(c_abi::SCOPED_C_STRING_VIEW_TYPE_ID.to_string())
            }
            _ => Self::c_raw_call_type(binding, ty),
        }
    }

    /// Return one compiler-internal nominal identity for a resource scoped by its binding declaration.
    pub(super) fn c_resource_type_identity(binding: &str, resource: &str) -> String {
        format!("__incan_c_resource::{binding}::{resource}")
    }

    /// Return the compiler-owned nominal identity for a pointer supported by the direct checked-C bridge.
    pub(super) fn c_pointer_type_identity(mutable: bool, pointee: &CBindingType) -> Option<String> {
        let CBindingType::Scalar(scalar) = pointee else {
            return None;
        };
        Some(c_abi::pointer_type_identity(
            mutable,
            c_abi::scalar_type_as_str(*scalar),
        ))
    }

    /// Bind ordinary local output handles to exact checked C output parameters. Retain the closed constructor type
    /// for inferred Body IR bindings too: the assignment was checked before this raw call supplied its slot contract.
    pub(super) fn validate_c_raw_output_slots(
        &mut self,
        binding: &str,
        symbol: &CBindingSymbol,
        resources: &[crate::typechecker::CBindingResource],
        args: &[CallArg],
    ) -> std::collections::HashMap<String, String> {
        let resource_names = resources
            .iter()
            .map(|resource| resource.name.clone())
            .collect::<HashSet<_>>();
        let struct_names = HashSet::new();
        let mut next_positional = 0usize;
        let mut slots_by_parameter = std::collections::HashMap::new();
        let mut seen_slots = HashSet::new();

        for arg in args {
            let (parameter, expr) = match arg {
                CallArg::Positional(expr) => {
                    let parameter = symbol.parameters.get(next_positional);
                    next_positional += 1;
                    (parameter, expr)
                }
                CallArg::Named(name, expr) => (
                    symbol.parameters.iter().find(|parameter| parameter.name == name.node),
                    expr,
                ),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => continue,
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let CBindingType::Output { mode, value } = &parameter.ty else {
                continue;
            };
            let Some((local_name, slot)) =
                self.check_pending_c_raw_output_slot(binding, &symbol.name, &parameter.name, expr, *mode)
            else {
                continue;
            };
            if !seen_slots.insert(local_name.to_owned()) {
                self.errors.push(CompileError::type_error(
                    format!("C output slot `{local_name}` may be passed to only one output parameter per call"),
                    expr.span,
                ));
                continue;
            }

            let value_is_valid = match mode {
                COutputMode::Out => {
                    slot.declared_type
                        .as_deref()
                        .and_then(|source| Self::c_binding_type(source, &resource_names, &struct_names, false))
                        == Some((**value).clone())
                }
                COutputMode::InOut => slot
                    .initial_type
                    .as_ref()
                    .is_some_and(|initial| self.types_compatible(initial, &Self::c_raw_call_type(binding, value))),
            };
            if !value_is_valid {
                self.errors.push(CompileError::type_error(
                    format!(
                        "C output slot `{local_name}` does not match `{binding}.{}.{}`'s checked value contract",
                        symbol.name, parameter.name
                    ),
                    expr.span,
                ));
                continue;
            }

            let slot_identity = c_abi::output_slot_type_identity(
                binding,
                &symbol.name,
                &parameter.name,
                slot.constructor_span.start,
                slot.constructor_span.end,
            );
            self.record_expr_type(slot.constructor_span, ResolvedType::Named(slot_identity.clone()));
            self.type_info
                .expressions
                .body_ir_expr_types
                .insert(slot.constructor_span, ResolvedType::Named(slot_identity.clone()));
            self.record_expr_type(expr.span, ResolvedType::Named(slot_identity.clone()));
            if let Some(symbol_id) = self.symbols.lookup(local_name)
                && let Some(symbol) = self.symbols.get_mut(symbol_id)
                && let SymbolKind::Variable(variable) = &mut symbol.kind
            {
                variable.ty = ResolvedType::Named(slot_identity.clone());
            }
            if let Some(pending) = self.pending_c_abi_output_slots.get_mut(local_name) {
                pending.bound = true;
            }
            if matches!(mode, COutputMode::InOut)
                && symbol
                    .outcomes
                    .iter()
                    .all(|outcome| !outcome.invalidates.contains(&parameter.name))
            {
                self.available_c_abi_output_slots.insert(slot_identity.clone());
            }
            self.type_info.c_abi.output_slots.push(CAbiOutputSlot {
                closure_depth: slot.closure_depth,
                constructor_span: slot.constructor_span,
                identity: slot_identity.clone(),
                local_name: local_name.to_owned(),
                binding: binding.to_string(),
                symbol: symbol.name.clone(),
                parameter: parameter.name.clone(),
                mode: *mode,
                value: (**value).clone(),
            });
            slots_by_parameter.insert(parameter.name.clone(), slot_identity);
        }
        slots_by_parameter
    }

    /// Require an unbound local slot with the parameter's mode before duplicate and value-contract checks.
    pub(super) fn check_pending_c_raw_output_slot<'a>(
        &mut self,
        binding: &str,
        symbol: &str,
        parameter: &str,
        expr: &'a Spanned<Expr>,
        mode: COutputMode,
    ) -> Option<(&'a str, PendingCAbiOutputSlot)> {
        let Expr::Ident(local_name) = &expr.node else {
            self.errors.push(CompileError::type_error(
                format!("C output parameter `{binding}.{symbol}.{parameter}` requires a local c.out[...]() or c.inout(...) slot"),
                expr.span,
            ));
            return None;
        };
        let Some(slot) = self.pending_c_abi_output_slots.get(local_name).cloned() else {
            self.errors.push(CompileError::type_error(
                format!("C output parameter `{binding}.{symbol}.{parameter}` requires local `{local_name}` to be initialized by c.out[...]() or c.inout(...)"),
                expr.span,
            ));
            return None;
        };
        if slot.bound {
            self.errors.push(CompileError::type_error(
                format!("C output slot `{local_name}` is already bound to an earlier raw call"),
                expr.span,
            ));
            return None;
        }
        if slot.mode != mode {
            self.errors.push(CompileError::type_error(
                format!(
                    "C output parameter `{binding}.{symbol}.{parameter}` requires {}, but `{local_name}` was created as {}",
                    if matches!(mode, COutputMode::Out) { "c.out[...]()" } else { "c.inout(...)" },
                    if matches!(slot.mode, COutputMode::Out) { "c.out[...]()" } else { "c.inout(...)" },
                ),
                expr.span,
            ));
            return None;
        }
        Some((local_name, slot))
    }

    /// Record local handle transfers for direct raw C calls.
    pub(super) fn record_c_raw_owned_resource_transfers(&mut self, symbol: &CBindingSymbol, args: &[CallArg]) {
        self.for_each_c_raw_resource_argument(symbol, args, CResourceAccess::Owned, |this, name, span| {
            this.transferred_c_resource_bindings.insert(name.to_string(), span);
        });
    }

    /// Reject an immutable local binding when a direct C call requires exclusive access to its resource wrapper.
    pub(super) fn validate_c_raw_mutable_resource_borrows(&mut self, symbol: &CBindingSymbol, args: &[CallArg]) {
        self.for_each_c_raw_resource_argument(symbol, args, CResourceAccess::BorrowedMut, |this, name, span| {
            if !this.mutable_bindings.contains(name) {
                this.errors.push(errors::mutable_c_borrow_requires_mut(name, span));
            }
        });
    }

    /// Apply one ownership-mode check to each direct local resource argument in a fixed C call signature.
    pub(super) fn for_each_c_raw_resource_argument(
        &mut self,
        symbol: &CBindingSymbol,
        args: &[CallArg],
        expected_access: CResourceAccess,
        mut apply: impl FnMut(&mut Self, &str, Span),
    ) {
        let mut next_positional = 0usize;
        for arg in args {
            let (parameter, expr) = match arg {
                CallArg::Positional(expr) => {
                    let parameter = symbol.parameters.get(next_positional);
                    next_positional += 1;
                    (parameter, expr)
                }
                CallArg::Named(name, expr) => (
                    symbol.parameters.iter().find(|parameter| parameter.name == name.node),
                    expr,
                ),
                CallArg::PositionalUnpack(_) | CallArg::KeywordUnpack(_) => continue,
            };
            let Some(CBindingType::Resource { access, .. }) = parameter.map(|parameter| &parameter.ty) else {
                continue;
            };
            if *access != expected_access {
                continue;
            }
            let Some((name, resource_span)) = Self::c_raw_resource_ident(expr) else {
                continue;
            };
            apply(self, name, resource_span);
        }
    }

    /// Return the source binding named by a transparent parenthesized resource expression.
    pub(super) fn c_raw_resource_ident(expr: &Spanned<Expr>) -> Option<(&str, Span)> {
        match &expr.node {
            Expr::Ident(name) => Some((name, expr.span)),
            Expr::Paren(inner) => Self::c_raw_resource_ident(inner),
            _ => None,
        }
    }
}
