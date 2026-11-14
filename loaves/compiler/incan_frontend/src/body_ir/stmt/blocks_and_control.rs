//! Block and statement lowering, statement temporaries, returns, unsafe regions, `break` and `yield`.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower every statement in `stmts` into `out`, within `scope`. Statements are lowered in source order and each
    /// one is given the statement suffix that follows it (`&stmts[index + 1..]`), so last-use countdowns seeded by
    /// [`Self::declare_new_local`] only count reads that can still occur after the declaration. Nested blocks cannot
    /// change an enclosing static binding's handle selection: that needs a runtime handle fact, so it refuses
    /// explicitly rather than retaining whichever branch happened to lower last. Non-escaping compiler temporaries
    /// end at the source statement boundary, so a native borrow guard cannot remain live into the next statement.
    pub(in crate::body_ir) fn lower_block_into(
        &mut self,
        stmts: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) {
        // ---- Context: storage bindings in the enclosing scope ----
        let nested = self.scopes.iter().any(|info| info.id == scope && info.parent.is_some());
        let enclosing_aliases = self.static_aliases.clone();
        let enclosing_handles = self.static_binding_locals.clone();

        // ---- Context: source-order statements, on a path that ends here when the block returns ----
        // Inside a loop the block is terminal only when nothing in it can break or continue out of that loop first.
        let terminal = nested
            && matches!(stmts.last().map(|stmt| &stmt.node), Some(ast::Statement::Return(_)))
            && (self.loop_scopes.is_empty() || !may_leave_enclosing_loop(stmts));
        if terminal {
            self.enter_terminal_path(stmts, scope);
        }
        for (index, stmt) in stmts.iter().enumerate() {
            let first_temporary = self.locals.len();
            self.lower_stmt_into(stmt, &stmts[index + 1..], scope, out);
            self.insert_statement_temporary_drops(out, scope, first_temporary, None, hir_span(stmt.span));
            if self.input_paths_checked && !super::conditional_inputs::reaches_join(std::slice::from_ref(stmt)) {
                break;
            }
        }
        if terminal {
            self.terminal_paths.pop();
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

    /// End fresh compiler-owned expression storage after its source statement, in reverse construction order.
    /// Named bindings keep their lexical lifetime. Nested blocks handle their own locals, and moved temporaries
    /// carry no drop obligation; recording these ends prevents a second lexical-scope drop for the same storage.
    /// `kept` stays live past this point: a scrutinee's value, which its arms still read.
    pub(in crate::body_ir) fn insert_statement_temporary_drops(
        &mut self,
        out: &mut Vec<bir::Statement>,
        scope: bir::ScopeId,
        first: usize,
        kept: Option<bir::LocalId>,
        span: HirSourceSpan,
    ) {
        let candidates = self.locals[first..]
            .iter()
            .rev()
            .filter(|local| local.scope == scope && local.origin == bir::LocalOrigin::Temporary)
            .filter(|local| !local.ty.abi_v0_facts().ownership.is_trivially_copy())
            .filter(|local| !self.retained_statement_temporaries.contains(&local.id))
            .filter(|local| !self.moved_out.contains(&local.id) && Some(local.id) != kept)
            .map(|local| local.id)
            .collect::<Vec<_>>();
        for local in candidates {
            out.push(bir::Statement {
                kind: bir::StatementKind::Drop { local },
                span,
            });
            self.moved_out.insert(local);
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
    pub(in crate::body_ir) fn lower_stmt_into(
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
            ast::Statement::Return(Some(returned)) if self.returned_view.is_some() => {
                let view = self.returned_view.as_ref().map(|source| source.view.clone());
                let lowered = match view {
                    Some(view) if !view.component.is_empty() && view.carried.is_none() => {
                        self.lower_aggregate_return(returned, scope, span, out);
                        Ok(())
                    }
                    Some(view) => match view.carried {
                        Some(payload) => self.lower_carried_return(returned, payload, scope, span, out),
                        None => self.lower_view_return(returned, scope, span, out),
                    },
                    None => Ok(()),
                };
                if let Err(reason) = lowered {
                    self.push_unsupported_stmt(reason, span, out);
                }
            }
            ast::Statement::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|returned| self.lower_return_operand(returned, scope, out));
                out.push(bir::Statement {
                    kind: bir::StatementKind::Return { value },
                    span,
                });
            }
            ast::Statement::If(if_stmt) => self.lower_if(if_stmt, scope, span, out),
            ast::Statement::Loop(loop_stmt) => {
                self.lower_input_loop(false, |builder| builder.lower_loop_stmt(loop_stmt, scope, span, out))
            }
            ast::Statement::While(while_stmt) => {
                // A literal `true` condition never fails, so only a `break` leaves the loop.
                let endless = matches!(&while_stmt.condition,
                    ast::Condition::Expr(condition) if matches!(condition.node, ast::Expr::Literal(ast::Literal::Bool(true))));
                self.lower_input_loop(!endless, |builder| builder.lower_while(while_stmt, scope, span, out))
            }
            ast::Statement::For(for_stmt) => {
                self.lower_input_loop(true, |builder| builder.lower_for(for_stmt, scope, span, out))
            }
            ast::Statement::Unsafe(region) => self.lower_unsafe_region(region, remaining, scope, span, out),
            ast::Statement::Expr(expr) => {
                // `yield value` parses as an ordinary expression statement wrapping `ast::Expr::Yield(Some(_))`
                // (there is no separate `ast::Statement::Yield` AST node). A bare `yield` falls through to ordinary
                // expression lowering; only a statement with a value reaches the suspension operation here.
                if let ast::Expr::Yield(Some(value)) = &expr.node {
                    self.lower_yield(value, scope, span, out);
                } else {
                    let value = self.lower_expr_to_operand(expr, scope, out);
                    // A discarded view call, or a carried view `?` took out, already ran for its effects; its
                    // substituted place is not read.
                    if self.type_info.selected_call_view(expr.span).is_none() && !self.carried_view_try(expr) {
                        out.push(bir::Statement {
                            kind: bir::StatementKind::Expr { value },
                            span,
                        });
                    }
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

    /// Read a return value on a terminal function path, where enclosing loops cannot repeat the read.
    /// New loops inside the return expression still establish their own repeated-read scopes. Borrowed receivers and
    /// parameters retain their normal ownership rules, and Result constructors use the declared return channels.
    /// Reads in sibling return branches cannot keep an owned local alive on this terminal path.
    pub(in crate::body_ir) fn lower_return_operand(
        &mut self,
        returned: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let destination = self.owner_return_type.clone();
        self.settle_open_result_value(returned, &destination);
        let enclosing_loops = std::mem::take(&mut self.loop_scopes);
        let mut operand = self.lower_boundary_value(returned, Some(&destination), scope, out);
        self.loop_scopes = enclosing_loops;
        if let bir::Operand::Place(read) = &mut operand
            && read.place.projection.is_empty()
            && read.fact == bir::OwnershipFact::Clone
            && let Some(local) = read.place.local_id()
            && !self.is_receiver_local(local)
            && !self.is_borrowed_parameter(local)
            && !self.pending_argument_reads.contains(&local)
        {
            read.fact = bir::OwnershipFact::Move;
            read.last_use = true;
            self.moved_out.insert(local);
        }
        self.owned_parameter_operand(returned, operand)
    }

    /// Retain a source acknowledgment and its operations without introducing an Incan binding or drop scope.
    ///
    /// Every declaration sees the remaining region statements and then the enclosing suffix, so later reads retain
    /// the same ownership countdown as legacy's flattened statement list. Admission remains the consumer's decision.
    pub(super) fn lower_unsafe_region(
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
                    receipt: CompilerNodeId::statement_at(self.module_identity, span),
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

    /// Lower a `break` / `break value` statement. A value routes into the innermost enclosing loop's result place
    /// when that loop is a value-producing `loop:` expression (see [`Self::lower_loop_expr`]) -- otherwise it stays
    /// on the `Break` statement itself, matching [`bir::StatementKind::Break`]'s documented default. The innermost
    /// context comes from [`Self::loop_break_targets`], which every loop-lowering path pushes/pops around its own
    /// body so a `break` always targets the loop it is lexically inside, never an outer one.
    /// The exited loop cannot repeat its value read, so that loop's ownership scope is suspended while the value is
    /// evaluated. Enclosing loops and new loops within the value still protect their own repeated reads.
    pub(in crate::body_ir) fn lower_break(
        &mut self,
        value: Option<&ast::Spanned<ast::Expr>>,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let target = self.loop_break_targets.last().copied().flatten();
        let destination = target.map(|local| self.locals[local.index()].ty.clone());
        let exited_loop = self.loop_scopes.pop();
        let operand = value.map(|expr| self.lower_boundary_value(expr, destination.as_ref(), scope, out));
        self.loop_scopes.extend(exited_loop);
        self.retain_input_break();
        match (operand, target) {
            (Some(operand), Some(result_local)) => {
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
            (operand, _) => {
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
    /// `std::sync::mpsc::sync_channel` -- see `loaves/stdlib/primitives/rust/src/iter.rs`'s
    /// `Generator`/`SpawnedGenerator`):
    /// a named runtime helper (mirroring how [`Self::lower_fstring`] records `"fstring"` without a new
    /// [`bir::HelperOp`] variant, since `Yield` is its own statement kind, not a [`bir::Callee::Helper`] call),
    /// [`AbiV0RuntimeRequirement::HostedStd`] (the spawned-thread/channel machinery is not freestanding-compatible),
    /// and [`AbiV0RuntimeRequirement::Allocator`] (the channel and boxed iterator both allocate).
    pub(in crate::body_ir) fn lower_yield(
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
