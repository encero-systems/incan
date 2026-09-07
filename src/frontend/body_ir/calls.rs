//! Lowering for calls: argument planning and binding, callable identity, nominal construction, method calls.

use super::args::*;
use super::primitives::*;
use super::*;
use incan_core::lang::builtins::BuiltinFnId;

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
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Result<DirectCallDeclaration, String> {
        let declarations = &self.type_info.declarations;
        let local_declarations = self.local_function_declarations.get(name);
        let Some(local_declarations) = local_declarations else {
            let canonical = self.type_info.resolved_identity(callee_span).cloned();
            let builtin = self.type_info.resolved_builtin_call(call_span);
            let canonical_builtin_id = canonical.as_ref().and_then(canonical_builtin);
            if builtin != canonical_builtin_id && (builtin.is_some() || canonical_builtin_id.is_some()) {
                return Err(format!(
                    "checked builtin call `{name}` does not retain its canonical builtin identity"
                ));
            }
            return Ok(DirectCallDeclaration {
                slots: declarations
                    .function_bindings
                    .get(name)
                    .map(|binding| binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: None,
                builtin,
                canonical,
            });
        };
        let is_overloaded = local_declarations.len() > 1;

        if is_overloaded {
            let Some(selected) = self.type_info.resolved_identity(callee_span) else {
                return Err(format!(
                    "call to overloaded function `{name}` without a resolved canonical declaration"
                ));
            };
            let selected_span = local_declarations.iter().find(|candidate_span| {
                declarations
                    .function_bindings_by_span
                    .get(&(candidate_span.start, candidate_span.end))
                    .and_then(|binding| binding.identity.as_ref())
                    .is_some_and(|identity| identity == selected)
            });
            let Some(selected_span) = selected_span else {
                return Err(format!(
                    "call to overloaded function `{name}` whose canonical declaration is not present in this module"
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
                // Consumed from the declaration's checked binding — minted once by the typechecker at the
                // declaration site, never re-derived here from module path plus spelling.
                canonical: binding.identity.clone(),
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
        let canonical = self.type_info.resolved_identity(callee_span).cloned();
        if canonical.as_ref() != binding.identity.as_ref() {
            return Err(format!(
                "direct call to `{name}` does not retain the selected canonical declaration"
            ));
        }
        Ok(DirectCallDeclaration {
            slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id: Some(CompilerNodeId::declaration_span(
                self.module_identity,
                declaration_span.start,
                declaration_span.end,
            )),
            builtin: None,
            // Consumed from the declaration's checked binding — minted once by the typechecker at the declaration
            // site, never re-derived here from module path plus spelling.
            canonical,
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
        callee_span: ast::Span,
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
        let canonical = self.type_info.resolved_identity(callee_span).cloned();
        let (direct_declaration_id, canonical_field_layout) = self
            .local_nominal_declarations
            .get(name)
            .filter(|declaration| {
                declaration.fields.len() == field_binding.field_count
                    && canonical.as_ref() == Some(&declaration.canonical)
            })
            .map(|declaration| {
                (
                    Some(declaration.direct_declaration_id.clone()),
                    Some(declaration.fields.clone()),
                )
            })
            .unwrap_or((None, None));
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
                        name: name.node.clone(),
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
        let declaration = self.local_fieldless_enum_declarations.get(enum_name)?;
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
        let declaration = self.local_value_enum_declarations.get(enum_name)?;
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
    fn lower_checked_isinstance_call(
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
            bir::Rvalue::IsInstance {
                value,
                value_ty,
                target: bir::IsInstanceTarget {
                    ty: semantic_type_from_resolved(&target.ty),
                    canonical: target.canonical.clone(),
                    span: target_span,
                },
            },
            ty,
            scope,
            call_span,
            out,
        )
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
        if self.type_info.resolved_builtin_call(span) == Some(BuiltinFnId::IsInstance) {
            return self.lower_checked_isinstance_call(type_args, args, span, scope, out);
        }
        let ast::Expr::Ident(name) = &callee.node else {
            return self.unsupported_operand("indirect call target".to_string(), scope, hir_span_value, out);
        };
        let name = name.clone();

        // A retained zero-argument constructor fact distinguishes builtin construction from a same-spelled source
        // callable. Only empty Set/Dict construction is admitted here; iterable conversions stay on their existing
        // path.
        if args.is_empty()
            && type_args.is_empty()
            && let Some(constructor) = self.type_info.resolved_collection_constructor(span)
        {
            match constructor {
                CollectionTypeId::Set => {
                    return self.lower_aggregate(bir::AggregateKind::Set, &[], span, scope, out);
                }
                CollectionTypeId::Dict => return self.lower_dict(&[], span, scope, out),
                _ => {}
            }
        }

        // A recorded constructor field binding is the typechecker's own statement that this spelling constructs a
        // nominal value, which is what distinguishes `P(x=1)` from a call to a function that happens to be named
        // `P`. A construction may carry call-site type arguments (`Box[int]()` is accepted), but the typechecker
        // records no monomorphization for them, and the constructed value's own type already carries the resolved
        // arguments -- so this deliberately does not duplicate them on the constructor target rather than claiming
        // construction cannot be generic.
        if self.type_info.constructor_field_binding(span).is_some() {
            return self.lower_nominal_construction(&name, args, span, callee.span, scope, out);
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

        let declaration = match self.declared_slots_for_direct_call(&name, callee.span, span) {
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

    /// Lower a method-shaped explicit builtin call from the typechecker's retained builtin selection.
    ///
    /// `std.builtins.len(value)` reaches the AST as a method call, but the namespace is not a runtime receiver. The
    /// typechecker records the selected [`BuiltinFnId`] at the full call span. Consuming that closed fact here keeps
    /// the call distinct from a user-defined function or method with the same spelling and avoids evaluating the
    /// namespace as a value.
    #[allow(clippy::too_many_arguments)]
    fn lower_checked_builtin_method_call(
        &mut self,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        let builtin = self.type_info.resolved_builtin_call(span)?;
        if builtin == BuiltinFnId::IsInstance {
            return None;
        }
        let call_span = hir_span(span);
        let Some(canonical) = crate::frontend::symbols::canonical_builtin_function_identity(builtin) else {
            return Some(self.unsupported_operand(
                "checked builtin call has no canonical registry identity".to_string(),
                scope,
                call_span,
                out,
            ));
        };
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => return Some(self.unsupported_operand(description, scope, call_span, out)),
        };
        let declared = self
            .type_info
            .call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let display_name = canonical.declaration_name.clone();
        let (operands, binding) =
            match self.bind_declared_args(&format!("builtin `{display_name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => return Some(self.unsupported_operand(description, scope, call_span, out)),
            };
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
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

    /// Lower `module.function(args)` as a direct call to the declaration its receiver qualifies.
    ///
    /// A module binding is a namespace, not a value. `identity_provider.imported_path(17)` reaches the AST as a
    /// method call, but there is no receiver to read, and treating the qualifier as a place refuses at a name that
    /// never named one. The typechecker has already resolved this call to exactly one declaration; consuming that
    /// identity keeps the call on the direct path instead of re-deriving a target from the two spellings the source
    /// happens to join with a dot. The receiver's own identity decides whether this applies, so a local variable
    /// that happens to share a module's name still lowers as an ordinary method call.
    ///
    /// Returns `None` when the receiver is not a module, leaving every other receiver shape untouched.
    #[allow(clippy::too_many_arguments)]
    fn lower_module_qualified_call(
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
        let resolved_type_args = match self.call_site_type_arguments(span, type_args) {
            Ok(resolved_type_args) => resolved_type_args,
            Err(description) => return Some(self.unsupported_operand(description, scope, hir_span_value, out)),
        };
        let declared: Option<Vec<DeclaredSlot>> = self
            .type_info
            .call_site_callable_params(span)
            .map(|params| params.iter().map(DeclaredSlot::from_checked_param).collect());
        let (operands, binding) =
            match self.bind_declared_args(&format!("function `{name}`"), declared, args, scope, out) {
                Ok(bound) => bound,
                Err(description) => return Some(self.unsupported_operand(description, scope, hir_span_value, out)),
            };
        let ty = self.resolve_ty(span);
        Some(self.push_call_temp(
            // The declaration lives in another module, so it has no span identity in this one. `canonical` is the
            // fact that survives the boundary, and it is the only one a consumer may dispatch on here.
            bir::Callee::Function(bir::CallableTarget::Named(bir::NamedCallableTarget {
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
        if self.type_info.resolved_builtin_call(span) == Some(BuiltinFnId::IsInstance) {
            return self.lower_checked_isinstance_call(type_args, args, span, scope, out);
        }
        if let Some(lowered) = self.lower_checked_builtin_method_call(type_args, args, span, scope, out) {
            return lowered;
        }
        if let Some(lowered) = self.lower_module_qualified_call(recv, name, type_args, args, span, scope, out) {
            return lowered;
        }
        if self.type_info.resolved_identity(recv.span).is_some_and(|identity| {
            identity.namespace == incan_semantics_core::SymbolNamespace::OrdinaryLexical
                && matches!(
                    &identity.kind,
                    SemanticSourceTargetKind::Model
                        | SemanticSourceTargetKind::Class
                        | SemanticSourceTargetKind::Newtype
                        | SemanticSourceTargetKind::Rusttype
                        | SemanticSourceTargetKind::Enum
                        | SemanticSourceTargetKind::TypeAlias
                        | SemanticSourceTargetKind::Trait
                )
        }) {
            return self.unsupported_operand(
                format!("static member `{name}` on a type has no Body IR value representation"),
                scope,
                hir_span_value,
                out,
            );
        }
        let helper = match self.checked_string_helper_for_method_call(recv, name, span) {
            Ok(helper) => helper,
            Err(description) => return self.unsupported_operand(description, scope, hir_span_value, out),
        };
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
            && self.local_value_enum_variant_target(base, member, recv.span).is_some()
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
        if let Some(helper) = helper {
            self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_string()));
            self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
            return self.push_call_temp(
                bir::Callee::Helper(helper),
                call_args,
                ty,
                scope,
                hir_span_value,
                false,
                out,
            );
        }
        self.push_call_temp(
            bir::Callee::Method(bir::MethodTarget {
                name: name.to_string(),
                canonical: self.type_info.resolved_identity(span).cloned(),
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

    /// Return the helper operation selected by a checked string-method call, or a source-span-preserving refusal.
    ///
    /// Body IR only maps the retained [`StringMethodId`](incan_core::lang::surface::string_methods::StringMethodId)
    /// to a [`bir::HelperOp`]. The source registry is consulted solely to validate that an admitted spelling still
    /// agrees with the checked identity, so a missing or corrupted fact cannot turn a raw method name into runtime
    /// dispatch.
    fn checked_string_helper_for_method_call(
        &self,
        receiver: &ast::Spanned<ast::Expr>,
        name: &str,
        span: ast::Span,
    ) -> Result<Option<bir::HelperOp>, String> {
        let checked = self.type_info.resolved_string_helper_call(span);
        let runtime_string_receiver = matches!(
            self.resolve_ty(receiver.span),
            IncanType::Primitive(IncanPrimitiveType::Str)
        );
        if !runtime_string_receiver {
            return match checked {
                Some(_) => Err("checked string helper identity has a non-string receiver".to_string()),
                None => Ok(None),
            };
        }
        let source = incan_core::lang::surface::string_methods::from_str(name);
        match checked {
            Some(method) => {
                let Some(helper) = bir::HelperOp::for_selected_string_method(method) else {
                    return Err(format!("unadmitted checked string helper identity `{method:?}`"));
                };
                if source != Some(method) {
                    return Err("checked string helper identity does not match the source method".to_string());
                }
                Ok(Some(helper))
            }
            None if source.and_then(bir::HelperOp::for_selected_string_method).is_some() => {
                Err("selected string helper call is missing its checked string helper identity".to_string())
            }
            None => Ok(None),
        }
    }
}

/// Project a compiler-owned builtin id only from its retained RFC 120 identity.
///
/// The call-site spelling is deliberately absent from this function. A same-spelled source declaration, import, or
/// alias cannot become a builtin by textual resemblance, while a canonical builtin alias retains the registry's
/// declaration name and therefore still projects to the same [`BuiltinFnId`].
fn canonical_builtin(identity: &CanonicalSymbolId) -> Option<incan_core::lang::builtins::BuiltinFnId> {
    (identity.origin == incan_semantics_core::SymbolOrigin::Builtin
        && identity.namespace == incan_semantics_core::SymbolNamespace::OrdinaryLexical
        && identity.kind == SemanticSourceTargetKind::Builtin
        && identity.scope_discriminant.is_none()
        && identity.declaration_span == HirSourceSpan::new(0, 0))
    .then(|| incan_core::lang::builtins::from_str(&identity.declaration_name))
    .flatten()
}
