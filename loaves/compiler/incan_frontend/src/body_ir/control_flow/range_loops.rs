//! Range loop sources and counting loops over builtin and materialized ranges.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Classify a `for` header's iterable as a range the counting loop can drive, lowering it to a place when it
    /// is a range *value*, or `None` when the loop belongs on the general-iteration path.
    ///
    /// Every range source is evaluated here, once, before the loop, as legacy's native range is: an inline
    /// `start..end` header lowers its start and then snapshots its end, so the loop body reassigning a bound does not
    /// move the end. A bound range needs an independently proven local [`bir::AggregateKind::Range`] producer, and the
    /// loop iterates a snapshot of it, so reassigning the range inside the loop does not change the iteration. A
    /// `Range[T]` type spelling alone cannot prove the aggregate layout: parameters, call results, imported values, and
    /// user declarations may carry it without `start`/`end`/`step`/`inclusive` fields. Those cases refuse visibly
    /// rather than inventing a private range ABI. A checker-proven inline `range()` call instead retains scalar bounds
    /// evaluated once and an explicit constructor validation fact; an unproven same-spelled call keeps general
    /// iteration.
    pub(super) fn range_loop_source(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        iter_ty: &IncanType,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<Option<RangeLoopSource>, String> {
        if let Some(source) = self.retained_range_loop_source(iter_expr, scope, out)? {
            return Ok(Some(source));
        }
        if range_value_element_type(iter_ty).is_some() {
            return Err("range value without a source-local Body IR range aggregate".to_string());
        }
        Ok(None)
    }

    /// Select only explicit range bounds, a checked builtin call, or a source-local range aggregate.
    /// An unproven type spelling leaves general iteration on its existing checked protocol path.
    pub(super) fn retained_range_loop_source(
        &mut self,
        iter_expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<Option<RangeLoopSource>, String> {
        let span = hir_span(iter_expr.span);
        if let Some(range) = self.inline_range_loop_source(iter_expr, scope, out) {
            return Ok(Some(range));
        }
        if let Some(range) = self.builtin_range_loop_source(iter_expr, scope, out)? {
            return Ok(Some(range));
        }
        let Some(local) = self.materialized_range_local(iter_expr) else {
            return Ok(None);
        };
        let place = bir::Place::from_local(local);
        let range_ty = self.locals[local.index()].ty.clone();
        let (fact, last_use) = self.ownership_fact_for_place(&place, &range_ty);
        let snapshot = self.snapshot_local(bir::Operand::place(place, fact, last_use), range_ty, scope, span, out);
        Ok(Some(RangeLoopSource::Value(bir::Place::from_local(snapshot))))
    }

    /// Evaluate one inline range's bounds once, in source order, retaining its inclusive or exclusive end.
    /// A checked `range(a..b)` call shares this representation with a direct `for item in a..b` header.
    /// Both bounds are snapshots: evaluating the upper bound may change the storage read for the lower bound.
    pub(super) fn inline_range_loop_source(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Option<RangeLoopSource> {
        let (start, end, inclusive) = inline_range_parts(expr)?;
        let start_ty = self.resolve_ty(start.span);
        let start_value = self.lower_expr_to_operand(start, scope, out);
        let start = self.snapshot_local(start_value, start_ty, scope, hir_span(start.span), out);
        let end_ty = self.resolve_ty(end.span);
        let end_operand = self.lower_expr_to_operand(end, scope, out);
        let end = self.snapshot_local(end_operand, end_ty, scope, hir_span(expr.span), out);
        Some(RangeLoopSource::Header {
            start: index_read(start),
            end,
            inclusive,
        })
    }

    /// Copy a once-evaluated range bound or range value into a fresh compiler-owned local the loop re-reads.
    pub(super) fn snapshot_local(
        &mut self,
        value: bir::Operand,
        ty: IncanType,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::LocalId {
        let local = self.new_temp(ty, scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(local),
                rvalue: bir::Rvalue::Use(value),
            },
            span,
        });
        local
    }

    /// Normalize only a checker-proven builtin range call, retaining argument evaluation before iteration.
    ///
    /// This is a representation of builtin iterator facts, not recognition of a same-spelled user function.
    /// Bounds and step are copied into fresh locals once; a constructor validation call preserves the zero-step
    /// error before any iteration. A single inline range expression retains its own bounds and inclusivity rather than
    /// being cast to an integer. Bound range values and other iterators keep their distinct existing paths.
    pub(super) fn builtin_range_loop_source(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<Option<RangeLoopSource>, String> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.builtin_range_loop_source(inner, scope, out);
        }
        let ast::Expr::Call(callee, type_args, args) = &expr.node else {
            return Ok(None);
        };
        if self.type_info.resolved_builtin_call(expr.span) != Some(bir::BuiltinFnId::Range) {
            return Ok(None);
        }
        let ast::Expr::Ident(name) = &callee.node else {
            return Err("unsupported builtin range callee shape".into());
        };
        let declaration = self.declared_slots_for_direct_call(name, callee.span, expr.span)?;
        if declaration.builtin != Some(bir::BuiltinFnId::Range) || !type_args.is_empty() {
            return Err("builtin range lacks a matching checked canonical identity".into());
        }
        if args.is_empty() || args.len() > 3 || args.iter().any(|arg| !matches!(arg, ast::CallArg::Positional(_))) {
            return Err("unsupported builtin range argument binding".into());
        }
        if let [ast::CallArg::Positional(argument)] = args.as_slice()
            && let Some(range) = self.inline_range_loop_source(argument, scope, out)
        {
            return Ok(Some(range));
        }
        let span = hir_span(expr.span);
        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let mut values = Vec::new();
        for argument in args {
            let ast::CallArg::Positional(argument) = argument else {
                return Err("unsupported builtin range argument".into());
            };
            let operand = self.lower_expr_to_operand(argument, scope, out);
            values.push(self.push_assign_temp(bir::Rvalue::Use(operand), int_ty.clone(), scope, span, out));
        }
        let (start, end, step) = match values.as_slice() {
            [end] => (
                bir::Operand::Constant(bir::Constant::Int(0)),
                end.clone(),
                bir::Operand::Constant(bir::Constant::Int(1)),
            ),
            [start, end] => (
                start.clone(),
                end.clone(),
                bir::Operand::Constant(bir::Constant::Int(1)),
            ),
            [start, end, step] => (start.clone(), end.clone(), step.clone()),
            _ => return Err("unsupported builtin range arity".into()),
        };
        let helper = bir::HelperOp::RangeValidate;
        self.record_runtime_requirement(AbiV0RuntimeRequirement::RuntimeHelper(helper.as_str().into()));
        self.record_runtime_requirement(AbiV0RuntimeRequirement::PanicStrategy);
        self.panic_facts.push(bir::PanicFact {
            span,
            reason: bir::PanicReason::HelperMayPanic(helper),
        });
        out.push(bir::Statement {
            kind: bir::StatementKind::Call {
                destination: None,
                callee: bir::Callee::Helper(helper),
                args: fixed_elements(vec![start.clone(), end.clone(), step.clone()]),
                may_panic: true,
            },
            span,
        });
        Ok(Some(RangeLoopSource::Builtin(Box::new(BuiltinRangeBounds {
            start,
            end,
            step,
        }))))
    }

    /// Retain the exact count of a canonical zero-based unit-step range, evaluating its bound once.
    /// Other range shapes remain on their existing iteration path. The count is clamped before a list fill so a
    /// negative upper bound constructs an empty list rather than requesting a negative repetition count.
    pub(in crate::body_ir) fn zero_based_unit_range_count(
        &mut self,
        expr: &ast::Spanned<ast::Expr>,
        scope: bir::ScopeId,
        out: &mut Vec<bir::Statement>,
    ) -> Result<Option<bir::Operand>, String> {
        if let ast::Expr::Paren(inner) = &expr.node {
            return self.zero_based_unit_range_count(inner, scope, out);
        }
        let ast::Expr::Call(_, _, args) = &expr.node else {
            return Ok(None);
        };
        if self.type_info.resolved_builtin_call(expr.span) != Some(bir::BuiltinFnId::Range)
            || !zero_based_unit_arguments(args)
        {
            return Ok(None);
        }
        let Some(RangeLoopSource::Builtin(bounds)) = self.builtin_range_loop_source(expr, scope, out)? else {
            return Err("exact comprehension count lacks checked scalar range bounds".into());
        };
        let span = hir_span(expr.span);
        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let count = self.new_temp(int_ty, scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(count),
                rvalue: bir::Rvalue::Use(bounds.end),
            },
            span,
        });
        let zero = bir::Operand::Constant(bir::Constant::Int(0));
        let negative = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::Lt, index_read(count), zero.clone()),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            scope,
            span,
            out,
        );
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond: negative,
                then_block: bir::Block {
                    scope: self.new_scope(Some(scope), span),
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Assign {
                            place: bir::Place::from_local(count),
                            rvalue: bir::Rvalue::Use(zero),
                        },
                        span,
                    }],
                },
                else_block: None,
            },
            span,
        });
        Ok(Some(index_read(count)))
    }

    /// Select positive-step or negative-step exhaustion from the retained scalar builtin arguments.
    pub(super) fn builtin_range_stop_condition(
        &mut self,
        end: &bir::Operand,
        step: &bir::Operand,
        index: bir::LocalId,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let bool_ty = IncanType::Primitive(IncanPrimitiveType::Bool);
        let positive = self.push_assign_temp(
            bir::Rvalue::BinaryOp(
                bir::BinOp::Gt,
                step.clone(),
                bir::Operand::Constant(bir::Constant::Int(0)),
            ),
            bool_ty.clone(),
            scope,
            span,
            out,
        );
        let condition = self.new_temp(bool_ty, scope, span);
        let positive_scope = self.new_scope(Some(scope), span);
        let negative_scope = self.new_scope(Some(scope), span);
        let stop = |op| bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(condition),
                rvalue: bir::Rvalue::BinaryOp(op, index_read(index), end.clone()),
            },
            span,
        };
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond: positive,
                then_block: bir::Block {
                    scope: positive_scope,
                    stmts: vec![stop(bir::BinOp::Ge)],
                },
                else_block: Some(bir::Block {
                    scope: negative_scope,
                    stmts: vec![stop(bir::BinOp::Le)],
                }),
            },
            span,
        });
        bir::Operand::place(bir::Place::from_local(condition), bir::OwnershipFact::Copy, false)
    }

    /// Whether `expr` is a source-local value whose current Body-IR representation has the declared range layout.
    /// This follows only direct range literals, parentheses, and copies of another proven local; calls, imports,
    /// parameters, fields, and arbitrary type spellings remain unproven.
    pub(in crate::body_ir) fn expr_has_materialized_range_layout(&self, expr: &ast::Spanned<ast::Expr>) -> bool {
        match &expr.node {
            ast::Expr::Range { .. } => true,
            ast::Expr::Paren(inner) => self.expr_has_materialized_range_layout(inner),
            ast::Expr::Ident(name) => self
                .bindings
                .get(name)
                .is_some_and(|local| self.materialized_range_locals.contains(local)),
            _ => false,
        }
    }

    /// Return the local holding a range value whose layout provenance is still live.
    pub(super) fn materialized_range_local(&self, expr: &ast::Spanned<ast::Expr>) -> Option<bir::LocalId> {
        match &expr.node {
            ast::Expr::Paren(inner) => self.materialized_range_local(inner),
            ast::Expr::Ident(name) => self
                .bindings
                .get(name)
                .copied()
                .filter(|local| self.materialized_range_locals.contains(local)),
            _ => None,
        }
    }

    /// Read one declared field off a materialized [`bir::AggregateKind::Range`] value.
    ///
    /// Every range field is a scalar read through a non-empty projection, so this always resolves to
    /// [`bir::OwnershipFact::Copy`] and never consumes the range local's last use -- a loop may read the same
    /// field on every iteration without the range appearing to be moved out from under itself.
    pub(super) fn read_range_field(&mut self, range: &bir::Place, field: &str, field_ty: &IncanType) -> bir::Operand {
        let mut place = range.clone();
        place.projection.push(bir::PlaceElem::structural_field(field));
        let (fact, last_use) = self.ownership_fact_for_place(&place, field_ty);
        bir::Operand::place(place, fact, last_use)
    }

    /// Build the break condition for a loop over a range *value*: stop once the index has passed the range's end,
    /// or has reached an end the range does not include.
    ///
    /// This is the one place the two range spellings differ, and the reason is that inclusivity is a property of
    /// the value rather than of the loop. An inline header knows which of `>` and `>=` it wants at lowering time,
    /// because `..` versus `..=` is written right there. A bound range does not: the statement that built the
    /// value and the loop that consumes it are different statements, and nothing stops a program from binding one
    /// range on one branch and another on the other. Choosing the comparison here from whichever construction
    /// lowering happened to see first would be a guess that a reassignment silently invalidates. So this computes
    /// *both* of the comparisons the inline form picks between, and lets the range's own
    /// [`bir::AggregateKind::RANGE_FIELD_INCLUSIVE`] operand select between them -- the same decision, made from
    /// the value instead of from the syntax.
    ///
    /// Every field is re-read per iteration, matching the inline form, which also re-lowers its `end` expression
    /// each time around rather than snapshotting it before the loop.
    pub(super) fn range_value_stop_condition(
        &mut self,
        range: &bir::Place,
        idx_local: bir::LocalId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        body_stmts: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let bool_ty = IncanType::Primitive(IncanPrimitiveType::Bool);
        let past_end = {
            let end = self.read_range_field(range, bir::AggregateKind::RANGE_FIELD_END, &int_ty);
            let idx = index_read(idx_local);
            self.push_assign_temp(
                bir::Rvalue::BinaryOp(bir::BinOp::Gt, idx, end),
                bool_ty.clone(),
                loop_scope,
                span,
                body_stmts,
            )
        };
        let at_or_past_end = {
            let end = self.read_range_field(range, bir::AggregateKind::RANGE_FIELD_END, &int_ty);
            let idx = index_read(idx_local);
            self.push_assign_temp(
                bir::Rvalue::BinaryOp(bir::BinOp::Ge, idx, end),
                bool_ty.clone(),
                loop_scope,
                span,
                body_stmts,
            )
        };
        let inclusive = self.read_range_field(range, bir::AggregateKind::RANGE_FIELD_INCLUSIVE, &bool_ty);
        let exclusive = self.push_assign_temp(
            bir::Rvalue::UnaryOp(bir::UnOp::Not, inclusive),
            bool_ty.clone(),
            loop_scope,
            span,
            body_stmts,
        );
        let stops_at_end = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::And, at_or_past_end, exclusive),
            bool_ty.clone(),
            loop_scope,
            span,
            body_stmts,
        );
        self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::Or, past_end, stops_at_end),
            bool_ty,
            loop_scope,
            span,
            body_stmts,
        )
    }

    /// Retain the range's checked index update as operations shared by fallthrough and each `continue` edge.
    ///
    /// A source range advances by its unit step, so an index at the maximum integer has visited an inclusive range's
    /// last item: the loop ends there, as legacy's native range does, instead of advancing past it. A builtin range
    /// keeps its own runtime advancement contract.
    pub(super) fn range_increment_steps(
        &mut self,
        source: &RangeLoopSource,
        idx_local: bir::LocalId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> Vec<bir::Statement> {
        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let mut body_stmts = Vec::new();
        if !matches!(source, RangeLoopSource::Builtin(_)) {
            self.push_range_maximum_exit(idx_local, loop_scope, span, &mut body_stmts);
        }
        let step = match source {
            RangeLoopSource::Header { .. } => bir::Operand::Constant(bir::Constant::Int(RANGE_UNIT_STEP)),
            RangeLoopSource::Builtin(bounds) => bounds.step.clone(),
            RangeLoopSource::Value(range) => {
                self.read_range_field(range, bir::AggregateKind::RANGE_FIELD_STEP, &int_ty)
            }
        };
        let incremented = self.push_assign_temp(
            bir::Rvalue::BinaryOp(bir::BinOp::Add, index_read(idx_local), step),
            int_ty,
            loop_scope,
            span,
            &mut body_stmts,
        );
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(idx_local),
                rvalue: bir::Rvalue::Use(incremented),
            },
            span,
        });
        body_stmts
    }

    /// Break out of a unit-step counting loop whose index already holds the maximum integer, before it advances.
    pub(super) fn push_range_maximum_exit(
        &mut self,
        idx_local: bir::LocalId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let exhausted = self.push_assign_temp(
            bir::Rvalue::BinaryOp(
                bir::BinOp::Eq,
                index_read(idx_local),
                bir::Operand::Constant(bir::Constant::Int(i64::MAX)),
            ),
            IncanType::Primitive(IncanPrimitiveType::Bool),
            loop_scope,
            span,
            out,
        );
        let break_scope = self.new_scope(Some(loop_scope), span);
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond: exhausted,
                then_block: bir::Block {
                    scope: break_scope,
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Break { value: None },
                        span,
                    }],
                },
                else_block: None,
            },
            span,
        });
    }

    /// Lower one range-shaped `for` loop into the normalized counting `Loop` both range spellings share: seed an
    /// index from the range's start, break once it reaches the end, bind the loop pattern from the index, run the
    /// body, then advance the index by the range's step.
    ///
    /// Where the two spellings get those three pieces from is the only difference, and each is taken from
    /// `source`: an inline header knows its step and inclusivity statically, while a range value reads them back off
    /// its snapshot. [`Self::range_loop_source`] already evaluated the range once, before the loop, as legacy's native
    /// range does; a checker-proven builtin call also retains signed exhaustion.
    pub(super) fn lower_range_counting_loop(
        &mut self,
        for_stmt: &ast::ForStmt,
        item_ty: &IncanType,
        source: &RangeLoopSource,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let loop_scope = self.new_scope(Some(scope), span);
        let reads = |name: &str| count_reads_in_stmts(name, &for_stmt.body);
        let item = self.declare_for_item_local(
            &for_stmt.pattern,
            item_ty,
            loop_scope,
            hir_span(for_stmt.pattern.span),
            &reads,
        );
        self.lower_counting_iteration(
            source,
            item,
            scope,
            loop_scope,
            span,
            out,
            |builder, loop_scope, stmts| {
                builder.bind_for_pattern(&for_stmt.pattern, item_ty, item, loop_scope, &reads, stmts);
                builder.lower_block_into(&for_stmt.body, loop_scope, stmts);
                builder.insert_scope_drops(stmts, loop_scope);
            },
        );
    }

    /// Drive a checked range with one counter for statement loops and comprehension clauses alike.
    /// The caller owns pattern binding and per-iteration cleanup; this boundary retains exhaustion and continue
    /// actions.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn lower_counting_iteration(
        &mut self,
        source: &RangeLoopSource,
        item: bir::LocalId,
        scope: bir::ScopeId,
        loop_scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        body_fn: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        // ---- Counter storage and loop entry ----
        let int_ty = IncanType::Primitive(IncanPrimitiveType::Int);
        let start_operand = match source {
            RangeLoopSource::Header { start, .. } => start.clone(),
            RangeLoopSource::Builtin(bounds) => bounds.start.clone(),
            RangeLoopSource::Value(range) => {
                self.read_range_field(range, bir::AggregateKind::RANGE_FIELD_START, &int_ty)
            }
        };
        let idx_local = self.new_temp(int_ty.clone(), scope, span);
        out.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(idx_local),
                rvalue: bir::Rvalue::Use(start_operand),
            },
            span,
        });

        // `for` never produces a value from `break` (same reasoning as `while` -- see `Self::lower_while`).
        self.loop_break_targets.push(None);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut body_stmts = Vec::new();

        // ---- Exhaustion before each item ----
        let cond = self.counting_loop_stop_condition(source, idx_local, loop_scope, span, &mut body_stmts);
        let break_scope = self.new_scope(Some(loop_scope), span);
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block: bir::Block {
                    scope: break_scope,
                    stmts: vec![bir::Statement {
                        kind: bir::StatementKind::Break { value: None },
                        span,
                    }],
                },
                else_block: None,
            },
            span,
        });

        // ---- Item binding and continued iteration ----
        body_stmts.push(bir::Statement {
            kind: bir::StatementKind::Assign {
                place: bir::Place::from_local(item),
                rvalue: bir::Rvalue::Use(index_read(idx_local)),
            },
            span,
        });

        let increment = self.range_increment_steps(source, idx_local, loop_scope, span);
        if let Some(actions) = self.loop_continue_actions.last_mut() {
            *actions = increment;
        }
        body_fn(self, loop_scope, &mut body_stmts);
        if let Some(actions) = self.loop_continue_actions.last() {
            body_stmts.extend(actions.iter().cloned());
        }
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();

        out.push(bir::Statement {
            kind: bir::StatementKind::Loop {
                body: bir::Block {
                    scope: loop_scope,
                    stmts: body_stmts,
                },
            },
            span,
        });
    }

    /// Retain exhaustion from the selected range's scalar bounds, validated step, or materialized layout.
    pub(super) fn counting_loop_stop_condition(
        &mut self,
        source: &RangeLoopSource,
        index: bir::LocalId,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        match source {
            RangeLoopSource::Header { end, inclusive, .. } => {
                let comparison = if *inclusive { bir::BinOp::Gt } else { bir::BinOp::Ge };
                self.push_assign_temp(
                    bir::Rvalue::BinaryOp(comparison, index_read(index), index_read(*end)),
                    IncanType::Primitive(IncanPrimitiveType::Bool),
                    scope,
                    span,
                    out,
                )
            }
            RangeLoopSource::Value(range) => self.range_value_stop_condition(range, index, scope, span, out),
            RangeLoopSource::Builtin(bounds) => {
                self.builtin_range_stop_condition(&bounds.end, &bounds.step, index, scope, span, out)
            }
        }
    }
}
