//! Assignment lowering, assignment targets, and field and index assignments.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower an assignment statement while preserving its checked lexical binding decision.
    ///
    /// A discard evaluates its checked value as an expression statement and creates no named local or write target.
    /// A plain inferred assignment and the internal `Reassign` spelling reuse the nearest active local when one is
    /// bound, falling back to a declaration for an otherwise unbound name. `let` and `mut` always declare a fresh
    /// local: those are the two source declaration forms that deliberately shadow an enclosing binding.
    /// A Result constructor rebind whose legacy payload is missing retains a named refusal even when Body IR keeps
    /// the local's closed carrier.
    /// An inferred binding of a nongeneric source call retains its providing module's closed return carrier, so a
    /// private import alias in the producer cannot become a consumer-side storage type.
    /// Inferred SDK task bindings likewise keep the checked call's executable output; explicitly annotated bindings
    /// keep their declared contract. A shared-reference binding retains a borrow fact and keeps its source owner
    /// initialized rather than transferring it into the new binding.
    pub(in crate::body_ir) fn lower_assignment(
        &mut self,
        assignment: &ast::AssignmentStmt,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        if let Some(result) = self.lower_local_input_binding(assignment, remaining, scope, span, out) {
            if let Err(reason) = result {
                self.push_unsupported_stmt(reason, span, out);
            }
            return;
        }
        let mut ty = self.assignment_binding_type(assignment, span, out);
        let materializes_range = self.expr_has_materialized_range_layout(&assignment.value);
        self.settle_open_result_value(&assignment.value, &ty);
        let value = self.lower_assignment_value(assignment, &mut ty, scope, out);
        if assignment.name == "_" {
            out.push(bir::Statement {
                kind: bir::StatementKind::Expr { value },
                span,
            });
            return;
        }
        let Some(place) = self.assignment_target_place(assignment, ty, remaining, scope, span, out) else {
            return;
        };
        self.finish_assignment(assignment, place, value, materializes_range, span, out);
    }

    /// Evaluate the replacement before writing it, counting only reads of the destination's current value.
    /// Later statements and loop backedges observe the replacement, while nested loops still protect repeated reads.
    /// Pending call operands and borrowed parameter storage retain their ordinary ownership restrictions.
    pub(super) fn lower_assignment_value(
        &mut self,
        assignment: &ast::AssignmentStmt,
        ty: &mut IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let replacement = self.reassigned_local(assignment);
        let enclosing_replacement = self.replacing_local;
        let reads = replacement.and_then(|local| self.remaining_reads.get(&local).copied());
        let current_reads = count_reads_in_expr(&assignment.name, &assignment.value.node);
        if let Some(local) = replacement {
            self.remaining_reads.insert(
                local,
                ReadCountdown {
                    remaining: current_reads,
                    projected: reads.is_some_and(|countdown| countdown.projected),
                },
            );
            self.replacing_local = Some((local, self.loop_scopes.len()));
        }
        let mut value = self.lower_boundary_value(&assignment.value, Some(ty), scope, out);
        self.replacing_local = enclosing_replacement;
        if let Some(local) = replacement {
            if let Some(mut reads) = reads {
                // Projection history still protects conditional scope-exit drops after the replacement.
                reads.projected |= self
                    .remaining_reads
                    .get(&local)
                    .is_some_and(|countdown| countdown.projected);
                reads.remaining = reads.remaining.saturating_sub(current_reads);
                self.remaining_reads.insert(local, reads);
            } else {
                self.remaining_reads.remove(&local);
            }
        }
        if matches!(ty, IncanType::Ref(_))
            && let bir::Operand::Place(read) = &mut value
        {
            self.retain_argument_borrow(read, false);
        }
        // An awaited method can retain its closed output only after its future has been constructed. Preserve that
        // checked native constraint through an inferred binding without changing an explicit annotation.
        if assignment.ty.is_none()
            && let Some(settled) = self.settled_types.get(&assignment.value.span)
        {
            *ty = self.fill_open_type_parts(ty, settled);
        }
        if let Some(actual) = self.result_operand_type(&value) {
            *ty = self.refine_result_channels(ty, &actual);
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
                *ty = storage.clone();
            }
        }
        value
    }

    /// Identify an existing canonical assignment destination; declarations introduce a new owner instead.
    pub(super) fn reassigned_local(&self, assignment: &ast::AssignmentStmt) -> Option<bir::LocalId> {
        if !matches!(
            assignment.binding,
            ast::BindingKind::Inferred | ast::BindingKind::Reassign
        ) {
            return None;
        }
        match self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
        {
            Some(identity) => self.identity_bindings.get(identity).copied(),
            None => self.bindings.get(&assignment.name).copied(),
        }
    }

    /// Publish the replacement and reestablish its drop obligation and checked binding metadata.
    pub(in crate::body_ir) fn finish_assignment(
        &mut self,
        assignment: &ast::AssignmentStmt,
        place: bir::Place,
        value: bir::Operand,
        materializes_range: bool,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        if assignment.ty.is_some()
            && !self.type_info.binds_capturing_callable(span)
            && let Some(local) = place.local_id()
            && matches!(self.locals[local.index()].ty, IncanType::Function { .. })
            && !self.function_pointer_locals.contains(&local)
        {
            self.function_pointer_locals.push(local);
        }
        if self.type_info.binds_capturing_callable(span)
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
            self.moved_out.remove(&local);
            if materializes_range {
                self.materialized_range_locals.insert(local);
            } else {
                self.materialized_range_locals.remove(&local);
            }
        }
    }

    /// Select the checked binding carrier before lowering the assigned value. Explicit annotations and atomic
    /// newtype storage keep their contract; inferred ordinary bindings may use the retained executable result.
    pub(super) fn assignment_binding_type(
        &mut self,
        assignment: &ast::AssignmentStmt,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> IncanType {
        let atomic_storage = self
            .type_info
            .assignment_binding_type(span)
            .is_some_and(|ty| self.type_info.body_ir_newtypes.controls_type(ty));
        let mut ty = self
            .type_info
            .body_ir_newtypes
            .assignment_type(span, self.type_info)
            .or_else(|| self.type_info.assignment_binding_type(span))
            .map(|ty| self.checked_type(ty))
            .or_else(|| self.callable_value_ty(&assignment.value))
            .unwrap_or_else(|| self.resolve_ty(assignment.value.span));
        if assignment.ty.is_none()
            && !atomic_storage
            && let Some(returned) = self
                .type_info
                .expressions
                .body_ir_expr_types
                .get(&assignment.value.span)
                .or_else(|| {
                    self.type_info
                        .calls
                        .inferred_call_result_types
                        .get(&assignment.value.span)
                })
                .map(|ty| self.checked_type(ty))
                .or_else(|| self.published_expression_type(&assignment.value))
        {
            ty = returned;
        }
        self.retained_result_binding_type(assignment, ty, out)
    }

    /// Resolve a write through its retained identity before using the existing unqualified-local fallback. A new
    /// local is declared only at its checked binding span; an unrepresentable canonical target emits a refusal.
    pub(in crate::body_ir) fn assignment_target_place(
        &mut self,
        assignment: &ast::AssignmentStmt,
        ty: IncanType,
        remaining: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> Option<bir::Place> {
        let binding_span = hir_span(assignment.name_span);
        let target_identity = self
            .type_info
            .resolved_write_identity(assignment.name_span, &assignment.name)
            .cloned();
        let place = match assignment.binding {
            ast::BindingKind::Inferred | ast::BindingKind::Reassign => {
                if let Some(identity) = target_identity {
                    if let Some(alias) = self.input_alias(&identity) {
                        if !alias.writable {
                            self.push_unsupported_stmt(
                                "explicit local alias does not permit writes".to_owned(),
                                span,
                                out,
                            );
                            return None;
                        }
                        alias.place.clone()
                    } else if let Some(&local) = self.identity_bindings.get(&identity) {
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
                        return None;
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
        Some(place)
    }

    /// Lower `obj.field = value` (including the compound `obj.field <op>= value` form). The parser already
    /// desugars a compound `FieldAssignmentStmt` so `value` is the full `obj.field <op> rhs` expression
    /// (`loaves/kernel/incan_syntax/src/parser/stmts.rs`'s `assignment_or_expr_stmt`) -- `fa.compound_op` is purely a
    /// formatter hint for round-tripping `+=` spelling and carries no separate lowering semantics here, so this
    /// only needs to build the write-side place and lower `value` normally.
    /// Field alias writes remain named refusals because legacy still emits their alias spelling as a Rust field.
    pub(in crate::body_ir) fn lower_field_assignment(
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
    /// A deferred index read reserves its owner until the value has been evaluated, as an earlier call argument does.
    pub(in crate::body_ir) fn lower_index_assignment(
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
        let pending_start = self.pending_argument_reads.len();
        self.retain_pending_operand_read(&index_operand);
        place.projection.push(bir::PlaceElem::Index(Box::new(index_operand)));
        if !place.permits_write() {
            self.pending_argument_reads.truncate(pending_start);
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
        self.pending_argument_reads.truncate(pending_start);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place,
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
    }
}
