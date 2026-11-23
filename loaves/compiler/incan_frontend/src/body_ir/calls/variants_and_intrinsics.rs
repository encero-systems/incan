//! Enum and value-enum variant calls, checked `isinstance` calls, and intrinsic result and option constructors.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Return the exact retained target for a qualified local fieldless normal-enum member, if safe to materialize.
    ///
    /// A bare type-name receiver and source-local registry membership are both required. This leaves ordinary forms
    /// not represented by the registry as generic field accesses that the direct executor visibly refuses, while
    /// preserving exact declaration identities for the one bounded unit-variant carrier profile.
    pub(in crate::body_ir) fn local_fieldless_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
        access_span: ast::Span,
    ) -> Option<bir::FieldlessEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name)
            || !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let declaration = self
            .local_fieldless_enum_declarations
            .values()
            .find(|declaration| self.type_info.resolved_identity(base.span) == Some(&declaration.canonical))?;
        if self.type_info.resolved_identity(base.span) != Some(&declaration.canonical) {
            return None;
        }
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        if self.type_info.resolved_identity(access_span) != Some(&variant.canonical) {
            return None;
        }
        Some(bir::FieldlessEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            enum_canonical: declaration.canonical.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            variant_canonical: variant.canonical.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Return an enum constructor selected by checked declaration identity, including module-qualified and explicitly
    /// applied type receivers.
    ///
    /// Type application and parentheses do not create runtime receivers. The underlying receiver must still be a
    /// checked type name, and the call/member span must select a variant. A shadowing local never gains constructor
    /// semantics from its spelling.
    pub(in crate::body_ir) fn checked_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        _variant_name: &str,
        variant_span: ast::Span,
    ) -> Option<bir::EnumVariantTarget> {
        let mut type_receiver = base;
        while let ast::Expr::Index(inner, _) | ast::Expr::Paren(inner) = &type_receiver.node {
            type_receiver = inner;
        }
        if !matches!(
            self.type_info.ident_kind(type_receiver.span),
            Some(IdentKind::TypeName | IdentKind::RustImport)
        ) || matches!(&type_receiver.node, ast::Expr::Ident(name) if self.bindings.contains_key(name))
        {
            return None;
        }
        let variant_canonical = self
            .type_info
            .resolved_identity(variant_span)
            .filter(|identity| identity.kind == SemanticSourceTargetKind::Variant)?
            .clone();
        let enum_canonical = match &variant_canonical.origin {
            SymbolOrigin::RustCrate(path) => bir::rust_item_identity(&path.join("::")),
            _ => self.type_info.resolved_identity(type_receiver.span).cloned(),
        };
        let enum_name = match &type_receiver.node {
            ast::Expr::Ident(name) => name.clone(),
            _ => enum_canonical
                .as_ref()
                .filter(|identity| identity.kind == SemanticSourceTargetKind::Enum)?
                .declaration_name
                .clone(),
        };
        Some(bir::EnumVariantTarget {
            enum_name,
            variant_name: variant_canonical.declaration_name.clone(),
            enum_canonical,
            variant_canonical,
            binding: bir::ArgumentBinding::resolved_positional(0),
        })
    }

    /// Construct `Type.Variant(payload)` as an [`bir::AggregateKind::EnumVariant`], binding the payload against the
    /// variant's checked parameters when the checker recorded them and positionally otherwise. Retained payload
    /// layouts provide ordinary parameter slots with their exact types when no callable-parameter record is present.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_enum_variant_call(
        &mut self,
        mut target: bir::EnumVariantTarget,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .body_ir_call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect())
            .or_else(|| {
                self.enum_payload_types(&target.variant_canonical, &self.resolve_constructor_ty(span))
                    .map(|fields| {
                        fields
                            .into_iter()
                            .map(|ty| DeclaredSlot {
                                name: None,
                                has_default: false,
                                is_partial_preset: false,
                                kind: IncanCallableParamKind::Normal,
                                ty,
                                is_mut: false,
                            })
                            .collect()
                    })
            });
        let what = format!("enum variant `{}.{}`", target.enum_name, target.variant_name);
        let (operands, binding) = match self.bind_declared_args(&what, declared, args, scope, out) {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        target.binding = binding;
        let ty = self.resolve_constructor_ty(span);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(bir::AggregateKind::EnumVariant(Box::new(target)), operands),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Return the exact retained target for a qualified local RFC 032 value-enum member or variant alias.
    ///
    /// The source-local registry is deliberately the only lookup used here. A function-local binding wins over a
    /// same-spelling declaration, and imported owners and ordinary enums are absent from this registry. The checked
    /// member identity selects the physical variant, so aliases add no storage slots or lookup policy. The resulting
    /// rvalue stores both declaration identities for runtime revalidation; spelling grants no execution authority.
    pub(in crate::body_ir) fn local_value_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        access_span: ast::Span,
    ) -> Option<bir::ValueEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name) {
            return None;
        }
        if !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName)) {
            return None;
        }
        let declaration = self
            .local_value_enum_declarations
            .values()
            .find(|declaration| self.type_info.resolved_identity(base.span) == Some(&declaration.canonical))?;
        let identity = self.type_info.resolved_identity(access_span)?;
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.canonical == *identity)?;
        Some(bir::ValueEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            enum_canonical: declaration.canonical.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            variant_canonical: variant.canonical.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Lower one compiler-owned `isinstance` call from the typechecker's retained builtin and target facts.
    pub(super) fn lower_checked_isinstance_call(
        &mut self,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let call_span = hir_span(span);
        let [ast::CallArg::Positional(value), ast::CallArg::Positional(target_expr)] = args else {
            return self.unsupported_operand(
                "isinstance outside the positional checked-target profile".to_string(),
                scope,
                call_span,
                out,
            );
        };
        if !type_args.is_empty() {
            return self.unsupported_operand(
                "isinstance with explicit call-site type arguments".to_string(),
                scope,
                call_span,
                out,
            );
        }
        let Some(target) = self.type_info.isinstance_target(span) else {
            return self.unsupported_operand(
                "isinstance without checked target evidence".to_string(),
                scope,
                hir_span(target_expr.span),
                out,
            );
        };
        let target_span = hir_span(target.span);
        let target_span = if target_span.start >= call_span.start && target_span.end <= call_span.end {
            target_span
        } else {
            call_span
        };
        let value_ty = self.resolve_ty(value.span);
        let value = self.lower_expr_to_operand(value, scope, out);
        let ty = self.resolve_ty(span);
        self.push_assign_temp(
            bir::Rvalue::IsInstance(Box::new(bir::IsInstanceRvalue {
                value,
                value_ty,
                target: bir::IsInstanceTarget {
                    ty: semantic_type_from_resolved(&target.ty),
                    canonical: target.canonical.clone(),
                    span: target_span,
                },
            })),
            ty,
            scope,
            call_span,
            out,
        )
    }

    /// The intrinsic `Result` variant a call of `callee` at `span` constructs: `Ok` or `Err` spelled as a bare name
    /// that no local, same-module function or source target binds. A same-spelled source binding stays an ordinary
    /// call that refuses unless its own direct callable facts are available; the direct runtime never resolves a
    /// constructor name dynamically.
    pub(in crate::body_ir) fn intrinsic_result_constructor(
        &self,
        callee: &ast::Spanned<ast::Expr>,
        span: ast::Span,
    ) -> Option<bir::ResultVariantKind> {
        let ast::Expr::Ident(name) = &callee.node else {
            return None;
        };
        if self.bindings.contains_key(name)
            || self.local_function_declarations.contains_key(name)
            || self.type_info.source_target(span).is_some()
        {
            return None;
        }
        result_variant_kind(name)
    }

    /// Authenticate the builtin Option constructor before retaining a destination's checked payload type.
    /// A source callable or local binding named Some never acquires the intrinsic constructor's representation.
    pub(in crate::body_ir) fn intrinsic_option_constructor(&self, callee: &ast::Spanned<ast::Expr>) -> bool {
        let ast::Expr::Ident(name) = &callee.node else {
            return false;
        };
        name == constructors::as_str(ConstructorId::Some)
            && !self.bindings.contains_key(name)
            && !self.local_function_declarations.contains_key(name)
            && self.type_info.resolved_identity(callee.span).is_some_and(|identity| {
                identity.origin == SymbolOrigin::Builtin
                    && identity.declaration_name == constructors::as_str(ConstructorId::Some)
            })
    }

    /// Settle each `Ok(...)` or `Err(...)` argument of the call at `span` to the parameter type the checker resolved
    /// for it, which carries the side the constructor leaves open as the call settled it (see
    /// [`Self::settle_open_result_value`]). A call with no resolved parameters, a rest parameter, or arguments that do
    /// not bind settles nothing; binding the arguments for the call itself still reports those.
    pub(in crate::body_ir) fn settle_open_call_arguments(&mut self, span: ast::Span, args: &[ast::CallArg]) {
        let type_info = self.type_info;
        let Some(params) = type_info.body_ir_call_site_callable_params(span) else {
            return;
        };
        let slots: Vec<DeclaredSlot> = params.iter().map(DeclaredSlot::from_checked_param).collect();
        let Ok(planned) = plan_declared_args("call", &slots, args) else {
            return;
        };
        for (slot, argument) in planned {
            if let Some(param) = params.get(slot) {
                let destination = self.checked_type(&param.ty);
                self.settle_open_result_value(argument, &destination);
            }
        }
    }
}
