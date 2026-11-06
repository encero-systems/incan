//! Argument planning and binding: borrows for `mut` parameters, planned and declared argument slots, and factory
//! arguments.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Keep the source trait instantiation independently of the method's own generic header. Synthetic calls with
    /// no retained dispatch stay unspecified and may select only an otherwise unambiguous implementation.
    pub(in crate::body_ir) fn checked_trait_arguments(
        &self,
        dispatch: Option<&crate::typechecker::ResolvedMethodDispatch>,
    ) -> Option<Vec<IncanType>> {
        let crate::typechecker::ResolvedMethodDispatch::Trait { type_args, .. } = dispatch?;
        Some(type_args.iter().map(|argument| self.checked_type(argument)).collect())
    }

    /// Materialize a native owned argument's planned copy before later arguments can transfer its original owner.
    /// Borrowing display and formatting operations never acquire an owned snapshot from this boundary rule.
    pub(super) fn retain_owned_native_argument(
        &mut self,
        expression: &ast::Spanned<ast::Expr>,
        value: bir::Operand,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let ty = self.resolve_ty(expression.span);
        let IncanType::Named(name) = &ty else { return value };
        let bir::Operand::Place(read) = &value else {
            return value;
        };
        if read.fact != bir::OwnershipFact::Clone
            || bir::rust_item_identity(name).is_none()
            || !self.type_info.rust.storage_types.values().any(|storage| storage == &ty)
        {
            return value;
        }
        self.push_assign_temp(bir::Rvalue::Use(value), ty, scope, hir_span(expression.span), out)
    }

    /// Project only the checked core ValueError function into its existing runtime operation. A foreign or shadowed
    /// spelling grants nothing; metadata-checked imports instead require the private validated diverging-call path
    /// because their legacy reference identity is absent. Unsupported arguments retain the ordinary refusal path. No
    /// unknown result local is allocated because the core function diverges. Legacy checker facts and emission
    /// remain unchanged (#1698).
    pub(super) fn lower_diverging_error_helper(
        &mut self,
        callee: &ast::Spanned<ast::Expr>,
        type_args: &[ast::Spanned<ast::Type>],
        args: &[ast::CallArg],
        span: ast::Span,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Operand> {
        if !type_args.is_empty() {
            return None;
        }
        let path = match self.type_info.resolved_identity(callee.span) {
            Some(identity) => {
                let SymbolOrigin::RustCrate(path) = &identity.origin else {
                    return None;
                };
                path
            }
            None if self.type_info.ident_kind(callee.span) == Some(IdentKind::RustImport) => {
                self.type_info.rust.body_ir_diverging_import_calls.get(&span)?
            }
            None => return None,
        };
        if !path.iter().map(String::as_str).eq([
            incan_lang::lang::stdlib::facets::CORE,
            incan_lang::lang::stdlib::INCAN_STD_ERRORS_MODULE,
            "raise_value_error",
        ]) {
            return None;
        }
        let [ast::CallArg::Positional(message)] = args else {
            return None;
        };
        if self.resolve_ty(message.span) != IncanType::Primitive(IncanPrimitiveType::Str) {
            return None;
        }
        let message = self.lower_expr_to_operand(message, scope, out);
        let helper = bir::HelperOp::RaiseValueError;
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().to_owned()));
        out.push(bir::Statement {
            kind: bir::StatementKind::Call {
                destination: None,
                callee: bir::Callee::Helper(helper),
                args: vec![bir::ArgumentElement::One(message)],
                may_panic: true,
            },
            span: hir_span(span),
        });
        Some(bir::Operand::Constant(bir::Constant::Unit))
    }

    /// The type a `Type.member(..)` call is made on, when its receiver is a type rather than a value.
    ///
    /// A bare identifier follows [`named_static_receiver_type`]. An explicitly instantiated nominal (`Box[int]`) is
    /// the checker's applied type for the whole receiver, so an associated call closes the owner's type parameters at
    /// exactly those arguments; an application the checker did not type as that nominal is a value receiver.
    /// Qualified generic heads must agree with the receiver's checked declaration identity, not its source spelling.
    /// A checked factory's retained owner closes a bare receiver even when its return value wraps Self in a Result.
    pub(in crate::body_ir) fn static_receiver_type(
        &self,
        recv: &ast::Spanned<ast::Expr>,
        call_span: ast::Span,
    ) -> Option<IncanType> {
        if let Some(ty) = self
            .type_info
            .body_ir_newtypes
            .static_receiver_type(call_span, self.type_info)
        {
            return Some(self.checked_type(ty));
        }
        match &recv.node {
            ast::Expr::Ident(name) => {
                let identity = self.type_info.resolved_identity(recv.span)?;
                let checked = named_static_receiver_type(name, identity)?;
                if let Some(factory_owner) = self
                    .type_info
                    .calls
                    .body_ir_factory_owner_types
                    .get(&call_span)
                    .map(|ty| self.checked_type(ty))
                    && let IncanType::Generic { base, .. } = &factory_owner
                    && parameter_types::retained_nominal_identity(base, self.type_info) == Some(identity)
                {
                    return Some(factory_owner);
                }
                if let Some(ty @ IncanType::Generic { base, .. }) = self.type_overrides.get(&recv.span)
                    && parameter_types::retained_nominal_identity(base, self.type_info) == Some(identity)
                {
                    return Some(ty.clone());
                }
                Some(self.type_info.qualify_semantic_type(checked))
            }
            ast::Expr::Index(owner, _) => {
                let ast::Expr::Ident(name) = &owner.node else {
                    return None;
                };
                let identity = self.type_info.resolved_identity(owner.span)?;
                named_static_receiver_type(name, identity)?;
                match self.resolve_ty(recv.span) {
                    IncanType::Generic { base, args }
                        if parameter_types::retained_nominal_identity(&base, self.type_info) == Some(identity) =>
                    {
                        Some(IncanType::Generic { base, args })
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Rewrite an argument bound to a `mut` parameter (#2022). A `mut` parameter of non-`Copy` type writes through to
    /// the caller, as the emitted route's `&mut T` does, so the argument is borrowed mutably instead of copied or
    /// moved: the caller keeps owning it (an operand that had selected a move is un-moved, so the caller still
    /// drops it) and sees the callee's changes. A `Copy` argument stays a copy, matching the emitted route's
    /// by-value `mut` scalar, and a compiler temporary keeps its move, since no caller can observe writes to it.
    pub(in crate::body_ir) fn borrow_for_mut_parameter(&mut self, operand: bir::Operand) -> bir::Operand {
        let bir::Operand::Place(mut source) = operand else {
            return operand;
        };
        let is_temporary = source
            .place
            .local_id()
            .and_then(|local| self.locals.get(local.index()))
            .is_some_and(|decl| matches!(decl.origin, bir::LocalOrigin::Temporary));
        if is_temporary
            || !matches!(
                source.fact,
                bir::OwnershipFact::Move | bir::OwnershipFact::Clone | bir::OwnershipFact::Borrow
            )
        {
            return bir::Operand::Place(source);
        }
        self.retain_argument_borrow(&mut source, true);
        bir::Operand::Place(source)
    }

    /// Retain a checked reference boundary's owner and passing mode without copying or snapshotting the referent.
    /// The caller selects shared versus exclusive from its declared parameter or binding type, including for foreign
    /// receivers.
    pub(in crate::body_ir) fn retain_argument_borrow(&mut self, read: &mut bir::PlaceOperand, mutable: bool) {
        if read.fact == bir::OwnershipFact::Move
            && read.place.projection.is_empty()
            && let Some(local) = read.place.local_id()
        {
            self.moved_out.remove(&local);
        }
        read.fact = if mutable {
            bir::OwnershipFact::MutBorrow
        } else {
            bir::OwnershipFact::Borrow
        };
        read.last_use = false;
    }

    /// Retain the caller's owner when a checked method-chain input passes its shared receiver reference.
    pub(super) fn share_argument_slots(
        &mut self,
        operands: &mut [bir::ArgumentElement],
        binding: &bir::ArgumentBinding,
        shared: &[usize],
    ) {
        let bir::ArgumentBinding::Resolved { arguments, .. } = binding else {
            return;
        };
        for (element, argument) in operands.iter_mut().zip(arguments) {
            if !shared.contains(&argument.slot) {
                continue;
            }
            let bir::ArgumentElement::One(bir::Operand::Place(read)) = element else {
                continue;
            };
            self.retain_argument_borrow(read, false);
        }
    }

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
    ///
    /// `mut_slots` marks, by declaration slot, which parameters are declared `mut`; an operand for such a slot goes
    /// through [`Self::borrow_for_mut_parameter`]. A slot past its end, or an empty slice for a surface with no
    /// declared parameters, is treated as not `mut`. `slot_types` retains numeric carrier/member conversions at checked
    /// by-value parameter boundaries; absent slot types use the recorded value destination, as constructor fields do.
    pub(in crate::body_ir) fn lower_planned_args(
        &mut self,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        slot_count: usize,
        mut_slots: &[bool],
        slot_types: &[IncanType],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::Operand>, bir::ArgumentBinding), String> {
        self.lower_planned_args_with_declaration_order(planned, slot_count, mut_slots, slot_types, scope, out, false)
    }

    /// Evaluate a checked binding in its recorded order, preserving source coordinates separately from ownership.
    ///
    /// Ordinary fixed named free calls and authenticated imported SDK factories retain the frozen emitter's
    /// declaration order. Other callers retain written order. Ownership decisions follow the actual visit order;
    /// original source positions remain provenance rather than an instruction to reorder materialized values.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep declaration slot facts separate from mutable lowering outputs, matching the adjacent argument-lowering helpers."
    )]
    pub(super) fn lower_planned_args_with_declaration_order(
        &mut self,
        planned: &[(usize, &ast::Spanned<ast::Expr>)],
        slot_count: usize,
        mut_slots: &[bool],
        slot_types: &[IncanType],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
        declaration_order: bool,
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
        let evaluation = declaration_order.then(|| {
            let mut order = planned.iter().enumerate().collect::<Vec<_>>();
            order.sort_by_key(|(_, (slot, _))| *slot);
            order
        });
        let pending_start = self.pending_argument_reads.len();
        for position in 0..planned.len() {
            let (written_position, (slot, expr)) = match &evaluation {
                Some(order) => order[position],
                None => (position, &planned[position]),
            };
            let reference = match slot_types.get(*slot) {
                Some(IncanType::Ref(_)) => Some(false),
                Some(IncanType::RefMut(_)) => Some(true),
                _ => None,
            };
            let mut operand = if let Some(adjusted) = self.lower_adjusted_argument(expr, scope, out) {
                adjusted?
            } else if reference.is_some() || mut_slots.get(*slot).copied().unwrap_or(false) {
                self.lower_expr_to_operand(expr, scope, out)
            } else {
                self.lower_boundary_value(expr, slot_types.get(*slot), scope, out)
            };
            if let Some(mutable) = reference {
                if let Some(IncanType::Ref(referent) | IncanType::RefMut(referent)) = slot_types.get(*slot) {
                    operand = self.materialize_argument_loans(expr, operand, referent, scope, out);
                }
                if let bir::Operand::Place(read) = &mut operand {
                    self.retain_argument_borrow(read, mutable);
                }
            } else if mut_slots.get(*slot).copied().unwrap_or(false) {
                operand = self.borrow_for_mut_parameter(operand);
            } else {
                operand = self.owned_parameter_operand(expr, operand);
                operand = self.retain_owned_native_argument(expr, operand, scope, out);
                operand = self.retain_evaluated_value_read(expr, operand, scope, out);
            }
            self.retain_pending_operand_read(&operand);
            if let Some(entry) = lowered.get_mut(*slot) {
                *entry = Some((operand, written_position));
            }
        }
        self.pending_argument_reads.truncate(pending_start);

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
        let binding = if declaration_order {
            bir::ArgumentBinding::ResolvedDeclarationOrder {
                arguments,
                defaulted_slots,
            }
        } else {
            bir::ArgumentBinding::Resolved {
                arguments,
                defaulted_slots,
            }
        };
        Ok((operands, binding))
    }

    /// Keep a local owner's deferred operand read live until the enclosing operation is fully evaluated.
    /// Calls, aggregates, dictionary writes, and comprehensions share this reservation. Copy scalars and hidden
    /// temporary values need none. The caller restores its stack boundary, preserving reservations made by an
    /// enclosing operation.
    pub(in crate::body_ir) fn retain_pending_operand_read(&mut self, operand: &bir::Operand) {
        let bir::Operand::Place(read) = operand else {
            return;
        };
        let Some(local) = read.place.local_id() else {
            return;
        };
        let Some(declaration) = self.locals.get(local.index()) else {
            return;
        };
        if matches!(declaration.origin, bir::LocalOrigin::Temporary)
            || declaration.ty.abi_v0_facts().ownership.is_trivially_copy()
        {
            return;
        }
        self.pending_argument_reads.push(local);
    }

    /// Bind only a checked imported SDK factory, with declaration-order effects and ownership from the frozen route.
    ///
    /// Declared slots and the canonical factory selection are already authenticated by the caller. Dynamic or rest
    /// arguments cannot substitute for that surface, so unsupported shapes stay refused before evaluation.
    pub(super) fn bind_sdk_factory_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let slots = declared.ok_or("checked SDK factory without retained parameter slots")?;
        if slots.iter().any(|slot| slot.kind != IncanCallableParamKind::Normal) {
            return Err("unsupported SDK factory with rest parameters".to_owned());
        }
        let planned = plan_declared_args(callee, &slots, args)?;
        let mutable = slots.iter().map(|slot| slot.is_mut).collect::<Vec<_>>();
        let types = slots.iter().map(|slot| slot.ty.clone()).collect::<Vec<_>>();
        let (operands, binding) =
            self.lower_planned_args_with_declaration_order(&planned, slots.len(), &mutable, &types, scope, out, true)?;
        Ok((fixed_elements(operands), binding))
    }

    /// Resolve the declaration surface and exact local identity for a direct named call.
    ///
    /// A direct declaration identity names only an executable body physically represented by this module. Imports
    /// retain their selected canonical identity and checked signature for project assembly to resolve elsewhere.
    /// Unresolved targets stay explicit refusals. Every unshadowed core builtin has a registry identity; individual
    /// consumers still admit only their documented subset, such as `range` for counting-loop lowering.
    ///
    /// Overloads are why this is resolved per call site rather than per name. `function_bindings` is keyed by bare
    /// source name, so for two same-name declarations it holds only one of them; binding a call against the wrong
    /// overload's parameter *names* would silently reorder its arguments, turning an honest refusal into a wrong
    /// answer. The typechecker already records which overload it selected for this call span, so this follows that
    /// decision to the declaration and reads that declaration's own signature. If a name is overloaded but no
    /// selection was recorded, this fails closed rather than picking one.
    pub(in crate::body_ir) fn declared_slots_for_direct_call(
        &self,
        name: &str,
        callee_span: ast::Span,
        call_span: ast::Span,
    ) -> Result<DirectCallDeclaration, String> {
        let declarations = &self.type_info.declarations;
        // A same-module alias keeps the selected function's identity; use that declaration's retained spans rather
        // than treating the alias spelling as an imported function with no executable body.
        let selected = self.type_info.resolved_identity(callee_span);
        let declaration_name = selected
            .filter(|identity| {
                matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Function | SemanticSourceTargetKind::Partial
                )
            })
            .filter(|identity| {
                incan_semantics_core::canonical_module_identity(identity).as_deref() == Some(self.module_identity)
            })
            .map_or(name, |identity| identity.declaration_name.as_str());
        let local_declarations = self.local_function_declarations.get(declaration_name);
        let Some(local_declarations) = local_declarations else {
            return self.external_call_declaration(name, callee_span, call_span);
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
                    .get(&candidate_span)
                    .and_then(|binding| binding.identity.as_ref())
                    .is_some_and(|identity| identity == selected)
            });
            let Some(selected_span) = selected_span else {
                return Err(format!(
                    "call to overloaded function `{name}` whose canonical declaration is not present in this module"
                ));
            };
            let Some(binding) = declarations.function_bindings_by_span.get(&selected_span) else {
                return Err(format!(
                    "call to overloaded function `{name}` whose selected declaration has no checked signature"
                ));
            };
            return Ok(DirectCallDeclaration {
                is_async: binding.is_async,
                slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
                direct_call_id: Some(CompilerNodeId::declaration_at(self.module_identity, *selected_span)),
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
        let Some(binding) = declarations.function_bindings_by_span.get(&declaration_span) else {
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
            is_async: binding.is_async,
            slots: Some(binding.params.iter().map(DeclaredSlot::from_checked_param).collect()),
            direct_call_id: Some(CompilerNodeId::declaration_at(self.module_identity, *declaration_span)),
            builtin: None,
            // Consumed from the declaration's checked binding — minted once by the typechecker at the declaration
            // site, never re-derived here from module path plus spelling.
            canonical,
        })
    }

    /// Refuse factory calls whose legacy parameter-order evaluation differs from Body IR's written-order facts.
    ///
    /// The selected factory supplies exact slots. A reordered named call needs its own retained evaluation policy;
    /// reordering statements here would contradict ArgumentBinding's ownership contract. Ordered supplied arguments
    /// keep the ordinary binder and declaration-owned defaults, with no extra source evaluation.
    pub(super) fn bind_checked_factory_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        let slots = declared
            .as_deref()
            .ok_or("checked constructor factory without retained parameter slots")?;
        let planned = plan_declared_args(callee, slots, args)?;
        if planned.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
            return Err("checked constructor factory argument evaluation outside declaration order".to_owned());
        }
        self.bind_declared_args(callee, declared, args, scope, out)
    }

    /// Bind a call's arguments against a declared parameter surface, falling back to positional lowering when there
    /// is none to bind against.
    ///
    /// A checked rest-bearing surface packs explicit values into owned list/dictionary operands in declaration
    /// order, after evaluating written values in source order. Checked fixed components expand before pack planning;
    /// unretained pack elements and passing modes stay refused by name. An absent declaration surface retains
    /// unresolved argument elements instead; it must not claim a computed slot map.
    pub(in crate::body_ir) fn bind_declared_args(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        self.bind_declared_args_with_order(callee, declared, args, scope, out, false)
    }

    /// Share fixed/rest/spread planning while keeping frozen ordinary named-call evaluation explicit.
    ///
    /// Only ordinary direct fixed calls request declaration order. Spread and rest capture retain their existing
    /// written-order contract, and no missing declaration can acquire an evaluation-policy fact.
    pub(super) fn bind_declared_args_with_order(
        &mut self,
        callee: &str,
        declared: Option<Vec<DeclaredSlot>>,
        args: &[ast::CallArg],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
        ordinary_direct: bool,
    ) -> Result<(Vec<bir::ArgumentElement>, bir::ArgumentBinding), String> {
        // ---- Declaration authority ----
        let Some(slots) = declared else {
            let elements = self.lower_spread_capable_args(args, scope, out);
            return Ok((elements, bir::ArgumentBinding::UnresolvedPositional));
        };

        // ---- Explicit spread and rest capture ----
        if fixed_spreads::has_checked_tuple_spread(self.type_info, args) {
            return self.bind_checked_tuple_spreads(callee, &slots, args, scope, out);
        }
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
        if slots.iter().any(|slot| slot.kind != IncanCallableParamKind::Normal) {
            return self.bind_rest_args(callee, &slots, &expanded, scope, out);
        }

        // ---- Fixed slots and retained evaluation policy ----
        let planned = plan_declared_args(callee, &slots, &expanded)?;
        let declaration_order = ordinary_direct
            && args
                .iter()
                .all(|arg| matches!(arg, ast::CallArg::Positional(_) | ast::CallArg::Named(_, _)))
            && planned.windows(2).any(|pair| pair[0].0 > pair[1].0);
        let (operands, binding) = self
            .lower_planned_args_with_declaration_order(
                &planned,
                slots.len(),
                &slots.iter().map(|slot| slot.is_mut).collect::<Vec<bool>>(),
                &slots.iter().map(|slot| slot.ty.clone()).collect::<Vec<IncanType>>(),
                scope,
                out,
                declaration_order,
            )
            .map_err(|description| format!("{callee}: {description}"))?;
        Ok((fixed_elements(operands), binding))
    }
}
