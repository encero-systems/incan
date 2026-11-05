//! Checked builtin method, length, list repetition and module-qualified calls.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Retain registry-owned integer helpers in Body IR only.
    ///
    /// A source method already has its checked identity and never reaches this fallback. Numeric receivers are
    /// admitted through the language registry. Generic Clone requires the checker's retained call-site identity and
    /// must remain absent when that fact is cleared during synthetic resolution.
    pub(super) fn direct_builtin_method_identity(
        &self,
        receiver: &IncanType,
        method: &str,
    ) -> Option<CanonicalSymbolId> {
        use incan_lang::lang::types::numerics;
        let numeric = match receiver {
            IncanType::Primitive(IncanPrimitiveType::Int) => Some(numerics::NumericTypeId::I64),
            IncanType::Primitive(IncanPrimitiveType::Numeric(kind)) => Some(*kind),
            _ => None,
        };
        if let Some(numeric) = numeric {
            let helper = numerics::integer_helper_from_str(method)?;
            if numerics::supports_integer_helper(numeric, helper) {
                return Some(crate::symbols::canonical_builtin_member_identity(
                    numerics::as_str(numeric),
                    helper.canonical,
                ));
            }
        }
        None
    }

    /// Lower a method-shaped explicit builtin call from the typechecker's retained builtin selection.
    ///
    /// `std.builtins.len(value)` reaches the AST as a method call, but the namespace is not a runtime receiver. The
    /// typechecker records the selected [`BuiltinFnId`] at the full call span. Consuming that closed fact here keeps
    /// the call distinct from a user-defined function or method with the same spelling and avoids evaluating the
    /// namespace as a value.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_checked_builtin_method_call(
        &mut self,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if let Some(value) = self.lower_checked_length_call(args, span, scope, out) {
            return Some(value);
        }
        let builtin = self.type_info.resolved_builtin_call(span)?;
        if builtin == BuiltinFnId::IsInstance {
            return None;
        }
        let call_span = hir_span(span);
        let Some(canonical) = crate::symbols::canonical_builtin_function_identity(builtin) else {
            return Some(self.unsupported_operand(
                "checked builtin call has no canonical registry identity".to_string(),
                scope,
                call_span,
                out,
            ));
        };
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return Some(self.unsupported_operand(description, scope, call_span, out));
            }
        };
        let declared = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let display_name = canonical.declaration_name.clone();
        let legacy_enumerate_argument = legacy_enumerate_argument(Some(builtin), args);
        let args = legacy_enumerate_argument.as_deref().unwrap_or(args);
        let (operands, binding) =
            match self.bind_declared_args(&format!("builtin `{display_name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return Some(self.unsupported_operand(description, scope, call_span, out));
                }
            };
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: display_name,
                direct_call_id: None,
                canonical: Some(canonical),
                builtin: Some(builtin),
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            call_span,
            false,
            out,
        ))
    }

    /// Preserve the checker's selected length hook as an ordinary method call, evaluating its receiver once.
    ///
    /// Builtin lists and strings have no user operator selection and retain their builtin calls. An imported model's
    /// hook keeps the checked canonical method identity so the SDK call collector can retain its signature. Native
    /// methods retain their exact return carrier before the ordinary explicit `int` conversion supplies len's result.
    pub(super) fn lower_checked_length_call(
        &mut self,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if self.type_info.resolved_builtin_call(span) != Some(BuiltinFnId::Len) {
            return None;
        }
        if self.type_info.rust.checked_calls.contains_key(&span) {
            let [ast::CallArg::Positional(receiver) | ast::CallArg::Named(_, receiver)] = args else {
                return None;
            };
            let value = self.lower_checked_rust_call(Some(receiver), &[], span, scope, out)?;
            return Some(self.push_call_temp(
                bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                    name: "int".into(),
                    direct_call_id: None,
                    builtin: Some(BuiltinFnId::Int),
                    type_args: Vec::new(),
                    binding: bir::ArgumentBinding::resolved_positional(1),
                    canonical: crate::symbols::canonical_builtin_function_identity(BuiltinFnId::Int),
                    receiver_type: None,
                })),
                vec![bir::ArgumentElement::One(value)],
                IncanType::Primitive(IncanPrimitiveType::Int),
                scope,
                hir_span(span),
                true,
                out,
            ));
        }
        let dispatch = self.type_info.resolved_operator_call(span)?;
        if dispatch.kind != ResolvedOperatorKind::Len {
            return None;
        }
        let method = dispatch.method.clone();
        let [ast::CallArg::Positional(receiver) | ast::CallArg::Named(_, receiver)] = args else {
            return None;
        };
        Some(self.lower_operator_dispatch(&method, receiver, &[], self.resolve_ty(span), span, scope, out))
    }

    /// Lower `list.repeat(value, count)` as the [`bir::HelperOp::ListRepeat`] call the typechecker resolved.
    ///
    /// The receiver is the builtin `list` type surface rather than a value, so nothing is read from it. The checker
    /// records the helper's member identity on the call and marks the receiver a type name, and both are required:
    /// a local binding that shadows `list` keeps its ordinary method call. Arguments bind to the helper's `value` and
    /// `count` slots by position or name, are evaluated in written order, and are passed in slot order.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_list_repeat_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let helper_id = BuiltinCollectionHelperId::ListRepeat;
        let ast::Expr::Ident(receiver) = &recv.node else {
            return None;
        };
        if receiver != collection_helpers::receiver(helper_id)
            || name != collection_helpers::member(helper_id)
            || self.bindings.contains_key(receiver)
            || !matches!(self.type_info.ident_kind(recv.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let identity = crate::symbols::canonical_builtin_member_identity(
            collections::as_str(CollectionTypeId::List),
            collection_helpers::member(helper_id),
        );
        if self.type_info.resolved_identity(span) != Some(&identity) {
            return None;
        }
        let call_span = hir_span(span);
        if !type_args.is_empty() {
            let description = "list.repeat with explicit type arguments".to_string();
            return Some(self.unsupported_operand(description, scope, call_span, out));
        }
        let slots = Vec::from(["value", "count"].map(|slot| DeclaredSlot {
            name: Some(slot.to_string()),
            has_default: false,
            is_partial_preset: false,
            kind: IncanCallableParamKind::Normal,
            ty: IncanType::Unknown,
            is_mut: false,
        }));
        let (operands, binding) = match self.bind_declared_args("builtin `list.repeat`", Some(slots), args, scope, out)
        {
            Ok(bound) => bound,
            Err(description) => {
                return Some(self.unsupported_operand(description, scope, call_span, out));
            }
        };
        let Some([value, count]) = slot_ordered_pair(operands, &binding) else {
            let description = "list.repeat without one value and one count".to_string();
            return Some(self.unsupported_operand(description, scope, call_span, out));
        };
        let helper = bir::HelperOp::ListRepeat;
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        self.panic_facts.push(bir::PanicFact {
            span: call_span,
            reason: bir::PanicReason::HelperMayPanic(helper),
        });
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            bir::Callee::Helper(helper),
            fixed_elements(vec![value, count]),
            ty,
            scope,
            call_span,
            true,
            out,
        ))
    }

    /// Lower a module-qualified function call or nominal construction using its selected declaration.
    ///
    /// A module binding is a namespace, not a value. `identity_provider.imported_path(17)` reaches the AST as a
    /// method call, but there is no receiver to read, and treating the qualifier as a place refuses at a name that
    /// never named one. The typechecker has already resolved this call to exactly one declaration; consuming that
    /// identity keeps the call on the direct path instead of re-deriving a target from the two spellings the source
    /// happens to join with a dot. The receiver's own identity decides whether this applies, so a local variable
    /// that happens to share a module's name still lowers as an ordinary method call.
    ///
    /// A selected nominal owner constructs an aggregate instead of invoking a function. Its checked field slots, or
    /// a complete set of checked field identities matching that owner's retained layout, determine the binding.
    /// Missing layout or identity evidence remains an explicit refusal.
    /// Validated async targets retain an owned future between construction and await rather than copying their
    /// eventual-output type; the private checker fact supplies that distinction.
    ///
    /// Returns `None` when the receiver is not a module, leaving every other receiver shape untouched.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_module_qualified_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if !self
            .type_info
            .resolved_identity(recv.span)
            .is_some_and(|identity| identity.kind == SemanticSourceTargetKind::Module)
        {
            return None;
        }
        let hir_span_value = hir_span(span);

        // Without the call's own resolved identity there is no target: the module qualifier names where to look,
        // not what was found. Refusing here rather than falling through to the method path reports the construct
        // the source actually wrote.
        let Some(canonical) = self.type_info.resolved_identity(span).cloned() else {
            return Some(self.unsupported_operand(
                format!("module-qualified call `{name}` without a resolved canonical declaration"),
                scope,
                hir_span_value,
                out,
            ));
        };
        if matches!(
            canonical.kind,
            SemanticSourceTargetKind::Model | SemanticSourceTargetKind::Class | SemanticSourceTargetKind::Newtype
        ) {
            if self.type_info.body_ir_constructor_field_binding(span).is_some() {
                return Some(self.lower_nominal_construction(name, args, span, span, scope, out));
            }
            let Some(slots) = self.supplied_module_constructor_slots(&canonical, args) else {
                return Some(self.unsupported_operand(
                    format!("module-qualified construction of `{name}` without a checked canonical field binding"),
                    scope,
                    hir_span_value,
                    out,
                ));
            };
            return Some(self.lower_nominal_construction_with_slots(
                name,
                args,
                span,
                span,
                scope,
                out,
                (slots.len(), &slots),
            ));
        }
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return Some(self.unsupported_operand(description, scope, hir_span_value, out));
            }
        };
        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let (operands, binding) =
            match self.bind_declared_args(&format!("function `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return Some(self.unsupported_operand(description, scope, hir_span_value, out));
                }
            };
        let returned = self.resolve_ty(span);
        let ty = if self.checked_module_async_call(&canonical, span) {
            IncanType::Generic {
                base: "Awaitable".to_owned(),
                args: vec![returned],
            }
        } else {
            returned
        };
        Some(self.push_call_temp(
            // The declaration lives in another module, so it has no span identity in this one. `canonical` is the
            // fact that survives the boundary, and it is the only one a consumer may dispatch on here.
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                receiver_type: None,
                name: name.to_string(),
                direct_call_id: None,
                canonical: Some(canonical),
                builtin: None,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        ))
    }

    /// The checked result type of the method call at `span`.
    ///
    /// In the standard library's own trait defaults, a call on an open `Self` receiver keeps its checked type open for
    /// legacy; its slot's declared result, which the checker retains for Body IR only, is the call's type instead.
    pub(in crate::body_ir) fn method_call_result_type(&self, span: ast::Span) -> IncanType {
        let checked = self.resolve_ty(span);
        if !matches!(checked, IncanType::Unknown) {
            return checked;
        }
        self.type_info
            .traits
            .self_method_result_types
            .get(&span)
            .map_or(checked, |declared| self.checked_type(declared))
    }
}
