//! Lowering for statements and every assignment form: field, index, compound, tuple, and chained.

use super::refusals::*;
use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower every statement in `stmts` into `out`, within `scope`. Statements are lowered in source order and each
    /// one is given the statement suffix that follows it (`&stmts[index + 1..]`), so last-use countdowns seeded by
    /// [`Self::declare_new_local`] only count reads that can still occur after the declaration. Nested blocks cannot
    /// change an enclosing static binding's handle selection: that needs a runtime handle fact, so it refuses
    /// explicitly rather than retaining whichever branch happened to lower last.
    pub(super) fn lower_block_into(
        &mut self,
        stmts: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        // ---- Context: storage bindings in the enclosing scope ----
        let nested = self.scopes.iter().any(|info| info.id == scope && info.parent.is_some());
        let enclosing_aliases = self.static_aliases.clone();
        let enclosing_handles = self.static_binding_locals.clone();

        // ---- Context: source-order statements ----
        for (index, stmt) in stmts.iter().enumerate() {
            self.lower_stmt_into(stmt, &stmts[index + 1..], scope, out);
        }

        // ---- Context: a handle selected by runtime control flow ----
        if nested {
            if enclosing_handles
                .iter()
                .any(|local| self.static_aliases.get(local) != enclosing_aliases.get(local))
            {
                self.push_unsupported_stmt(
                    "control-flow-dependent static binding handle".to_string(),
                    self.scope_span(scope),
                    out,
                );
            }
            self.static_aliases = enclosing_aliases;
        }
    }

    /// Lower one statement into `out`, dispatching on its AST kind. `remaining` is the statement suffix following
    /// `stmt` in its enclosing block, threaded through to [`Self::lower_assignment`] for last-use seeding. Statement
    /// kinds outside v0's covered subset fall through to an explicit [`Self::push_unsupported_stmt`] rather than
    /// panicking (see this module's module-level docs for the exact covered/uncovered split).
    ///
    /// Source unsafe regions retain their acknowledgment boundary rather than being silently flattened; see
    /// [`Self::lower_unsafe_region`]. Everything the catch-all still reaches is vocab/surface residue
    /// (`Surface`, `VocabBlock`, `VocabExpressionItem`), whose disposition belongs to the Body IR input-contract
    /// work rather than to any single construct: those nodes reach this module only when a caller skips the
    /// desugar pass the legacy pipeline runs first (#1166). No statement kind lowers under the bare `"statement"`
    /// label any more (#1162).
    pub(super) fn lower_stmt_into(
        &mut self,
        stmt: &ast::Spanned<ast::Statement>,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(stmt.span);
        match &stmt.node {
            ast::Statement::Assignment(assignment) => self.lower_assignment(assignment, remaining, scope, span, out),
            ast::Statement::FieldAssignment(field_assignment) => {
                self.lower_field_assignment(field_assignment, scope, span, out)
            }
            ast::Statement::IndexAssignment(index_assignment) => {
                // The raw AST span keys a resolved `__setitem__` hook, as for compound assignment below.
                self.lower_index_assignment(index_assignment, scope, stmt.span, out)
            }
            ast::Statement::CompoundAssignment(compound_assignment) => {
                // Passed the raw AST span, not `span`: the typechecker keys a resolved operator hook by source
                // span, and only the untranslated span can look one up.
                self.lower_compound_assignment(compound_assignment, scope, stmt.span, out)
            }
            ast::Statement::TupleUnpack(tuple_unpack) => {
                self.lower_tuple_unpack(tuple_unpack, remaining, scope, span, out)
            }
            ast::Statement::TupleAssign(tuple_assign) => {
                self.lower_tuple_assign(tuple_assign, remaining, scope, span, out)
            }
            ast::Statement::ChainedAssignment(chained_assignment) => {
                self.lower_chained_assignment(chained_assignment, remaining, scope, span, out)
            }
            ast::Statement::Return(value) => {
                // A returned `Ok(...)` or `Err(...)` builds the sides the declared return type settles for it.
                let value = value.as_ref().map(|returned| {
                    let destination = self.owner_return_type.clone();
                    self.settle_open_result_value(returned, &destination);
                    let operand = self.lower_boundary_value(returned, Some(&destination), scope, out);
                    self.owned_parameter_operand(returned, operand)
                });
                out.push(bir::Statement {
                    kind: bir::StatementKind::Return { value },
                    span,
                });
            }
            ast::Statement::If(if_stmt) => self.lower_if(if_stmt, scope, span, out),
            ast::Statement::Loop(loop_stmt) => self.lower_loop_stmt(loop_stmt, scope, span, out),
            ast::Statement::While(while_stmt) => self.lower_while(while_stmt, scope, span, out),
            ast::Statement::For(for_stmt) => self.lower_for(for_stmt, scope, span, out),
            ast::Statement::Unsafe(region) => self.lower_unsafe_region(region, remaining, scope, span, out),
            ast::Statement::Expr(expr) => {
                // `yield value` parses as an ordinary expression statement wrapping `ast::Expr::Yield(Some(_))`
                // (there is no separate `ast::Statement::Yield` AST node) -- mirror the existing Rust-emission
                // backend's own `lower_statement` (`loaves/compiler/incan_ir/src/lower/stmt.rs`), which special-cases
                // this exact shape before falling back to generic expression-statement lowering. A bare
                // `yield` (no value) falls through to the generic `Expr` arm below, same as that
                // backend, and lowers via the expression-position `yield` stub (see the module docs).
                if let ast::Expr::Yield(Some(value)) = &expr.node {
                    self.lower_yield(value, scope, span, out);
                } else {
                    let value = self.lower_expr_to_operand(expr, scope, out);
                    out.push(bir::Statement {
                        kind: bir::StatementKind::Expr { value },
                        span,
                    });
                }
            }
            ast::Statement::Assert(assert_stmt) => self.lower_assert(assert_stmt, remaining, scope, span, out),
            ast::Statement::Pass => {}
            ast::Statement::Break(value) => self.lower_break(value.as_ref(), scope, span, out),
            ast::Statement::Continue => {
                if let Some(actions) = self.loop_continue_actions.last() {
                    out.extend(actions.iter().cloned());
                }
                out.push(bir::Statement {
                    kind: bir::StatementKind::Continue,
                    span,
                });
            }
            other => self.push_unsupported_stmt(unsupported_stmt_label(other), span, out),
        }
    }

    /// Retain a source acknowledgment and its operations without introducing an Incan binding or drop scope.
    ///
    /// Every declaration sees the remaining region statements and then the enclosing suffix, so later reads retain
    /// the same ownership countdown as legacy's flattened statement list. Admission remains the consumer's decision.
    fn lower_unsafe_region(
        &mut self,
        region: &ast::UnsafeStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let mut operations = Vec::new();
        for (index, statement) in region.body.iter().enumerate() {
            let suffix = region.body[index + 1..]
                .iter()
                .chain(remaining)
                .cloned()
                .collect::<Vec<_>>();
            self.lower_stmt_into(statement, &suffix, scope, &mut operations);
        }
        out.push(bir::Statement {
            kind: bir::StatementKind::UnsafeRegion {
                acknowledgment: bir::UnsafeAcknowledgment {
                    receipt: CompilerNodeId::statement_span(self.module_identity, span.start, span.end),
                    span,
                    scope,
                },
                body: bir::Block {
                    scope,
                    stmts: operations,
                },
            },
            span,
        });
    }

    /// Lower an assignment statement while preserving its checked lexical binding decision.
    ///
    /// A plain inferred assignment and the internal `Reassign` spelling reuse the nearest active local when one is
    /// bound, falling back to a declaration for an otherwise unbound name. `let` and `mut` always declare a fresh
    /// local: those are the two source declaration forms that deliberately shadow an enclosing binding.
    /// An inferred binding of a nongeneric source call retains its providing module's closed return carrier, so a
    /// private import alias in the producer cannot become a consumer-side storage type.
    /// Inferred SDK task bindings likewise keep the checked call's executable output; explicitly annotated bindings
    /// keep their declared contract.
    ///
    /// A Result constructor rebind whose legacy payload is missing retains a named refusal even when Body IR keeps
    /// the local's closed carrier.
    pub(super) fn lower_assignment(
        &mut self,
        assignment: &ast::AssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        // A closure value already carries the typechecker's callable shape. A partial retains that full callable
        // type, with captured presets represented as named-overrideable defaults. Positional calls skip those preset
        // slots, and `LocalCallableTarget::binding` records the resulting declaration mapping. Keeping this type on
        // the binding makes the local call contract agree with the `Rvalue::Closure` that creates the value.
        let assignment_span = ast::Span::new(span.start, span.end);
        let mut ty = self
            .type_info
            .body_ir_newtypes
            .assignment_type(assignment_span, self.type_info)
            .or_else(|| self.type_info.assignment_binding_type(assignment_span))
            .map(|ty| self.checked_type(ty))
            .or_else(|| self.callable_value_ty(&assignment.value))
            .unwrap_or_else(|| self.resolve_ty(assignment.value.span));
        if assignment.ty.is_none()
            && let Some(returned) = self
                .type_info
                .expressions
                .body_ir_expr_types
                .get(&(assignment.value.span.start, assignment.value.span.end))
                .or_else(|| {
                    self.type_info
                        .calls
                        .inferred_call_result_types
                        .get(&(assignment.value.span.start, assignment.value.span.end))
                })
                .map(|ty| self.checked_type(ty))
                .or_else(|| self.published_expression_type(&assignment.value))
        {
            ty = returned;
        }
        ty = self.retained_result_binding_type(assignment, ty, out);
        let materializes_range = self.expr_has_materialized_range_layout(&assignment.value);
        self.settle_open_result_value(&assignment.value, &ty);
        let value = self.lower_boundary_value(&assignment.value, Some(&ty), scope, out);
        if let Some(actual) = self.result_operand_type(&value) {
            ty = self.refine_result_channels(&ty, &actual);
        }
        // Future construction is a distinct Body IR value even when the surface checker exposes its output type.
        // Preserve that carrier through a binding or alias instead of silently turning it into a scalar copy.
        // An inferred SDK task binding also keeps the individual checked call's concrete output argument.
        if let bir::Operand::Place(read) = &value
            && read.place.projection.is_empty()
            && let Some(local) = read.place.local_id()
        {
            let storage = &self.locals[local.index()].ty;
            let future = matches!(storage, IncanType::Generic { base, .. } if base == "Awaitable");
            let inferred_task =
                assignment.ty.is_none() && super::runtime_nominals::retains_task_storage(self.type_info, storage);
            if future || inferred_task {
                ty = storage.clone();
            }
        }
        let binding_span = hir_span(assignment.name_span);
        let target_identity = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .cloned();
        let place = match assignment.binding {
            ast::BindingKind::Inferred | ast::BindingKind::Reassign => {
                if let Some(identity) = target_identity {
                    if let Some(&local) = self.identity_bindings.get(&identity) {
                        bir::Place::from_local(local)
                    } else if let Some(global) = self.global_place(identity.clone(), ty.clone()) {
                        bir::Place::from_global(global)
                    } else if identity.kind == SemanticSourceTargetKind::Local
                        && identity.declaration_span == binding_span
                    {
                        let local = self.declare_new_local(assignment.name.clone(), ty, scope, binding_span, remaining);
                        bir::Place::from_local(local)
                    } else {
                        self.push_unsupported_stmt(
                            format!(
                                "assignment target `{}` has no Body IR place for canonical identity `{}`",
                                assignment.name,
                                identity.render_compact()
                            ),
                            span,
                            out,
                        );
                        return;
                    }
                } else {
                    let local = self.bindings.get(&assignment.name).copied().unwrap_or_else(|| {
                        self.declare_new_local(assignment.name.clone(), ty, scope, binding_span, remaining)
                    });
                    bir::Place::from_local(local)
                }
            }
            ast::BindingKind::Let | ast::BindingKind::Mutable => bir::Place::from_local(self.declare_new_local(
                assignment.name.clone(),
                ty,
                scope,
                binding_span,
                remaining,
            )),
        };
        if assignment.ty.is_some()
            && !self.type_info.binds_capturing_callable(assignment_span)
            && let Some(local) = place.local_id()
            && matches!(self.locals[local.index()].ty, IncanType::Function { .. })
            && !self.function_pointer_locals.contains(&local)
        {
            self.function_pointer_locals.push(local);
        }
        if self.type_info.binds_capturing_callable(assignment_span)
            && let Some(local) = place.local_id()
            && !self.closure_holding_locals.contains(&local)
        {
            self.closure_holding_locals.push(local);
        }
        if !place.permits_write() {
            let target = place
                .global()
                .map_or_else(|| assignment.name.clone(), |global| global.identity.render_compact());
            self.push_unsupported_stmt(format!("assignment target `{target}` is not writable"), span, out);
            return;
        }
        self.record_static_alias_assignment(assignment, &place, &value);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: place.clone(),
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
        if let Some(local) = place.local_id() {
            if materializes_range {
                self.materialized_range_locals.insert(local);
            } else {
                self.materialized_range_locals.remove(&local);
            }
        }
    }

    /// Lower `obj.field = value` (including the compound `obj.field <op>= value` form). The parser already
    /// desugars a compound `FieldAssignmentStmt` so `value` is the full `obj.field <op> rhs` expression
    /// (`loaves/kernel/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`) -- `fa.compound_op` is purely a
    /// formatter hint for round-tripping `+=` spelling and carries no separate lowering semantics here, so this
    /// only needs to build the write-side place and lower `value` normally.
    /// Field alias writes remain named refusals because legacy still emits their alias spelling as a Rust field.
    pub(super) fn lower_field_assignment(
        &mut self,
        field_assignment: &ast::FieldAssignmentStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let mut place = self.lower_expr_to_place(&field_assignment.object, scope, out);
        let projection = self.lower_checked_field_projection(
            &field_assignment.object,
            &field_assignment.field,
            field_assignment.target_span,
        );
        // Legacy writes still emit the source alias as a Rust field (#1337 lane repro). Do not admit a write
        // that legacy cannot compile merely because the checker already selected its canonical storage.
        if matches!(&projection, bir::PlaceElem::Field { name, .. } if name != &field_assignment.field) {
            self.push_unsupported_stmt(
                format!("model field alias assignment `{}`", field_assignment.field),
                span,
                out,
            );
            return;
        }
        place.projection.push(projection);
        if !place.permits_write() {
            let target = place.global().map_or_else(
                || "field assignment target".to_string(),
                |global| global.identity.render_compact(),
            );
            self.push_unsupported_stmt(format!("assignment target `{target}` is not writable"), span, out);
            return;
        }
        let value = self.lower_boundary_value(&field_assignment.value, None, scope, out);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
    }

    /// Lower `obj[index] = value` (including the compound `obj[index] <op>= value` form, pre-desugared into
    /// `value` by the parser -- see [`Self::lower_field_assignment`]'s docs for the same note on
    /// `IndexAssignmentStmt::compound_op`). The object place is lowered before the index operand, preserving the
    /// established assignment evaluation order in the Rust-emission backend: object, index, then assigned value.
    /// A write checking resolved to a user-defined `__setitem__` is that method's call, in the same order. Static
    /// storage is the exception: legacy computes the assigned value before it locks the storage to write the entry, so
    /// the value is lowered before the index there, and the index is the last read of what it names.
    pub(super) fn lower_index_assignment(
        &mut self,
        index_assignment: &ast::IndexAssignmentStmt,
        scope: bir::ScopeId,
        source_span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) {
        let span = hir_span(source_span);
        if let Some(dispatch) = self.type_info.resolved_operator_call(source_span)
            && dispatch.kind == ResolvedOperatorKind::IndexAssign
        {
            let method = dispatch.method.clone();
            let object = &index_assignment.object;
            let arguments = [&index_assignment.index, &index_assignment.value];
            let unit = IncanType::Primitive(IncanPrimitiveType::Unit);
            self.lower_operator_dispatch(&method, object, &arguments, unit, source_span, scope, out);
            return;
        }
        let mut place = self.lower_expr_to_place(&index_assignment.object, scope, out);
        let early_value = place
            .global()
            .is_some()
            .then(|| self.lower_boundary_value(&index_assignment.value, None, scope, out));
        let index_operand = self.lower_expr_to_operand(&index_assignment.index, scope, out);
        place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
        if !place.permits_write() {
            let target = place.global().map_or_else(
                || "index assignment target".to_string(),
                |global| global.identity.render_compact(),
            );
            self.push_unsupported_stmt(format!("assignment target `{target}` is not writable"), span, out);
            return;
        }
        let value = match early_value {
            Some(value) => value,
            None => self.lower_boundary_value(&index_assignment.value, None, scope, out),
        };
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
    }

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
    pub(super) fn lower_compound_assignment(
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
            self.identity_bindings
                .get(&identity)
                .copied()
                .map(bir::Place::from_local)
                .or_else(|| self.global_place(identity, lhs_ty.clone()).map(bir::Place::from_global))
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
    pub(super) fn bind_multi_target_name(
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
    /// left-to-right order. Element reads go through the same [`Self::ownership_fact_for_place`] a plain
    /// `.field`/`[index]` read anywhere else in v0 uses, so a non-Copy element borrows rather than moves (v0 does
    /// not track partial-move state out of a place, per [`Self::ownership_fact_for_place`]'s own docs) --
    /// consistent with, not a special case of, that existing policy.
    pub(super) fn lower_tuple_unpack(
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
        let value_operand = self.lower_expr_to_operand(&tuple_unpack.value, scope, out);
        let value_place = self.materialize_operand_to_place(value_operand, value_ty.clone(), scope, span, out);
        let element_types = tuple_element_types(&value_ty, tuple_unpack.names.len());

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
            let (fact, last_use) = self.ownership_fact_for_place(&element_place, element_ty);
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
    /// another's already-written results. This is genuinely new coverage: the existing Rust-emission backend does
    /// not implement `TupleAssign` at all (`loaves/compiler/incan_ir/src/lower/stmt.rs` returns a `LoweringError`), so
    /// there is no existing behavior to mirror here -- the evaluation order above is v0's own design, chosen
    /// specifically to make `a, b = b, a` swap correctly.
    ///
    /// A target written as a bare name (`items[0], fresh = ...`) binds that name as an ordinary assignment would,
    /// declaring it when the checker proved this is its first binding, so later reads find a local.
    pub(super) fn lower_tuple_assign(
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
    pub(super) fn lower_chained_assignment(
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
            .insert(value_local, chained_assignment.targets.len());
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

    /// Lower a chained assignment whose value the checker recorded as written per target: each target, left to right,
    /// gets its own evaluation of the value at that target's type, as a single `target = value` writes it, matching
    /// the legacy backend's `lower_chained_literal` (`loaves/compiler/incan_ir/src/lower/assignment_targets.rs`).
    ///
    /// The checker records this only for a value built from literals and builtin empty constructors over bound
    /// targets of different types (`n = f = 1` over an `int` and a `float`), so evaluating it once per target observes
    /// nothing more than evaluating it once. The checker checked the value once per target but keeps one type per
    /// source span, so each evaluation first installs its target's types for the value's nodes (see
    /// [`Self::chained_value_types`]). A name the chain declares takes the value checked without a destination, where
    /// an integer literal is an `int`.
    fn lower_chained_per_target(
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
    fn chained_bound_type(
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
    fn chained_value_types(
        &self,
        value: &ast::Spanned<ast::Expr>,
        target: &IncanType,
        overrides: &mut HashMap<(usize, usize), IncanType>,
    ) {
        let recorded = self.resolve_ty(value.span);
        if recorded == IncanType::Unknown || same_container_kind(&recorded, target) {
            overrides.insert((value.span.start, value.span.end), target.clone());
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

    /// Lower a `break` / `break value` statement. A value routes into the innermost enclosing loop's result place
    /// when that loop is a value-producing `loop:` expression (see [`Self::lower_loop_expr`]) -- otherwise it stays
    /// on the `Break` statement itself, matching [`bir::StatementKind::Break`]'s documented default. The innermost
    /// context comes from [`Self::loop_break_targets`], which every loop-lowering path pushes/pops around its own
    /// body so a `break` always targets the loop it is lexically inside, never an outer one.
    pub(super) fn lower_break(
        &mut self,
        value: Option<&ast::Spanned<ast::Expr>>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let target = self.loop_break_targets.last().copied().flatten();
        match (value, target) {
            (Some(expr), Some(result_local)) => {
                let destination = self.locals[result_local.index()].ty.clone();
                let operand = self.lower_boundary_value(expr, Some(&destination), scope, out);
                out.push(bir::Statement {
                    kind: bir::StatementKind::Assign {
                        place: bir::Place::from_local(result_local),
                        rvalue: bir::Rvalue::Use(operand),
                    },
                    span,
                });
                out.push(bir::Statement {
                    kind: bir::StatementKind::Break { value: None },
                    span,
                });
            }
            _ => {
                let operand = value.map(|v| self.lower_boundary_value(v, None, scope, out));
                out.push(bir::Statement {
                    kind: bir::StatementKind::Break { value: operand },
                    span,
                });
            }
        }
    }

    /// Lower a statement-position `yield value` (`ast::Expr::Yield(Some(value))` reached through
    /// [`Self::lower_stmt_into`]'s `ast::Statement::Expr` arm) into a [`bir::StatementKind::Yield`].
    ///
    /// The checked destination boundary retains numeric widening before the generator consumes the value; ownership
    /// facts and last-use tracking still apply to its source operand exactly as for another value read.
    ///
    /// Records the runtime dependencies the existing Rust-emission backend's own `yield` lowering actually needs
    /// (`__incan_yield.yield_value(..)` on a `GeneratorYield` handle backed by `std::thread::spawn` and
    /// `std::sync::mpsc::sync_channel` -- see `loaves/stdlib/core/rust/src/iter.rs`'s `Generator`/`SpawnedGenerator`):
    /// a named runtime helper (mirroring how [`Self::lower_fstring`] records `"fstring"` without a new
    /// [`bir::HelperOp`] variant, since `Yield` is its own statement kind, not a [`bir::Callee::Helper`] call),
    /// [`AbiV0RuntimeRequirement::HostedStd`] (the spawned-thread/channel machinery is not freestanding-compatible),
    /// and [`AbiV0RuntimeRequirement::Allocator`] (the channel and boxed iterator both allocate).
    pub(super) fn lower_yield(
        &mut self,
        value: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let operand = self.lower_boundary_value(value, None, scope, out);
        out.push(bir::Statement {
            kind: bir::StatementKind::Yield { value: operand },
            span,
        });
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper("generator".to_string()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::HostedStd);
        self.record_runtime_requirement(AbiV0RuntimeRequirement::Allocator);
    }
}

/// Record a numeric target's type for an integer literal, negated or parenthesized or not, as the legacy backend's
/// `retype_chained_integer_literals` rewrites one evaluation of a chained value: `n = f = 1` writes `1` to the `int`
/// and `1.0` to the `float`. Any other node, and any non-numeric target, keeps the checker's recorded type.
fn chained_integer_literal_types(
    value: &ast::Spanned<ast::Expr>,
    target: &IncanType,
    overrides: &mut HashMap<(usize, usize), IncanType>,
) {
    if !matches!(
        target,
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Numeric(_))
    ) {
        return;
    }
    match &value.node {
        ast::Expr::Paren(inner) => chained_integer_literal_types(inner, target, overrides),
        ast::Expr::Literal(ast::Literal::Int(_)) => {
            overrides.insert((value.span.start, value.span.end), target.clone());
        }
        ast::Expr::Unary(ast::UnaryOp::Neg, operand)
            if matches!(operand.node, ast::Expr::Literal(ast::Literal::Int(_))) =>
        {
            overrides.insert((value.span.start, value.span.end), target.clone());
            overrides.insert((operand.span.start, operand.span.end), target.clone());
        }
        _ => {}
    }
}

/// Return whether two types are the same kind of container: both `Option`, `List`, `Set` or `Dict`, or tuples of one
/// length, the only nodes a chained value's per-target evaluation retypes wholesale.
fn same_container_kind(left: &IncanType, right: &IncanType) -> bool {
    match (left, right) {
        (IncanType::Generic { base: left, .. }, IncanType::Generic { base: right, .. }) => {
            left == right && matches!(left.as_str(), "Option" | "List" | "Set" | "Dict")
        }
        (IncanType::Tuple(left), IncanType::Tuple(right)) => left.len() == right.len(),
        _ => false,
    }
}
