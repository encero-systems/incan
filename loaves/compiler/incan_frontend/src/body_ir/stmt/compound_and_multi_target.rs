//! Compound, multi-target, tuple-unpacking and chained assignments.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower `name <op>= value` (`x += y`, `x &= y`, `x <<= y`, ...). Unlike field/index compound assignment, the
    /// parser leaves `ca.value` as the plain right-hand operand rather than pre-desugaring it, so this explicitly
    /// reads `name`'s current value, combines it with `value` via [`Self::lower_binary_from_operands`] (shared with
    /// [`Self::lower_binary`], so every compound form gets exactly the operator representation its binary spelling
    /// gets — a primitive [`bir::BinOp`] for the arithmetic, bitwise, and shift operators, a `Callee::Helper` call
    /// for string concatenation), and writes the result back.
    ///
    /// Three cases refuse instead, each with an explicit unsupported placeholder rather than a panic or a guess:
    /// an operator with no Body IR equivalent (see [`lower_binary_op`]), a name that is not currently bound (should
    /// not happen after a successful typecheck), and a compound assignment the typechecker resolved to a
    /// user-defined operator hook.
    ///
    /// That last case is the one worth stating. `v &= w` on a type defining `__iand__` is a *method call*, and the
    /// typechecker records the resolved dispatch against this statement's span. Combining the operands with
    /// [`bir::BinOp::BitAnd`] here would claim a machine operation the source never asked for — the same wrong
    /// representation [`Self::lower_binary`] avoids by consulting the recorded dispatch first. Body IR has no
    /// place-targeted operator-dispatch form yet (`lower_operator_dispatch` needs two expression operands, and a
    /// compound assignment's left side is a bound name, not an expression), so the honest answer is a named
    /// refusal. Owner: #1101's operator-dispatch lowering. `@=` is permanently in this group — `__imatmul__` is a
    /// protocol hook with no primitive form at all.
    pub(in crate::body_ir) fn lower_compound_assignment(
        &mut self,
        compound_assignment: &ast::CompoundAssignmentStmt,
        scope: bir::ScopeId,
        source_span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(source_span);
        let target_identity = self
            .type_info
            .resolved_write_identity(compound_assignment.name_span, &compound_assignment.name)
            .cloned();
        let lhs_ty = self
            .type_info
            .resolved_write_type(compound_assignment.name_span, &compound_assignment.name)
            .map(|ty| self.checked_type(ty))
            .unwrap_or(IncanType::Unknown);
        let place = if let Some(identity) = target_identity {
            if let Some(alias) = self.input_alias(&identity) {
                if !alias.writable {
                    self.push_unsupported_stmt("explicit local alias does not permit writes".to_owned(), span, out);
                    return;
                }
                Some(alias.place.clone())
            } else {
                self.identity_bindings
                    .get(&identity)
                    .copied()
                    .map(bir::Place::from_local)
                    .or_else(|| self.global_place(identity, lhs_ty.clone()).map(bir::Place::from_global))
            }
        } else {
            self.bindings
                .get(&compound_assignment.name)
                .copied()
                .map(bir::Place::from_local)
        };
        let Some(lhs_place) = place else {
            self.push_unsupported_stmt(
                format!("compound assignment to unbound name `{}`", compound_assignment.name),
                span,
                out,
            );
            return;
        };
        if !lhs_place.permits_write() {
            let target = lhs_place.global().map_or_else(
                || compound_assignment.name.clone(),
                |global| global.identity.render_compact(),
            );
            self.push_unsupported_stmt(format!("assignment target `{target}` is not writable"), span, out);
            return;
        }

        // A resolved operator hook makes this a method call, not a primitive combination. Checked before the
        // operand is lowered, matching the "never partially lower an operator we will refuse" rule the binary
        // path holds to.
        if let Some(dispatch) = self.type_info.resolved_operator_call(source_span)
            && dispatch.kind == ResolvedOperatorKind::Binary
        {
            self.push_unsupported_stmt(
                format!("compound assignment through operator hook `{}`", dispatch.method),
                span,
                out,
            );
            return;
        }

        let lhs_ty = lhs_place
            .local_id()
            .and_then(|local| self.locals.get(local.index()).map(|decl| decl.ty.clone()))
            .or_else(|| lhs_place.global().map(|global| global.ty.clone()))
            .unwrap_or(lhs_ty);
        let op = compound_assignment.op.binary_op();
        let rhs_ty = self.resolve_ty(compound_assignment.value.span);
        if !Self::binary_op_is_supported(op, &lhs_ty, &rhs_ty) {
            self.push_unsupported_stmt(
                format!("compound assignment operator {:?}", compound_assignment.op),
                span,
                out,
            );
            return;
        }
        let read_place = self.static_alias_read_place(&lhs_place);
        let (fact, last_use) = self.ownership_fact_for_place(&read_place, &lhs_ty);
        let lhs_operand = bir::Operand::place(read_place, fact, last_use);
        let rhs_operand = self.lower_expr_to_operand(&compound_assignment.value, scope, out);
        let result = self.lower_binary_from_operands(
            op,
            &lhs_ty,
            lhs_operand,
            &rhs_ty,
            rhs_operand,
            lhs_ty.clone(),
            scope,
            span,
            out,
        );
        if let Some(local) = lhs_place.local_id() {
            self.static_aliases.remove(&local);
        }
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: lhs_place,
                rvalue: bir::Rvalue::Use(result),
            },
            span,
        });
    }

    /// Resolve or declare the local for one name bound by a multi-target assignment (tuple unpack or chained
    /// assignment). A plain inferred assignment and the internal `Reassign` spelling reuse an active local exactly
    /// like [`Self::lower_assignment`] does for a single target; `let` and `mut` always declare a fresh local,
    /// matching source-level shadowing semantics.
    pub(in crate::body_ir) fn bind_multi_target_name(
        &mut self,
        name: &str,
        ty: IncanType,
        binding: ast::BindingKind,
        scope: bir::ScopeId,
        target_span: ast::Span,
        remaining: &[ast::Spanned<ast::Statement>],
    ) -> Result<bir::Place, String> {
        let declaration_span = hir_span(target_span);
        let place = match binding {
            ast::BindingKind::Inferred | ast::BindingKind::Reassign => {
                let identity = self.type_info.resolved_write_identity(target_span, name).cloned();
                if let Some(identity) = identity {
                    if let Some(&local) = self.identity_bindings.get(&identity) {
                        return Ok(bir::Place::from_local(local));
                    }
                    if let Some(global) = self.global_place(identity.clone(), ty.clone()) {
                        let place = bir::Place::from_global(global);
                        return place.permits_write().then_some(place).ok_or_else(|| {
                            format!("assignment target `{}` is not writable", identity.render_compact())
                        });
                    }
                    if identity.kind != SemanticSourceTargetKind::Local || identity.declaration_span != declaration_span
                    {
                        return Err(format!(
                            "assignment target `{name}` has no Body IR place for canonical identity `{}`",
                            identity.render_compact()
                        ));
                    }
                    bir::Place::from_local(self.declare_new_local(
                        name.to_string(),
                        ty,
                        scope,
                        declaration_span,
                        remaining,
                    ))
                } else {
                    let local = self.bindings.get(name).copied().unwrap_or_else(|| {
                        self.declare_new_local(name.to_string(), ty, scope, declaration_span, remaining)
                    });
                    bir::Place::from_local(local)
                }
            }
            ast::BindingKind::Let | ast::BindingKind::Mutable => {
                bir::Place::from_local(self.declare_new_local(name.to_string(), ty, scope, declaration_span, remaining))
            }
        };
        place
            .permits_write()
            .then_some(place)
            .ok_or_else(|| format!("assignment target `{name}` is not writable"))
    }

    /// Lower `a, b = value` / `let a, b = value` into a sequence of single-target `Assign` statements: materialize
    /// `value` once, then bind each name to the corresponding `.{index}` tuple-field projection off it, in
    /// left-to-right order. A freshly materialized result transfers non-Copy fields without requiring Clone.
    /// Reads from source-bound tuples retain the ordinary projected ownership policy; their owner stays initialized.
    pub(in crate::body_ir) fn lower_tuple_unpack(
        &mut self,
        tuple_unpack: &ast::TupleUnpackStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let value_ty = self.resolve_ty(tuple_unpack.value.span);
        if let Some(reason) = unsupported_tuple_destructure(&value_ty, tuple_unpack.names.len()) {
            self.push_unsupported_stmt(reason, span, out);
            return;
        }
        if let Some((_, payload)) = self.carried_view_call(&tuple_unpack.value)
            && tuple_type_elements(&value_ty).is_some()
        {
            if let Err(reason) = self.lower_carried_unpack(tuple_unpack, payload, remaining, scope, span, out) {
                self.push_unsupported_stmt(reason, span, out);
            }
            return;
        }
        let value_operand = self.lower_expr_to_operand(&tuple_unpack.value, scope, out);
        let value_ty = self.retained_operand_type(&value_operand).unwrap_or(value_ty);
        let consumes_tuple = matches!(&value_operand, bir::Operand::Place(read)
            if read.fact == bir::OwnershipFact::Move && read.last_use && read.place.projection.is_empty());
        let value_place = self.materialize_operand_to_place(value_operand, value_ty.clone(), scope, span, out);
        let element_types = tuple_element_types(&value_ty, tuple_unpack.names.len());
        // A fresh result has no source alias to preserve. Its fields transfer individually, including non-Clone
        // SDK handles; borrowed source tuples continue through the ordinary projected ownership policy.
        let transfers_fields = consumes_tuple
            || (value_place.projection.is_empty()
                && value_place
                    .local_id()
                    .is_some_and(|local| matches!(self.locals[local.index()].origin, bir::LocalOrigin::Temporary)));

        if tuple_unpack.names.len() != tuple_unpack.name_spans.len() {
            self.push_unsupported_stmt(
                "tuple binding names and source spans are misaligned".to_string(),
                span,
                out,
            );
            return;
        }

        for (index, ((name, target_span), element_ty)) in tuple_unpack
            .names
            .iter()
            .zip(&tuple_unpack.name_spans)
            .zip(&element_types)
            .enumerate()
        {
            let mut element_place = value_place.clone();
            element_place
                .projection
                .push(bir::PlaceElem::structural_field(index.to_string()));
            let (fact, last_use) = if transfers_fields && !element_ty.abi_v0_facts().ownership.is_trivially_copy() {
                (bir::OwnershipFact::Move, true)
            } else {
                self.ownership_fact_for_place(&element_place, element_ty)
            };
            let element_operand = bir::Operand::place(element_place, fact, last_use);
            let place = match self.bind_multi_target_name(
                name,
                element_ty.clone(),
                tuple_unpack.binding,
                scope,
                *target_span,
                remaining,
            ) {
                Ok(place) => place,
                Err(reason) => {
                    self.push_unsupported_stmt(reason, span, out);
                    return;
                }
            };
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(element_operand),
                },
                span,
            });
        }
    }

    /// Lower `t1, t2 = value` where the targets are lvalue expressions (`arr[i], arr[j] = ...`), not new bindings
    /// -- used for swaps and other multi-target reassignments. Materializes `value` once, then reads and
    /// materializes each element into its own fresh temporary *before* writing to any target, so aliased targets
    /// and sources (for example `arr[i], arr[j] = arr[j], arr[i]`) read the pre-assignment values rather than one
    /// another's already-written results. This evaluation order makes `a, b = b, a` swap correctly.
    ///
    /// A target written as a bare name (`items[0], fresh = ...`) binds that name as an ordinary assignment would,
    /// declaring it when the checker proved this is its first binding, so later reads find a local.
    pub(in crate::body_ir) fn lower_tuple_assign(
        &mut self,
        tuple_assign: &ast::TupleAssignStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let value_ty = self.resolve_ty(tuple_assign.value.span);
        if let Some(reason) = unsupported_tuple_destructure(&value_ty, tuple_assign.targets.len()) {
            self.push_unsupported_stmt(reason, span, out);
            return;
        }
        let value_operand = self.lower_expr_to_operand(&tuple_assign.value, scope, out);
        let value_place = self.materialize_operand_to_place(value_operand, value_ty.clone(), scope, span, out);
        let element_types = tuple_element_types(&value_ty, tuple_assign.targets.len());

        let mut element_operands = Vec::with_capacity(tuple_assign.targets.len());
        for (index, element_ty) in element_types.iter().enumerate() {
            let mut element_place = value_place.clone();
            element_place
                .projection
                .push(bir::PlaceElem::structural_field(index.to_string()));
            let (fact, last_use) = self.ownership_fact_for_place(&element_place, element_ty);
            let element_operand = bir::Operand::place(element_place, fact, last_use);
            element_operands.push(self.push_assign_temp(
                bir::Rvalue::Use(element_operand),
                element_ty.clone(),
                scope,
                span,
                out,
            ));
        }

        for ((target, value), element_ty) in tuple_assign.targets.iter().zip(element_operands).zip(element_types) {
            let place = if let ast::Expr::Ident(name) = &target.node {
                match self.bind_multi_target_name(
                    name,
                    element_ty,
                    ast::BindingKind::Inferred,
                    scope,
                    target.span,
                    remaining,
                ) {
                    Ok(place) => place,
                    Err(reason) => {
                        self.push_unsupported_stmt(reason, span, out);
                        return;
                    }
                }
            } else {
                self.lower_expr_to_place(target, scope, out)
            };
            if !place.permits_write() {
                let target = place.global().map_or_else(
                    || "tuple assignment target".to_string(),
                    |global| global.identity.render_compact(),
                );
                self.push_unsupported_stmt(format!("assignment target `{target}` is not writable"), span, out);
                return;
            }
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(value),
                },
                span,
            });
        }
    }

    /// Evaluate a shared chained value once, then assign its original type to every target in written order.
    /// Destination injection must not flow back into another target: `xs = maybe = make_list()` gives both targets
    /// the list, even though `maybe` wraps it in `Some`. A value the checker recorded as written per target is instead
    /// evaluated once per target at that target's type (see [`Self::lower_chained_per_target`]).
    pub(in crate::body_ir) fn lower_chained_assignment(
        &mut self,
        chained_assignment: &ast::ChainedAssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        if chained_assignment.targets.len() != chained_assignment.target_spans.len() {
            self.push_unsupported_stmt(
                "chained assignment targets and source spans are misaligned".to_string(),
                span,
                out,
            );
            return;
        }
        if self
            .type_info
            .chained_value_is_written_per_target(chained_assignment.value.span)
        {
            self.lower_chained_per_target(chained_assignment, remaining, scope, span, out);
            return;
        }
        if chained_assignment.targets.is_empty() {
            self.push_unsupported_stmt("empty chained assignment".to_string(), span, out);
            return;
        }
        let value_ty = self.resolve_ty(chained_assignment.value.span);
        let value_operand = self.lower_expr_to_operand(&chained_assignment.value, scope, out);
        let value_local = self.new_temp(value_ty.clone(), scope, span);
        let value_place = bir::Place::from_local(value_local);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: value_place.clone(),
                rvalue: bir::Rvalue::Use(value_operand),
            },
            span,
        });
        // The synthetic value is read once per destination; the last read moves its owner and earlier reads clone it.
        self.remaining_reads
            .insert(value_local, ReadCountdown::new(chained_assignment.targets.len()));
        for (name, target_span) in chained_assignment.targets.iter().zip(&chained_assignment.target_spans) {
            let (fact, last_use) = self.ownership_fact_for_place(&value_place, &value_ty);
            let operand = bir::Operand::place(value_place.clone(), fact, last_use);
            let place = match self.bind_multi_target_name(
                name,
                value_ty.clone(),
                chained_assignment.binding,
                scope,
                *target_span,
                remaining,
            ) {
                Ok(place) => place,
                Err(reason) => {
                    self.push_unsupported_stmt(reason, span, out);
                    return;
                }
            };
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(operand),
                },
                span,
            });
        }
    }

    /// Lower a chained assignment with a checked per-target value evaluation. Targets are processed left to right,
    /// and each value is lowered at its target's type, as a single `target = value` writes it.
    ///
    /// The checker records this only for a value built from literals and builtin empty constructors over bound
    /// targets of different types (`n = f = 1` over an `int` and a `float`), so evaluating it once per target observes
    /// nothing more than evaluating it once. The checker checked the value once per target but keeps one type per
    /// source span, so each evaluation first installs its target's types for the value's nodes (see
    /// [`Self::chained_value_types`]). A name the chain declares takes the value checked without a destination, where
    /// an integer literal is an `int`.
    pub(super) fn lower_chained_per_target(
        &mut self,
        chained_assignment: &ast::ChainedAssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let value = &chained_assignment.value;
        for (name, target_span) in chained_assignment.targets.iter().zip(&chained_assignment.target_spans) {
            let bound = match self.chained_bound_type(name, chained_assignment.binding, *target_span) {
                Ok(bound) => bound,
                Err(reason) => {
                    self.push_unsupported_stmt(reason, span, out);
                    return;
                }
            };
            let mut overrides = HashMap::new();
            match &bound {
                Some(target_ty) => self.chained_value_types(value, target_ty, &mut overrides),
                None => {
                    chained_integer_literal_types(value, &IncanType::Primitive(IncanPrimitiveType::Int), &mut overrides)
                }
            }
            self.type_overrides = overrides;
            let value_ty = self.resolve_ty(value.span);
            let operand = self.lower_expr_to_operand(value, scope, out);
            self.type_overrides.clear();
            let place = match self.bind_multi_target_name(
                name,
                value_ty,
                chained_assignment.binding,
                scope,
                *target_span,
                remaining,
            ) {
                Ok(place) => place,
                Err(reason) => {
                    self.push_unsupported_stmt(reason, span, out);
                    return;
                }
            };
            out.push(bir::Statement {
                kind: bir::StatementKind::Assign {
                    place,
                    rvalue: bir::Rvalue::Use(operand),
                },
                span,
            });
        }
    }

    /// Return the type a chained-assignment target already has, or `None` for a name the chain declares, the way
    /// [`Self::bind_multi_target_name`] resolves the same target. Module storage is refused: its place carries no
    /// declared type here to retype the value for.
    pub(super) fn chained_bound_type(
        &self,
        name: &str,
        binding: ast::BindingKind,
        target_span: ast::Span,
    ) -> Result<Option<IncanType>, String> {
        if matches!(binding, ast::BindingKind::Let | ast::BindingKind::Mutable) {
            return Ok(None);
        }
        let local = match self.type_info.resolved_write_identity(target_span, name) {
            Some(identity) => match self.identity_bindings.get(identity) {
                Some(&local) => local,
                None if matches!(
                    identity.kind,
                    SemanticSourceTargetKind::Static | SemanticSourceTargetKind::Const
                ) =>
                {
                    return Err(format!(
                        "chained assignment written per target to module storage `{}`",
                        identity.render_compact()
                    ));
                }
                None => return Ok(None),
            },
            None => match self.bindings.get(name) {
                Some(&local) => local,
                None => return Ok(None),
            },
        };
        Ok(self.locals.get(local.index()).map(|declaration| declaration.ty.clone()))
    }

    /// Record the type one evaluation of a chained value gives each of its nodes for one bound target, as the legacy
    /// backend's `retype_literal` and `retype_chained_integer_literals` retype them.
    ///
    /// A node of the target's own container kind, or of unknown type, takes the target's type, and its elements take
    /// the element types; an integer literal, negated or not, takes a numeric target's type. Any other scalar keeps the
    /// type the checker recorded, so the conversion a single assignment applies (`5` into an `Option[int]`) still
    /// applies.
    pub(super) fn chained_value_types(
        &self,
        value: &ast::Spanned<ast::Expr>,
        target: &IncanType,
        overrides: &mut HashMap<crate::ast::Span, IncanType>,
    ) {
        let recorded = self.resolve_ty(value.span);
        if recorded == IncanType::Unknown || same_container_kind(&recorded, target) {
            overrides.insert(value.span, target.clone());
        }
        match (&value.node, target) {
            (ast::Expr::Paren(inner), _) => self.chained_value_types(inner, target, overrides),
            (ast::Expr::List(entries), IncanType::Generic { base, args }) if base == "List" && args.len() == 1 => {
                for entry in entries {
                    if let ast::ListEntry::Element(item) = entry {
                        self.chained_value_types(item, &args[0], overrides);
                    }
                }
            }
            (ast::Expr::Set(items), IncanType::Generic { base, args }) if base == "Set" && args.len() == 1 => {
                for item in items {
                    self.chained_value_types(item, &args[0], overrides);
                }
            }
            (ast::Expr::Tuple(items), IncanType::Tuple(types)) if items.len() == types.len() => {
                for (item, ty) in items.iter().zip(types) {
                    self.chained_value_types(item, ty, overrides);
                }
            }
            (ast::Expr::Dict(entries), IncanType::Generic { base, args }) if base == "Dict" && args.len() == 2 => {
                for entry in entries {
                    if let ast::DictEntry::Pair(key, item) = entry {
                        self.chained_value_types(key, &args[0], overrides);
                        self.chained_value_types(item, &args[1], overrides);
                    }
                }
            }
            _ => chained_integer_literal_types(value, target, overrides),
        }
    }
}
