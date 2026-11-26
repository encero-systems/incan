//! Checked C ABI string, span and output-slot constructors and span methods.

use super::*;

impl TypeChecker {
    /// Recognize `c.cstr(value)` as the explicit conversion from Incan text to temporary NUL-terminated storage.
    ///
    /// It returns a private compiler-known carrier rather than a raw pointer. The only admitted extraction is
    /// `as_const_ptr()` inside an `unsafe:` region, so the temporary's storage remains live through the raw call.
    pub(in crate::typechecker::check_expr) fn check_c_abi_c_string_constructor(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if member != "cstr" {
            return None;
        }
        let Expr::Ident(namespace) = &base.node else {
            return None;
        };
        if !self
            .import_binding_path(namespace)
            .is_some_and(|segments| c_abi::is_interop_namespace_path(segments.iter().map(String::as_str)))
        {
            return None;
        }
        if !type_args.is_empty() || args.len() != 1 || !matches!(args.first(), Some(CallArg::Positional(_))) {
            self.errors.push(CompileError::type_error(
                "c.cstr(value) requires exactly one positional str argument and no type arguments".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let actual = self
            .check_call_arg_types(args)
            .into_iter()
            .next()
            .unwrap_or(ResolvedType::Unknown);
        if !self.types_compatible(&actual, &ResolvedType::Str) {
            self.errors.push(CompileError::type_error(
                format!("c.cstr(value) requires str, found {actual}"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.type_info
            .c_abi
            .bridge_calls
            .push(crate::typechecker::type_info::CAbiBridgeCall {
                span,
                operation: incan_semantics_core::body_ir::CheckedBridgeOperation::MakeCString,
                slot_identity: None,
            });
        Some(ResolvedType::Generic(
            "Result".to_string(),
            vec![
                ResolvedType::Named(c_abi::C_STRING_TYPE_ID.to_string()),
                ResolvedType::Str,
            ],
        ))
    }

    /// Recognize compiler-owned typed carriers used for bounded C span calls.
    ///
    /// Each constructor moves one owned source allocation into a private carrier. The carrier has no ordinary pointer,
    /// indexing, return, or storage surface: only the methods checked below can reach a declared raw C call.
    pub(in crate::typechecker::check_expr) fn check_c_abi_span_constructor(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let (identity, kind, value_type, value_label) = match member {
            "bytes_span" => (
                c_abi::C_BYTES_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::U8,
                    mutable: false,
                },
                ResolvedType::Bytes,
                "bytes",
            ),
            "mutable_bytes_span" => (
                c_abi::C_MUTABLE_BYTES_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::U8,
                    mutable: true,
                },
                ResolvedType::Bytes,
                "bytes",
            ),
            "f32_span" => (
                c_abi::C_F32_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::F32,
                    mutable: false,
                },
                Self::c_abi_span_storage_type(c_abi::ScalarTypeId::F32),
                "list[f32]",
            ),
            "mutable_f32_span" => (
                c_abi::C_MUTABLE_F32_SPAN_TYPE_ID,
                CAbiSpanKind {
                    element: c_abi::ScalarTypeId::F32,
                    mutable: true,
                },
                Self::c_abi_span_storage_type(c_abi::ScalarTypeId::F32),
                "list[f32]",
            ),
            _ => return None,
        };
        let Expr::Ident(namespace) = &base.node else {
            return None;
        };
        if !self
            .import_binding_path(namespace)
            .is_some_and(|segments| c_abi::is_interop_namespace_path(segments.iter().map(String::as_str)))
        {
            return None;
        }
        let [CallArg::Positional(value)] = args else {
            self.errors.push(CompileError::type_error(
                format!(
                    "c.{member}(value) requires exactly one positional {value_label} argument and no type arguments"
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        };
        if !type_args.is_empty() {
            self.errors.push(CompileError::type_error(
                format!(
                    "c.{member}(value) requires exactly one positional {value_label} argument and no type arguments"
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let actual = self.check_expr_with_expected(value, Some(&value_type));
        if !self.types_compatible(&actual, &value_type) {
            self.errors.push(CompileError::type_error(
                format!("c.{member}(value) requires {value_label}, found {actual}"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.record_c_abi_span_operation(span, CheckedBridgeOperation::MakeSpan);
        self.unbound_c_abi_span_constructors.entry(span).or_insert(kind);
        Some(ResolvedType::Named(identity.to_string()))
    }

    /// Type-check the closed typed-span bridge surface without admitting a general pointer API.
    pub(in crate::typechecker::check_expr) fn check_c_abi_span_method(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let local_name = Self::c_checked_span_local_name(base)?;
        let local = self.c_abi_span_bindings.get(local_name).copied()?;
        if let Some(consumed_at) = self.consumed_c_abi_span_bindings.get(&local.binding_span).copied() {
            self.errors.push(CompileError::type_error(
                format!(
                    "checked mutable C span `{local_name}` was already consumed at byte range {}..{}; it cannot be used again",
                    consumed_at.start, consumed_at.end
                ),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        // The receiver has no value surface, so it is never checked as a value; Body IR still reads it as its local.
        self.record_c_abi_span_receiver(base, local_name);
        let mutable = local.kind.mutable;
        let immutable = !mutable;
        let (length_method, capacity_method, finish_method) = match local.kind.element {
            c_abi::ScalarTypeId::U8 => ("byte_length", "byte_capacity", "into_bytes"),
            c_abi::ScalarTypeId::F32 => ("element_count", "element_capacity", "into_f32s"),
            _ => return None,
        };
        let requires_unsafe = matches!(method, "as_const_ptr" | "as_mut_ptr")
            || method == length_method
            || method == capacity_method
            || method == finish_method;
        if requires_unsafe && self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                "checked C span bridge operations require an enclosing `unsafe:` acknowledgment".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let (expected_method, access, return_type) = match (immutable, mutable, method) {
            (true, false, "as_const_ptr") => (
                "as_const_ptr",
                CheckedBridgeOperation::SpanConstPointer,
                ResolvedType::Named(c_abi::pointer_type_identity(
                    false,
                    c_abi::scalar_type_as_str(local.kind.element),
                )),
            ),
            (true, false, _) if method == length_method => (
                length_method,
                CheckedBridgeOperation::SpanCount,
                ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::USize),
            ),
            (false, true, "as_mut_ptr") => {
                if let Some(local) = Self::c_checked_span_local_name(base)
                    && !self.mutable_bindings.contains(local)
                {
                    self.errors.push(errors::mutable_c_borrow_requires_mut(local, span));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                (
                    "as_mut_ptr",
                    CheckedBridgeOperation::SpanMutPointer,
                    ResolvedType::Named(c_abi::pointer_type_identity(
                        true,
                        c_abi::scalar_type_as_str(local.kind.element),
                    )),
                )
            }
            (false, true, _) if method == capacity_method => (
                capacity_method,
                CheckedBridgeOperation::SpanCount,
                ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::USize),
            ),
            (false, true, _) if method == finish_method => {
                return Some(self.check_c_abi_span_finish(
                    finish_method,
                    type_args,
                    args,
                    span,
                    local.binding_span,
                    local.kind.element,
                ));
            }
            _ => return None,
        };
        if method != expected_method || !type_args.is_empty() || !args.is_empty() {
            self.errors.push(CompileError::type_error(
                format!("a checked C span does not support `{method}` with type or value arguments"),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        self.record_c_abi_span_operation(span, access);
        Some(return_type)
    }

    /// Validate the written element count before consuming a checked mutable C span's binding.
    ///
    /// Argument shapes or count types refused here leave the binding unconsumed. A checked finish retains the
    /// operation and returns the corresponding owned storage through the existing fallible result carrier.
    pub(super) fn check_c_abi_span_finish(
        &mut self,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
        binding_span: Span,
        element: c_abi::ScalarTypeId,
    ) -> ResolvedType {
        if !type_args.is_empty() || args.len() != 1 || !matches!(args.first(), Some(CallArg::Positional(_))) {
            self.errors.push(CompileError::type_error(
                format!("a checked mutable C span requires {method}(written) with one positional usize"),
                span,
            ));
            self.check_call_args(args);
            return ResolvedType::Unknown;
        }
        let actual = self
            .check_call_arg_types(args)
            .into_iter()
            .next()
            .unwrap_or(ResolvedType::Unknown);
        let count_type = ResolvedType::Numeric(incan_lang::lang::types::numerics::NumericTypeId::USize);
        if !self.types_compatible(&actual, &count_type)
            && !Self::checked_c_integer_bridge_compatible(&actual, &count_type)
        {
            self.errors.push(CompileError::type_error(
                format!("{method}(written) requires an integer count, found {actual}"),
                span,
            ));
            return ResolvedType::Unknown;
        }
        self.record_c_abi_span_operation(span, CheckedBridgeOperation::FinishSpan);
        self.consumed_c_abi_span_bindings.insert(binding_span, span);
        ResolvedType::Generic(
            "Result".to_string(),
            vec![Self::c_abi_span_storage_type(element), ResolvedType::Str],
        )
    }

    /// Retain one authorized span bridge operation at its call exactly once, so Body IR lowers the checked operation
    /// instead of rediscovering it from the method spelling.
    pub(super) fn record_c_abi_span_operation(&mut self, span: Span, operation: CheckedBridgeOperation) {
        let bridge_calls = &mut self.type_info.c_abi.bridge_calls;
        if bridge_calls.iter().any(|existing| existing.span == span) {
            return;
        }
        bridge_calls.push(CAbiBridgeCall {
            slot_identity: None,
            span,
            operation,
        });
    }

    /// Record at a span method's receiver what an ordinary read of the span local records, its binding identity and
    /// carrier type, without the value-surface refusal: a method receiver is the one read a span admits.
    pub(super) fn record_c_abi_span_receiver(&mut self, base: &Spanned<Expr>, name: &str) {
        let receiver = match &base.node {
            Expr::Paren(inner) => return self.record_c_abi_span_receiver(inner, name),
            Expr::Ident(_) => base.span,
            _ => return,
        };
        let Some(symbol) = self.symbols.lookup(name) else {
            return;
        };
        if let Some(identity) = self.symbols.identity_of(symbol).cloned() {
            self.type_info.record_resolved_identity(receiver, identity);
        }
        let ty = match self.symbols.get(symbol).map(|symbol| &symbol.kind) {
            Some(SymbolKind::Variable(info)) => info.ty.clone(),
            _ => return,
        };
        self.record_expr_type(receiver, ty);
    }

    /// Return the ordinary source storage moved into one exact checked C span representation.
    pub(super) fn c_abi_span_storage_type(element: c_abi::ScalarTypeId) -> ResolvedType {
        match element {
            c_abi::ScalarTypeId::U8 => ResolvedType::Bytes,
            c_abi::ScalarTypeId::F32 => ResolvedType::Generic(
                "List".to_string(),
                vec![ResolvedType::Numeric(
                    incan_lang::lang::types::numerics::NumericTypeId::F32,
                )],
            ),
            _ => ResolvedType::Unknown,
        }
    }

    /// Type-check the sole raw extraction admitted for a validated temporary C string.
    pub(in crate::typechecker::check_expr) fn check_c_abi_c_string_pointer(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if method != "as_const_ptr"
            || !matches!(self.check_expr(base), ResolvedType::Named(identity) if identity == c_abi::C_STRING_TYPE_ID)
        {
            return None;
        }
        if !type_args.is_empty() || !args.is_empty() {
            self.errors.push(CompileError::type_error(
                "a checked C string pointer takes no type or value arguments".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                "extracting a checked C string pointer requires an enclosing `unsafe:` acknowledgment".to_string(),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        if !self.c_abi_raw_argument_spans.contains(&span) {
            self.errors.push(CompileError::type_error(
                "a checked C string pointer may only be extracted as a direct checked raw-call argument".to_string(),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.type_info
            .c_abi
            .bridge_calls
            .push(crate::typechecker::type_info::CAbiBridgeCall {
                span,
                operation: incan_semantics_core::body_ir::CheckedBridgeOperation::CStringPointer,
                slot_identity: None,
            });
        Some(ResolvedType::Named(c_abi::pointer_type_identity(false, "c.c_char")))
    }

    /// Type-check the only owning conversion admitted for one returned scoped C string view.
    ///
    /// A caller must name a positive upper bound because a foreign terminator scan has no safe unbounded form. The
    /// helper validates UTF-8 and copies before the raw view can escape the unsafe bridge.
    pub(in crate::typechecker::check_expr) fn check_c_abi_scoped_c_string_copy(
        &mut self,
        base: &Spanned<Expr>,
        method: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        if method != "copy_utf8" {
            return None;
        }
        // The receiver of `copy_utf8` is the one position a view may be read in (RFC 116).
        let mut receiver = base;
        while let Expr::Paren(inner) = &receiver.node {
            receiver = inner;
        }
        let receiver_key = receiver.span;
        self.scoped_c_string_view_receivers.insert(receiver_key);
        let base_ty = self.check_expr(base);
        self.scoped_c_string_view_receivers.remove(&receiver_key);
        if !matches!(base_ty, ResolvedType::Named(identity) if identity == c_abi::SCOPED_C_STRING_VIEW_TYPE_ID) {
            return None;
        }
        self.unbound_scoped_c_string_views.remove(&receiver_key);
        let [CallArg::Named(name, _)] = args else {
            self.errors.push(CompileError::type_error(
                "a scoped C string view requires copy_utf8(max_bytes=<positive int>)".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        };
        if !type_args.is_empty() || name.node != "max_bytes" {
            self.errors.push(CompileError::type_error(
                "a scoped C string view requires copy_utf8(max_bytes=<positive int>)".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        if self.unsafe_depth == 0 {
            self.errors.push(CompileError::type_error(
                "copying a scoped C string view requires an enclosing `unsafe:` acknowledgment".to_string(),
                span,
            ));
            self.check_call_args(args);
            return Some(ResolvedType::Unknown);
        }
        let actual = self
            .check_call_arg_types(args)
            .into_iter()
            .next()
            .unwrap_or(ResolvedType::Unknown);
        if !self.types_compatible(&actual, &ResolvedType::Int) {
            self.errors.push(CompileError::type_error(
                format!("copy_utf8(max_bytes=...) requires int, found {actual}"),
                span,
            ));
            return Some(ResolvedType::Unknown);
        }
        self.type_info
            .c_abi
            .bridge_calls
            .push(crate::typechecker::type_info::CAbiBridgeCall {
                span,
                operation: incan_semantics_core::body_ir::CheckedBridgeOperation::CopyUtf8,
                slot_identity: None,
            });
        Some(ResolvedType::Generic(
            "Result".to_string(),
            vec![ResolvedType::Str, ResolvedType::Str],
        ))
    }

    /// Recognize ordinary `c.out[...]()` and `c.inout(...)` calls without adding parser grammar.
    ///
    /// The local name is supplied by the enclosing assignment, after which a checked raw symbol binds the temporary
    /// handle to one exact output parameter. The temporary itself has no user-visible runtime representation.
    pub(in crate::typechecker::check_expr) fn check_c_abi_output_slot_constructor(
        &mut self,
        base: &Spanned<Expr>,
        member: &str,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
        span: Span,
    ) -> Option<ResolvedType> {
        let Expr::Ident(namespace) = &base.node else {
            return None;
        };
        if !self
            .import_binding_path(namespace)
            .is_some_and(|segments| c_abi::is_interop_namespace_path(segments.iter().map(String::as_str)))
        {
            return None;
        }

        let slot = match member {
            "out" => {
                if type_args.len() != 1 || !args.is_empty() {
                    self.errors.push(CompileError::type_error(
                        "c.out[...] requires exactly one C value type and no value arguments".to_string(),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                PendingCAbiOutputSlot {
                    closure_depth: self.closure_depth,
                    constructor_span: span,
                    mode: COutputMode::Out,
                    declared_type: Some(type_args[0].node.to_string()),
                    initial_type: None,
                    bound: false,
                }
            }
            "inout" => {
                if !type_args.is_empty() || args.len() != 1 {
                    self.errors.push(CompileError::type_error(
                        "c.inout(value) requires exactly one ordinary value argument and no type arguments".to_string(),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                }
                let CallArg::Positional(value) = &args[0] else {
                    self.errors.push(CompileError::type_error(
                        "c.inout(value) accepts one positional value argument".to_string(),
                        span,
                    ));
                    self.check_call_args(args);
                    return Some(ResolvedType::Unknown);
                };
                PendingCAbiOutputSlot {
                    closure_depth: self.closure_depth,
                    constructor_span: span,
                    mode: COutputMode::InOut,
                    declared_type: None,
                    initial_type: Some(self.check_expr(value)),
                    bound: false,
                }
            }
            _ => return None,
        };

        self.unbound_c_abi_output_slot_constructors.insert(span, slot);
        Some(ResolvedType::Unknown)
    }
}
