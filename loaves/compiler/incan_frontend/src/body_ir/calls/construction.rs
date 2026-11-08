//! Callee declarations, call-site type arguments, newtype coercions, nominal construction, and spread-capable
//! arguments.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Retain a nonlocal callable's checked signature by the exact selected identity, including imported overloads.
    ///
    /// An overload-set spelling cannot select a signature. Its canonical call-site identity must match a retained
    /// candidate; absent proof keeps the binding unresolved rather than borrowing another overload's slots.
    /// Constructor syntax keeps the static method selected at the full call span instead of the nominal identity
    /// selected at the callee token, using that method's checked parameter surface.
    /// A module-qualified async call consumes the private validated target fact to retain its lazy future carrier;
    /// the existing checked call parameters supply its slots without inferring a signature from its spelling.
    /// A fixed scalar re-export call consumes the private manifest target and validated parameters; its unresolved
    /// legacy binding does not supply the executable identity.
    pub(super) fn external_call_declaration(
        &self,
        name: &str,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Result<DirectCallDeclaration, String> {
        if let Some(factory) = self.checked_nominal_factory_declaration(callee_span, call_span) {
            return Ok(factory);
        }
        let reexport = self.type_info.calls.body_ir_reexport_calls.get(&call_span);
        let canonical = self
            .type_info
            .resolved_call_identity(name, callee_span, reexport.map(|call| &call.canonical))
            .cloned();
        let builtin = self.type_info.resolved_builtin_call(call_span);
        let canonical_builtin_id = canonical.as_ref().and_then(canonical_builtin);
        if builtin != canonical_builtin_id && (builtin.is_some() || canonical_builtin_id.is_some()) {
            return Err(format!(
                "checked builtin call `{name}` does not retain its canonical builtin identity"
            ));
        }
        let signature = match self.type_info.function_overloads(name) {
            Some(candidates) => canonical.as_ref().and_then(|selected| {
                candidates
                    .iter()
                    .find(|candidate| candidate.identity.as_ref() == Some(selected))
                    .map(|candidate| (candidate.info.is_async, candidate.info.params.as_slice()))
            }),
            None => self
                .type_info
                .declarations
                .function_bindings
                .get(name)
                .map(|binding| (binding.is_async, binding.params.as_slice())),
        };
        let checked_module_async = canonical
            .as_ref()
            .is_some_and(|identity| self.checked_module_async_call(identity, call_span));
        let parameters = signature
            .map(|(_, parameters)| parameters)
            .or_else(|| reexport.map(|call| call.parameters.as_slice()))
            .or_else(|| {
                checked_module_async
                    .then(|| self.type_info.body_ir_call_site_callable_params(call_span))
                    .flatten()
            });
        Ok(DirectCallDeclaration {
            is_async: checked_module_async || signature.is_some_and(|(is_async, _)| is_async),
            slots: parameters.map(|parameters| {
                self.type_info
                    .body_ir_call_site_callable_params(call_span)
                    .unwrap_or(parameters)
                    .iter()
                    .map(DeclaredSlot::from_checked_param)
                    .collect()
            }),
            direct_call_id: None,
            builtin,
            canonical,
        })
    }

    /// Consume the private async target selected by successful module-call validation, independent of its spelling.
    pub(super) fn checked_module_async_call(&self, canonical: &CanonicalSymbolId, span: ast::Span) -> bool {
        self.type_info.calls.body_ir_async_call_targets.get(&span) == Some(canonical)
    }

    /// Keep the static factory the checker selected for constructor syntax, independently of the named type.
    ///
    /// The callee span names the nominal owner; the full call span selects its executable method and substituted
    /// parameters. Reading the former as a function would lose that selection. Ordinary functions and unresolved
    /// nominal calls keep their existing path, and no source spelling supplies a factory identity.
    /// A same-module hook also keeps its checked local declaration ID; package calls remain canonically addressed.
    pub(super) fn checked_nominal_factory_declaration(
        &self,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Option<DirectCallDeclaration> {
        let owner = self.type_info.resolved_identity(callee_span)?;
        if !matches!(
            owner.kind,
            SemanticSourceTargetKind::Model
                | SemanticSourceTargetKind::Class
                | SemanticSourceTargetKind::Newtype
                | SemanticSourceTargetKind::Enum
        ) {
            return None;
        }
        let factory = self.type_info.resolved_identity(call_span)?;
        if factory.kind != SemanticSourceTargetKind::Method {
            return None;
        }
        let parameters = self.type_info.call_site_callable_params(call_span)?;
        let direct_call_id =
            if incan_semantics_core::canonical_module_identity(factory).as_deref() == Some(self.module_identity) {
                self.type_info
                    .declarations
                    .method_bindings_by_span
                    .iter()
                    .find(|(_, binding)| binding.identity.as_ref() == Some(factory))
                    .map(|(span, _)| CompilerNodeId::declaration_at(self.module_identity, *span))
            } else {
                None
            };
        Some(DirectCallDeclaration {
            is_async: false,
            slots: Some(parameters.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id,
            builtin: None,
            canonical: Some(factory.clone()),
        })
    }

    /// Retain an applied factory owner from the checked construction result, independently of method type arguments.
    ///
    /// A selected static factory returning its own generic nominal carries that owner's arguments on the associated
    /// call. A wrapper result, ordinary function, absent factory selection or mismatched nominal cannot supply them.
    /// This consumes existing checked facts only, comparing retained declaration identities after carrier projection;
    /// the legacy inference tables remain unchanged.
    pub(super) fn checked_nominal_factory_owner(
        &self,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Option<IncanType> {
        self.checked_nominal_factory_declaration(callee_span, call_span)?;
        let owner = self.type_info.resolved_identity(callee_span)?;
        let result = self.resolve_ty(call_span);
        match &result {
            IncanType::Generic { base, .. }
                if parameter_types::retained_nominal_identity(base, self.type_info) == Some(owner) =>
            {
                Some(result)
            }
            _ => None,
        }
    }

    /// Read a constructor method's own checked binders; owner arguments stay on the retained receiver.
    /// An absent method instantiation supplies no method arguments and never substitutes written owner arguments.
    pub(super) fn checked_factory_method_type_arguments(&self, span: ast::Span) -> Vec<IncanType> {
        self.type_info
            .calls
            .call_site_monomorph_type_args
            .get(&span)
            .into_iter()
            .flatten()
            .map(|ty| parameter_types::physical_type_argument(self.checked_type(ty)))
            .collect()
    }

    /// Retain checked explicit or inferred call-site type arguments, refusing unresolved explicit arguments.
    ///
    /// Type arguments are part of a call's resolved identity, so Body IR takes the typechecker's
    /// monomorphized selection rather than re-lowering the written AST type nodes -- which is also the only way a
    /// `_` placeholder resolves to a real type instead of an unknown. A call that wrote type arguments the
    /// typechecker did not resolve is refused by name rather than represented with a guess.
    pub(in crate::body_ir) fn call_site_type_arguments(
        &self,
        span: ast::Span,
        type_args: &[ast::Spanned<ast::Type>],
    ) -> Result<Vec<IncanType>, String> {
        if let Some(application) = self.type_info.declarations.body_ir_decorator_applications.get(&span)
            && !application.factory_type_args.is_empty()
        {
            return Ok(application
                .factory_type_args
                .iter()
                .map(|ty| {
                    let ty = crate::resolved_type_subst::substitute_resolved_type(ty, &application.type_bindings);
                    parameter_types::physical_type_argument(self.checked_type(&ty))
                })
                .collect());
        }
        // A retained raw owner instance has no method binders; its owner arguments live in the receiver type.
        if type_args.is_empty()
            && self
                .type_info
                .body_ir_newtypes
                .callable_params(span, self.type_info)
                .is_some()
        {
            return Ok(Vec::new());
        }
        let Some(resolved) = self
            .type_info
            .calls
            .body_ir_call_type_arguments
            .get(&span)
            .or_else(|| self.type_info.calls.call_site_monomorph_type_args.get(&span))
        else {
            if type_args.is_empty() {
                return Ok(Vec::new());
            }
            return Err("call with unresolved explicit type arguments".to_string());
        };
        Ok(resolved
            .iter()
            .map(|ty| parameter_types::physical_type_argument(self.checked_type(ty)))
            .collect())
    }

    /// Select the declaration identity a checked construction builds.
    ///
    /// An ordinary callee resolves to that declaration. A classmethod's `cls(...)` resolves its callee to the `cls`
    /// receiver binding instead, while checking types the construction as the owner; the owner's declaration then
    /// comes from the checker's module-scope nominal bindings for that type, never from the callee's spelling.
    /// Project carrier qualification may replace the owner name with its kernel identity key. The checked owner
    /// lookup recognizes that key without decoding a spelling and refuses conflicting bindings; local lowering keeps
    /// its existing direct binding lookup.
    pub(super) fn constructed_declaration_identity(
        &self,
        callee_span: ast::Span,
        span: ast::Span,
    ) -> Option<CanonicalSymbolId> {
        if let Some(owner) = self.type_info.body_ir_native_constructor_identity(span) {
            return Some(owner.clone());
        }
        let identity = self.type_info.resolved_identity(callee_span)?;
        if identity.kind != SemanticSourceTargetKind::Receiver || identity.declaration_name != "cls" {
            return Some(identity.clone());
        }
        let owner = match self.resolve_ty(span) {
            IncanType::Named(name) => name,
            IncanType::Generic { base, .. } => base,
            _ => return None,
        };
        if self.retain_nominal_carriers {
            parameter_types::retained_nominal_identity(&owner, self.type_info).cloned()
        } else {
            self.type_info.declarations.named_type_identities.get(&owner).cloned()
        }
    }

    /// Whether field `slot` of the source-local model or class `owner` declares an effect-free fresh value.
    ///
    /// Literal construction is admitted directly. A zero-argument local nominal construction is admitted only when
    /// every field it omits has the same proof recursively. Missing fields, supplied arguments, cycles, imported
    /// owners and ordinary calls stay observable and therefore cannot cross a supplied argument's evaluation.
    pub(super) fn field_default_is_effect_free(&self, owner: &str, slot: usize) -> bool {
        let Some(declaration) = self.local_nominal_declarations.get(owner) else {
            return false;
        };
        self.function_default_sources
            .get(owner)
            .filter(|fields| fields.len() == declaration.fields.len())
            .and_then(|fields| fields.get(slot))
            .and_then(|field| field.default.as_ref())
            .is_some_and(|default| self.default_source_is_effect_free(default, &mut HashSet::new()))
    }

    /// Prove fresh default construction from source declarations without executing a user callable.
    fn default_source_is_effect_free(&self, value: &ast::Spanned<ast::Expr>, visiting: &mut HashSet<String>) -> bool {
        if value.is_literal_construction(&|span| self.type_info.resolved_collection_constructor(span).is_some()) {
            return true;
        }
        let (owner, arguments) = match &value.node {
            ast::Expr::Constructor(owner, arguments) => (owner, arguments.as_slice()),
            ast::Expr::Call(callee, type_arguments, arguments) if type_arguments.is_empty() => {
                let ast::Expr::Ident(owner) = &callee.node else {
                    return false;
                };
                (owner, arguments.as_slice())
            }
            _ => return false,
        };
        if !arguments.is_empty() || !visiting.insert(owner.clone()) {
            return false;
        }
        let proven = self
            .local_nominal_declarations
            .get(owner)
            .and_then(|declaration| {
                self.function_default_sources
                    .get(owner)
                    .filter(|fields| fields.len() == declaration.fields.len())
            })
            .is_some_and(|fields| {
                fields.iter().all(|field| {
                    field
                        .default
                        .as_ref()
                        .is_some_and(|default| self.default_source_is_effect_free(default, visiting))
                })
            });
        visiting.remove(owner);
        proven
    }

    /// Lower a value the checker coerces into a newtype where it is used (RFC 017).
    ///
    /// Source-local nongeneric chains evaluate the input once and retain every selected hook or predicate before its
    /// canonical wrapper. Aggregate-field error collection and unretained owners remain specifically refused.
    pub(in crate::body_ir) fn lower_newtype_coercion(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        coercion: &ValidatedNewtypeCoercionInfo,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        self.lower_newtype_ingress(expr, coercion, scope, out)
    }

    /// Refuse a supplied computation that would precede an observable omitted default. Canonically matched local
    /// literal defaults and declaration-owned literal construction have no source effect to reorder.
    /// The checked local source or declaring frame supplies this evidence; absent or unproven defaults stay refused.
    pub(super) fn construction_interleaves_default_effects(
        &self,
        canonical: Option<&CanonicalSymbolId>,
        owner: &str,
        supplied_slots: &[usize],
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
    ) -> bool {
        if supplied_slots.windows(2).any(|pair| pair[0] > pair[1]) {
            return false;
        }
        let declaration = canonical.and_then(|identity| {
            self.local_nominal_declarations
                .values()
                .find(|value| &value.canonical == identity)
        });
        let defaults = declaration.and_then(|declaration| declaration.field_default_body.as_deref());
        let local_source = self
            .local_nominal_declarations
            .get(owner)
            .filter(|source| Some(&source.canonical) == canonical);
        planned.iter().any(|(slot, expression)| {
            !matches!(expression.node, ast::Expr::Literal(_))
                && (0..*slot).any(|prior| {
                    !supplied_slots.contains(&prior)
                        && !(local_source.is_some() && self.field_default_is_effect_free(owner, prior))
                        && defaults
                            .and_then(|frame| frame.params.get(prior))
                            .is_none_or(|parameter| !defaults::is_literal_default(&parameter.default))
                })
        })
    }

    /// Lower a `model`/`class` or plain-newtype construction into a [`bir::AggregateKind::Constructor`] aggregate.
    ///
    /// Models/classes bind named fields; a newtype binds its single positional argument to tuple slot zero. Lowering
    /// consumes the typechecker's own recorded decision
    /// ([`TypeCheckInfo::constructor_field_binding`](crate::typechecker::TypeCheckInfo::constructor_field_binding))
    /// rather than re-resolving field aliases or rediscovering declared field order, both of which live in the
    /// symbol table this stage deliberately cannot reach. Operands are emitted in declared field order while the
    /// argument expressions are lowered in written source order, exactly as for a call.
    pub(in crate::body_ir) fn lower_nominal_construction(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        callee_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let Some(field_binding) = self.type_info.body_ir_constructor_field_binding(span).cloned() else {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        };
        self.lower_nominal_construction_with_slots(
            name,
            args,
            span,
            callee_span,
            scope,
            out,
            (field_binding.field_count, &field_binding.argument_slots),
        )
    }

    /// Materialize a construction from checker-selected slots or a complete canonical field-identity binding.
    ///
    /// Both producers preserve written evaluation order and the same selected owner. This boundary never binds a
    /// source field name: the supplied slot layout must already belong to that owner's canonical declaration.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_nominal_construction_with_slots(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        callee_span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
        layout: (usize, &[usize]),
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let (field_count, argument_slots) = layout;
        let written_exprs = match nominal_argument_expressions(name, args) {
            Ok(values) => values,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };
        if written_exprs.len() != argument_slots.len() {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let planned: Vec<(usize, &ast::Spanned<ast::Expr>)> =
            argument_slots.iter().copied().zip(written_exprs).collect();
        let canonical = self.constructed_declaration_identity(callee_span, span);
        // Supplied operands precede deferred defaults; only retained effect-free defaults may cross that boundary.
        if self.construction_interleaves_default_effects(canonical.as_ref(), name, argument_slots, &planned) {
            return self.unsupported_operand(
                format!("model field-default evaluation interleaved with supplied arguments on `{name}`"),
                scope,
                hir_span_value,
                out,
            );
        }
        let contextual_types = self.retain_constructor_argument_carriers(canonical.as_ref(), field_count, &planned);
        let single_field =
            self.single_checked_field_ingress(canonical.as_ref(), &planned, field_count, scope, hir_span_value, out);
        let lowered = match single_field {
            Some(lowered) => lowered,
            None => self.lower_planned_args(&planned, field_count, &[], &[], scope, out),
        };
        self.restore_constructor_argument_carriers(contextual_types);
        let (operands, binding) = match lowered {
            Ok(bound) => bound,
            Err(description) => {
                return self.unsupported_operand(
                    format!("construction of `{name}`: {description}"),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };
        let ty = self.resolve_ty(span);
        // A constructor field binding proves argument slots, but not that this constructor names one of the plain
        // models admitted to this lowering context. Preserve the selected declaration's identity and layout together;
        // imported aliases use their original canonical identity, while absent or unsupported context retains
        // neither fact and is refused without guessing from `name`.
        let (direct_declaration_id, canonical_field_layout) =
            self.nominal_constructor_layout(canonical.as_ref(), field_count);
        if let Some(checked) =
            self.checked_newtype_construction(name, canonical.as_ref(), &operands, &ty, scope, hir_span_value, out)
        {
            return checked;
        }
        let operands = self.constrain_nominal_arguments(name, canonical.as_ref(), operands, scope, hir_span_value, out);
        self.push_assign_temp(
            bir::Rvalue::Aggregate(
                bir::AggregateKind::Constructor(Box::new(bir::ConstructorTarget {
                    name: name.to_string(),
                    canonical,
                    direct_declaration_id,
                    canonical_field_layout,
                    binding,
                })),
                fixed_elements(operands),
            ),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Carry only the selected declaration's canonical field order; argument arity alone cannot name a layout.
    ///
    /// This retains the existing aggregate boundary for aliases and absent declarations. Validation ingress must
    /// separately prove field types, hook identity, and failure strategy before it evaluates any supplied input.
    pub(super) fn nominal_constructor_layout(
        &self,
        canonical: Option<&CanonicalSymbolId>,
        field_count: usize,
    ) -> (Option<CompilerNodeId>, Option<Vec<String>>) {
        self.local_nominal_declarations
            .values()
            .find(|declaration| declaration.fields.len() == field_count && canonical == Some(&declaration.canonical))
            .map(|declaration| {
                (
                    Some(declaration.direct_declaration_id.clone()),
                    Some(declaration.fields.clone()),
                )
            })
            .unwrap_or((None, None))
    }

    /// Bind fully supplied module constructor fields by checked identities against the same retained owner.
    ///
    /// No source spelling, default, generic field or spread supplies execution authority here. Missing, duplicate or
    /// mismatched identities leave construction refused without altering the constructor table legacy consumes.
    pub(super) fn supplied_module_constructor_slots(
        &self,
        canonical: &CanonicalSymbolId,
        args: &[ast::CallArg],
    ) -> Option<Vec<usize>> {
        let declaration = self
            .local_nominal_declarations
            .values()
            .find(|declaration| &declaration.canonical == canonical)?;
        if args.is_empty()
            || args.len() != declaration.fields.len()
            || declaration.field_identities.len() != declaration.fields.len()
            || !declaration.type_parameters.is_empty()
        {
            return None;
        }
        let mut slots = Vec::with_capacity(args.len());
        for argument in args {
            let ast::CallArg::Named(field, _) = argument else {
                return None;
            };
            let identity = self.type_info.resolved_identity(field.span)?;
            let slot = declaration
                .field_identities
                .iter()
                .position(|field| field == identity)?;
            if slots.contains(&slot) {
                return None;
            }
            slots.push(slot);
        }
        Some(slots)
    }

    /// Lower call arguments positionally, admitting spreads, for a call whose arity is not statically known.
    ///
    /// Every argument keeps its written form: a positional value, a named value, or a spread. None of them can be
    /// resolved to a declared slot here, because a spread supplies an unknown number of arguments at runtime —
    /// which is exactly why the resulting call records [`bir::ArgumentBinding::UnresolvedPositional`] rather than a
    /// slot map asserting a binding nobody checked. A name is preserved on its element rather than discarded, so a
    /// later consumer can still bind it once the arity is known. Scalar and owned-text global values are captured
    /// at each argument's evaluation point; absent slot knowledge does not permit deferring their reads until after
    /// later argument effects. This does not invent mutable destination slots or alter collection-handle semantics.
    pub(in crate::body_ir) fn lower_spread_capable_args(
        &mut self,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::ArgumentElement> {
        let mut elements = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    let operand = self.retain_evaluated_value_read(expr, operand, scope, out);
                    elements.push(bir::ArgumentElement::One(operand));
                }
                ast::CallArg::PositionalUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out));
                }
                ast::CallArg::KeywordUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out));
                }
                ast::CallArg::Named(name, expr) => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    let operand = self.retain_evaluated_value_read(expr, operand, scope, out);
                    elements.push(bir::ArgumentElement::Named {
                        name: name.node.clone(),
                        operand,
                    });
                }
            }
        }
        elements
    }
}
