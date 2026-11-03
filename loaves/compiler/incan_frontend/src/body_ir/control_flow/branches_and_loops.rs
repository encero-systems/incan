//! Lowering of `if`, `if let`, `while let`, `loop`, `while` and `for` statements and branch blocks.

use super::*;

impl<'type_info, 'source> BodyBuilder<'type_info, 'source> {
    /// Lower `if`/`elif`/`else` into a [`bir::StatementKind::If`] chain. Evaluate ownership facts in written order,
    /// then fold the already lowered `elif` branches into nested `else { if ... }` wrappers from the last branch
    /// inward. Reversing the ownership visit would move an owner in an earlier condition before a later condition
    /// reads it. Pattern conditions use the dedicated `if let` boundary.
    /// An elif condition is itself conditional, so its enclosing owners remain live when an earlier arm wins.
    /// Its temporary owners drop at the wrapper's exit, before an earlier enclosing owner is released.
    pub(in crate::body_ir) fn lower_if(
        &mut self,
        if_stmt: &ast::IfStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let cond_expr = match &if_stmt.condition {
            ast::Condition::Expr(cond_expr) => cond_expr,
            ast::Condition::Let { pattern, value } => {
                self.lower_if_let(pattern, value, &if_stmt.then_body, scope, span, out);
                return;
            }
        };
        let cond = self.lower_expr_to_operand(cond_expr, scope, out);

        let subjects = self.input_subject_locals();
        let mut pending_moves = self.input_move_state(&subjects);
        let mut reaching_moves = Vec::new();
        let range_layouts_before = self.materialized_range_locals.clone();
        let then_block = self.lower_input_branch_block(&if_stmt.then_body, scope, span);
        self.retain_reaching_input_moves(&if_stmt.then_body, &subjects, &mut reaching_moves);
        let mut range_layouts_after_branches = vec![self.materialized_range_locals.clone()];

        let mut lowered_branches = Vec::with_capacity(if_stmt.elif_branches.len());
        for (elif_cond, elif_body) in &if_stmt.elif_branches {
            self.restore_input_move_state(&subjects, &pending_moves);
            self.materialized_range_locals = range_layouts_before.clone();
            let mut wrapper = Vec::new();
            let wrapper_scope = self.new_scope(Some(scope), span);
            self.conditional_scopes.push(wrapper_scope);
            self.input_if_scopes.push(wrapper_scope);
            let cond_operand = self.lower_expr_to_operand(elif_cond, wrapper_scope, &mut wrapper);
            self.input_if_scopes.pop();
            self.conditional_scopes.pop();
            pending_moves = self.input_move_state(&subjects);
            let then_block = self.lower_input_branch_block(elif_body, wrapper_scope, span);
            self.retain_reaching_input_moves(elif_body, &subjects, &mut reaching_moves);
            lowered_branches.push((wrapper_scope, wrapper, cond_operand, then_block));
            range_layouts_after_branches.push(self.materialized_range_locals.clone());
        }

        self.restore_input_move_state(&subjects, &pending_moves);
        self.materialized_range_locals = range_layouts_before;
        let mut else_block = if_stmt
            .else_body
            .as_ref()
            .map(|else_body| self.lower_input_branch_block(else_body, scope, span));
        if let Some(body) = &if_stmt.else_body {
            self.retain_reaching_input_moves(body, &subjects, &mut reaching_moves);
        } else {
            reaching_moves.push(self.input_move_state(&subjects));
        }
        self.restore_input_move_state(&subjects, &intersect_local_states(reaching_moves));
        range_layouts_after_branches.push(self.materialized_range_locals.clone());

        // Only assembly runs backward; every read's ownership fact is already fixed in written order.
        for (wrapper_scope, mut wrapper, cond_operand, then_block) in lowered_branches.into_iter().rev() {
            wrapper.push(bir::Statement {
                kind: bir::StatementKind::If {
                    cond: cond_operand,
                    then_block,
                    else_block,
                },
                span,
            });
            self.insert_scope_drops(&mut wrapper, wrapper_scope);
            else_block = Some(bir::Block {
                scope: wrapper_scope,
                stmts: wrapper,
            });
        }

        self.materialized_range_locals = intersect_local_states(range_layouts_after_branches);

        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block,
                else_block,
            },
            span,
        });
    }

    /// Lower one `if`/`elif`/`else` branch body into its own scoped [`bir::Block`]: allocate a child scope, lower
    /// the statements into it, then insert scope-exit drops. Shared by [`Self::lower_if`]'s then/else/elif bodies
    /// and [`Self::lower_if_expr`]'s then/else bodies, since both need exactly this shape.
    /// Enclosing owners stay live across the conditional path; branch-local owners can still transfer.
    pub(in crate::body_ir) fn lower_branch_block(
        &mut self,
        body: &[ast::Spanned<ast::Statement>],
        parent_scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::Block {
        self.lower_branch_with_input_policy(body, parent_scope, span, false)
    }

    /// Statement-if branches join authenticated whole-owner state separately; expression and pattern branches do not.
    pub(super) fn lower_input_branch_block(
        &mut self,
        body: &[ast::Spanned<ast::Statement>],
        parent_scope: bir::ScopeId,
        span: HirSourceSpan,
    ) -> bir::Block {
        self.lower_branch_with_input_policy(body, parent_scope, span, true)
    }

    /// Keep lexical binding restoration common while recording only the conditional scopes with an explicit join.
    pub(super) fn lower_branch_with_input_policy(
        &mut self,
        body: &[ast::Spanned<ast::Statement>],
        parent_scope: bir::ScopeId,
        span: HirSourceSpan,
        input_policy: bool,
    ) -> bir::Block {
        // Body IR retains every branch local for the backend to lower, but its name lookup must return to the
        // enclosing lexical environment afterwards. Without this snapshot a `let x` in one branch becomes the
        // binding read after the `if`, even though source resolution scopes it to the branch alone.
        let enclosing_bindings = self.bindings.clone();
        let branch_scope = self.new_scope(Some(parent_scope), span);
        let mut stmts = Vec::new();
        self.conditional_scopes.push(branch_scope);
        if input_policy {
            self.input_if_scopes.push(branch_scope);
        }
        self.lower_block_into(body, branch_scope, &mut stmts);
        if input_policy {
            self.input_if_scopes.pop();
        }
        self.conditional_scopes.pop();
        self.insert_scope_drops(&mut stmts, branch_scope);
        self.bindings = enclosing_bindings;
        bir::Block {
            scope: branch_scope,
            stmts,
        }
    }

    /// Lower an expression-position `if` (`ast::Expr::If`) into the same [`bir::StatementKind::If`] shape
    /// statement-position `if` uses (see [`Self::lower_if`]), reusing [`Self::lower_branch_block`] for both
    /// branches. The typechecker gives an expression-position `if` type `Unit` unconditionally (`check_if_expr` in
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/control_flow.rs` discards any branch value and always
    /// returns `ResolvedType::Unit`) -- unlike a `loop` expression, an `if` expression cannot yet produce a value
    /// from its branches, so its Body IR operand is always the `Unit` constant rather than a place read.
    pub(in crate::body_ir) fn lower_if_expr(
        &mut self,
        if_expr: &ast::IfExpr,
        scope: bir::ScopeId,
        span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let cond = self.lower_expr_to_operand(&if_expr.condition, scope, out);
        let range_layouts_before = self.materialized_range_locals.clone();
        let then_block = self.lower_branch_block(&if_expr.then_body, scope, hir_span_value);
        let then_range_layouts = self.materialized_range_locals.clone();
        self.materialized_range_locals = range_layouts_before;
        let else_block = if_expr
            .else_body
            .as_ref()
            .map(|body| self.lower_branch_block(body, scope, hir_span_value));
        self.materialized_range_locals =
            intersect_local_states(vec![then_range_layouts, self.materialized_range_locals.clone()]);
        out.push(bir::Statement {
            kind: bir::StatementKind::If {
                cond,
                then_block,
                else_block,
            },
            span: hir_span_value,
        });
        bir::Operand::Constant(bir::Constant::Unit)
    }

    /// Push one [`bir::StatementKind::Loop`] whose body `lower_body` fills in, owning the two pieces of loop state
    /// every unconditional-loop spelling needs: the `break` target inner `break` statements resolve against, and
    /// the end-of-iteration drops for locals the body declared in `loop_scope`.
    ///
    /// `break_target` is the only thing that separates the spellings routed through here. `Some(result_local)` is
    /// the value-producing `loop:` expression, whose `break value` statements assign into that place before exiting
    /// (see [`Self::lower_break`]). `None` is every statement-position loop, where `break` is a plain valueless
    /// exit. Pushing `None` is not a formality: it is what stops a `break` inside a statement-position loop that is
    /// lexically nested in a `loop:` expression from being rewritten into an assignment to the *outer* loop's
    /// result place (see [`Self::loop_break_targets`] for why the stack exists).
    ///
    /// `loop_scope` is the caller's to create rather than this helper's. A `loop:` expression declares its result in
    /// the enclosing scope before lowering its body, so that result survives every loop-body exit.
    ///
    /// The two `for` paths deliberately do not route through here. [`Self::lower_range_counting_loop`] drops its
    /// scope mid-body, before the index advance it appends afterwards, and [`Self::lower_general_iteration`] leaves
    /// the drops to its caller's body closure; both would need this helper's fixed "body, then drops" order relaxed
    /// into a per-caller choice, which would buy nothing but an extra parameter.
    pub(super) fn push_loop(
        &mut self,
        loop_scope: bir::ScopeId,
        break_target: Option<bir::LocalId>,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
        lower_body: impl FnOnce(&mut Self, bir::ScopeId, &mut Vec<bir::Statement>),
    ) {
        // A loop body runs zero or more times, so a range-layout fact established inside it holds on entry only if
        // it also held before. Intersecting entry state with exit state is what keeps that conservative -- and
        // doing it here rather than at each caller is why every loop spelling gets it, including the statement
        // `loop:` this helper was extracted to serve (#1165 established the fact, #1162 extracted the tail).
        let range_layouts_before = self.materialized_range_locals.clone();
        // A loop body's locals are real Body-IR locals, but the lexical names they introduce are not visible after
        // the loop. `for` manages its header binding separately; every loop lowered through this helper needs the
        // same restoration for names declared in its body.
        let enclosing_bindings = self.bindings.clone();
        self.loop_break_targets.push(break_target);
        self.loop_scopes.push(loop_scope);
        self.loop_continue_actions.push(Vec::new());
        let mut body_stmts = Vec::new();
        lower_body(self, loop_scope, &mut body_stmts);
        self.insert_scope_drops(&mut body_stmts, loop_scope);
        self.loop_break_targets.pop();
        self.loop_scopes.pop();
        self.loop_continue_actions.pop();
        self.materialized_range_locals =
            intersect_local_states(vec![range_layouts_before, self.materialized_range_locals.clone()]);
        self.bindings = enclosing_bindings;

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

    /// Lower `if let P = subject:` as a two-arm [`bir::Rvalue::Match`].
    ///
    /// Desugaring to a match rather than an `If` plus a synthesized pattern test is deliberate, and follows
    /// [`bir::Rvalue::Match`]'s own rule: match stays one structured node so a target backend's native `match`
    /// performs the destructuring and dispatch, instead of this stage re-deriving them as a chain of tests.
    ///
    /// The parser accepts no `else` or `elif` on an `if let` (see its `test_parse_if_let_rejects_else_branch`), so
    /// the fallback arm is always empty. That also settles the binding scope for free: the pattern's names exist in
    /// the matched arm only, and there is no sibling branch that could observe them.
    ///
    /// A failed match is ordinary control flow, so no [`bir::PanicFact`] is recorded -- unlike `assert value is P`,
    /// which panics on the same shape.
    pub(super) fn lower_if_let(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        value: &ast::Spanned<ast::Expr>,
        then_body: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        if !match_pattern_is_supported(&pattern.node) {
            self.push_unsupported_stmt("`if let` with a byte-string literal pattern".to_string(), span, out);
            return;
        }
        let scrutinee_ty = self.resolve_ty(value.span);
        let in_place = self.matches_in_place(value.span, &scrutinee_ty);
        let first_temporary = self.locals.len();
        let scrutinee_place = self.lower_expr_to_place(value, scope, out);
        self.end_scrutinee_temporaries(value.span, &scrutinee_place, in_place, first_temporary, scope, out);
        let scrutinee = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

        let layouts_before = self.materialized_range_locals.clone();
        let matched_arm = self.lower_statement_pattern_arm(
            pattern,
            &scrutinee_ty,
            &scrutinee_place,
            in_place,
            scope,
            then_body,
            span,
        );
        let layouts_after_match = self.materialized_range_locals.clone();

        // The unmatched path runs none of the body, so it leaves the entry facts untouched.
        self.materialized_range_locals = intersect_local_states(vec![layouts_before, layouts_after_match]);

        let unmatched_arm = bir::MatchArm {
            pattern: bir::Pattern::Wildcard,
            guard_stmts: Vec::new(),
            guard: None,
            body_stmts: Vec::new(),
            result: bir::Operand::Constant(bir::Constant::Unit),
        };
        self.push_assign_temp(
            bir::Rvalue::Match {
                scrutinee,
                arms: vec![matched_arm, unmatched_arm],
            },
            IncanType::Primitive(IncanPrimitiveType::Unit),
            scope,
            span,
            out,
        );
    }

    /// Lower `while let P = subject:` as a [`bir::StatementKind::Loop`] whose body matches, then breaks.
    ///
    /// The subject is lowered *inside* the loop body so it is re-evaluated each iteration, which is what makes the
    /// loop terminate: `while let Some(item) = iterator.next():` depends on the call running again every time.
    /// Hoisting it would turn a draining loop into an infinite one.
    ///
    /// The unmatched arm breaks rather than recording a panic fact, because exhausting the pattern is how this loop
    /// is meant to end.
    pub(super) fn lower_while_let(
        &mut self,
        pattern: &ast::Spanned<ast::Pattern>,
        value: &ast::Spanned<ast::Expr>,
        body: &[ast::Spanned<ast::Statement>],
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        if !match_pattern_is_supported(&pattern.node) {
            self.push_unsupported_stmt("`while let` with a byte-string literal pattern".to_string(), span, out);
            return;
        }
        let loop_scope = self.new_scope(Some(scope), span);
        self.push_loop(loop_scope, None, span, out, |builder, loop_scope, body_stmts| {
            let scrutinee_ty = builder.resolve_ty(value.span);
            let in_place = builder.matches_in_place(value.span, &scrutinee_ty);
            let first_temporary = builder.locals.len();
            let scrutinee_place = builder.lower_expr_to_place(value, loop_scope, body_stmts);
            builder.end_scrutinee_temporaries(
                value.span,
                &scrutinee_place,
                in_place,
                first_temporary,
                loop_scope,
                body_stmts,
            );
            let scrutinee = bir::Operand::place(scrutinee_place.clone(), bir::OwnershipFact::Borrow, false);

            // Exactly one arm runs per iteration, so a fact the matched arm established must not survive into code
            // that could instead have reached the exhausted arm. `push_loop` intersects at the loop boundary, which
            // covers today's shape because the match is the body's only statement -- but relying on that would make
            // this correct by accident, and wrong the moment a statement is appended after it.
            let layouts_before_arms = builder.materialized_range_locals.clone();
            let matched_arm = builder.lower_statement_pattern_arm(
                pattern,
                &scrutinee_ty,
                &scrutinee_place,
                in_place,
                loop_scope,
                body,
                span,
            );
            builder.materialized_range_locals =
                intersect_local_states(vec![layouts_before_arms, builder.materialized_range_locals.clone()]);
            let exhausted_arm = bir::MatchArm {
                pattern: bir::Pattern::Wildcard,
                guard_stmts: Vec::new(),
                guard: None,
                body_stmts: vec![bir::Statement {
                    kind: bir::StatementKind::Break { value: None },
                    span,
                }],
                result: bir::Operand::Constant(bir::Constant::Unit),
            };
            builder.push_assign_temp(
                bir::Rvalue::Match {
                    scrutinee,
                    arms: vec![matched_arm, exhausted_arm],
                },
                IncanType::Primitive(IncanPrimitiveType::Unit),
                loop_scope,
                span,
                body_stmts,
            );
        });
    }

    /// Lower a value-producing `loop:` expression (`ast::Expr::Loop`) into a [`bir::StatementKind::Loop`] plus a
    /// dedicated result local that every `break value` inside the loop's *own* body (not a nested loop's --
    /// enforced by [`Self::loop_break_targets`]) assigns into before exiting. The typechecker resolves this
    /// expression's type from the union of its `break value` operand types (`check_loop_expr` in
    /// `loaves/compiler/incan_frontend/src/typechecker/check_expr/control_flow.rs`), so -- unlike an `if` expression,
    /// which is always `Unit` -- a `loop` expression's produced value genuinely comes from its branches and needs
    /// this merge-into-one-place treatment; see [`Self::lower_break`] for the other half of the mechanism.
    pub(in crate::body_ir) fn lower_loop_expr(
        &mut self,
        loop_expr: &ast::LoopExpr,
        scope: bir::ScopeId,
        span: ast::Span,
        out: &mut Vec<bir::Statement>,
    ) -> bir::Operand {
        let hir_span_value = hir_span(span);
        let ty = self.resolve_ty(span);
        let loop_scope = self.new_scope(Some(scope), hir_span_value);
        let result_local = self.new_temp(ty.clone(), scope, hir_span_value);

        self.push_loop(
            loop_scope,
            Some(result_local),
            hir_span_value,
            out,
            |builder, loop_scope, body_stmts| builder.lower_block_into(&loop_expr.body, loop_scope, body_stmts),
        );
        self.temp_operand(result_local, &ty)
    }

    /// Lower a statement-position `loop: body` into the same [`bir::StatementKind::Loop`] the expression spelling
    /// produces, minus the result place: the loop runs its body until a `break` exits it and yields nothing.
    ///
    /// The two spellings differ in exactly one fact and it is the break target. The typechecker already draws the
    /// same line — `check_loop_stmt` pushes a `LoopContextKind::Statement` context and rejects `break value` inside
    /// it with `break_value_requires_loop_expression`, while `check_loop_expr` pushes an `Expression` context and
    /// unifies the break types (both in `loaves/compiler/incan_frontend/src/typechecker/check_stmt.rs`). Passing `None`
    /// here is that same rule read off the typechecker rather than a second one invented in lowering: a well-typed
    /// program has no value-carrying `break` to route anywhere, and a hand-built AST that has one keeps its value
    /// on the `Break` statement per [`bir::StatementKind::Break`]'s documented default instead of being silently
    /// merged into some enclosing loop's result.
    ///
    /// Scoping mirrors [`Self::lower_while`], which the typechecker treats identically (`check_while_stmt` opens
    /// the same `ScopeKind::Block` and pushes the same statement loop context). `continue` needs nothing here at
    /// all: it lowers to [`bir::StatementKind::Continue`] in [`Self::lower_stmt_into`] and means the innermost
    /// enclosing loop wherever it appears.
    pub(in crate::body_ir) fn lower_loop_stmt(
        &mut self,
        loop_stmt: &ast::LoopStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let loop_scope = self.new_scope(Some(scope), span);
        self.push_loop(loop_scope, None, span, out, |builder, loop_scope, body_stmts| {
            builder.lower_block_into(&loop_stmt.body, loop_scope, body_stmts)
        });
    }

    /// Lower `while cond: body` into Body IR's single normalized loop shape: a [`bir::StatementKind::Loop`] whose
    /// body opens with `if not cond: break`, followed by the lowered loop body. A `while let` pattern condition —
    /// not yet modeled by v0 — lowers to an explicit unsupported placeholder instead of the real loop.
    ///
    /// The condition is lowered *inside* the loop body, in `loop_scope`, so it is re-evaluated every iteration.
    ///
    /// A literal `true` condition is the infinite loop `loop:` spells, and lowers exactly as that statement does, with
    /// no exit test: only the body's own `break` or `return` leaves it. Legacy emission makes the same decision (it
    /// emits Rust's `loop` for a literal `true`), so a value-returning body that ends in such a loop has no
    /// fallthrough.
    pub(in crate::body_ir) fn lower_while(
        &mut self,
        while_stmt: &ast::WhileStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let cond_expr = match &while_stmt.condition {
            ast::Condition::Expr(cond_expr) => cond_expr,
            ast::Condition::Let { pattern, value } => {
                self.lower_while_let(pattern, value, &while_stmt.body, scope, span, out);
                return;
            }
        };
        if matches!(cond_expr.node, ast::Expr::Literal(ast::Literal::Bool(true))) {
            let loop_scope = self.new_scope(Some(scope), span);
            self.push_loop(loop_scope, None, span, out, |builder, loop_scope, body_stmts| {
                builder.lower_block_into(&while_stmt.body, loop_scope, body_stmts)
            });
            return;
        }

        let loop_scope = self.new_scope(Some(scope), span);
        // `while` never produces a value from `break`, so the break target is `None` -- see `Self::push_loop`.
        self.push_loop(loop_scope, None, span, out, |builder, loop_scope, body_stmts| {
            let cond_operand = builder.lower_expr_to_operand(cond_expr, loop_scope, body_stmts);
            let negated = builder.negate_operand(cond_operand, loop_scope, span, body_stmts);
            let break_scope = builder.new_scope(Some(loop_scope), span);
            let break_block = bir::Block {
                scope: break_scope,
                stmts: vec![bir::Statement {
                    kind: bir::StatementKind::Break { value: None },
                    span,
                }],
            };
            body_stmts.push(bir::Statement {
                kind: bir::StatementKind::If {
                    cond: negated,
                    then_block: break_block,
                    else_block: None,
                },
                span,
            });

            builder.lower_block_into(&while_stmt.body, loop_scope, body_stmts);
        });
    }

    /// Lower a `for` statement. Range-shaped iterables lower into a normalized counting `Loop`, preserving
    /// #1103's original range-loop shape for the inline `for x in start..end:` header unchanged. Every other
    /// iterable -- builtin collections (`List`/`Dict`/`String`) and user-defined iterables implementing the RFC
    /// 068 `__iter__`/`__next__` protocol, including the fallible `for item in iterable?:` form (RFC 115) --
    /// lowers through [`Self::lower_general_iteration`], sharing its per-clause iteration primitive with
    /// comprehensions and generator expressions (see [`Self::lower_comprehension_clauses`]).
    ///
    /// "Range-shaped" covers two spellings, and both reach the same counting loop (#1165). One is the inline
    /// header, whose bounds are still lowered straight out of the AST. The other is a range *value* -- `r = 0..10`
    /// then `for i in r:` -- which reaches this point as an ordinary expression of the checked range type; it is
    /// materialized as a place and the loop reads its declared [`bir::AggregateKind::RANGE_FIELDS`] back, so a
    /// bound range iterates with the same facts as the range it was bound from instead of degrading to an opaque
    /// [`bir::StatementKind::IterNext`] poll. See [`Self::range_loop_source`] for why reading those fields back is
    /// sound, and [`Self::range_value_stop_condition`] for the one place the two spellings genuinely differ.
    ///
    /// Both paths accept the same loop-pattern subset the typechecker accepts -- a plain binding, `_`, and
    /// (recursively) a tuple of those, per `TypeChecker::define_for_pattern_bindings` in
    /// `loaves/compiler/incan_frontend/src/typechecker/check_stmt.rs` (#1125). A plain `for x in ...` binds the
    /// produced item directly; every other shape writes it into a per-iteration temporary that
    /// [`Self::bind_for_pattern`] then projects one real named binding out of per bound name. Any shape outside
    /// that subset -- which the typechecker already rejects with its own diagnostic before lowering ever runs --
    /// lowers to `Unsupported` naming the offending shape, checked up front so a refusal never leaves half-emitted
    /// bindings behind (the same "check before partially lowering" precedent as [`Self::lower_binary`] and
    /// [`Self::lower_match`]). The same up-front check also refuses a tuple pattern whose produced item is not a
    /// tuple of matching arity, so lowering can never invent `.0`/`.1` projections into a value that has no such
    /// fields -- see [`unsupported_for_pattern`].
    pub(in crate::body_ir) fn lower_for(
        &mut self,
        for_stmt: &ast::ForStmt,
        scope: bir::ScopeId,
        span: HirSourceSpan,
        out: &mut Vec<bir::Statement>,
    ) {
        let iter_ty = self.retained_expression_type(&for_stmt.iter);
        let item_ty = self.for_item_type(&for_stmt.pattern, &iter_ty);
        if let Some(reason) = unsupported_for_pattern(&for_stmt.pattern.node, &item_ty) {
            self.push_unsupported_stmt(reason, span, out);
            return;
        }
        // The typechecker enters a lexical block scope for the loop header/body, so every binding introduced by the
        // pattern must disappear after the statement. Keep the active lookup map for restoration while leaving the
        // loop locals themselves in Body IR for the loop's statements to reference.
        let enclosing_bindings = self.bindings.clone();
        let range_layouts_before = self.materialized_range_locals.clone();
        let range = match self.range_loop_source(&for_stmt.iter, &iter_ty, scope, out) {
            Ok(range) => range,
            Err(reason) => {
                self.push_unsupported_stmt(reason, span, out);
                self.bindings = enclosing_bindings;
                self.materialized_range_locals = range_layouts_before;
                return;
            }
        };
        let Some(range) = range else {
            let loop_scope = self.new_scope(Some(scope), span);
            let item_local = self.declare_for_item_local(
                &for_stmt.pattern,
                &item_ty,
                loop_scope,
                hir_span(for_stmt.pattern.span),
                &|name| count_reads_in_stmts(name, &for_stmt.body),
            );
            self.lower_general_iteration(
                &for_stmt.iter,
                item_local,
                scope,
                loop_scope,
                span,
                out,
                |builder, loop_scope, body_stmts| {
                    builder.bind_for_pattern(
                        &for_stmt.pattern,
                        &item_ty,
                        item_local,
                        loop_scope,
                        &|name| count_reads_in_stmts(name, &for_stmt.body),
                        body_stmts,
                    );
                    builder.lower_block_into(&for_stmt.body, loop_scope, body_stmts);
                    builder.insert_scope_drops(body_stmts, loop_scope);
                },
            );
            self.bindings = enclosing_bindings;
            self.materialized_range_locals =
                intersect_local_states(vec![range_layouts_before, self.materialized_range_locals.clone()]);
            return;
        };
        self.lower_range_counting_loop(for_stmt, &item_ty, &range, scope, span, out);
        self.bindings = enclosing_bindings;
        self.materialized_range_locals =
            intersect_local_states(vec![range_layouts_before, self.materialized_range_locals.clone()]);
    }

    /// The item type a `for` loop's pattern binds, falling back to a range value's own element type.
    ///
    /// A builtin list's retained closed element type supplies its declaring owner's identity even when that owner
    /// is not imported in this module. An open element keeps the checker's contextual pattern type. The generic
    /// `Range[T]` spelling carries its item type, so it supplies the recovery fallback only when the checker has no
    /// fact. That type fallback never grants aggregate-layout authority; [`Self::range_loop_source`] separately
    /// requires local materialization provenance before field projection.
    pub(in crate::body_ir) fn for_item_type(
        &self,
        pattern: &ast::Spanned<ast::Pattern>,
        iter_ty: &IncanType,
    ) -> IncanType {
        let checked = self.resolve_ty(pattern.span);
        if let Some([element]) = iter_ty.collection_arguments(CollectionTypeId::List) {
            if !self.leaves_type_open(element) {
                return element.clone();
            }
            return self.refine_task_storage(&checked, element);
        }
        if !matches!(checked, IncanType::Unknown) {
            return checked;
        }
        range_value_element_type(iter_ty).cloned().unwrap_or(checked)
    }
}
