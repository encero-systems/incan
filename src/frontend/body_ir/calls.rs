//! Lowering for calls: argument planning and binding, callable identity, nominal construction, method calls.

use super::args::*;
use super::primitives::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower planned call arguments in written source order, then place them into declaration-slot order.
    ///
    /// Both orders are part of the source contract and they differ whenever a caller writes named arguments out of
    /// declaration order. Argument expressions are therefore lowered here strictly left to right, so the emitted
    /// statement sequence observes written evaluation order, while the returned operand vector is in declaration
    /// order and the returned [`bir::ArgumentBinding`] records which slot each operand fills and where it was
    /// written. A declaration slot the call site never supplied becomes a defaulted slot rather than an operand:
    /// this call site evaluates nothing for it, so it has no ownership fact to record and the default's computation
    /// stays owned by the declaration.
    ///
    /// Because ownership is decided during that written-order pass, each operand's [`bir::OwnershipFact`] and
    /// last-use marker are sequenced by `written_position` and **not** by operand index -- see
    /// [`bir::ArgumentBinding`]'s own docs, which state the invariant a consumer has to honor.
    pub(super) fn lower_planned_args(
        &mut self,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        slot_count: usize,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        // Both planners derive their slots from the same declaration surface they report the count of, so an
        // out-of-range slot is unreachable. It is still refused rather than skipped: dropping the operand while
        // leaving the statements that computed it in `out` would produce a silently wrong call, which is the worst
        // failure mode this node has.
        if let Some((slot, _)) = planned.iter().find(|(slot, _)| *slot >= slot_count) {
            return Err(format!(
                "argument bound to declaration slot {slot} outside the callee's {slot_count} declared slots"
            ));
        }
        let mut lowered: Vec<Option<(bir::Operand, usize)>> = (0..slot_count).map(|_| None).collect();
        for (written_position, (slot, expr)) in planned.iter().enumerate() {
            let operand = self.lower_expr_to_operand(expr, scope, out);
            if let Some(entry) = lowered.get_mut(*slot) {
                *entry = Some((operand, written_position));
            }
        }

        let mut operands = Vec::with_capacity(planned.len());
        let mut arguments = Vec::with_capacity(planned.len());
        let mut defaulted_slots = Vec::new();
        for (slot, entry) in lowered.into_iter().enumerate() {
            match entry {
                Some((operand, written_position)) => {
                    operands.push(operand);
                    arguments.push(bir::BoundArgument { slot, written_position });
                }
                None => defaulted_slots.push(slot),
            }
        }
        Ok((
            operands,
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            },
        ))
    }

    /// Build the RFC 120 identity of a declaration *this* module owns, from the declaration the call selected.
    ///
    /// The span is the one the caller already resolved to build [`DirectCallDeclaration::direct_call_id`], so the two
    /// facts on a `NamedCallableTarget` are derived from one decision and cannot name different declarations. That
    /// matters because locality must not be inferred from the recorded source target: an import binding of the same
    /// spelling wins in `TypeChecker::source_target_for_symbol` regardless of what the call actually bound, so a local
    /// declaration shadowed by a same-name import would otherwise be given the *import's* identity.
    pub(super) fn local_callable_identity(
        &self,
        declared_name: &str,
        declaration_span: ast::Span,
    ) -> CanonicalSymbolId {
        CanonicalSymbolId::module_declaration(
            self.module_path.to_vec(),
            declared_name,
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(declaration_span.start, declaration_span.end),
        )
    }

    /// Return the RFC 120 identity of an imported callable, when import resolution proved one.
    ///
    /// Only reached when this module declares no function of that spelling, so there is no local declaration for an
    /// import to be confused with. The proven identity carries its own declaration kind, so a binding that resolved to
    /// something other than a function yields nothing rather than a function identity.
    ///
    /// An overloaded import is refused upstream, in `TypeChecker::dependency_member_identity`: an overloaded symbol
    /// keeps only the first candidate's span, so an identity minted from it would name an arbitrary overload.
    pub(super) fn imported_callable_identity(&self, call_site_name: &str) -> Option<CanonicalSymbolId> {
        let identity = self.type_info.resolved_import_identity(call_site_name)?;
        (identity.kind == SemanticSourceTargetKind::Function).then(|| identity.clone())
    }

    /// Resolve the declaration surface and exact local identity for a direct named call.
    ///
    /// A direct executable target must be physically represented by this Body-IR module. Imports and unresolved
    /// names deliberately retain their existing call representation with no direct declaration identity, so this
    /// frontend does not turn a source-representation gap into a new source diagnostic. The replacement executor
    /// then refuses those targets at the original call span. Every unshadowed core builtin has a registry identity;
    /// individual consumers still admit only their documented subset, such as `range` for counting-loop lowering.
    ///
    /// Overloads are why this is resolved per call site rather than per name. `function_bindings` is keyed by bare
    /// source name, so for two same-name declarations it holds only one of them; binding a call against the wrong
    /// overload's parameter *names* would silently reorder its arguments, turning an honest refusal into a wrong
    /// answer. The typechecker already records which overload it selected for this call span, so this follows that
    /// decision to the declaration and reads that declaration's own signature. If a name is overloaded but no
    /// selection was recorded, this fails closed rather than picking one.
    pub(super) fn declared_slots_for_direct_call(
        &self,
        name: &str,
        span: ast::Span,
    ) -> Result<DirectCallDeclaration, String> {
        let declarations = &self.type_info.declarations;
        let local_declarations = self.local_function_declarations.get(name);
        let Some(local_declarations) = local_declarations else {
            return Ok(DirectCallDeclaration {
                slots: declarations
                    .function_bindings
                    .get(name)
                    .map(|binding| binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: None,
                builtin: resolved_builtin(
                    name,
                    self.type_info.source_target(span).is_none()
                        && !declarations.function_bindings.contains_key(name)
                        && !declarations.function_overloads.contains_key(name),
                ),
                canonical: self.imported_callable_identity(name),
            });
        };
        let is_overloaded = local_declarations.len() > 1;

        if is_overloaded {
            let Some(selected) = self.type_info.selected_function_emitted_name(span) else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration was not resolved"
                ));
            };
            let selected_span = local_declarations.iter().find(|candidate_span| {
                declarations
                    .function_emitted_names
                    .get(&(candidate_span.start, candidate_span.end))
                    .is_some_and(|emitted| emitted == selected)
            });
            let Some(selected_span) = selected_span else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration could not be located"
                ));
            };
            let Some(binding) = declarations
                .function_bindings_by_span
                .get(&(selected_span.start, selected_span.end))
            else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration has no checked signature"
                ));
            };
            return Ok(DirectCallDeclaration {
                slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: Some(CompilerNodeId::declaration_span(
                    self.module_identity,
                    selected_span.start,
                    selected_span.end,
                )),
                builtin: None,
                // The identity anchors to the declaration span, so the selected overload is as nameable as any other
                // declaration; it is the *spelling* that cannot separate them, and the spelling is not the identity.
                canonical: Some(self.local_callable_identity(name, *selected_span)),
            });
        }

        let [declaration_span] = local_declarations.as_slice() else {
            return Err(format!(
                "direct call to `{name}` has no unambiguous same-module declaration identity"
            ));
        };
        let Some(binding) = declarations
            .function_bindings_by_span
            .get(&(declaration_span.start, declaration_span.end))
        else {
            return Err(format!(
                "same-module declaration `{name}` has no checked callable signature"
            ));
        };
        Ok(DirectCallDeclaration {
            slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id: Some(CompilerNodeId::declaration_span(
                self.module_identity,
                declaration_span.start,
                declaration_span.end,
            )),
            builtin: None,
            canonical: Some(self.local_callable_identity(name, *declaration_span)),
        })
    }

    /// Bind a call's arguments against a declared parameter surface, falling back to positional lowering when there
    /// is none to bind against.
    ///
    /// Shared by the direct-call and method paths so both treat an unresolved or rest-bearing signature the same
    /// way. A rest (`*args`/`**kwargs`) parameter means a written argument no longer corresponds one-to-one with a
    /// declared slot, so those calls keep lowering their arguments — refusing them would drop a delivered language
    /// capability — but record [`bir::ArgumentBinding::UnresolvedPositional`] rather than a slot map this stage did
    /// not compute. Spread arguments lower there, because a spread genuinely has no slot to bind to. A *named*
    /// argument with no spread beside it is still refused: its arity is perfectly well known, so binding it into a
    /// rest parameter is variadic-binding work this issue does not own.
    pub(super) fn bind_declared_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let has_rest = declared
            .as_ref()
            .is_some_and(|slots| slots.iter().any(|slot| slot.is_rest));
        let fixed_slots = declared.filter(|_| !has_rest);
        let Some(slots) = fixed_slots else {
            let elements = self.lower_spread_capable_args(args, scope, out);
            return Ok((elements, bir::ArgumentBinding::UnresolvedPositional));
        };
        // A spread whose shape the typechecker proved is an ordinary fixed-arity call in disguise: `add(*(1, 2))`
        // really is `add(1, 2)`. Expanding it here means it binds through the same declaration-slot planner as any
        // other call, instead of being pushed onto the runtime-arity path it does not belong on.
        let expanded: Vec<ast::CallArg> = args
            .iter()
            .flat_map(|arg| match expand_shaped_spread(self.type_info, arg) {
                Some(expansion) => expansion,
                None => vec![arg.clone()],
            })
            .collect();
        let planned = plan_declared_args(callee, &slots, &expanded)?;
        let (operands, binding) = self
            .lower_planned_args(&planned, slots.len(), scope, out)
            .map_err(|description| format!("{callee}: {description}"))?;
        Ok((fixed_elements(operands), binding))
    }

    /// Resolve a call site's explicit type arguments to semantic types, or describe why they cannot be represented.
    ///
    /// Explicit type arguments are part of a call's resolved identity, so Body IR takes the typechecker's
    /// monomorphized selection rather than re-lowering the written AST type nodes -- which is also the only way a
    /// `_` placeholder resolves to a real type instead of an unknown. A call that wrote type arguments the
    /// typechecker did not resolve is refused by name rather than represented with a guess.
    pub(super) fn call_site_type_arguments(
        &self,
        span: ast::Span,
        type_args: &[ast::Spanned<ast::Type>],
    ) -> Result<Vec<IncanType>, String> {
        if type_args.is_empty() {
            return Ok(Vec::new());
        }
        let Some(resolved) = self
            .type_info
            .calls
            .call_site_monomorph_type_args
            .get(&(span.start, span.end))
        else {
            return Err("call with unresolved explicit type arguments".to_string());
        };
        Ok(resolved.iter().map(semantic_type_from_resolved).collect())
    }

    /// Lower a `model`/`class` construction into a [`bir::AggregateKind::Constructor`] aggregate.
    ///
    /// Source-level construction is named-only, so the argument-to-field binding is the whole representation
    /// problem. Lowering consumes the typechecker's own recorded decision
    /// ([`TypeCheckInfo::constructor_field_binding`](crate::frontend::typechecker::TypeCheckInfo::constructor_field_binding))
    /// rather than re-resolving field aliases or rediscovering declared field order, both of which live in the
    /// symbol table this stage deliberately cannot reach. Operands are emitted in declared field order while the
    /// argument expressions are lowered in written source order, exactly as for a call.
    pub(super) fn lower_nominal_construction(
        &mut self,
        name: &str,
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let Some(field_binding) = self.type_info.constructor_field_binding(span).cloned() else {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        };

        // The typechecker records one slot per *written* argument, so a spread -- which supplies an unknown number
        // of fields -- can never appear in a recorded binding. Refuse it by name: no stage records how a spread maps
        // onto declared fields, and #1159's spread representation deliberately stopped short of construction layouts.
        let mut written_exprs = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) | ast::CallArg::Named(_, expr) => written_exprs.push(expr),
                ast::CallArg::PositionalUnpack(_) => {
                    return self.unsupported_operand(
                        format!("construction of `{name}` with a positional argument spread"),
                        scope,
                        hir_span_value,
                        out,
                    );
                }
                ast::CallArg::KeywordUnpack(_) => {
                    return self.unsupported_operand(
                        format!("construction of `{name}` with a keyword argument spread"),
                        scope,
                        hir_span_value,
                        out,
                    );
                }
            }
        }
        if written_exprs.len() != field_binding.argument_slots.len() {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let planned: Vec<(usize, &ast::Spanned<ast::Expr>)> = field_binding
            .argument_slots
            .iter()
            .copied()
            .zip(written_exprs)
            .collect();
        let (operands, binding) = match self.lower_planned_args(&planned, field_binding.field_count, scope, out) {
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
        // source-local models this Body-IR module retained. Preserve the selected declaration's identity and layout
        // together; imports, aliases, classes, generic models, and absent/malformed names retain neither fact, so a
        // direct executor can refuse at this construction span rather than guessing from `name`.
        let (direct_declaration_id, canonical_field_layout) = self
            .local_nominal_declarations
            .get(name)
            .filter(|declaration| declaration.fields.len() == field_binding.field_count)
            .map(|declaration| {
                (
                    Some(declaration.direct_declaration_id.clone()),
                    Some(declaration.fields.clone()),
                )
            })
            .unwrap_or((None, None));
        self.push_assign_temp(
            bir::Rvalue::Aggregate(
                bir::AggregateKind::Constructor(bir::ConstructorTarget {
                    name: name.to_string(),
                    direct_declaration_id,
                    canonical_field_layout,
                    binding,
                }),
                fixed_elements(operands),
            ),
            ty,
            scope,
            hir_span_value,
            out,
        )
    }

    /// Lower call arguments positionally, admitting spreads, for a call whose arity is not statically known.
    ///
    /// Every argument keeps its written form: a positional value, a named value, or a spread. None of them can be
    /// resolved to a declared slot here, because a spread supplies an unknown number of arguments at runtime —
    /// which is exactly why the resulting call records [`bir::ArgumentBinding::UnresolvedPositional`] rather than a
    /// slot map asserting a binding nobody checked. A name is preserved on its element rather than discarded, so a
    /// later consumer can still bind it once the arity is known.
    pub(super) fn lower_spread_capable_args(
        &mut self,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Vec<bir::ArgumentElement> {
        let mut elements = Vec::with_capacity(args.len());
        for arg in args {
            match arg {
                ast::CallArg::Positional(expr) => {
                    elements.push(bir::ArgumentElement::One(self.lower_expr_to_operand(expr, scope, out)));
                }
                ast::CallArg::PositionalUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Sequence, scope, out));
                }
                ast::CallArg::KeywordUnpack(source) => {
                    elements.push(self.lower_spread_element(source, bir::SpreadKind::Mapping, scope, out));
                }
                ast::CallArg::Named(name, expr) => {
                    let operand = self.lower_expr_to_operand(expr, scope, out);
                    elements.push(bir::ArgumentElement::Named {
                        name: name.clone(),
                        operand,
                    });
                }
            }
        }
        elements
    }

    /// Return the exact retained target for a qualified local fieldless normal-enum member, if safe to materialize.
    ///
    /// A bare type-name receiver and source-local registry membership are both required. This leaves ordinary forms
    /// not represented by the registry as generic field accesses that the direct executor visibly refuses, while
    /// preserving exact declaration identities for the one bounded unit-variant carrier profile.
    pub(super) fn local_fieldless_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
    ) -> Option<bir::FieldlessEnumVariantTarget> {
        let ast::Expr::Ident(enum_name) = &base.node else {
            return None;
        };
        if self.bindings.contains_key(enum_name)
            || !matches!(self.type_info.ident_kind(base.span), Some(IdentKind::TypeName))
        {
            return None;
        }
        let declaration = self.local_fieldless_enum_declarations.get(enum_name)?;
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        Some(bir::FieldlessEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Return the exact retained target for a qualified local RFC 032 value-enum member, if this spelling is safe to
    /// materialize directly.
    ///
    /// The source-local registry is deliberately the only lookup used here. A function-local binding wins over a
    /// same-spelling declaration, and any import, alias, ordinary enum, payload member, or behavior-bearing enum is
    /// absent from the registry. The resulting rvalue stores both declaration identities for runtime revalidation;
    /// it does not make the spelling itself an execution authority.
    pub(super) fn local_value_enum_variant_target(
        &self,
        base: &ast::Spanned<ast::Expr>,
        variant_name: &str,
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
        let declaration = self.local_value_enum_declarations.get(enum_name)?;
        let variant = declaration
            .variants
            .iter()
            .find(|variant| variant.name == variant_name)?;
        Some(bir::ValueEnumVariantTarget {
            enum_declaration_id: declaration.direct_declaration_id.clone(),
            variant_declaration_id: variant.direct_declaration_id.clone(),
            enum_name: declaration.name.clone(),
            variant_name: variant.name.clone(),
        })
    }

    /// Lower a call to a locally held callable value, a nominal construction, or a direct named function.
    ///
    /// A bare identifier that resolves to one of this body's locals is deliberately a
    /// [`bir::CallableTarget::Local`] call: it carries the local read's ownership fact, so a closure's lexical
    /// environment is not lost by pretending the identifier were a declaration. Its callable signature also
    /// enforces the stored value's fixed callable contract before any call arguments are lowered. An identifier the
    /// typechecker resolved to a `model`/`class` construction lowers to a constructor aggregate instead of a call
    /// (see [`Self::lower_nominal_construction`]) -- construction is not invocation, and representing it as a call
    /// would invite a consumer to execute it as one. Any other bare identifier remains a direct
    /// [`bir::Callee::Function`] call.
    ///
    /// Every one of those paths binds its arguments through the same [`plan_declared_args`] planner and records the
    /// result as a [`bir::ArgumentBinding`], so named, out-of-order, and defaulted spellings resolve identically
    /// regardless of how the callee was reached. A direct call whose signature the typechecker did not resolve
    /// (notably a builtin) still lowers its arguments faithfully, recording
    /// [`bir::ArgumentBinding::UnresolvedPositional`], and refuses only a named spelling it cannot bind without one.
    /// Argument spreads lower as [`bir::ArgumentElement::Spread`] elements, since a spread has no declared slot to
    /// bind to by construction. A non-identifier callee remains an explicit unsupported form; v0 has no
    /// dynamic-call-target node for it yet.
    pub(super) fn lower_call(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ast::Expr::Ident(name) = &callee.node else {
            return self.unsupported_operand("indirect call target".to_string(), scope, hir_span_value, out);
        };
        let name = name.clone();

        // A recorded constructor field binding is the typechecker's own statement that this spelling constructs a
        // nominal value, which is what distinguishes `P(x=1)` from a call to a function that happens to be named
        // `P`. A construction may carry call-site type arguments (`Box[int]()` is accepted), but the typechecker
        // records no monomorphization for them, and the constructed value's own type already carries the resolved
        // arguments -- so this deliberately does not duplicate them on the constructor target rather than claiming
        // construction cannot be generic.
        if self.type_info.constructor_field_binding(span).is_some() {
            return self.lower_nominal_construction(&name, args, span, scope, out);
        }

        // `Ok` and `Err` are intrinsic Result constructors, not ordinary direct calls. Retain that checked
        // distinction explicitly: a same-spelled source binding (a local callable, local function, or imported
        // target) must remain on the normal call path and refuse unless its own direct callable facts are available.
        // The direct runtime never resolves a constructor name dynamically.
        if !self.bindings.contains_key(&name)
            && !self.local_function_declarations.contains_key(&name)
            && self.type_info.source_target(span).is_none()
            && type_args.is_empty()
            && let Some(kind) = result_variant_kind(&name)
        {
            let result_ty = self.resolve_ty(span);
            let Some((ok_type, error_type)) = result_type_parts(&result_ty) else {
                return self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` without a resolved Result carrier"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let [ast::CallArg::Positional(payload)] = args else {
                return self.unsupported_operand(
                    format!("intrinsic Result constructor `{name}` requires one positional payload"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let payload = self.lower_expr_to_operand(payload, scope, out);
            return self.push_assign_temp(
                bir::Rvalue::ResultVariant(bir::ResultVariant {
                    kind,
                    payload,
                    ok_type: ok_type.clone(),
                    error_type: error_type.clone(),
                }),
                result_ty,
                scope,
                hir_span_value,
                out,
            );
        }

        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        if let Some(&local) = self.bindings.get(&name) {
            let local_ty = self.locals[local.index()].ty.clone();
            let IncanType::Function { params, return_type: _ } = local_ty else {
                return self.unsupported_operand(
                    format!("call to non-callable local `{name}`"),
                    scope,
                    hir_span_value,
                    out,
                );
            };
            let slots: Vec<DeclaredSlot> = params.iter().map(DeclaredSlot::from_semantic_param).collect();
            let planned = match plan_declared_args(&format!("local callable `{name}`"), &slots, args) {
                Ok(planned) => planned,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };

            // Source evaluation observes the callable value before its arguments. The target read also performs the
            // one ownership/last-use decision for that lexical environment, which `CallableTarget::Local` preserves
            // for a later executor instead of re-deriving it from the local's source spelling.
            let place = bir::Place::from_local(local);
            let (fact, last_use) = self.ownership_fact_for_place(&place, &self.locals[local.index()].ty.clone());
            let (operands, binding) = match self.lower_planned_args(&planned, slots.len(), scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };
            let callee = bir::Callee::Function(bir::CallableTarget::Local(bir::LocalCallableTarget {
                operand: bir::PlaceOperand { place, fact, last_use },
                binding,
            }));
            let ty = self.resolve_ty(span);
            return self.push_call_temp(callee, fixed_elements(operands), ty, scope, hir_span_value, false, out);
        }

        // A name that resolves to a nominal type but has no recorded field binding is a construction the checker
        // declined to bind (a duplicate or unknown field). Refusing it as a call to an unknown function would name
        // the wrong construct entirely.
        if self.type_info.declarations.class_layouts.contains_key(&name)
            || self.type_info.declarations.model_field_visibilities.contains_key(&name)
        {
            return self.unsupported_operand(
                format!("construction of `{name}` with an unresolved field layout"),
                scope,
                hir_span_value,
                out,
            );
        }

        let declaration = match self.declared_slots_for_direct_call(&name, span) {
            Ok(declaration) => declaration,
            Err(description) => {
                return self.unsupported_operand(description, scope, hir_span_value, out);
            }
        };

        // A provider operation is selected by the canonical identity this call already resolved to, never by the
        // spelling written here (#1213). The lookup therefore sits after resolution rather than in place of it, and
        // a callee with no proven identity finds nothing rather than being guessed at by name.
        if let Some(operation) = declaration.canonical.clone()
            && let Some(record) = self.provider_operation_record(&operation)
        {
            return self.lower_provider_operation(&name, &operation, record, declaration, args, span, scope, out);
        }

        let (operands, binding) =
            match self.bind_declared_args(&format!("function `{name}`"), declaration.slots, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };

        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
                name,
                direct_call_id: declaration.direct_call_id,
                canonical: declaration.canonical,
                builtin: declaration.builtin,
                type_args: resolved_type_args,
                binding,
            })),
            operands,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }

    /// Return the typechecker's callable type for a closure or local partial value that Body IR constructs itself.
    ///
    /// Local partials use the typechecker's canonical full signature with overrideable preset-default slots, so the
    /// binding, its [`bir::Rvalue::Closure`], and a later [`Self::lower_call`] share one arity/default contract.
    pub(super) fn callable_value_ty(&self, expr: &ast::Spanned<ast::Expr>) -> Option<IncanType> {
        match &expr.node {
            ast::Expr::Closure(_, _) | ast::Expr::Partial(_) => Some(self.resolve_ty(expr.span)),
            _ => None,
        }
    }

    /// Lower a method call `recv.name(args)` to a [`bir::Callee::Method`] call, with the receiver prepended to
    /// `args[0]` as a [`bir::OwnershipFact::Borrow`] operand (see the inline comment on the receiver-borrow decision
    /// below).
    ///
    /// Argument binding goes through the same [`plan_declared_args`] planner every other call shape uses, against
    /// the typechecker's own rest-aware call-site signature for this span -- which already has the receiver's
    /// generic arguments substituted, so a generic method's slots are concrete here. The receiver is deliberately
    /// outside the recorded binding: its slots index the method's declared parameters, so a consumer reads
    /// `args[0]` as the receiver and `args[1..]` as the bound arguments. A method call whose signature the
    /// typechecker did not record still lowers positional arguments faithfully and refuses only the spellings it
    /// cannot bind — a named spelling with no spread beside it — matching [`Self::lower_call`]'s treatment of an
    /// unresolved direct callee. Spread arguments lower here too, after the receiver.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_method_call(
        &mut self,
        recv: &ast::Spanned<ast::Expr>,
        name: &str,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(_) => {
                return self.unsupported_operand(
                    "method call with unresolved explicit type arguments".to_string(),
                    scope,
                    hir_span_value,
                    out,
                );
            }
        };

        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());

        // The receiver is read before the arguments, matching source evaluation order: `recv.m(f())` observes the
        // receiver place first. Method receivers are treated as borrowed rather than moved/cloned, mirroring how the
        // existing Rust-emission backend's ownership planner treats most method receivers
        // (`src/backend/ir/ownership.rs`) -- see this module's rustdoc for the full precedent discussion.
        let receiver_operand = if let ast::Expr::Field(base, member) = &recv.node
            && self.local_value_enum_variant_target(base, member).is_some()
        {
            self.lower_expr_to_operand(recv, scope, out)
        } else {
            let recv_place = self.lower_expr_to_place(recv, scope, out);
            bir::Operand::place(recv_place, bir::OwnershipFact::Borrow, false)
        };

        let (mut arg_operands, binding) =
            match self.bind_declared_args(&format!("method `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => {
                    return self.unsupported_operand(description, scope, hir_span_value, out);
                }
            };

        // The receiver is `args[0]` and is never spliced, so it is always a single-value element.
        let mut call_args = Vec::with_capacity(arg_operands.len() + 1);
        call_args.push(bir::ArgumentElement::One(receiver_operand));
        call_args.append(&mut arg_operands);
        let ty = self.resolve_ty(span);
        self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                name: name.to_string(),
                type_args: resolved_type_args,
                binding,
            }),
            call_args,
            ty,
            scope,
            hir_span_value,
            false,
            out,
        )
    }
}

/// Resolve a called name to the canonical builtin it names, when nothing in source has taken that spelling.
///
/// The spelling comes from `incan_core`'s builtin registry rather than a literal, so a builtin added there is
/// recognized here without a second edit — the same single-source rule the collection and constructor registries
/// already carry. Body IR previously recognized exactly one builtin, `range`, by comparing the name against the
/// string `"range"`. That both duplicated the registry and left every other builtin arriving as an ordinary named
/// call, indistinguishable to a consumer from a user-declared function of the same name.
///
/// `unshadowed` carries the caller's proof that the name is not a declared, imported, or overloaded function. A
/// module that declares its own `print` means that declaration, and resolving the builtin there would silently
/// redirect the call to something the source never named.
fn resolved_builtin(name: &str, unshadowed: bool) -> Option<incan_core::lang::builtins::BuiltinFnId> {
    unshadowed.then(|| incan_core::lang::builtins::from_str(name)).flatten()
}
